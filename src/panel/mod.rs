//! The web panel in the home network: the player, the queue, search, likes, playlists,
//! the wave and charts, from a phone or a computer at `http://<speaker>:8765`.
//! Off unless the owner turns it on in `/admin`; every browser signs in with the PIN the
//! owner sees there. It talks to soundiya through the bot, like the chat does.
pub mod auth;
pub mod http;

use crate::{bot::Bot, soundiya::Entry};
use auth::Login;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Semaphore,
    time::{Instant, timeout},
};

const PAGE: &str = include_str!("page/index.html");
const STYLE: &str = include_str!("page/app.css");
const SCRIPT: &str = include_str!("page/app.js");
const MANIFEST: &str = include_str!("page/manifest.webmanifest");

/// Browsers served at once; more wait for a free slot.
const CONNECTIONS: usize = 16;
/// The whole request must arrive within this.
const READ_WITHIN: Duration = Duration::from_secs(10);
/// Listings wait for SoundCloud (likes come in batches).
const LISTING_WITHIN: Duration = Duration::from_secs(30);
/// Likes, playlists and charts are kept this long; searches are not.
const CACHE_FOR: Duration = Duration::from_secs(5 * 60);
/// Tracks one `play` (and the wave) puts in the queue.
const QUEUE: usize = 25;
/// How often the server checks that the owner still wants it on.
const CHECK_EVERY: Duration = Duration::from_secs(2);

/// What the panel keeps between requests.
pub struct Panel {
    pub auth: Mutex<auth::Auth>,
    cache: Mutex<HashMap<String, (Instant, String)>>,
    /// At most two listings at a time reach soundiya.
    listings: Semaphore,
    port: u16,
}

impl Panel {
    pub fn new(auth: auth::Auth, port: u16) -> Panel {
        Panel {
            auth: Mutex::new(auth),
            cache: Mutex::default(),
            listings: Semaphore::new(2),
            port,
        }
    }

    /// `http://192.168.1.50:8765`: the speaker's address in the home network.
    pub fn address(&self) -> String {
        let ip = std::net::UdpSocket::bind("0.0.0.0:0")
            .and_then(|s| {
                // No packet is sent: this only asks the kernel which address it would use.
                s.connect("192.0.2.1:9")?;
                s.local_addr()
            })
            .map_or_else(|_| "<адрес колонки>".to_owned(), |a| a.ip().to_string());
        format!("http://{ip}:{}", self.port)
    }
}

/// Runs while the owner keeps the panel on; stops listening when it is turned off.
pub async fn serve(bot: Arc<Bot>) {
    let slots = Arc::new(Semaphore::new(CONNECTIONS));
    let mut warned = false;
    loop {
        tokio::time::sleep(CHECK_EVERY).await;
        if !bot.panel_on() {
            continue;
        }
        let port = bot.panel().port;
        let listener = match TcpListener::bind(("0.0.0.0", port)).await {
            Ok(l) => l,
            Err(e) => {
                if !warned {
                    eprintln!("panel: port {port}: {e}");
                    warned = true;
                }
                tokio::time::sleep(Duration::from_secs(30)).await;
                continue;
            }
        };
        warned = false;
        eprintln!("panel: listening on port {port}");
        let mut check = tokio::time::interval(CHECK_EVERY);
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let Ok((stream, peer)) = accepted else { continue };
                    let Ok(slot) = slots.clone().acquire_owned().await else { continue };
                    let bot = bot.clone();
                    tokio::spawn(async move {
                        let _ = connection(&bot, stream, peer).await;
                        drop(slot);
                    });
                }
                _ = check.tick() => {
                    if !bot.panel_on() {
                        eprintln!("panel: turned off");
                        break;
                    }
                }
            }
        }
    }
}

async fn connection(bot: &Bot, mut stream: TcpStream, peer: SocketAddr) -> std::io::Result<()> {
    if !http::peer_allowed(peer.ip()) {
        // Not the home network (a forwarded port, say): nothing at all.
        return Ok(());
    }
    let answer = match timeout(READ_WITHIN, read_request(&mut stream)).await {
        Ok(Ok((request, body))) => route(bot, &request, &body, peer.ip()).await,
        Ok(Err(status)) => plain(status, ""),
        Err(_) => plain("408 Request Timeout", ""),
    };
    stream.write_all(&answer).await?;
    stream.shutdown().await
}

