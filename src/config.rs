//! `config.json`: the bot token and who may use the bot.
use serde::Deserialize;
use std::{fs, os::unix::fs::PermissionsExt, path::Path};

pub const DEFAULT_PATH: &str = "/data/opt/soundiya-tg/config.json";
const DEFAULT_API: &str = "https://api.telegram.org";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// From @BotFather.
    pub token: String,
    /// Telegram user ids allowed to use the bot (from @userinfobot); everyone else is ignored.
    pub users: Vec<i64>,
    /// A new track posts a new card with a notification sound; `false` posts it silently.
    #[serde(default = "yes")]
    pub notify: bool,
    /// The Bot API server; only tests and self-hosted Bot API servers change it.
    #[serde(default = "default_api")]
    pub api: String,
    /// Check GitHub for new soundiya releases and for news on `watch_issue`.
    #[serde(default = "yes")]
    pub updates: bool,
    /// The soundiya repository, `owner/name`.
    #[serde(default = "default_repo")]
    pub github_repo: String,
    /// An issue in `github_repo` to report news about (an answer, closed); 0, the default, turns it off.
    #[serde(default = "default_issue")]
    pub watch_issue: u64,
    /// GitHub endpoints; only tests change them.
    #[serde(default = "default_github_api")]
    pub github_api: String,
    #[serde(default = "default_github_raw")]
    pub github_raw: String,
}

fn yes() -> bool {
    true
}
fn default_api() -> String {
    DEFAULT_API.to_owned()
}
fn default_repo() -> String {
    "ShadowBlade83/soundiya".to_owned()
}
fn default_issue() -> u64 {
    0
}
fn default_github_api() -> String {
    "https://api.github.com".to_owned()
}
fn default_github_raw() -> String {
    "https://raw.githubusercontent.com".to_owned()
}
fn secure_url(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://127.0.0.1")
}

impl Config {
    pub fn load(path: &Path) -> Result<Self, String> {
        let meta = fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
        // The file holds the bot token: whoever reads it controls the bot.
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(format!(
                "{} is readable by others; run: chmod 600 {0}",
                path.display()
            ));
        }
        let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let config: Config =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), String> {
        if !valid_token(&self.token) {
            return Err(
                "token does not look like a bot token from @BotFather (123456:ABC...)".into(),
            );
        }
        if self.users.is_empty() || self.users.iter().any(|&id| id <= 0) {
            return Err("users must list your numeric Telegram id (from @userinfobot)".into());
        }
        if !secure_url(&self.api) || !secure_url(&self.github_api) || !secure_url(&self.github_raw)
        {
            return Err("api, github_api and github_raw must be https:// URLs".into());
        }
        let name = |part: &str| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        };
        match self.github_repo.split_once('/') {
            Some((owner, repo)) if name(owner) && name(repo) => Ok(()),
            _ => Err("github_repo must look like owner/name".into()),
        }
    }

    pub fn allowed(&self, user: i64) -> bool {
        self.users.contains(&user)
    }
}

fn valid_token(token: &str) -> bool {
    let Some((id, secret)) = token.split_once(':') else {
        return false;
    };
    !id.is_empty()
        && id.bytes().all(|b| b.is_ascii_digit())
        && secret.len() >= 30
        && secret
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const TOKEN: &str = "123456:AAbbCCddEEffGGhhIIjjKKllMMnnOOpp_-";

    fn write(mode: u32, text: &str) -> tempfile::Path {
        tempfile::write(mode, text)
    }

    #[test]
    fn accepts_a_private_valid_config() {
        let file = write(0o600, &format!(r#"{{"token":"{TOKEN}","users":[42]}}"#));
        let config = Config::load(file.path()).unwrap();
        assert!(config.allowed(42));
        assert!(!config.allowed(43));
        assert!(config.notify);
        assert_eq!(config.api, "https://api.telegram.org");
        assert!(config.updates);
        assert_eq!(config.github_repo, "ShadowBlade83/soundiya");
        assert_eq!(config.watch_issue, 0);
    }

    #[test]
    fn refuses_a_config_others_can_read() {
        let file = write(0o644, &format!(r#"{{"token":"{TOKEN}","users":[42]}}"#));
        assert!(
            Config::load(file.path())
                .err()
                .unwrap()
                .contains("chmod 600")
        );
    }

    #[test]
    fn refuses_bad_values() {
        for text in [
            r#"{"token":"nope","users":[42]}"#.to_owned(),
            format!(r#"{{"token":"{TOKEN}","users":[]}}"#),
            format!(r#"{{"token":"{TOKEN}","users":[-5]}}"#),
            format!(r#"{{"token":"{TOKEN}","users":[42],"api":"http://evil"}}"#),
            format!(r#"{{"token":"{TOKEN}","users":[42],"typo":1}}"#),
            format!(r#"{{"token":"{TOKEN}","users":[42],"github_repo":"a/b/../c"}}"#),
            format!(r#"{{"token":"{TOKEN}","users":[42],"github_api":"http://evil"}}"#),
        ] {
            let file = write(0o600, &text);
            assert!(Config::load(file.path()).is_err(), "{text}");
        }
    }

    /// A tiny temporary file helper, to avoid a dependency for three tests.
    mod tempfile {
        use super::*;
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        pub struct Path(std::path::PathBuf);
        impl Path {
            pub fn path(&self) -> &std::path::Path {
                &self.0
            }
        }
        impl Drop for Path {
            fn drop(&mut self) {
                let _ = fs::remove_file(&self.0);
            }
        }
        pub fn write(mode: u32, text: &str) -> Path {
            let n = N.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("soundiya-tg-{}-{n}.json", std::process::id()));
            let mut file = fs::File::create(&path).unwrap();
            file.write_all(text.as_bytes()).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
            Path(path)
        }
    }
}
