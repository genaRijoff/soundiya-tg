//! The soundiya command socket: one line in, `OK ...` or `ERROR ...` back.
use serde::Deserialize;
use std::{path::Path, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    time::timeout,
};

pub const DEFAULT_SOCKET: &str = "/run/soundiya.control";
const TIMEOUT: Duration = Duration::from_secs(5);
const MAX_REPLY: u64 = 256 * 1024;
const MAX_ARGUMENT: usize = 300;

/// What `soundiya state` reports; see `StateView` in soundiya.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct State {
    pub state: String,
    /// The playing queue entry, from 1; 0 when nothing of ours plays.
    pub index: usize,
    pub position: u64,
    pub repeat: bool,
    pub sleep: Option<u64>,
    pub signed_in: bool,
    /// Percent; missing in soundiya builds before the volume step.
    #[serde(default)]
    pub volume: Option<u16>,
    /// A voice message is playing; missing in builds before `VOICE`.
    #[serde(default)]
    pub voice: bool,
    pub queue: Vec<Entry>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct Entry {
    pub id: u64,
    pub artist: String,
    pub title: String,
    pub duration: u64,
}

impl State {
    pub fn current(&self) -> Option<&Entry> {
        self.index.checked_sub(1).and_then(|i| self.queue.get(i))
    }
    pub fn playing(&self) -> bool {
        self.state == "playing"
    }
}

/// Makes user text safe for the one-line protocol: no line breaks or control characters,
/// bounded length.
pub fn clean_argument(text: &str) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let words: Vec<&str> = flat.split_whitespace().collect();
    words.join(" ").chars().take(MAX_ARGUMENT).collect()
}

pub async fn command(socket: &Path, command: &str, argument: &str) -> Result<String, String> {
    let line = format!("{command} {}\n", clean_argument(argument));
    let exchange = async {
        let mut stream = UnixStream::connect(socket)
            .await
            .map_err(|_| "soundiya не запущен".to_owned())?;
        stream
            .write_all(line.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        stream.shutdown().await.map_err(|e| e.to_string())?;
        let mut reply = String::new();
        stream
            .take(MAX_REPLY)
            .read_to_string(&mut reply)
            .await
            .map_err(|e| e.to_string())?;
        Ok::<_, String>(reply)
    };
    let reply = timeout(TIMEOUT, exchange)
        .await
        .map_err(|_| "soundiya не отвечает".to_owned())??;
    parse_reply(&reply)
}

fn parse_reply(reply: &str) -> Result<String, String> {
    let reply = reply.trim_end();
    if let Some(text) = reply.strip_prefix("OK") {
        Ok(text.trim_start().to_owned())
    } else if let Some(text) = reply.strip_prefix("ERROR") {
        Err(text.trim_start().to_owned())
    } else {
        Err("непонятный ответ soundiya".into())
    }
}

pub async fn state(socket: &Path) -> Result<State, String> {
    let text = command(socket, "STATE", "").await.map_err(|e| {
        if e == "unknown command" {
            "эта версия soundiya не знает команду STATE — нужна версия с поддержкой бота".to_owned()
        } else {
            e
        }
    })?;
    if text.is_empty() {
        // Started a moment ago: the first status is written on the next tick.
        return Ok(State {
            state: "idle".into(),
            ..State::default()
        });
    }
    serde_json::from_str(&text).map_err(|e| format!("STATE: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{io::AsyncBufReadExt, net::UnixListener};

    #[test]
    fn arguments_stay_on_one_line() {
        assert_eq!(
            clean_argument("  Daft\nPunk\r\n\tone  more "),
            "Daft Punk one more"
        );
        assert_eq!(
            clean_argument(&"я".repeat(1000)).chars().count(),
            MAX_ARGUMENT
        );
    }

    #[test]
    fn replies_are_parsed() {
        assert_eq!(parse_reply("OK sent\n"), Ok("sent".into()));
        assert_eq!(parse_reply("OK \n"), Ok(String::new()));
        assert_eq!(
            parse_reply("ERROR the receiver is busy\n"),
            Err("the receiver is busy".into())
        );
        assert!(parse_reply("garbage").is_err());
    }

    #[test]
    fn state_json_from_soundiya_is_understood() {
        let text = r#"{"state":"playing","index":2,"position":61,"repeat":false,"sleep":30,"signed_in":true,"queue":[{"id":1,"artist":"A","title":"One","duration":200},{"id":2,"artist":"B","title":"Two","duration":0}]}"#;
        let state: State = serde_json::from_str(text).unwrap();
        assert!(state.playing());
        assert_eq!(state.current().unwrap().title, "Two");
        assert_eq!(state.sleep, Some(30));
        assert_eq!(state.volume, None, "soundiya without the volume field");
        let with_volume: State = serde_json::from_str(
            &text.replace(r#""signed_in":true,"#, r#""signed_in":true,"volume":40,"#),
        )
        .unwrap();
        assert_eq!(with_volume.volume, Some(40));
        let idle = State { index: 0, ..state };
        assert!(idle.current().is_none());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn talks_to_the_socket() {
        let path =
            std::env::temp_dir().join(format!("soundiya-tg-test-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = socket.into_split();
            let mut line = String::new();
            tokio::io::BufReader::new(reader)
                .read_line(&mut line)
                .await
                .unwrap();
            writer
                .write_all(format!("OK got {}", line.trim()).as_bytes())
                .await
                .unwrap();
        });
        let reply = command(&path, "PLAY", "a\nb").await;
        server.await.unwrap();
        assert_eq!(reply, Ok("got PLAY a b".into()));
        let _ = std::fs::remove_file(&path);
        assert_eq!(
            command(&path, "STATE", "").await,
            Err("soundiya не запущен".into())
        );
    }
}
