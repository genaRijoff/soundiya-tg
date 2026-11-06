//! Telegram updates in, soundiya commands out; a watcher keeps the player cards current.
use crate::{
    access::{self, Access, Role},
    admin,
    config::{Config, VoiceFrom},
    soundiya::{self, State},
    telegram::{Api, CallbackQuery, Error, Message, Update, User},
    updates::{self, GitHub, Release, Seen},
    view, voice,
};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{sync::Notify, time::Instant};

/// How often the watcher reads soundiya's state.
const WATCH_EVERY: Duration = Duration::from_secs(1);
/// While a track plays, its card's progress bar is redrawn this often.
const PROGRESS_EVERY: Duration = Duration::from_secs(15);
/// Longest pause between attempts to reach Telegram, so the bot is back soon after the network.
const MAX_BACKOFF: u64 = 15;
/// After a jump, the queue message is redrawn once the new track is current.
const JUMP_REDRAW_AFTER: Duration = Duration::from_millis(900);

pub struct Bot {
    api: Api,
    config: Config,
    socket: PathBuf,
    /// The live player card in each chat, which the watcher edits.
    cards: Mutex<HashMap<i64, i64>>,
    /// Chats whose card shows the sleep timer choices instead of the controls.
    sleep_menus: Mutex<HashSet<i64>>,
    /// Chats that asked for a fresh card at the bottom of the chat.
    wanted: Mutex<HashSet<i64>>,
    wake: Notify,
    github: Option<GitHub>,
    /// What was already reported about releases and the watched issue.
    seen_path: PathBuf,
    /// An install or rollback is running: a second press waits for it.
    updating: AtomicBool,
    /// A voice message is on its way to the speaker: the next one waits.
    voice_busy: AtomicBool,
    /// Listeners the owners let in, and where they are kept.
    access: Mutex<Access>,
    access_path: PathBuf,
    /// Strangers who asked for access since the start, with their names; each asks once.
    requests: Mutex<HashMap<i64, Request>>,
    last_action: Mutex<Option<Action>>,
}

/// More pending requests than this are ignored, so strangers cannot flood the owners.
const MAX_REQUESTS: usize = 50;
/// An unanswered request may be repeated after this; a refused one after `REFUSED_FOR`.
const ASK_AGAIN: Duration = Duration::from_secs(10 * 60);
const REFUSED_FOR: Duration = Duration::from_secs(60 * 60);

struct Request {
    name: String,
    at: Instant,
    refused: bool,
}
impl Request {
    fn expired(&self) -> bool {
        self.at.elapsed() >= if self.refused { REFUSED_FOR } else { ASK_AGAIN }
    }
}
/// A bot command counts as the cause of a track change this long after it was given.
const ACTION_FRESH: Duration = Duration::from_secs(30);

/// The last bot command that may change the track, and who gave it.
struct Action {
    user: i64,
    at: Instant,
    /// A search, playlist, chart or likes: a new queue that this user started.
    loads: bool,
}

/// Who gets the new-track card with a sound (`loud`) and who gets it silently (`quiet`).
/// `lead` started the queue from the bot (None: by voice); `actor` just switched the track.
fn recipients(
    owners: &[i64],
    lead: Option<i64>,
    actor: Option<i64>,
    muted: &[i64],
) -> (Vec<i64>, Vec<i64>) {
    let mut loud: Vec<i64> = lead.map_or_else(|| owners.to_vec(), |u| vec![u]);
    if let Some(a) = actor.filter(|a| !loud.contains(a)) {
        loud.push(a);
    }
    let quiet = owners
        .iter()
        .copied()
        .filter(|o| !loud.contains(o))
        .collect();
    let audible = |list: Vec<i64>| {
        list.into_iter()
            .filter(|u| !muted.contains(u))
            .collect::<Vec<_>>()
    };
    (audible(loud), audible(quiet))
}

/// Tells one queue from another: a new one means a new search, playlist or chart.
fn queue_signature(state: &Result<State, String>) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    let state = state.as_ref().ok()?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for entry in &state.queue {
        entry.id.hash(&mut hasher);
    }
    Some(hasher.finish())
}

/// First update check after start, then this often.
const UPDATES_FIRST: Duration = Duration::from_secs(120);
const UPDATES_EVERY: Duration = Duration::from_secs(6 * 3600);

/// What the card shows, apart from the progress: a change means a redraw now.
fn look(state: &Result<State, String>) -> String {
    match state {
        Ok(s) => format!(
            "{}|{}|{}|{:?}|{}|{:?}",
            s.state,
            s.index,
            s.repeat,
            s.sleep,
            s.queue.len(),
            s.volume
        ),
        Err(e) => format!("error|{e}"),
    }
}

fn track(state: &Result<State, String>) -> Option<(usize, u64)> {
    state
        .as_ref()
        .ok()
        .and_then(|s| s.current().map(|t| (s.index, t.id)))
}

impl Bot {
    pub fn new(
        api: Api,
        config: Config,
        socket: PathBuf,
        seen_path: PathBuf,
        access_path: PathBuf,
    ) -> Self {
        let github = config
            .updates
            .then(|| GitHub::new(&config))
            .and_then(|g| g.map_err(|e| eprintln!("updates: {e}")).ok());
        Bot {
            access: Mutex::new(Access::load(&access_path)),
            access_path,
            requests: Mutex::default(),
            last_action: Mutex::default(),
            github,
            seen_path,
            updating: AtomicBool::new(false),
            voice_busy: AtomicBool::new(false),
            api,
            config,
            socket,
            cards: Mutex::default(),
            sleep_menus: Mutex::default(),
            wanted: Mutex::default(),
            wake: Notify::new(),
        }
    }

    async fn state(&self) -> Result<State, String> {
        soundiya::state(&self.socket).await
    }

    async fn command(&self, command: &str, argument: &str) -> Result<String, String> {
        soundiya::command(&self.socket, command, argument).await
    }

    fn want_card(&self, chat: i64) {
        self.wanted.lock().unwrap().insert(chat);
        self.wake.notify_one();
    }

    fn card_for(&self, chat: i64, state: &Result<State, String>) -> (String, serde_json::Value) {
        let (text, keyboard) = view::card(state);
        let menu = self.sleep_menus.lock().unwrap().contains(&chat);
        match state {
            Ok(s) if menu && s.current().is_some() => (text, view::sleep_keyboard(s)),
            _ => (text, keyboard),
        }
    }

    // ---- the watcher ----