async fn read_request(stream: &mut TcpStream) -> Result<(http::Request, Vec<u8>), &'static str> {
    let mut buffer = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    let end = loop {
        let n = stream
            .read(&mut chunk)
            .await
            .map_err(|_| "400 Bad Request")?;
        if n == 0 {
            return Err("400 Bad Request");
        }
        buffer.extend_from_slice(&chunk[..n]);
        if let Some(at) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
            break at;
        }
        if buffer.len() > http::MAX_HEAD {
            return Err("431 Request Header Fields Too Large");
        }
    };
    // The end may come in the same read that went past the limit.
    if end > http::MAX_HEAD {
        return Err("431 Request Header Fields Too Large");
    }
    let head = std::str::from_utf8(&buffer[..end]).map_err(|_| "400 Bad Request")?;
    let request = http::parse_head(head).map_err(|_| "400 Bad Request")?;
    if request.length > http::MAX_BODY {
        return Err("413 Content Too Large");
    }
    let mut body = buffer[end + 4..].to_vec();
    if body.len() > request.length {
        return Err("400 Bad Request");
    }
    while body.len() < request.length {
        let n = stream
            .read(&mut chunk)
            .await
            .map_err(|_| "400 Bad Request")?;
        if n == 0 {
            return Err("400 Bad Request");
        }
        body.extend_from_slice(&chunk[..n]);
        if body.len() > request.length {
            return Err("400 Bad Request");
        }
    }
    Ok((request, body))
}

fn plain(status: &str, text: &str) -> Vec<u8> {
    http::response(status, "text/plain; charset=utf-8", text.as_bytes(), "")
}
fn json(status: &str, body: &str) -> Vec<u8> {
    http::response(status, "application/json", body.as_bytes(), "")
}
fn error(status: &str, text: &str) -> Vec<u8> {
    json(status, &serde_json::json!({ "error": text }).to_string())
}

async fn route(bot: &Bot, request: &http::Request, body: &[u8], peer: IpAddr) -> Vec<u8> {
    if !http::host_allowed(&request.host) {
        return plain("421 Misdirected Request", "");
    }
    let method = request.method.as_str();
    if method == "POST" {
        // Another site's page cannot send commands: same origin, JSON only.
        if !http::origin_allowed(request.origin.as_deref(), &request.host) {
            return error("403 Forbidden", "запрос не со страницы панели");
        }
        if !request
            .content_type
            .as_deref()
            .is_some_and(|t| t.starts_with("application/json"))
        {
            return error("415 Unsupported Media Type", "нужен JSON");
        }
    } else if method != "GET" {
        return plain("405 Method Not Allowed", "");
    }
    let panel = bot.panel();
    let signed_in = request
        .session
        .as_deref()
        .is_some_and(|s| panel.auth.lock().unwrap().session_ok(s));
    match (method, request.path.as_str()) {
        ("GET", "/") => http::response("200 OK", "text/html; charset=utf-8", PAGE.as_bytes(), ""),
        ("GET", "/app.css") => {
            http::response("200 OK", "text/css; charset=utf-8", STYLE.as_bytes(), "")
        }
        ("GET", "/app.js") => http::response(
            "200 OK",
            "text/javascript; charset=utf-8",
            SCRIPT.as_bytes(),
            "",
        ),
        ("GET", "/manifest.webmanifest") => http::response(
            "200 OK",
            "application/manifest+json",
            MANIFEST.as_bytes(),
            "",
        ),
        ("GET", "/api/me") => json(
            "200 OK",
            &serde_json::json!({ "signed_in": signed_in }).to_string(),
        ),
        ("POST", "/api/login") => login(bot, body, peer).await,
        ("POST", "/api/logout") => {
            if let Some(s) = request.session.as_deref() {
                panel.auth.lock().unwrap().logout(s);
            }
            http::response("200 OK", "application/json", b"{}", &http::clear_cookie())
        }
        (_, path) if path.starts_with("/api/") && !signed_in => {
            error("401 Unauthorized", "нужен PIN")
        }
        ("GET", "/api/state") => match bot.player().await {
            Ok(state) => json("200 OK", &page_state(&state)),
            Err(e) => error("503 Service Unavailable", &e),
        },
        ("POST", "/api/cmd") => match serde_json::from_slice::<Cmd>(body) {
            Ok(cmd) => match command(bot, cmd).await {
                Ok(text) => json("200 OK", &serde_json::json!({ "ok": text }).to_string()),
                Err(e) => error("200 OK", &e),
            },
            Err(_) => error("400 Bad Request", "непонятная команда"),
        },
        ("POST", "/api/list") => match serde_json::from_slice::<List>(body) {
            Ok(list) => match listing(bot, list).await {
                Ok(text) => json("200 OK", &text),
                Err(e) => error("200 OK", &e),
            },
            Err(_) => error("400 Bad Request", "непонятный список"),
        },
        _ => plain("404 Not Found", ""),
    }
}

