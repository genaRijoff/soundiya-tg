//! The few Bot API calls the bot needs.
//!
//! The token is part of every request URL, so errors never carry the URL: reqwest errors
//! go through `without_url`, and nothing here logs a request.
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{sync::RwLock, time::Duration};

/// Long polling: Telegram holds `getUpdates` open this long when there is nothing new.
/// Kept well under common NAT idle timeouts (WARP, CGNAT), so a held request is not cut silently.
pub const POLL_SECONDS: u64 = 25;

pub struct Api {
    /// Replaced after a network error: pooled connections that died during an outage would
    /// otherwise be reused and fail one by one (`os error 110`).
    http: RwLock<reqwest::Client>,
    base: String,
}

/// Telegram's answers are small; a cap keeps a broken or hostile peer from filling the RAM.
const MAX_REPLY: usize = 8 << 20;

/// Reads a response body, refusing one larger than `limit` bytes.
pub async fn read_limited(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, String> {
    if response.content_length().is_some_and(|n| n > limit as u64) {
        return Err("response too large".into());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(describe)? {
        if body.len() + chunk.len() > limit {
            return Err("response too large".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        // A dead peer is noticed in ~30 s instead of the kernel's default ~2 h.
        .tcp_keepalive(Duration::from_secs(15))
        .tcp_keepalive_interval(Duration::from_secs(5))
        .tcp_keepalive_retries(3)
        .pool_idle_timeout(Duration::from_secs(30))
        .pool_max_idle_per_host(2)
        .build()
        .map_err(describe)
}

#[derive(Debug)]
pub enum Error {
    /// No answer: DNS, TLS, timeout, connection reset.
    Network(String),
    /// Telegram answered `ok: false`.
    Api {
        code: i64,
        description: String,
        retry_after: Option<u64>,
    },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Network(e) => write!(f, "network: {e}"),
            Error::Api {
                code, description, ..
            } => write!(f, "Telegram {code}: {description}"),
        }
    }
}

impl Error {
    /// Editing a message to the text it already has; harmless.
    pub fn not_modified(&self) -> bool {
        matches!(self, Error::Api { description, .. } if description.contains("message is not modified"))
    }
    pub fn code(&self) -> Option<i64> {
        match self {
            Error::Api { code, .. } => Some(*code),
            Error::Network(_) => None,
        }
    }
    pub fn retry_after(&self) -> Option<u64> {
        match self {
            Error::Api { retry_after, .. } => *retry_after,
            Error::Network(_) => None,
        }
    }
}

/// The error and its causes ("tcp connect error: Connection refused"), without the URL.
fn describe(error: reqwest::Error) -> String {
    let error = error.without_url();
    let mut text = error.to_string();
    let mut source = std::error::Error::source(&error);
    while let Some(cause) = source {
        text = format!("{text}: {cause}");
        source = cause.source();
    }
    text
}

#[derive(Deserialize)]
struct Reply<T> {
    ok: bool,
    result: Option<T>,
    #[serde(default)]
    description: String,
    #[serde(default)]
    error_code: i64,
    parameters: Option<ReplyParameters>,
}
#[derive(Deserialize)]
struct ReplyParameters {
    retry_after: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct Update {
    pub update_id: i64,
    pub message: Option<Message>,
    pub callback_query: Option<CallbackQuery>,
}
#[derive(Debug, Deserialize)]
pub struct Message {
    pub message_id: i64,
    pub chat: Chat,
    pub from: Option<User>,
    pub text: Option<String>,
}
#[derive(Debug, Deserialize)]
pub struct Chat {
    pub id: i64,
    #[serde(rename = "type")]
    pub kind: String,
}
#[derive(Debug, Deserialize)]
pub struct User {
    pub id: i64,
    pub username: Option<String>,
    pub first_name: Option<String>,
}
#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    pub id: String,
    pub from: User,
    pub message: Option<Message>,
    pub data: Option<String>,
}

impl Api {
    pub fn new(api: &str, token: &str) -> Result<Self, String> {
        Ok(Api {
            http: RwLock::new(http_client()?),
            base: format!("{}/bot{token}", api.trim_end_matches('/')),
        })
    }

    /// Drops every pooled connection; the next request connects afresh.
    pub fn reset(&self) {
        match http_client() {
            Ok(client) => *self.http.write().unwrap() = client,
            Err(e) => eprintln!("telegram: cannot rebuild the HTTP client: {e}"),
        }
    }

    async fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        body: &Value,
        wait: Duration,
    ) -> Result<T, Error> {
        let http = self.http.read().unwrap().clone();
        let response = http
            .post(format!("{}/{method}", self.base))
            .timeout(wait)
            .json(body)
            .send()
            .await
            .map_err(|e| Error::Network(describe(e)))?;
        let body = read_limited(response, MAX_REPLY)
            .await
            .map_err(Error::Network)?;
        let reply: Reply<T> =
            serde_json::from_slice(&body).map_err(|e| Error::Network(format!("bad reply: {e}")))?;
        match (reply.ok, reply.result) {
            (true, Some(result)) => Ok(result),
            _ => Err(Error::Api {
                code: reply.error_code,
                description: reply.description,
                retry_after: reply.parameters.and_then(|p| p.retry_after),
            }),
        }
    }

    async fn quick<T: DeserializeOwned>(&self, method: &str, body: &Value) -> Result<T, Error> {
        self.call(method, body, Duration::from_secs(20)).await
    }

    pub async fn get_me(&self) -> Result<User, Error> {
        self.quick("getMe", &json!({})).await
    }

    pub async fn get_updates(&self, offset: i64) -> Result<Vec<Update>, Error> {
        let body = json!({
            "offset": offset,
            "timeout": POLL_SECONDS,
            "allowed_updates": ["message", "callback_query"],
        });
        self.call("getUpdates", &body, Duration::from_secs(POLL_SECONDS + 10))
            .await
    }

    pub async fn send(
        &self,
        chat: i64,
        text: &str,
        keyboard: Option<Value>,
        silent: bool,
    ) -> Result<Message, Error> {
        let mut body = json!({
            "chat_id": chat,
            "text": text,
            "parse_mode": "HTML",
            "disable_notification": silent,
            "link_preview_options": {"is_disabled": true},
        });
        if let Some(keyboard) = keyboard {
            body["reply_markup"] = keyboard;
        }
        self.quick("sendMessage", &body).await
    }

    pub async fn edit(
        &self,
        chat: i64,
        message: i64,
        text: &str,
        keyboard: Option<Value>,
    ) -> Result<(), Error> {
        let mut body = json!({
            "chat_id": chat,
            "message_id": message,
            "text": text,
            "parse_mode": "HTML",
            "link_preview_options": {"is_disabled": true},
        });
        if let Some(keyboard) = keyboard {
            body["reply_markup"] = keyboard;
        }
        match self.quick::<Value>("editMessageText", &body).await {
            Err(e) if e.not_modified() => Ok(()),
            other => other.map(|_| ()),
        }
    }

    pub async fn delete(&self, chat: i64, message: i64) -> Result<(), Error> {
        self.quick::<bool>(
            "deleteMessage",
            &json!({"chat_id": chat, "message_id": message}),
        )
        .await
        .map(|_| ())
    }

    pub async fn answer(&self, callback: &str, text: &str) -> Result<(), Error> {
        let mut body = json!({"callback_query_id": callback});
        if !text.is_empty() {
            body["text"] = text.into();
        }
        self.quick::<bool>("answerCallbackQuery", &body)
            .await
            .map(|_| ())
    }

    /// The command menu next to the input field.
    pub async fn set_commands(&self, commands: &[(&str, &str)]) -> Result<(), Error> {
        let list: Vec<Value> = commands
            .iter()
            .map(|(command, description)| json!({"command": command, "description": description}))
            .collect();
        self.quick::<bool>("setMyCommands", &json!({"commands": list}))
            .await
            .map(|_| ())
    }
}