    pub async fn watch(self: Arc<Self>) {
        let mut tick = tokio::time::interval(WATCH_EVERY);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut last_track = None;
        let mut last_look = String::new();
        let mut last_draw = Instant::now();
        // Who started the playing queue from the bot; None when it was started by voice.
        let (mut last_queue, mut lead) = (None, None);
        loop {
            tokio::select! {
                _ = tick.tick() => {}
                () = self.wake.notified() => {}
            }
            let state = self.state().await;
            let (now_track, now_look) = (track(&state), look(&state));
            let wanted: Vec<i64> = self.wanted.lock().unwrap().drain().collect();
            if now_track.is_some() && now_track != last_track {
                // A new track: a new card, so the chat shows (and rings for) what plays now.
                let action = self
                    .last_action
                    .lock()
                    .unwrap()
                    .take()
                    .filter(|a| a.at.elapsed() < ACTION_FRESH);
                let queue = queue_signature(&state);
                if queue != last_queue {
                    lead = action.as_ref().filter(|a| a.loads).map(|a| a.user);
                    last_queue = queue;
                }
                let actor = action
                    .map(|a| a.user)
                    .filter(|&u| self.role(u) != Role::Stranger);
                let lead_user = lead.filter(|&u| self.role(u) != Role::Stranger);
                let muted = self.access.lock().unwrap().muted.clone();
                let (loud, quiet) = recipients(&self.config.users, lead_user, actor, &muted);
                for user in loud {
                    self.new_card(user, &state, !self.config.notify).await;
                }
                for user in quiet {
                    self.new_card(user, &state, true).await;
                }
                last_draw = Instant::now();
            } else {
                for chat in wanted {
                    self.new_card(chat, &state, true).await;
                }
                let playing = state.as_ref().is_ok_and(State::playing);
                if now_look != last_look || (playing && last_draw.elapsed() >= PROGRESS_EVERY) {
                    self.redraw_cards(&state).await;
                    last_draw = Instant::now();
                }
            }
            last_track = now_track;
            last_look = now_look;
        }
    }

    async fn new_card(&self, chat: i64, state: &Result<State, String>, silent: bool) {
        self.sleep_menus.lock().unwrap().remove(&chat);
        let old = self.cards.lock().unwrap().remove(&chat);
        if let Some(old) = old {
            let _ = self.api.delete(chat, old).await;
        }
        let (text, keyboard) = self.card_for(chat, state);
        match self.api.send(chat, &text, Some(keyboard), silent).await {
            Ok(message) => {
                self.cards.lock().unwrap().insert(chat, message.message_id);
            }
            // 403: the user has not pressed Start in the bot yet, or blocked it.
            Err(e) => eprintln!("telegram: card for {chat}: {e}"),
        }
    }

    async fn redraw_cards(&self, state: &Result<State, String>) {
        let cards: Vec<(i64, i64)> = self
            .cards
            .lock()
            .unwrap()
            .iter()
            .map(|(&c, &m)| (c, m))
            .collect();
        for (chat, message) in cards {
            let (text, keyboard) = self.card_for(chat, state);
            if let Err(e) = self.api.edit(chat, message, &text, Some(keyboard)).await {
                eprintln!("telegram: redraw card in {chat}: {e}");
                if e.code() == Some(400) {
                    // Deleted by the user; /now brings a new one.
                    self.cards.lock().unwrap().remove(&chat);
                }
            }
        }
    }

    // ---- updates ----

    /// Long-polls Telegram until the token turns out to be wrong.
    pub async fn poll(self: Arc<Self>) -> Result<(), String> {
        let mut offset = 0;
        let mut backoff = 1;
        loop {
            match self.api.get_updates(offset).await {
                Ok(updates) => {
                    backoff = 1;
                    for update in updates {
                        offset = update.update_id + 1;
                        self.clone().handle(update).await;
                    }
                }
                Err(e) if matches!(e.code(), Some(401 | 404)) => {
                    return Err(format!("{e} (check the token in config.json)"));
                }
                Err(e) => {
                    if e.code().is_none() {
                        // No answer at all: start over with fresh connections.
                        self.api.reset();
                    }
                    let wait = e.retry_after().unwrap_or(backoff);
                    eprintln!("telegram: {e}; retrying in {wait} s");
                    tokio::time::sleep(Duration::from_secs(wait)).await;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                }
            }
        }
    }

    async fn handle(self: Arc<Self>, update: Update) {
        if let Some(message) = update.message {
            if message.voice.is_some() {
                return self.on_voice(message).await;
            }
            self.on_message(message).await;
        } else if let Some(query) = update.callback_query {
            self.on_callback(query).await;
        }
    }

    async fn reply(&self, chat: i64, text: &str) {
        if let Err(e) = self.api.send(chat, text, None, true).await {
            eprintln!("telegram: reply in {chat}: {e}");
        }
    }

