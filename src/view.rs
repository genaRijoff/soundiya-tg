//! What the bot shows: the player card, the queue pages and their buttons.
//! Pure functions of the soundiya state, so they are tested without Telegram.
use crate::soundiya::State;
use serde_json::{Value, json};

pub const PAGE: usize = 10;
const ENTRY_CHARS: usize = 64;
const BAR: usize = 12;
pub const SEEK_SECONDS: i32 = 15;
pub const VOLUME_STEP: u16 = 10;
pub const SLEEP_MINUTES: [u32; 4] = [15, 30, 60, 90];

/// Escapes text for `parse_mode: HTML`, also inside attribute values (`href="..."`).
pub fn esc(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        text.chars().take(max - 1).collect::<String>() + "…"
    }
}

pub fn mmss(seconds: u64) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

fn bar(position: u64, duration: u64) -> String {
    // At most BAR, so the cast cannot truncate.
    let filled = (position.min(duration) * BAR as u64)
        .checked_div(duration)
        .map_or(0, |n| n as usize);
    let mut line: String = "━".repeat(filled);
    line.push('●');
    line.push_str(&"─".repeat(BAR - filled));
    line
}

fn button(text: &str, data: &str) -> Value {
    json!({"text": text, "callback_data": data})
}

/// The player card: what plays, where, and the controls.
pub fn card(state: &Result<State, String>) -> (String, Value) {
    let state = match state {
        Err(e) => {
            return (
                format!("⚠️ {}", esc(e)),
                json!({"inline_keyboard": [[button("🔄 Обновить", "m:card")]]}),
            );
        }
        Ok(state) => state,
    };
    let Some(track) = state.current() else {
        let text =
            "⏹ <b>Ничего не играет</b>\nСкажи «Алиса, включи …» или пришли сюда название трека."
                .to_owned();
        let mut rows = vec![vec![
            button("▶️ Продолжить", "c:resume"),
            button("❤️ Лайки", "c:fav"),
        ]];
        if !state.queue.is_empty() {
            rows.push(vec![button("📃 Последняя очередь", "q:-")]);
        }
        return (text, json!({"inline_keyboard": rows}));
    };
    let icon = match state.state.as_str() {
        "playing" => "▶️",
        "paused" => "⏸",
        _ => "⏳",
    };
    let mut text = format!(
        "{icon} <b>{}</b>\n{}\n",
        esc(&shorten(&track.title, 120)),
        esc(&shorten(&track.artist, 80))
    );
    if track.duration > 0 {
        text += &format!(
            "<code>{} {} {}</code>\n",
            mmss(state.position),
            bar(state.position, track.duration),
            mmss(track.duration)
        );
    } else {
        text += &format!("<code>{}</code>\n", mmss(state.position));
    }
    let mut details = vec![format!("трек {} из {}", state.index, state.queue.len())];
    if state.repeat {
        details.push("🔂 повтор".into());
    }
    if let Some(minutes) = state.sleep {
        details.push(format!("😴 через {minutes} мин"));
    }
    if let Some(volume) = state.volume {
        details.push(format!("🔊 {volume}%"));
    }
    text += &details.join(" · ");
    (text, card_keyboard(state))
}

fn card_keyboard(state: &State) -> Value {
    let toggle = if state.playing() { "⏸" } else { "▶️" };
    let repeat = if state.repeat {
        "🔂 Повтор вкл"
    } else {
        "🔁 Повтор"
    };
    json!({"inline_keyboard": [
        [button("⏮", "c:prev"), button(toggle, "c:toggle"), button("⏭", "c:next")],
        [button(&format!("⏪ {SEEK_SECONDS} с"), "c:back"), button(&format!("{SEEK_SECONDS} с ⏩"), "c:fwd")],
        [button(&format!("🔉 −{VOLUME_STEP}"), "c:voldown"), button(&format!("🔊 +{VOLUME_STEP}"), "c:volup")],
        [button("📃 Очередь", "q:-"), button(repeat, "c:repeat"), button("😴 Таймер", "m:sleep")],
    ]})
}