#[derive(Deserialize)]
struct Pin {
    pin: String,
}

async fn login(bot: &Bot, body: &[u8], peer: IpAddr) -> Vec<u8> {
    let Ok(Pin { pin }) = serde_json::from_slice::<Pin>(body) else {
        return error("400 Bad Request", "нужен PIN");
    };
    let result = bot
        .panel()
        .auth
        .lock()
        .unwrap()
        .login(&pin, Instant::now().into_std());
    match result {
        Login::Ok(token) => {
            eprintln!("panel: signed in from {peer}");
            http::response(
                "200 OK",
                "application/json",
                b"{}",
                &http::session_cookie(&token),
            )
        }
        Login::Wrong { left } => error(
            "401 Unauthorized",
            &format!("Неверный PIN. Осталось попыток: {left}"),
        ),
        Login::JustLocked => {
            eprintln!(
                "panel: {} wrong PINs in a row, from {peer}; locked",
                auth::MAX_FAILS
            );
            bot.tell_owners(&format!(
                "⚠️ Веб-панель: {} неверных PIN подряд с адреса {peer}. Вход закрыт на {} мин. \
                 Если это не ты — смени PIN в /admin.",
                auth::MAX_FAILS,
                auth::LOCK_FOR.as_secs() / 60
            ))
            .await;
            locked(auth::LOCK_FOR)
        }
        Login::Locked(left) => locked(left),
    }
}

fn locked(left: Duration) -> Vec<u8> {
    error(
        "429 Too Many Requests",
        &format!(
            "Слишком много неверных PIN. Попробуйте через {} мин.",
            left.as_secs().div_ceil(60).max(1)
        ),
    )
}

/// One track as the page gets it: cover and page links only to SoundCloud.
#[derive(Serialize, Deserialize)]
struct Track {
    id: u64,
    artist: String,
    title: String,
    duration: u64,
    #[serde(default)]
    art: String,
    #[serde(default)]
    url: String,
}

fn safe_art(url: &str) -> String {
    let ok = url
        .strip_prefix("https://")
        .and_then(|rest| rest.split('/').next())
        .is_some_and(|host| host.ends_with(".sndcdn.com"))
        && !url.contains(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == '\\');
    if ok { url.to_owned() } else { String::new() }
}
fn safe_page(url: &str) -> String {
    let ok = url.starts_with("https://soundcloud.com/")
        && !url.contains(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == '\\');
    if ok { url.to_owned() } else { String::new() }
}

impl Track {
    fn cleaned(self) -> Track {
        Track {
            art: safe_art(&self.art),
            url: safe_page(&self.url),
            ..self
        }
    }
    fn from_entry(e: &Entry) -> Track {
        Track {
            id: e.id,
            artist: e.artist.clone(),
            title: e.title.clone(),
            duration: e.duration,
            art: e.art.clone(),
            url: e.url.clone(),
        }
        .cleaned()
    }
}

fn page_state(state: &crate::soundiya::State) -> String {
    let queue: Vec<Track> = state.queue.iter().map(Track::from_entry).collect();
    serde_json::json!({
        "state": state.state,
        "index": state.index,
        "position": state.position,
        "repeat": state.repeat,
        "sleep": state.sleep,
        "volume": state.volume,
        "voice": state.voice,
        "signed_in": state.signed_in,
        "queue": queue,
    })
    .to_string()
}