    async fn on_message(&self, message: Message) {
        let Some(from) = message.from.as_ref() else {
            return;
        };
        let (user, chat) = (from.id, message.chat.id);
        // Private chats only: in a group anyone could press the buttons.
        if message.chat.kind != "private" {
            eprintln!(
                "bot: ignored a message from user {user} in a {} chat",
                message.chat.kind
            );
            return;
        }
        let role = self.role(user);
        if role == Role::Stranger {
            return self.stranger(from, chat).await;
        }
        let Some(text) = message
            .text
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
        else {
            return;
        };
        let Some(command) = text.strip_prefix('/') else {
            // Plain text is a search or a soundcloud.com link.
            return self.play(chat, "PLAY", text).await;
        };
        let (name, argument) = command
            .split_once(char::is_whitespace)
            .unwrap_or((command, ""));
        let name = name.split('@').next().unwrap_or("").to_ascii_lowercase();
        let argument = argument.trim();
        let control = |command: &'static str| (command, true);
        let (command, card) = match name.as_str() {
            "start" | "help" => {
                let owner = if role == Role::Owner {
                    view::OWNER_HELP
                } else {
                    ""
                };
                self.reply(chat, &format!("{}{owner}", view::HELP)).await;
                return self.want_card(chat);
            }
            "updates" | "users" | "allow" | "deny" if role != Role::Owner => {
                return self.reply(chat, "Это может только владелец колонки.").await;
            }
            "users" => return self.send_users(chat).await,
            "notify" => return self.notify_command(chat, argument).await,
            "voice" | "voicevol" | "admin" if role != Role::Owner => {
                return self.reply(chat, "Это может только владелец колонки.").await;
            }
            "voicevol" => return self.voicevol_command(chat, argument).await,
            "voice" => return self.voice_command(chat, argument).await,
            "admin" => return self.send_admin(chat).await,
            "allow" => return self.allow_command(chat, argument).await,
            "deny" => return self.deny_command(chat, argument).await,
            "now" => return self.want_card(chat),
            "queue" => return self.send_queue(chat).await,
            "updates" => return self.updates_status(chat).await,
            "play" if argument.is_empty() => {
                return self.reply(chat, "Напиши: /play исполнитель или трек").await;
            }
            "play" => return self.play(chat, "PLAY", argument).await,
            "playlist" if argument.is_empty() => {
                return self.reply(chat, "Напиши: /playlist название").await;
            }
            "playlist" => return self.play(chat, "PLAYLIST", argument).await,
            "fav" => return self.play(chat, "FAV", "").await,
            "top" => return self.play(chat, "TOP", argument).await,
            "trending" => return self.play(chat, "TRENDING", argument).await,
            "related" => return self.play(chat, "RELATED", "").await,
            "next" => control("NEXT"),
            "prev" => control("PREV"),
            "pause" => control("PAUSE"),
            "resume" => control("RESUME"),
            "restart" => control("RESTART"),
            "seek" => control("SEEK"),
            "repeat" => control("REPEAT"),
            "sleep" => control("SLEEP"),
            "vol" | "volume" => ("VOLUME", false),
            _ => return self.reply(chat, "Не знаю такую команду. /help").await,
        };
        match self.command(command, argument).await {
            Ok(answer) if !card => {
                self.reply(chat, &format!("✅ {}", view::esc(&answer)))
                    .await
            }
            Ok(_) => {
                self.note(chat, false);
                self.want_card(chat);
            }
            Err(e) => self.reply(chat, &format!("⚠️ {}", view::esc(&e))).await,
        }
    }

    // ---- voice messages ----

    async fn on_voice(self: Arc<Self>, message: Message) {
        let (Some(from), Some(voice)) = (message.from.as_ref(), message.voice.as_ref()) else {
            return;
        };
        let (user, chat) = (from.id, message.chat.id);
        if message.chat.kind != "private" {
            return;
        }
        let role = self.role(user);
        let (mode, marked) = {
            let access = self.access.lock().unwrap();
            let mode = access.voice_from.unwrap_or(self.config.voice);
            let marked = access
                .listeners
                .iter()
                .find(|l| l.id == user)
                .is_some_and(|l| admin::listener_voice(l, mode));
            (mode, marked)
        };
        match (role, mode) {
            (Role::Stranger, _) => return self.stranger(from, chat).await,
            (_, VoiceFrom::Off) => {
                return self.reply(chat, "🎙 Голосовые на колонку выключены.").await;
            }
            (Role::Listener, _) if !marked => {
                return self
                    .reply(
                        chat,
                        "🎙 Голосовые на колонку тебе не разрешены — попроси владельца.",
                    )
                    .await;
            }
            _ => {}
        }
        if let Err(text) = voice::limits(voice.duration, voice.file_size) {
            return self.reply(chat, text).await;
        }
        if self.voice_busy.swap(true, Ordering::AcqRel) {
            return self
                .reply(chat, "⏳ Ещё играет предыдущее голосовое.")
                .await;
        }
        let status = self
            .api
            .send(chat, "🎙 Включаю голосовое…", None, true)
            .await
            .ok()
            .map(|m| m.message_id);
        let (file_id, seconds) = (voice.file_id.clone(), voice.duration);
        let id = format!("v{chat}-{}", message.message_id);
        let name = from
            .first_name
            .clone()
            .or_else(|| from.username.clone())
            .unwrap_or_else(|| user.to_string());
        // Playing takes up to a minute: other updates go on meanwhile.
        tokio::spawn(async move {
            let result = self.play_voice(&file_id, &id, seconds).await;
            self.voice_busy.store(false, Ordering::Release);
            let text = match &result {
                Ok(()) => format!("🔊 Голосовое проиграно ({seconds} с)."),
                Err(e) => format!("⚠️ Голосовое: {}", view::esc(e)),
            };
            match status {
                Some(id) => {
                    if let Err(e) = self.api.edit(chat, id, &text, None).await {
                        eprintln!("telegram: voice status in {chat}: {e}");
                    }
                }
                None => self.reply(chat, &text).await,
            }
            if result.is_ok() && role == Role::Listener {
                let note = format!(
                    "🎙 {} включил(а) голосовое на колонке ({seconds} с).",
                    view::esc(&name)
                );
                for &owner in &self.config.users {
                    let _ = self.api.send(owner, &note, None, true).await;
                }
            }
        });
    }

    /// Download, decode, hand to soundiya and wait until it has played. The files are gone
    /// when this returns, whatever happened.
    async fn play_voice(&self, file_id: &str, id: &str, seconds: u64) -> Result<(), String> {
        let file = self
            .api
            .get_file(file_id)
            .await
            .map_err(|e| e.to_string())?;
        let path = file.file_path.ok_or("Telegram не отдал файл")?;
        let data = self
            .api
            .download(&path, voice::MAX_FILE as usize)
            .await
            .map_err(|e| e.to_string())?;
        let files = voice::Files::new(id);
        files.write_ogg(&data)?;
        files.decode(&self.config.gst).await?;
        let percent = self.access.lock().unwrap().voice_volume();
        files.level(percent)?;
        let pcm = files.pcm.to_str().ok_or("bad path")?;
        self.command("VOICE", pcm).await.map_err(|e| {
            if e == "unknown command" {
                "эта soundiya не умеет голосовые — обнови её через /updates".to_owned()
            } else {
                e
            }
        })?;
        // soundiya has read and deleted the file; it reports `voice` until the message is over.
        let deadline = Instant::now() + Duration::from_secs(seconds + 15);
        tokio::time::sleep(Duration::from_millis(500)).await;
        while Instant::now() < deadline {
            match self.state().await {
                Ok(state) if !state.voice => return Ok(()),
                _ => tokio::time::sleep(Duration::from_millis(500)).await,
            }
        }
        Ok(())
    }

