//! The owner's menu (`/admin`): listeners and their rights, voice messages, updates.
//! Pure views; the bot applies the button presses (callback data `x:...`).
use crate::{
    access::{Access, Listener},
    config::VoiceFrom,
    view::{button, esc},
    voice,
};
use serde_json::{Value, json};

/// The `/voicevol` step of the −/+ buttons.
pub const VOLUME_STEP: u16 = 10;

pub fn mode_text(mode: VoiceFrom) -> &'static str {
    match mode {
        VoiceFrom::Owners => "владелец и отмеченные слушатели",
        VoiceFrom::All => "все слушатели",
        VoiceFrom::Off => "выключены для всех",
    }
}

/// The voice mode button walks through the three modes.
pub fn next_mode(mode: VoiceFrom) -> VoiceFrom {
    match mode {
        VoiceFrom::Owners => VoiceFrom::All,
        VoiceFrom::All => VoiceFrom::Off,
        VoiceFrom::Off => VoiceFrom::Owners,
    }
}

/// May this listener send voice messages under `mode`?
pub fn listener_voice(listener: &Listener, mode: VoiceFrom) -> bool {
    match mode {
        VoiceFrom::All => true,
        VoiceFrom::Owners => listener.voice,
        VoiceFrom::Off => false,
    }
}

pub fn home(access: &Access, mode: VoiceFrom) -> (String, Value) {
    let volume = access.voice_volume();
    let text = format!(
        "⚙️ <b>Управление колонкой</b>\n\n\
         👥 Слушателей: {}\n\
         🎙 Голосовые: {}\n\
         🔊 Громкость голосовых: {volume} % от автоуровня",
        access.listeners.len(),
        mode_text(mode)
    );
    let keyboard = json!({"inline_keyboard": [
        [button(&format!("👥 Слушатели ({})", access.listeners.len()), "x:list")],
        [button(&format!("🎙 Голосовые: {} ▸", short_mode(mode)), "x:vm")],
        [
            button("🔉 −10", "x:vv:-"),
            button(&format!("🎙 {volume} %"), "x:home"),
            button("🔊 +10", "x:vv:+"),
        ],
        [button("🔄 Обновления soundiya", "x:upd")],
    ]});
    (text, keyboard)
}

fn short_mode(mode: VoiceFrom) -> &'static str {
    match mode {
        VoiceFrom::Owners => "отмеченным",
        VoiceFrom::All => "всем",
        VoiceFrom::Off => "выкл",
    }
}

pub fn list(access: &Access, owners: &[i64], mode: VoiceFrom) -> (String, Value) {
    let mut text = String::from("👥 <b>Слушатели</b>\n\nВладельцы: ");
    text += &owners
        .iter()
        .map(|id| format!("<code>{id}</code>"))
        .collect::<Vec<_>>()
        .join(", ");
    if access.listeners.is_empty() {
        text += "\n\nСлушателей нет. Кто напишет боту — тебе придёт запрос с кнопками \
                 (или /allow ID имя).";
    } else {
        text += "\n\nНажми на имя — права и удаление.\n🎙 — голосовые, 🔕 — без уведомлений";
    }
    let mut rows: Vec<Value> = access
        .listeners
        .iter()
        .map(|l| {
            let mut label = format!("👤 {}", l.name);
            if listener_voice(l, mode) {
                label += " · 🎙";
            }
            if access.muted.contains(&l.id) {
                label += " · 🔕";
            }
            json!([button(&label, &format!("x:l:{}", l.id))])
        })
        .collect();
    rows.push(json!([button("⬅️ Назад", "x:home")]));
    (text, json!({"inline_keyboard": rows}))
}

/// The listener's voice button: flips what this listener may do now and keeps everyone else
/// as they are. Under «всем» or «выкл» the marks are set to what they mean there and the mode
/// becomes «отмеченным», where a mark is what counts. Returns the new state, or None if the
/// listener is gone.
pub fn toggle_voice(access: &mut Access, mode: VoiceFrom, id: i64) -> Option<bool> {
    if !access.contains(id) {
        return None;
    }
    if mode != VoiceFrom::Owners {
        for l in &mut access.listeners {
            l.voice = listener_voice(l, mode);
        }
        access.voice_from = Some(VoiceFrom::Owners);
    }
    let l = access.listeners.iter_mut().find(|l| l.id == id)?;
    l.voice = !l.voice;
    Some(l.voice)
}

pub fn listener(access: &Access, mode: VoiceFrom, id: i64) -> Option<(String, Value)> {
    let l = access.listeners.iter().find(|l| l.id == id)?;
    let muted = access.muted.contains(&id);
    let allowed = listener_voice(l, mode);
    let voice_line = match mode {
        VoiceFrom::All => "✅ разрешены (сейчас всем)",
        VoiceFrom::Off => "⛔ выключены для всех",
        VoiceFrom::Owners if allowed => "✅ разрешены",
        VoiceFrom::Owners => "⛔ нельзя",
    };
    let text = format!(
        "👤 <b>{}</b> — <code>{id}</code>\n\n\
         🎵 Музыка: управляет\n\
         🎙 Голосовые на колонку: {voice_line}\n\
         🔔 Уведомления о треках: {}",
        esc(&l.name),
        if muted {
            "выключены"
        } else {
            "включены"
        }
    );
    let keyboard = json!({"inline_keyboard": [
        [button(
            if allowed { "🎙 Голосовые: ✅ разрешены" } else { "🎙 Голосовые: ⛔ нельзя" },
            &format!("x:v:{id}"),
        )],
        [button(
            if muted { "🔕 Уведомления: выкл" } else { "🔔 Уведомления: вкл" },
            &format!("x:n:{id}"),
        )],
        [button("🗑 Забрать доступ", &format!("x:d:{id}"))],
        [button("⬅️ К списку", "x:list")],
    ]});
    Some((text, keyboard))
}