/// What the page asks the speaker to do.
#[derive(Deserialize)]
#[serde(tag = "do", rename_all = "lowercase")]
enum Cmd {
    Toggle,
    Next,
    Prev,
    /// To this second of the current track.
    Seek {
        to: u64,
    },
    Volume {
        level: u16,
    },
    Repeat {
        on: bool,
    },
    /// Minutes; 0 turns the timer off.
    Sleep {
        minutes: u32,
    },
    /// A queue entry, from 1.
    Jump {
        n: usize,
    },
    Similar,
    Likes,
    Wave,
    Chart {
        trending: bool,
        genre: Option<String>,
    },
    Playlist {
        id: u64,
    },
    /// These tracks, in this order.
    Play {
        ids: Vec<u64>,
    },
    Search {
        q: String,
    },
}

async fn command(bot: &Bot, cmd: Cmd) -> Result<String, String> {
    let (name, argument) = match cmd {
        Cmd::Toggle => {
            let playing = bot.player().await?.playing();
            (if playing { "PAUSE" } else { "RESUME" }, String::new())
        }
        Cmd::Next => ("NEXT", String::new()),
        Cmd::Prev => ("PREV", String::new()),
        Cmd::Seek { to } => {
            let state = bot.player().await?;
            let duration = state.current().map_or(0, |t| t.duration);
            let to = if duration > 0 {
                to.min(duration.saturating_sub(1))
            } else {
                to
            };
            let step = to as i64 - state.position as i64;
            if step == 0 {
                return Ok("ok".into());
            }
            ("SEEK", step.clamp(-3600, 3600).to_string())
        }
        Cmd::Volume { level } => ("VOLUME", level.min(100).to_string()),
        Cmd::Repeat { on } => ("REPEAT", if on { "on" } else { "off" }.to_owned()),
        Cmd::Sleep { minutes: 0 } => ("SLEEP", "off".to_owned()),
        Cmd::Sleep { minutes } if minutes <= 720 => ("SLEEP", minutes.to_string()),
        Cmd::Sleep { .. } => return Err("таймер — до 12 часов".into()),
        Cmd::Jump { n } if n >= 1 => ("JUMP", n.to_string()),
        Cmd::Jump { .. } => return Err("нет такого трека в очереди".into()),
        Cmd::Similar => ("RELATED", String::new()),
        Cmd::Likes => ("FAV", String::new()),
        Cmd::Wave => ("PLAYIDS", ids_argument(&wave(bot).await?)),
        Cmd::Chart { trending, genre } => (
            if trending { "TRENDING" } else { "TOP" },
            genre.unwrap_or_default(),
        ),
        Cmd::Playlist { id } => ("OPENPL", id.to_string()),
        Cmd::Play { ids } if !ids.is_empty() => ("PLAYIDS", ids_argument(&ids)),
        Cmd::Play { .. } => return Err("нечего включать".into()),
        Cmd::Search { q } if !q.trim().is_empty() => ("PLAY", q),
        Cmd::Search { .. } => return Err("пустой запрос".into()),
    };
    bot.soundiya(name, &argument)
        .await
        .map_err(|e| unsupported(&e))
}

/// Ids for `PLAYIDS`, as many as fit the one-line protocol.
fn ids_argument(ids: &[u64]) -> String {
    let mut out = String::new();
    for id in ids.iter().take(QUEUE) {
        let next = if out.is_empty() {
            id.to_string()
        } else {
            format!(",{id}")
        };
        if out.len() + next.len() > 290 {
            break;
        }
        out += &next;
    }
    out
}

/// An older soundiya without the panel commands says so plainly.
fn unsupported(error: &str) -> String {
    if error == "unknown command" {
        "эта версия soundiya не умеет это — обнови её в /updates".to_owned()
    } else {
        error.to_owned()
    }
}

/// What the page lists.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum List {
    Likes,
    Playlists,
    Chart {
        trending: bool,
        genre: Option<String>,
    },
    Find {
        q: String,
    },
    Similar {
        id: u64,
    },
}

