//! Who besides the owners may use the bot: listeners, kept in `access.json` next to config.json.
//! Owners (`users` in config.json) grant and revoke access from the chat.
use serde::{Deserialize, Serialize};
use std::{fs, os::unix::fs::PermissionsExt, path::Path};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listener {
    pub id: i64,
    pub name: String,
    /// May send voice messages while only owners and marked listeners may (`/admin`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub voice: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Access {
    #[serde(default)]
    pub listeners: Vec<Listener>,
    /// Owners and listeners who turned off new-track notifications (`/notify off`).
    #[serde(default)]
    pub muted: Vec<i64>,
    /// Voice message loudness after the automatic leveling, percent (`/voicevol`); 100 if unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_volume: Option<u16>,
    /// Who may send voice messages, set from the chat (`/voice`); config.json's `voice` if unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice_from: Option<crate::config::VoiceFrom>,
    /// "Алиса, передай …" goes to the bot's users (`/admin`); off unless the owner turns it on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker_messages: Option<bool>,
    /// The web panel in the home network (`/admin`); off unless the owner turns it on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panel: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Listed in config.json: everything, including updates and access.
    Owner,
    /// Music only: search, play, controls, queue, volume, sleep timer.
    Listener,
    Stranger,
}

impl Access {
    pub fn load(path: &Path) -> Access {
        fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let tmp = path.with_extension("tmp");
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        fs::write(&tmp, text).map_err(|e| e.to_string())?;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
        fs::rename(&tmp, path).map_err(|e| e.to_string())
    }

    pub fn panel(&self) -> bool {
        self.panel.unwrap_or(false)
    }

    pub fn speaker_messages(&self) -> bool {
        self.speaker_messages.unwrap_or(false)
    }

    pub fn voice_volume(&self) -> u16 {
        self.voice_volume.unwrap_or(100)
    }

    pub fn contains(&self, id: i64) -> bool {
        self.listeners.iter().any(|l| l.id == id)
    }

    /// Adds or renames; returns false for an invalid id.
    pub fn add(&mut self, id: i64, name: &str) -> bool {
        if id <= 0 {
            return false;
        }
        let name = clean_name(name, id);
        match self.listeners.iter_mut().find(|l| l.id == id) {
            Some(l) => l.name = name,
            None => self.listeners.push(Listener {
                id,
                name,
                voice: false,
            }),
        }
        true
    }

    pub fn set_muted(&mut self, id: i64, muted: bool) {
        self.muted.retain(|&m| m != id);
        if muted {
            self.muted.push(id);
        }
    }

    pub fn remove(&mut self, id: i64) -> Option<Listener> {
        let index = self.listeners.iter().position(|l| l.id == id)?;
        Some(self.listeners.remove(index))
    }
}

/// A display name from Telegram: one line, bounded, never empty.
pub fn clean_name(name: &str, id: i64) -> String {
    let flat: String = name
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let flat = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    let short: String = flat.chars().take(40).collect();
    if short.is_empty() {
        format!("id {id}")
    } else {
        short
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listeners_are_added_renamed_and_removed() {
        let mut access = Access::default();
        assert!(access.add(7, "Вася"));
        assert!(access.add(7, "Василий\n<b>"));
        assert!(!access.add(-3, "group"));
        assert_eq!(
            access.listeners,
            vec![Listener {
                id: 7,
                name: "Василий <b>".into(),
                voice: false,
            }]
        );
        assert!(access.contains(7));
        assert_eq!(access.remove(7).unwrap().id, 7);
        assert!(access.remove(7).is_none());
        assert_eq!(clean_name("  ", 9), "id 9");
    }

    #[test]
    fn access_file_is_private_and_round_trips() {
        let path =
            std::env::temp_dir().join(format!("soundiya-tg-access-{}.json", std::process::id()));
        let mut access = Access::default();
        access.add(5, "Петя");
        access.save(&path).unwrap();
        assert_eq!(Access::load(&path), access);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let _ = fs::remove_file(&path);
        assert_eq!(Access::load(&path), Access::default());
    }
}