pub fn confirm_remove(access: &Access, id: i64) -> Option<(String, Value)> {
    let l = access.listeners.iter().find(|l| l.id == id)?;
    let text = format!(
        "🗑 Забрать доступ у <b>{}</b>?\nМузыкой и голосовыми он больше управлять не сможет.",
        esc(&l.name)
    );
    let keyboard = json!({"inline_keyboard": [[
        button("✅ Да, забрать", &format!("x:D:{id}")),
        button("↩️ Нет", &format!("x:l:{id}")),
    ]]});
    Some((text, keyboard))
}

/// The voice volume after a −/+ press, within the `/voicevol` range.
pub fn step_volume(now: u16, up: bool) -> u16 {
    let next = if up {
        now.saturating_add(VOLUME_STEP)
    } else {
        now.saturating_sub(VOLUME_STEP)
    };
    next.clamp(voice::VOLUME_MIN, voice::VOLUME_MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn access() -> Access {
        let mut a = Access::default();
        a.add(77, "Друг");
        a.add(88, "Сосед");
        a.listeners[0].voice = true;
        a.set_muted(88, true);
        a
    }

    fn datas(keyboard: &Value) -> Vec<String> {
        keyboard["inline_keyboard"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|row| row.as_array().unwrap().iter())
            .map(|b| b["callback_data"].as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn modes_cycle_and_decide_for_listeners() {
        let a = access();
        let (friend, neighbour) = (&a.listeners[0], &a.listeners[1]);
        assert!(listener_voice(friend, VoiceFrom::Owners));
        assert!(!listener_voice(neighbour, VoiceFrom::Owners));
        assert!(listener_voice(neighbour, VoiceFrom::All));
        assert!(!listener_voice(friend, VoiceFrom::Off));
        let mut mode = VoiceFrom::Owners;
        for _ in 0..3 {
            mode = next_mode(mode);
        }
        assert_eq!(mode, VoiceFrom::Owners);
    }

    #[test]
    fn views_link_to_each_other() {
        let a = access();
        let (text, keyboard) = home(&a, VoiceFrom::Owners);
        assert!(text.contains("Слушателей: 2") && text.contains("100 %"));
        assert_eq!(
            datas(&keyboard),
            ["x:list", "x:vm", "x:vv:-", "x:home", "x:vv:+", "x:upd"]
        );

        let (text, keyboard) = list(&a, &[42], VoiceFrom::Owners);
        assert!(text.contains("<code>42</code>"));
        assert_eq!(datas(&keyboard), ["x:l:77", "x:l:88", "x:home"]);
        let labels = keyboard["inline_keyboard"].to_string();
        assert!(labels.contains("Друг · 🎙") && labels.contains("Сосед · 🔕"));

        let (text, keyboard) = listener(&a, VoiceFrom::Owners, 88).unwrap();
        assert!(text.contains("⛔ нельзя") && text.contains("выключены"));
        assert_eq!(datas(&keyboard), ["x:v:88", "x:n:88", "x:d:88", "x:list"]);
        assert!(listener(&a, VoiceFrom::Owners, 5).is_none());

        let (_, keyboard) = confirm_remove(&a, 77).unwrap();
        assert_eq!(datas(&keyboard), ["x:D:77", "x:l:77"]);
    }

    #[test]
    fn the_listener_button_always_does_what_it_says() {
        // «отмеченным»: only this listener's mark flips.
        let mut a = access();
        assert_eq!(toggle_voice(&mut a, VoiceFrom::Owners, 88), Some(true));
        assert!(a.listeners.iter().all(|l| l.voice));
        assert_eq!(a.voice_from, None);
        // «всем»: everyone keeps voice except the one pressed; the mode becomes «отмеченным».
        let mut a = access();
        assert_eq!(toggle_voice(&mut a, VoiceFrom::All, 77), Some(false));
        assert_eq!(a.voice_from, Some(VoiceFrom::Owners));
        assert!(!a.listeners[0].voice && a.listeners[1].voice);
        // «выкл»: nobody but the one pressed.
        let mut a = access();
        assert_eq!(toggle_voice(&mut a, VoiceFrom::Off, 88), Some(true));
        assert_eq!(a.voice_from, Some(VoiceFrom::Owners));
        assert!(!a.listeners[0].voice && a.listeners[1].voice);
        // The page shows what is allowed now, not the stored mark.
        let a = access();
        let (_, keyboard) = listener(&a, VoiceFrom::All, 88).unwrap();
        assert!(keyboard.to_string().contains("✅ разрешены"));
        let mut a = access();
        assert_eq!(toggle_voice(&mut a, VoiceFrom::All, 5), None);
        assert_eq!(a.voice_from, None);
    }

    #[test]
    fn names_are_escaped() {
        let mut a = Access::default();
        a.add(77, "<b>x</b>");
        let (text, _) = listener(&a, VoiceFrom::All, 77).unwrap();
        assert!(text.contains("&lt;b&gt;x&lt;/b&gt;"));
    }

    #[test]
    fn volume_steps_stay_in_range() {
        assert_eq!(step_volume(100, true), 110);
        assert_eq!(step_volume(195, true), 200);
        assert_eq!(step_volume(55, false), 50);
        assert_eq!(step_volume(50, false), 50);
    }
}