async fn listing(bot: &Bot, list: List) -> Result<String, String> {
    let (name, argument, cached) = match list {
        List::Likes => ("LIKES", String::new(), true),
        List::Playlists => ("PLAYLISTS", String::new(), true),
        List::Chart { trending, genre } => (
            "CHART",
            format!(
                "{} {}",
                if trending { "trending" } else { "top" },
                genre.unwrap_or_default()
            ),
            true,
        ),
        List::Find { q } if !q.trim().is_empty() => ("FIND", q, false),
        List::Find { .. } => return Err("пустой запрос".into()),
        List::Similar { id } => ("SIMILAR", id.to_string(), true),
    };
    let key = format!("{name} {argument}");
    if cached
        && let Some((at, text)) = bot.panel().cache.lock().unwrap().get(&key)
        && at.elapsed() < CACHE_FOR
    {
        return Ok(text.clone());
    }
    let text = raw_listing(bot, name, &argument).await?;
    let clean = if name == "PLAYLISTS" {
        clean_playlists(&text)?
    } else {
        clean_tracks(&text)?
    };
    if cached {
        let mut cache = bot.panel().cache.lock().unwrap();
        cache.retain(|_, (at, _)| at.elapsed() < CACHE_FOR);
        cache.insert(key, (Instant::now(), clean.clone()));
    }
    Ok(clean)
}

async fn raw_listing(bot: &Bot, name: &str, argument: &str) -> Result<String, String> {
    let _slot = bot
        .panel()
        .listings
        .acquire()
        .await
        .map_err(|e| e.to_string())?;
    bot.soundiya_slow(name, argument, LISTING_WITHIN)
        .await
        .map_err(|e| match e.as_str() {
            e if e.starts_with("not signed in") => {
                "нужен вход в SoundCloud: soundiya login".to_owned()
            }
            e => unsupported(e),
        })
}

fn clean_tracks(text: &str) -> Result<String, String> {
    let tracks: Vec<Track> =
        serde_json::from_str(text).map_err(|_| "непонятный ответ soundiya".to_owned())?;
    let tracks: Vec<Track> = tracks.into_iter().map(Track::cleaned).collect();
    serde_json::to_string(&tracks).map_err(|e| e.to_string())
}

#[derive(Serialize, Deserialize)]
struct Playlist {
    id: u64,
    title: String,
    count: u64,
    #[serde(default)]
    art: String,
    #[serde(default)]
    user: String,
}

fn clean_playlists(text: &str) -> Result<String, String> {
    let lists: Vec<Playlist> =
        serde_json::from_str(text).map_err(|_| "непонятный ответ soundiya".to_owned())?;
    let lists: Vec<Playlist> = lists
        .into_iter()
        .map(|p| Playlist {
            art: safe_art(&p.art),
            ..p
        })
        .collect();
    serde_json::to_string(&lists).map_err(|e| e.to_string())
}

/// The wave: tracks like a few of the likes (or like the current track), new to the
/// listener, mixed so that no one source comes in a row.
async fn wave(bot: &Bot) -> Result<Vec<u64>, String> {
    let likes: Vec<Track> = match listing(bot, List::Likes).await {
        Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
        Err(_) => Vec::new(),
    };
    let mut seeds: Vec<u64> = likes.iter().take(60).map(|t| t.id).collect();
    shuffle(&mut seeds);
    seeds.truncate(5);
    if seeds.is_empty() {
        let state = bot.player().await?;
        seeds.extend(state.current().map(|t| t.id));
    }
    if seeds.is_empty() {
        return Err("для волны нужен вход в SoundCloud с лайками или играющий трек".into());
    }
    let mut lists: Vec<Vec<u64>> = Vec::new();
    for seed in &seeds {
        if let Ok(text) = listing(bot, List::Similar { id: *seed }).await {
            let similar: Vec<Track> = serde_json::from_str(&text).unwrap_or_default();
            lists.push(similar.into_iter().map(|t| t.id).collect());
        }
    }
    let known: std::collections::HashSet<u64> = likes
        .iter()
        .map(|t| t.id)
        .chain(seeds.iter().copied())
        .collect();
    let mixed = interleave(&lists, &known, QUEUE);
    if mixed.is_empty() {
        return Err("SoundCloud не подобрал похожих треков, попробуйте ещё раз".into());
    }
    Ok(mixed)
}