    /// Starts something new; the card follows when the first track starts.
    async fn play(&self, chat: i64, command: &str, argument: &str) {
        let what = match command {
            "FAV" => "❤️ Включаю лайки…".to_owned(),
            "RELATED" => "🔀 Ищу похожие…".to_owned(),
            "TOP" | "TRENDING" => "📈 Включаю чарт…".to_owned(),
            _ => format!(
                "🔎 Ищу «{}»…",
                view::esc(&soundiya::clean_argument(argument))
            ),
        };
        match self.command(command, argument).await {
            Ok(_) => {
                self.note(chat, true);
                self.reply(chat, &what).await;
            }
            Err(e) => self.reply(chat, &format!("⚠️ {}", view::esc(&e))).await,
        }
    }

    async fn send_queue(&self, chat: i64) {
        match self.state().await {
            Ok(state) => {
                let (text, keyboard) = view::queue(&state, view::page_of_current(&state));
                if let Err(e) = self.api.send(chat, &text, Some(keyboard), true).await {
                    eprintln!("telegram: queue in {chat}: {e}");
                }
            }
            Err(e) => self.reply(chat, &format!("⚠️ {}", view::esc(&e))).await,
        }
    }

    async fn answer(&self, query: &CallbackQuery, text: &str) {
        if let Err(e) = self.api.answer(&query.id, text).await {
            eprintln!("telegram: answer: {e}");
        }
    }

