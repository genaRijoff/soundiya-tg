//! The panel's PIN and the browsers signed in with it, kept in `panel.json` (0600) next to
//! config.json. A new PIN signs every browser out; wrong PINs in a row lock the sign-in.
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// Digits of a PIN.
const PIN_DIGITS: usize = 6;
/// Signed-in browsers kept; the oldest goes first.
const MAX_SESSIONS: usize = 20;
/// A browser stays signed in this long.
const SESSION_LIFE: Duration = Duration::from_secs(180 * 24 * 3600);
/// Wrong PINs in a row before the sign-in locks, and for how long.
pub const MAX_FAILS: u32 = 5;
pub const LOCK_FOR: Duration = Duration::from_secs(15 * 60);

#[derive(Default, Serialize, Deserialize)]
struct Stored {
    #[serde(default)]
    pin: String,
    #[serde(default)]
    sessions: Vec<Session>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Session {
    token: String,
    /// Seconds since 1970 when the browser signed in.
    since: u64,
}

pub struct Auth {
    path: PathBuf,
    stored: Stored,
    fails: u32,
    locked_until: Option<Instant>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Login {
    /// Signed in: the session token for the cookie.
    Ok(String),
    Wrong {
        left: u32,
    },
    /// This attempt used up the last try.
    JustLocked,
    /// Still locked for this long.
    Locked(Duration),
}

/// `N` bytes from the kernel's random source.
pub fn random_bytes<const N: usize>() -> Result<[u8; N], String> {
    let mut bytes = [0u8; N];
    fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .map_err(|e| format!("/dev/urandom: {e}"))?;
    Ok(bytes)
}

/// A random number below `n` (no bias: values past the last whole multiple are drawn again).
pub fn random_below(n: u32) -> Result<u32, String> {
    let limit = u32::MAX - u32::MAX % n;
    loop {
        let v = u32::from_le_bytes(random_bytes::<4>()?);
        if v < limit {
            return Ok(v % n);
        }
    }
}

fn random_pin() -> Result<String, String> {
    Ok(format!("{:06}", random_below(1_000_000)?))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Compares without stopping at the first difference, so timing tells nothing.
fn same(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn valid_pin(pin: &str) -> bool {
    pin.len() == PIN_DIGITS && pin.bytes().all(|b| b.is_ascii_digit())
}

impl Auth {
    /// Reads `panel.json`; a missing or broken PIN is replaced by a new random one.
    pub fn load(path: &Path) -> Auth {
        let stored: Stored = fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        let mut auth = Auth {
            path: path.to_owned(),
            stored,
            fails: 0,
            locked_until: None,
        };
        if !valid_pin(&auth.stored.pin) {
            match random_pin() {
                Ok(pin) => {
                    auth.stored = Stored {
                        pin,
                        sessions: Vec::new(),
                    };
                    if let Err(e) = auth.save() {
                        eprintln!("panel: {e}");
                    }
                }
                Err(e) => eprintln!("panel: no PIN ({e}); sign-in stays closed"),
            }
        }
        auth
    }

    fn save(&self) -> Result<(), String> {
        let tmp = self.path.with_extension("tmp");
        let text = serde_json::to_string_pretty(&self.stored).map_err(|e| e.to_string())?;
        let _ = fs::remove_file(&tmp);
        // Created 0600 from the start: the PIN and sessions are never readable by others.
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|e| format!("{}: {e}", tmp.display()))?;
        file.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        fs::rename(&tmp, &self.path).map_err(|e| e.to_string())
    }

    pub fn pin(&self) -> &str {
        &self.stored.pin
    }

    /// A new PIN; every signed-in browser is signed out.
    pub fn new_pin(&mut self) -> Result<(), String> {
        self.stored = Stored {
            pin: random_pin()?,
            sessions: Vec::new(),
        };
        self.fails = 0;
        self.locked_until = None;
        self.save()
    }

    pub fn login(&mut self, pin: &str, now: Instant) -> Login {
        if let Some(until) = self.locked_until {
            if now < until {
                return Login::Locked(until - now);
            }
            self.locked_until = None;
        }
        if valid_pin(&self.stored.pin) && same(pin.trim(), &self.stored.pin) {
            let token = match random_bytes::<32>() {
                Ok(bytes) => hex(&bytes),
                Err(e) => {
                    eprintln!("panel: {e}");
                    return Login::Wrong { left: MAX_FAILS };
                }
            };
            self.fails = 0;
            let (expired, limit) = (
                now_secs().saturating_sub(SESSION_LIFE.as_secs()),
                MAX_SESSIONS - 1,
            );
            self.stored.sessions.retain(|s| s.since > expired);
            if self.stored.sessions.len() > limit {
                let drop = self.stored.sessions.len() - limit;
                self.stored.sessions.drain(..drop);
            }
            self.stored.sessions.push(Session {
                token: token.clone(),
                since: now_secs(),
            });
            if let Err(e) = self.save() {
                eprintln!("panel: {e}");
            }
            return Login::Ok(token);
        }
        self.fails += 1;
        if self.fails >= MAX_FAILS {
            self.fails = 0;
            self.locked_until = Some(now + LOCK_FOR);
            return Login::JustLocked;
        }
        Login::Wrong {
            left: MAX_FAILS - self.fails,
        }
    }

    pub fn session_ok(&self, token: &str) -> bool {
        let expired = now_secs().saturating_sub(SESSION_LIFE.as_secs());
        // Every session is compared, so the time taken does not depend on which one matched.
        self.stored.sessions.iter().fold(false, |found, s| {
            (same(token, &s.token) && s.since > expired) | found
        })
    }

    pub fn logout(&mut self, token: &str) {
        let before = self.stored.sessions.len();
        self.stored.sessions.retain(|s| !same(token, &s.token));
        if self.stored.sessions.len() != before
            && let Err(e) = self.save()
        {
            eprintln!("panel: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("soundiya-tg-{name}-{}.json", std::process::id()));
        let _ = fs::remove_file(&path);
        path
    }

    #[test]
    fn a_pin_is_made_once_and_kept_private() {
        use std::os::unix::fs::PermissionsExt;
        let path = temp("pin");
        let auth = Auth::load(&path);
        assert!(valid_pin(auth.pin()), "{}", auth.pin());
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(Auth::load(&path).pin(), auth.pin(), "kept across restarts");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn sign_in_sessions_and_a_new_pin() {
        let path = temp("session");
        let mut auth = Auth::load(&path);
        let pin = auth.pin().to_owned();
        let Login::Ok(token) = auth.login(&format!(" {pin} "), Instant::now()) else {
            panic!()
        };
        assert_eq!(token.len(), 64);
        assert!(auth.session_ok(&token));
        assert!(Auth::load(&path).session_ok(&token), "kept across restarts");
        assert!(!auth.session_ok("0".repeat(64).as_str()));
        assert!(!auth.session_ok(""));
        auth.new_pin().unwrap();
        assert_ne!(auth.pin(), pin);
        assert!(!auth.session_ok(&token), "a new PIN signs everyone out");
        let pin = auth.pin().to_owned();
        let Login::Ok(token) = auth.login(&pin, Instant::now()) else {
            panic!()
        };
        auth.logout(&token);
        assert!(!auth.session_ok(&token));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn wrong_pins_lock_the_sign_in() {
        let path = temp("lock");
        let mut auth = Auth::load(&path);
        let pin = auth.pin().to_owned();
        let wrong = if pin == "000000" { "000001" } else { "000000" };
        let start = Instant::now();
        for left in (1..MAX_FAILS).rev() {
            assert_eq!(auth.login(wrong, start), Login::Wrong { left });
        }
        assert_eq!(auth.login(wrong, start), Login::JustLocked);
        // Locked: even the right PIN waits.
        assert!(matches!(
            auth.login(&pin, start + Duration::from_secs(60)),
            Login::Locked(_)
        ));
        assert!(matches!(auth.login(&pin, start + LOCK_FOR), Login::Ok(_)));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn sessions_are_bounded() {
        let path = temp("many");
        let mut auth = Auth::load(&path);
        let pin = auth.pin().to_owned();
        let tokens: Vec<String> = (0..MAX_SESSIONS + 3)
            .map(|_| match auth.login(&pin, Instant::now()) {
                Login::Ok(t) => t,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(auth.stored.sessions.len(), MAX_SESSIONS);
        assert!(!auth.session_ok(&tokens[0]), "the oldest went first");
        assert!(auth.session_ok(tokens.last().unwrap()));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn random_numbers_stay_in_range() {
        for _ in 0..200 {
            assert!(random_below(7).unwrap() < 7);
        }
        assert!(same("abc", "abc") && !same("abc", "abd") && !same("abc", "ab"));
    }
}