/// Takes from each list in turn, skipping known and repeated tracks.
fn interleave(
    lists: &[Vec<u64>],
    known: &std::collections::HashSet<u64>,
    limit: usize,
) -> Vec<u64> {
    let mut out: Vec<u64> = Vec::new();
    let longest = lists.iter().map(Vec::len).max().unwrap_or(0);
    for i in 0..longest {
        for list in lists {
            if let Some(&id) = list.get(i)
                && !known.contains(&id)
                && !out.contains(&id)
            {
                out.push(id);
                if out.len() == limit {
                    return out;
                }
            }
        }
    }
    out
}

fn shuffle<T>(items: &mut [T]) {
    for i in (1..items.len()).rev() {
        let Ok(j) = auth::random_below(i as u32 + 1) else {
            return;
        };
        items.swap(i, j as usize);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covers_and_pages_only_from_soundcloud() {
        assert_eq!(
            safe_art("https://i1.sndcdn.com/artworks-1-large.jpg"),
            "https://i1.sndcdn.com/artworks-1-large.jpg"
        );
        for bad in [
            "http://i1.sndcdn.com/a.jpg",
            "https://evil.com/a.jpg",
            "https://sndcdn.com.evil.com/a",
            "javascript:alert(1)",
            "https://i1.sndcdn.com/a\".jpg",
            "",
        ] {
            assert_eq!(safe_art(bad), "", "{bad}");
        }
        assert_eq!(
            safe_page("https://soundcloud.com/a/b"),
            "https://soundcloud.com/a/b"
        );
        for bad in [
            "https://soundcloud.com.evil/a",
            "http://soundcloud.com/a",
            "javascript:x",
            "https://soundcloud.com/a b",
        ] {
            assert_eq!(safe_page(bad), "", "{bad}");
        }
    }

    #[test]
    fn listings_are_rebuilt_not_passed_through() {
        let text = r#"[{"id":1,"artist":"A","title":"<b>T</b>","duration":10,"art":"https://evil.com/x.jpg","url":"https://soundcloud.com/a/t","extra":"x"}]"#;
        let out = clean_tracks(text).unwrap();
        assert_eq!(
            out,
            r#"[{"id":1,"artist":"A","title":"<b>T</b>","duration":10,"art":"","url":"https://soundcloud.com/a/t"}]"#
        );
        assert!(clean_tracks("not json").is_err());
        let lists = clean_playlists(
            r#"[{"id":5,"title":"P","count":3,"art":"https://i1.sndcdn.com/p.jpg","user":"U"}]"#,
        )
        .unwrap();
        assert!(lists.contains("https://i1.sndcdn.com/p.jpg"));
    }

    #[test]
    fn commands_are_typed() {
        for ok in [
            r#"{"do":"toggle"}"#,
            r#"{"do":"seek","to":30}"#,
            r#"{"do":"volume","level":40}"#,
            r#"{"do":"play","ids":[1,2]}"#,
            r#"{"do":"chart","trending":true,"genre":"рок"}"#,
            r#"{"do":"search","q":"bonobo"}"#,
        ] {
            assert!(serde_json::from_str::<Cmd>(ok).is_ok(), "{ok}");
        }
        for bad in [
            r#"{"do":"rm"}"#,
            r#"{"do":"volume","level":-1}"#,
            r#"{"do":"seek","to":"x"}"#,
            r#"{}"#,
            "[]",
        ] {
            assert!(serde_json::from_str::<Cmd>(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn playids_fit_the_protocol() {
        let ids: Vec<u64> = (0..40).map(|i| 9_000_000_000 + i).collect();
        let arg = ids_argument(&ids);
        assert!(arg.len() <= 290);
        assert_eq!(arg.split(',').count(), 26.min(QUEUE).min(290 / 11));
        assert_eq!(ids_argument(&[1, 2, 3]), "1,2,3");
    }

    #[test]
    fn the_wave_mixes_sources_and_skips_known_tracks() {
        let known: std::collections::HashSet<u64> = [1, 2].into_iter().collect();
        let lists = vec![vec![1, 10, 11, 12], vec![20, 10, 21], vec![30]];
        assert_eq!(interleave(&lists, &known, 25), [20, 30, 10, 11, 21, 12]);
        assert_eq!(interleave(&lists, &known, 2), [20, 30]);
        let mut v: Vec<u32> = (0..50).collect();
        shuffle(&mut v);
        let mut sorted = v.clone();
        sorted.sort();
        assert_eq!(sorted, (0..50).collect::<Vec<_>>());
    }
}