    async fn on_callback(self: Arc<Self>, query: CallbackQuery) {
        let role = self.role(query.from.id);
        if role == Role::Stranger {
            // Old buttons of someone whose access was taken away, or a forged callback.
            return self
                .answer(
                    &query,
                    "🔒 Нет доступа. Напиши боту сообщение, чтобы попросить его.",
                )
                .await;
        }
        let (Some(message), Some(data)) = (query.message.as_ref(), query.data.as_deref()) else {
            return self.answer(&query, "").await;
        };
        let (chat, message_id) = (message.chat.id, message.message_id);
        let (kind, value) = data.split_once(':').unwrap_or((data, ""));
        if matches!(kind, "u" | "a" | "x") && role != Role::Owner {
            return self.answer(&query, "⛔ Только владелец").await;
        }
        match kind {
            "c" => {
                let result = match value {
                    "prev" => self.command("PREV", "").await,
                    "next" => self.command("NEXT", "").await,
                    "back" => {
                        self.command("SEEK", &(-view::SEEK_SECONDS).to_string())
                            .await
                    }
                    "fwd" => self.command("SEEK", &view::SEEK_SECONDS.to_string()).await,
                    "voldown" => {
                        self.command("VOLUME", &format!("-{}", view::VOLUME_STEP))
                            .await
                    }
                    "volup" => {
                        self.command("VOLUME", &format!("+{}", view::VOLUME_STEP))
                            .await
                    }
                    "resume" => self.command("RESUME", "").await,
                    "fav" => self.command("FAV", "").await,
                    "toggle" => match self.state().await {
                        Ok(s) if s.playing() => self.command("PAUSE", "").await,
                        Ok(_) => self.command("RESUME", "").await,
                        Err(e) => Err(e),
                    },
                    "repeat" => match self.state().await {
                        Ok(s) => {
                            self.command("REPEAT", if s.repeat { "off" } else { "on" })
                                .await
                        }
                        Err(e) => Err(e),
                    },
                    _ => Err("неизвестная кнопка".into()),
                };
                if result.is_ok() && matches!(value, "prev" | "next" | "fav" | "resume") {
                    self.note(query.from.id, matches!(value, "fav" | "resume"));
                }
                match result {
                    // "volume 50%" from soundiya, as a toast.
                    Ok(text) if value.starts_with("vol") => {
                        let level = text.trim_start_matches("volume ");
                        self.answer(&query, &format!("🔊 {level}")).await;
                    }
                    Ok(_) => self.answer(&query, "").await,
                    Err(e) => self.answer(&query, &format!("⚠️ {e}")).await,
                }
                self.wake.notify_one();
            }
            "m" => {
                if value == "sleep" {
                    self.sleep_menus.lock().unwrap().insert(chat);
                } else {
                    self.sleep_menus.lock().unwrap().remove(&chat);
                }
                self.answer(&query, "").await;
                let state = self.state().await;
                let (text, keyboard) = self.card_for(chat, &state);
                if let Err(e) = self.api.edit(chat, message_id, &text, Some(keyboard)).await {
                    eprintln!("telegram: card menu: {e}");
                }
            }
            "s" => {
                // Only the offered choices: a forged callback carries anything.
                let offered = value == "off"
                    || value
                        .parse::<u32>()
                        .is_ok_and(|m| view::SLEEP_MINUTES.contains(&m));
                if !offered {
                    return self.answer(&query, "").await;
                }
                self.sleep_menus.lock().unwrap().remove(&chat);
                let toast = if value == "off" {
                    "Таймер отменён".to_owned()
                } else {
                    format!("😴 Выключу через {value} мин")
                };
                match self.command("SLEEP", value).await {
                    Ok(_) => self.answer(&query, &toast).await,
                    Err(e) => self.answer(&query, &format!("⚠️ {e}")).await,
                }
                // The timer starts a moment later in soundiya; give it that moment.
                tokio::time::sleep(Duration::from_millis(300)).await;
                let state = self.state().await;
                let (text, keyboard) = self.card_for(chat, &state);
                let _ = self.api.edit(chat, message_id, &text, Some(keyboard)).await;
            }
            "q" => {
                self.answer(&query, "").await;
                match self.state().await {
                    // From the card: a queue message of its own, at the playing track.
                    Ok(state) if value == "-" => {
                        let (text, keyboard) = view::queue(&state, view::page_of_current(&state));
                        let _ = self.api.send(chat, &text, Some(keyboard), true).await;
                    }
                    Ok(state) => {
                        let page = value.parse().unwrap_or(0);
                        let (text, keyboard) = view::queue(&state, page);
                        let _ = self.api.edit(chat, message_id, &text, Some(keyboard)).await;
                    }
                    Err(e) => self.answer(&query, &format!("⚠️ {e}")).await,
                }
            }
            "j" => {
                let Ok(n) = value.parse::<usize>() else {
                    return self.answer(&query, "").await;
                };
                match self.command("JUMP", &n.to_string()).await {
                    Ok(_) => {
                        self.note(query.from.id, false);
                        self.answer(&query, &format!("▶️ Трек {n}")).await;
                    }
                    Err(e) => return self.answer(&query, &format!("⚠️ {e}")).await,
                }
                // Redraw the queue with the new mark without holding up other updates.
                let bot = self.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(JUMP_REDRAW_AFTER).await;
                    if let Ok(state) = bot.state().await {
                        let (text, keyboard) = view::queue(&state, (n - 1) / view::PAGE);
                        let _ = bot.api.edit(chat, message_id, &text, Some(keyboard)).await;
                    }
                });
            }
            "u" => self.on_update_button(&query, chat, message_id, value).await,
            "a" => self.on_access_button(&query, chat, message_id, value).await,
            "x" => self.on_admin_button(&query, chat, message_id, value).await,
            _ => self.answer(&query, "").await,
        }
    }

    // ---- access ----

    /// Remembers who gave a command that may change the track (see `recipients`).
    fn note(&self, user: i64, loads: bool) {
        *self.last_action.lock().unwrap() = Some(Action {
            user,
            at: Instant::now(),
            loads,
        });
    }

    async fn voice_command(&self, chat: i64, argument: &str) {
        let describe = admin::mode_text;
        let chosen = match argument.trim().to_lowercase().as_str() {
            "all" | "все" => VoiceFrom::All,
            "owners" | "owner" | "владелец" => VoiceFrom::Owners,
            "off" | "выкл" => VoiceFrom::Off,
            _ => {
                let now = self
                    .access
                    .lock()
                    .unwrap()
                    .voice_from
                    .unwrap_or(self.config.voice);
                return self
                    .reply(
                        chat,
                        &format!(
                            "🎙 Голосовые на колонку: {}.\n/voice all — все слушатели, \
                             /voice owners — отмеченные в /admin, /voice off — никто",
                            describe(now)
                        ),
                    )
                    .await;
            }
        };
        self.access.lock().unwrap().voice_from = Some(chosen);
        if let Err(e) = self.save_access() {
            return self.reply(chat, &format!("⚠️ {}", view::esc(&e))).await;
        }
        self.reply(
            chat,
            &format!("🎙 Голосовые на колонку: {}.", describe(chosen)),
        )
        .await;
    }

    async fn voicevol_command(&self, chat: i64, argument: &str) {
        let (min, max) = (voice::VOLUME_MIN, voice::VOLUME_MAX);
        let Ok(percent) = argument.trim().trim_end_matches('%').parse::<u16>() else {
            let now = self.access.lock().unwrap().voice_volume();
            return self
                .reply(
                    chat,
                    &format!(
                        "🎙 Громкость голосовых: {now} %.\n\
                         Голосовые сами выравниваются до одного уровня, а это поправка к нему.\n\
                         /voicevol {min}…{max}, например /voicevol 150"
                    ),
                )
                .await;
        };
        if !(min..=max).contains(&percent) {
            return self
                .reply(
                    chat,
                    &format!("⚠️ От {min} до {max} %, например /voicevol 150"),
                )
                .await;
        }
        self.access.lock().unwrap().voice_volume = Some(percent);
        if let Err(e) = self.save_access() {
            return self.reply(chat, &format!("⚠️ {}", view::esc(&e))).await;
        }
        self.reply(chat, &format!("🎙 Громкость голосовых: {percent} %."))
            .await;
    }

    async fn notify_command(&self, chat: i64, argument: &str) {
        let muted = match argument.trim().to_lowercase().as_str() {
            "off" | "выкл" | "0" => true,
            "on" | "вкл" | "1" => false,
            _ => {
                let off = self.access.lock().unwrap().muted.contains(&chat);
                let state = if off {
                    "🔕 выключены"
                } else {
                    "🔔 включены"
                };
                return self
                    .reply(
                        chat,
                        &format!("Уведомления о новых треках {state}.\n/notify on или /notify off"),
                    )
                    .await;
            }
        };
        self.access.lock().unwrap().set_muted(chat, muted);
        if let Err(e) = self.save_access() {
            return self.reply(chat, &format!("⚠️ {}", view::esc(&e))).await;
        }
        let text = if muted {
            "🔕 Уведомления о новых треках выключены. Что играет — /now."
        } else {
            "🔔 Уведомления о новых треках включены."
        };
        self.reply(chat, text).await;
    }

    fn role(&self, user: i64) -> Role {
        if self.config.allowed(user) {
            Role::Owner
        } else if self.access.lock().unwrap().contains(user) {
            Role::Listener
        } else {
            Role::Stranger
        }
    }

    fn save_access(&self) -> Result<(), String> {
        let access = self.access.lock().unwrap().clone();
        access.save(&self.access_path)
    }

    /// Someone not on any list wrote: one request per person goes to the owners.
    async fn stranger(&self, from: &User, chat: i64) {
        let name = access::clean_name(from.first_name.as_deref().unwrap_or(""), from.id);
        {
            let mut requests = self.requests.lock().unwrap();
            requests.retain(|_, r| !r.expired());
            // Asked a moment ago or refused recently: silence, so nobody can spam the owners.
            if requests.contains_key(&from.id) || requests.len() >= MAX_REQUESTS {
                return;
            }
            let request = Request {
                name: name.clone(),
                at: Instant::now(),
                refused: false,
            };
            requests.insert(from.id, request);
        }
        eprintln!("bot: access request from user {}", from.id);
        self.reply(
            chat,
            "🔒 Это личный пульт колонки. Запрос на доступ отправлен владельцу — дождись ответа.",
        )
        .await;
        let username = from
            .username
            .as_deref()
            .map(|u| format!(" (@{})", view::esc(&access::clean_name(u, from.id))))
            .unwrap_or_default();
        let text = format!(
            "👤 Просит доступ к музыке: <b>{}</b>{username}, id <code>{}</code>",
            view::esc(&name),
            from.id
        );
        let keyboard = serde_json::json!({"inline_keyboard": [[
            {"text": "✅ Разрешить", "callback_data": format!("a:y:{}", from.id)},
            {"text": "⛔ Отклонить", "callback_data": format!("a:n:{}", from.id)},
        ]]});
        for &owner in &self.config.users {
            if let Err(e) = self
                .api
                .send(owner, &text, Some(keyboard.clone()), false)
                .await
            {
                eprintln!("telegram: access request: {e}");
            }
        }
    }

    fn users_view(&self) -> (String, serde_json::Value) {
        let access = self.access.lock().unwrap();
        let mut text = String::from("👥 <b>Доступ к музыке</b>\n\nВладельцы: ");
        text += &self
            .config
            .users
            .iter()
            .map(|id| format!("<code>{id}</code>"))
            .collect::<Vec<_>>()
            .join(", ");
        if access.listeners.is_empty() {
            text += "\nСлушателей нет. Кто напишет боту — придёт запрос с кнопками.";
        } else {
            text += "\n\nСлушатели (музыка, без обновлений и прав):";
            for l in &access.listeners {
                text += &format!("\n• {} — <code>{}</code>", view::esc(&l.name), l.id);
            }
        }
        let rows: Vec<serde_json::Value> = access
            .listeners
            .iter()
            .map(|l| serde_json::json!([{"text": format!("❌ {}", l.name), "callback_data": format!("a:d:{}", l.id)}]))
            .collect();
        (text, serde_json::json!({"inline_keyboard": rows}))
    }

    // ---- the owner's menu ----

    fn voice_mode(&self) -> VoiceFrom {
        self.access
            .lock()
            .unwrap()
            .voice_from
            .unwrap_or(self.config.voice)
    }

    async fn send_admin(&self, chat: i64) {
        // The mode first: `voice_mode` takes the same lock.
        let mode = self.voice_mode();
        let (text, keyboard) = admin::home(&self.access.lock().unwrap(), mode);
        if let Err(e) = self.api.send(chat, &text, Some(keyboard), true).await {
            eprintln!("telegram: admin: {e}");
        }
    }

    /// `x:` buttons; only owners get here. Every press is checked against the current list,
    /// so a forged or stale button changes nothing it should not.
    async fn on_admin_button(
        &self,
        query: &CallbackQuery,
        chat: i64,
        message_id: i64,
        value: &str,
    ) {
        let (action, arg) = value.split_once(':').unwrap_or((value, ""));
        let id = arg.parse::<i64>().ok();
        let mode = self.voice_mode();
        let mut toast = String::new();
        let view = match (action, id) {
            ("home", _) => Some(admin::home(&self.access.lock().unwrap(), mode)),
            ("list", _) => Some(admin::list(
                &self.access.lock().unwrap(),
                &self.config.users,
                mode,
            )),
            ("vm", _) => {
                let next = admin::next_mode(mode);
                self.access.lock().unwrap().voice_from = Some(next);
                toast = format!("🎙 Голосовые: {}", admin::mode_text(next));
                Some(admin::home(&self.access.lock().unwrap(), next))
            }
            ("vv", _) if matches!(arg, "+" | "-") => {
                let mut access = self.access.lock().unwrap();
                let level = admin::step_volume(access.voice_volume(), arg == "+");
                access.voice_volume = Some(level);
                toast = format!("🎙 {level} %");
                Some(admin::home(&access, mode))
            }
            ("upd", _) => {
                self.answer(query, "").await;
                return self.updates_status(chat).await;
            }
            ("l", Some(id)) => admin::listener(&self.access.lock().unwrap(), mode, id),
            ("v", Some(id)) => {
                let mut access = self.access.lock().unwrap();
                match access.listeners.iter_mut().find(|l| l.id == id) {
                    Some(l) => {
                        l.voice = !l.voice;
                        toast = if l.voice {
                            "🎙 Голосовые разрешены"
                        } else {
                            "🎙 Голосовые запрещены"
                        }
                        .into();
                        admin::listener(&access, mode, id)
                    }
                    None => None,
                }
            }
            ("n", Some(id)) => {
                let mut access = self.access.lock().unwrap();
                if access.contains(id) {
                    let muted = !access.muted.contains(&id);
                    access.set_muted(id, muted);
                    toast = if muted {
                        "🔕 Уведомления выключены"
                    } else {
                        "🔔 Уведомления включены"
                    }
                    .into();
                }
                admin::listener(&access, mode, id)
            }
            ("d", Some(id)) => admin::confirm_remove(&self.access.lock().unwrap(), id),
            ("D", Some(id)) => match self.revoke(id).await {
                Ok(name) => {
                    toast = format!("Доступ закрыт: {name}");
                    Some(admin::list(
                        &self.access.lock().unwrap(),
                        &self.config.users,
                        mode,
                    ))
                }
                Err(e) => {
                    toast = format!("⚠️ {e}");
                    Some(admin::list(
                        &self.access.lock().unwrap(),
                        &self.config.users,
                        mode,
                    ))
                }
            },
            _ => None,
        };
        if matches!(action, "vm" | "vv" | "v" | "n")
            && let Err(e) = self.save_access()
        {
            toast = format!("⚠️ {e}");
        }
        let Some((text, keyboard)) = view else {
            // A listener removed meanwhile, or a forged button: back to the list.
            self.answer(query, "Этого слушателя уже нет").await;
            let (text, keyboard) =
                admin::list(&self.access.lock().unwrap(), &self.config.users, mode);
            let _ = self.api.edit(chat, message_id, &text, Some(keyboard)).await;
            return;
        };
        self.answer(query, &toast).await;
        if let Err(e) = self.api.edit(chat, message_id, &text, Some(keyboard)).await {
            eprintln!("telegram: admin menu: {e}");
        }
    }

    async fn send_users(&self, chat: i64) {
        let (text, keyboard) = self.users_view();
        if let Err(e) = self.api.send(chat, &text, Some(keyboard), true).await {
            eprintln!("telegram: users: {e}");
        }
    }

    async fn grant(&self, id: i64, name: &str) -> Result<String, String> {
        if self.config.allowed(id) {
            return Err("это владелец, у него и так полный доступ".into());
        }
        let name = {
            let mut access = self.access.lock().unwrap();
            if !access.add(id, name) {
                return Err("неверный id".into());
            }
            access
                .listeners
                .iter()
                .find(|l| l.id == id)
                .map(|l| l.name.clone())
                .unwrap_or_default()
        };
        self.save_access()?;
        self.requests.lock().unwrap().remove(&id);
        eprintln!("bot: access granted to user {id}");
        let welcome = format!("✅ Доступ к пульту открыт!\n\n{}", view::HELP);
        if let Err(e) = self.api.send(id, &welcome, None, false).await {
            eprintln!("telegram: welcome for {id}: {e}");
        }
        Ok(name)
    }

    async fn revoke(&self, id: i64) -> Result<String, String> {
        let removed = self
            .access
            .lock()
            .unwrap()
            .remove(id)
            .ok_or("такого слушателя нет")?;
        // May ask again right away.
        self.requests.lock().unwrap().remove(&id);
        self.save_access()?;
        eprintln!("bot: access revoked from user {id}");
        let _ = self
            .api
            .send(id, "Доступ к пульту закрыт.", None, true)
            .await;
        self.cards.lock().unwrap().remove(&id);
        Ok(removed.name)
    }

    async fn allow_command(&self, chat: i64, argument: &str) {
        let (id, name) = argument
            .split_once(char::is_whitespace)
            .unwrap_or((argument, ""));
        let Ok(id) = id.parse::<i64>() else {
            return self
                .reply(
                    chat,
                    "Напиши: /allow ID [имя]. ID пришлёт бот, когда человек напишет ему /start.",
                )
                .await;
        };
        match self.grant(id, name).await {
            Ok(name) => {
                self.reply(
                    chat,
                    &format!("✅ {} теперь может управлять музыкой.", view::esc(&name)),
                )
                .await
            }
            Err(e) => self.reply(chat, &format!("⚠️ {}", view::esc(&e))).await,
        }
    }

    async fn deny_command(&self, chat: i64, argument: &str) {
        let Ok(id) = argument.trim().parse::<i64>() else {
            return self.reply(chat, "Напиши: /deny ID (список — /users)").await;
        };
        match self.revoke(id).await {
            Ok(name) => {
                self.reply(chat, &format!("Доступ у {} закрыт.", view::esc(&name)))
                    .await
            }
            Err(e) => self.reply(chat, &format!("⚠️ {}", view::esc(&e))).await,
        }
    }

    async fn on_access_button(
        &self,
        query: &CallbackQuery,
        chat: i64,
        message_id: i64,
        value: &str,
    ) {
        let (action, id) = value.split_once(':').unwrap_or((value, ""));
        let Ok(id) = id.parse::<i64>() else {
            return self.answer(query, "").await;
        };
        let pending = self
            .requests
            .lock()
            .unwrap()
            .get(&id)
            .map(|r| r.name.clone());
        match action {
            "y" => match self.grant(id, pending.as_deref().unwrap_or("")).await {
                Ok(name) => {
                    self.answer(query, "Доступ выдан").await;
                    let text = format!(
                        "✅ {} теперь может управлять музыкой. Забрать — /users",
                        view::esc(&name)
                    );
                    let _ = self.api.edit(chat, message_id, &text, None).await;
                }
                Err(e) => self.answer(query, &format!("⚠️ {e}")).await,
            },
            "n" => {
                // Kept as refused: the same person may ask again only after `REFUSED_FOR`.
                let refused = Request {
                    name: pending.clone().unwrap_or_else(|| format!("id {id}")),
                    at: Instant::now(),
                    refused: true,
                };
                self.requests.lock().unwrap().insert(id, refused);
                self.answer(query, "Отклонено").await;
                let name = pending.unwrap_or_else(|| format!("id {id}"));
                let text = format!("⛔ Запрос от {} отклонён.", view::esc(&name));
                let _ = self.api.edit(chat, message_id, &text, None).await;
            }
            "d" => {
                match self.revoke(id).await {
                    Ok(_) => self.answer(query, "Доступ закрыт").await,
                    Err(e) => self.answer(query, &format!("⚠️ {e}")).await,
                }
                let (text, keyboard) = self.users_view();
                let _ = self.api.edit(chat, message_id, &text, Some(keyboard)).await;
            }
            _ => self.answer(query, "").await,
        }
    }

    // ---- soundiya updates ----

    pub async fn watch_updates(self: Arc<Self>) {
        if self.github.is_none() {
            return;
        }
        tokio::time::sleep(UPDATES_FIRST).await;
        loop {
            self.check_updates().await;
            tokio::time::sleep(UPDATES_EVERY).await;
        }
    }

    async fn fetch_updates(&self) -> (Option<Release>, Option<updates::Issue>) {
        let Some(github) = &self.github else {
            return (None, None);
        };
        let release = github
            .latest()
            .await
            .map_err(|e| eprintln!("updates: latest release: {e}"))
            .ok();
        let issue = match self.config.watch_issue {
            0 => None,
            n => github
                .issue(n)
                .await
                .map_err(|e| eprintln!("updates: issue #{n}: {e}"))
                .ok(),
        };
        (release, issue)
    }

    /// Tells the users what is new since the last check; the very first check only takes note.
    async fn check_updates(&self) {
        let (release, issue) = self.fetch_updates().await;
        let first = Seen::load(&self.seen_path).is_none();
        let mut seen = Seen::load(&self.seen_path).unwrap_or_default();
        let news = updates::news(
            &seen,
            release.as_ref(),
            issue.as_ref(),
            self.config.watch_issue,
        );
        if !first {
            for &user in &self.config.users {
                if let Some(r) = &news.release {
                    let (text, keyboard) = release_offer(r);
                    if let Err(e) = self.api.send(user, &text, Some(keyboard), false).await {
                        eprintln!("telegram: release news: {e}");
                    }
                }
                if let Some(text) = &news.issue {
                    self.reply(user, &view::esc(text)).await;
                }
            }
        }
        updates::remember(&mut seen, release.as_ref(), issue.as_ref());
        seen.save(&self.seen_path);
    }

    async fn updates_status(&self, chat: i64) {
        if self.github.is_none() {
            return self
                .reply(
                    chat,
                    "Проверка обновлений выключена (\"updates\": false в config.json).",
                )
                .await;
        }
        let (release, issue) = self.fetch_updates().await;
        let build = match self.state().await {
            Ok(_) => "сборка с STATE (для бота)",
            Err(e) if e.contains("STATE") => "без STATE — очередь и карточка в боте не работают",
            Err(_) => "не отвечает",
        };
        let mut text = format!("🎧 Сейчас soundiya: {build}\n");
        match &issue {
            Some(i) => {
                text += &format!(
                    "📌 Issue #{}: {}, комментариев: {}\n",
                    self.config.watch_issue,
                    if i.state == "closed" {
                        "закрыт"
                    } else {
                        "открыт"
                    },
                    i.comments
                );
            }
            None if self.config.watch_issue > 0 => text += "📌 Issue: не удалось проверить\n",
            None => {}
        }
        match &release {
            Some(r) => {
                let (offer, keyboard) = release_offer(r);
                text += &format!("\n{offer}");
                if let Err(e) = self.api.send(chat, &text, Some(keyboard), true).await {
                    eprintln!("telegram: updates status: {e}");
                }
            }
            None => {
                self.reply(
                    chat,
                    &(text + "\nGitHub сейчас недоступен, попробуй позже."),
                )
                .await
            }
        }
    }

    async fn on_update_button(
        &self,
        query: &CallbackQuery,
        chat: i64,
        message_id: i64,
        value: &str,
    ) {
        let (action, tag) = value.split_once(':').unwrap_or((value, ""));
        match action {
            "s" | "i" if !updates::valid_tag(tag) => self.answer(query, "").await,
            "s" => {
                let mut seen = Seen::load(&self.seen_path).unwrap_or_default();
                seen.skipped = Some(tag.to_owned());
                seen.save(&self.seen_path);
                self.answer(query, "Не ставлю").await;
                let text = format!(
                    "Ок, {} не ставлю и больше не предлагаю. /updates — проверить снова.",
                    view::esc(tag)
                );
                let _ = self.api.edit(chat, message_id, &text, None).await;
            }
            "i" | "r" => {
                if self.updating.swap(true, Ordering::AcqRel) {
                    return self.answer(query, "Уже выполняется, подожди").await;
                }
                let install = action == "i";
                self.answer(
                    query,
                    if install {
                        "Ставлю…"
                    } else {
                        "Откатываю…"
                    },
                )
                .await;
                let wait = if install {
                    "⏳ Скачиваю и ставлю soundiya… до 5 минут, в конце будет короткий гудок проверки звука."
                } else {
                    "⏳ Возвращаю прежнюю сборку…"
                };
                let _ = self.api.edit(chat, message_id, wait, None).await;
                if install {
                    // The autoupdater waits for a pause in the music; give it one.
                    let _ = self.command("PAUSE", "").await;
                }
                let result = match (&self.github, install) {
                    (Some(github), true) => github.install(tag, &self.config.autoupdate).await,
                    (None, true) => Err("проверка обновлений выключена".to_owned()),
                    (_, false) => updates::rollback(&self.config.autoupdate).await,
                };
                tokio::time::sleep(Duration::from_secs(5)).await;
                let (text, keyboard) = self.after_update(install, result).await;
                let _ = self.api.edit(chat, message_id, &text, keyboard).await;
                self.updating.store(false, Ordering::Release);
                self.wake.notify_one();
            }
            _ => self.answer(query, "").await,
        }
    }

    async fn after_update(
        &self,
        install: bool,
        result: Result<String, String>,
    ) -> (String, Option<serde_json::Value>) {
        let log = |t: &str| format!("<pre>{}</pre>", view::esc(t.trim()));
        let output = match &result {
            Ok(t) | Err(t) => t.clone(),
        };
        match (result.is_ok(), self.state().await) {
            (true, Ok(_)) => (
                format!("✅ Готово, бот работает полностью.\n{}", log(&output)),
                None,
            ),
            (true, Err(e)) if e.contains("STATE") && install => (
                format!(
                    "⚠️ Установлено, но в этой версии нет STATE: карточка и очередь в боте не работают.\n{}",
                    log(&output)
                ),
                Some(serde_json::json!({"inline_keyboard": [[
                    {"text": "↩️ Вернуть прежнюю сборку", "callback_data": "u:r"}
                ]]})),
            ),
            (true, Err(e)) => (
                format!(
                    "⚠️ Выполнено, но soundiya: {}\n{}",
                    view::esc(&e),
                    log(&output)
                ),
                None,
            ),
            (false, _) => (format!("❌ Не получилось.\n{}", log(&output)), None),
        }
    }
}