/// The card's controls swapped for the sleep timer choices.
pub fn sleep_keyboard(state: &State) -> Value {
    let mut rows = vec![
        SLEEP_MINUTES
            .iter()
            .map(|m| button(&format!("{m} мин"), &format!("s:{m}")))
            .collect::<Vec<_>>(),
    ];
    if state.sleep.is_some() {
        rows.push(vec![button("❌ Отменить таймер", "s:off")]);
    }
    rows.push(vec![button("⬅️ Назад", "m:card")]);
    json!({"inline_keyboard": rows})
}

/// The page that holds the playing track, or the first one.
pub fn page_of_current(state: &State) -> usize {
    state.index.saturating_sub(1) / PAGE
}

pub fn pages(state: &State) -> usize {
    state.queue.len().div_ceil(PAGE).max(1)
}

/// One page of the queue; each number button plays that track.
pub fn queue(state: &State, page: usize) -> (String, Value) {
    if state.queue.is_empty() {
        return (
            "📃 Очередь пуста".into(),
            json!({"inline_keyboard": [[button("🔄", "q:-")]]}),
        );
    }
    let page = page.min(pages(state) - 1);
    let first = page * PAGE;
    let mut text = format!("📃 <b>Очередь</b> · {} треков\n\n", state.queue.len());
    let mut numbers = Vec::new();
    for (i, entry) in state.queue.iter().enumerate().skip(first).take(PAGE) {
        let n = i + 1;
        let playing = n == state.index;
        let label = shorten(&format!("{} — {}", entry.artist, entry.title), ENTRY_CHARS);
        let length = if entry.duration > 0 {
            format!(" ({})", mmss(entry.duration))
        } else {
            String::new()
        };
        if playing {
            text += &format!("▶️ <b>{n}. {}</b>{length}\n", esc(&label));
        } else {
            text += &format!("{n}. {}{length}\n", esc(&label));
        }
        let caption = if playing {
            format!("▶{n}")
        } else {
            n.to_string()
        };
        numbers.push(button(&caption, &format!("j:{n}")));
    }
    let mut rows: Vec<Vec<Value>> = numbers.chunks(5).map(<[Value]>::to_vec).collect();
    let mut nav = Vec::new();
    if page > 0 {
        nav.push(button("◀️", &format!("q:{}", page - 1)));
    }
    nav.push(button(
        &format!("{}/{}", page + 1, pages(state)),
        &format!("q:{page}"),
    ));
    if page + 1 < pages(state) {
        nav.push(button("▶️", &format!("q:{}", page + 1)));
    }
    rows.push(nav);
    text += "\nНажми номер, чтобы включить трек.";
    (text, json!({"inline_keyboard": rows}))
}

pub const HELP: &str = "🎧 <b>soundiya</b> — пульт SoundCloud на колонке\n\n\
Пришли название трека или ссылку soundcloud.com — включу.\n\n\
/now — карточка плеера\n\
/queue — очередь, переход к любому треку\n\
/fav — мои лайки\n\
/top, /trending [жанр] — чарты\n\
/related — похожие на текущий\n\
/next, /prev, /pause, /resume\n\
/seek 30 или /seek -15 — перемотка\n\
/sleep 30 или /sleep off — таймер\n\
/repeat on|off — повтор трека\n\
/vol 50, /vol +10, /vol -10 — громкость\n\
/notify on|off — уведомления о новых треках";

pub const OWNER_HELP: &str = "\n\n<b>Владельцу:</b>\n\
/users — кто ещё управляет музыкой\n\
/allow ID [имя], /deny ID — выдать или забрать доступ\n\
/updates — новые версии soundiya";