/// The message about a release: whether the bot keeps working on it, and the buttons.
fn release_offer(release: &Release) -> (String, serde_json::Value) {
    let state = match release.has_state {
        Some(true) => "✅ В ней есть STATE: бот продолжит работать полностью.",
        Some(false) => {
            "⚠️ В ней нет STATE: после установки в боте перестанут работать карточка и очередь. Откатить можно кнопкой."
        }
        None => "❔ Не удалось проверить, есть ли в ней STATE.",
    };
    let text = format!(
        "🆕 Последний релиз soundiya: <a href=\"{}\">{}</a>\n{state}",
        view::esc(&release.url),
        view::esc(&release.tag)
    );
    let keyboard = serde_json::json!({"inline_keyboard": [[
        {"text": "⬇️ Установить", "callback_data": format!("u:i:{}", release.tag)},
        {"text": "✖ Не ставить", "callback_data": format!("u:s:{}", release.tag)},
    ]]});
    (text, keyboard)
}

/// Waits for Telegram at startup: after boot or a router restart the network may not be up
/// yet. Only a rejected token is final.
pub async fn wait_for_telegram(api: &Api) -> Result<crate::telegram::User, String> {
    let mut backoff = 1;
    loop {
        match api.get_me().await {
            Ok(me) => return Ok(me),
            Err(e) if matches!(e.code(), Some(401 | 404)) => {
                return Err(format!("Telegram: {e} (check the token in config.json)"));
            }
            Err(e) => {
                if e.code().is_none() {
                    api.reset();
                }
                eprintln!("telegram: {e}; retrying in {backoff} s");
                tokio::time::sleep(Duration::from_secs(backoff)).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }
}

/// For `soundiya-tg check`: the bot can reach Telegram and soundiya.
pub async fn check(api: &Api, socket: &std::path::Path) -> Result<(), String> {
    let me = api
        .get_me()
        .await
        .map_err(|e: Error| format!("Telegram: {e}"))?;
    println!("Telegram: ok, bot @{}", me.username.unwrap_or_default());
    let state = soundiya::state(socket)
        .await
        .map_err(|e| format!("soundiya: {e}"))?;
    match state.current() {
        Some(t) => println!(
            "soundiya: ok, {} — {} ({} in queue)",
            t.artist,
            t.title,
            state.queue.len()
        ),
        None => println!(
            "soundiya: ok, nothing playing ({} in queue)",
            state.queue.len()
        ),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::recipients;

    #[test]
    fn the_one_who_started_the_queue_hears_it_owners_see_it_quietly() {
        // By voice: owners, with sound.
        assert_eq!(recipients(&[1, 2], None, None, &[]), (vec![1, 2], vec![]));
        // A listener's search: the listener with sound, owners quietly.
        assert_eq!(recipients(&[1], Some(9), None, &[]), (vec![9], vec![1]));
        // The owner's own search: only the owner, with sound.
        assert_eq!(recipients(&[1], Some(1), None, &[]), (vec![1], vec![]));
        // Someone else switched the track in the listener's queue: both hear it.
        assert_eq!(
            recipients(&[1], Some(9), Some(8), &[]),
            (vec![9, 8], vec![1])
        );
        assert_eq!(
            recipients(&[1], Some(9), Some(1), &[]),
            (vec![9, 1], vec![])
        );
        // /notify off wins everywhere.
        assert_eq!(recipients(&[1], Some(9), None, &[1, 9]), (vec![], vec![]));
    }
}