pub const COMMANDS: &[(&str, &str)] = &[
    ("now", "Что играет"),
    ("queue", "Очередь"),
    ("fav", "Мои лайки"),
    ("top", "Топ SoundCloud"),
    ("related", "Похожие треки"),
    ("next", "Следующий"),
    ("prev", "Предыдущий"),
    ("pause", "Пауза"),
    ("resume", "Продолжить"),
    ("sleep", "Таймер сна, минуты"),
    ("vol", "Громкость: 50, +10, -10"),
    ("notify", "Уведомления о треках: on/off"),
    ("users", "Доступ (владельцу)"),
    ("updates", "Обновления (владельцу)"),
    ("help", "Помощь"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::soundiya::Entry;

    fn state(index: usize, tracks: usize) -> State {
        State {
            state: "playing".into(),
            index,
            position: 65,
            repeat: false,
            sleep: None,
            signed_in: true,
            volume: None,
            queue: (1..=tracks)
                .map(|i| Entry {
                    id: i as u64,
                    artist: format!("Artist {i}"),
                    title: format!("Song <{i}> & co"),
                    duration: 200,
                })
                .collect(),
        }
    }

    fn callbacks(keyboard: &Value) -> Vec<String> {
        keyboard["inline_keyboard"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|row| row.as_array().unwrap().iter())
            .map(|b| b["callback_data"].as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn card_shows_the_track_escaped_with_controls() {
        let (text, keyboard) = card(&Ok(state(3, 25)));
        assert!(text.starts_with("▶️ <b>Song &lt;3&gt; &amp; co</b>\nArtist 3\n"));
        assert!(text.contains("<code>1:05 ━━━●───────── 3:20</code>"));
        assert!(text.ends_with("трек 3 из 25"));
        let data = callbacks(&keyboard);
        assert!(data.iter().all(|d| d.len() <= 64));
        assert_eq!(data[..3], ["c:prev", "c:toggle", "c:next"]);
        assert!(data.contains(&"c:voldown".to_owned()) && data.contains(&"c:volup".to_owned()));
    }

    #[test]
    fn card_mentions_repeat_and_sleep() {
        let mut s = state(1, 2);
        s.repeat = true;
        s.sleep = Some(12);
        s.volume = Some(40);
        s.state = "paused".into();
        let (text, _) = card(&Ok(s));
        assert!(text.starts_with("⏸"));
        assert!(text.ends_with("трек 1 из 2 · 🔂 повтор · 😴 через 12 мин · 🔊 40%"));
    }

    #[test]
    fn idle_and_errors_have_their_own_cards() {
        let (text, keyboard) = card(&Ok(state(0, 3)));
        assert!(text.contains("Ничего не играет"));
        assert!(callbacks(&keyboard).contains(&"q:-".to_owned()));
        let (text, _) = card(&Err("soundiya <не> запущен".into()));
        assert_eq!(text, "⚠️ soundiya &lt;не&gt; запущен");
    }

    #[test]
    fn queue_pages_number_the_tracks_and_mark_the_current_one() {
        let s = state(12, 25);
        assert_eq!(page_of_current(&s), 1);
        assert_eq!(pages(&s), 3);
        let (text, keyboard) = queue(&s, 1);
        assert!(text.contains("▶️ <b>12. Artist 12 — Song &lt;12&gt; &amp; co</b> (3:20)"));
        assert!(text.contains("\n11. Artist 11"));
        assert!(!text.contains("\n10. "));
        let data = callbacks(&keyboard);
        assert_eq!(
            data[..10],
            [
                "j:11", "j:12", "j:13", "j:14", "j:15", "j:16", "j:17", "j:18", "j:19", "j:20"
            ]
        );
        assert_eq!(data[10..], ["q:0", "q:1", "q:2"]);
        // A page past the end shows the last one.
        let (text, _) = queue(&s, 99);
        assert!(text.contains("\n25. "));
    }

    #[test]
    fn long_titles_are_shortened() {
        let mut s = state(1, 1);
        s.queue[0].title = "x".repeat(500);
        let (text, _) = queue(&s, 0);
        assert!(text.lines().any(|l| l.ends_with("…</b> (3:20)")));
        assert!(text.chars().count() < 400);
    }
}
