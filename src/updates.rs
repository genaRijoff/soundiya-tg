//! New soundiya releases and news on the watched issue, from GitHub.
//!
//! Only reads public data; installing is done by the station's own autoupdater script, and only
//! when a listed user presses the button.
use serde::{Deserialize, Serialize};
use std::{fs, os::unix::fs::PermissionsExt, path::Path, time::Duration};

pub const AUTOUPDATE: &str = "/data/opt/soundiya-autoupdate/soundiya-autoupdate.sh";
pub const AUTOUPDATE_DIR: &str = "/data/opt/soundiya-autoupdate";
const APP: &str = "/data/opt/soundiya";
/// GitHub answers here are a few KB; anything bigger is refused.
const MAX_BODY: usize = 1 << 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub url: String,
    /// The release's `control.rs` knows `STATE`: the bot keeps working on it.
    pub has_state: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Issue {
    pub state: String,
    pub comments: u64,
    pub html_url: String,
}

/// What was already reported, kept next to config.json.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seen {
    pub release: Option<String>,
    /// A release the user chose not to install: not offered again.
    pub skipped: Option<String>,
    pub issue_state: Option<String>,
    pub issue_comments: Option<u64>,
}

impl Seen {
    pub fn load(path: &Path) -> Option<Seen> {
        serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
    }
    pub fn save(&self, path: &Path) {
        let tmp = path.with_extension("tmp");
        let text = serde_json::to_string_pretty(self).unwrap_or_default();
        let ok = fs::write(&tmp, text).is_ok()
            && fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600)).is_ok()
            && fs::rename(&tmp, path).is_ok();
        if !ok {
            eprintln!("updates: cannot save {}", path.display());
        }
    }
}

/// What to tell the user, by comparing a fresh check with what was reported before.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct News {
    pub release: Option<Release>,
    pub issue: Option<String>,
}

pub fn news(
    seen: &Seen,
    release: Option<&Release>,
    issue: Option<&Issue>,
    issue_number: u64,
) -> News {
    let release = release
        .filter(|r| {
            seen.release.as_deref() != Some(&r.tag) && seen.skipped.as_deref() != Some(&r.tag)
        })
        .cloned();
    let issue = issue.and_then(|i| {
        let closed = i.state == "closed" && seen.issue_state.as_deref() != Some("closed");
        let more = seen.issue_comments.is_some_and(|c| i.comments > c);
        match (closed, more) {
            (true, _) => Some(format!(
                "📌 Issue #{issue_number} закрыт автором: {}",
                i.html_url
            )),
            (false, true) => Some(format!(
                "💬 В issue #{issue_number} новый комментарий ({} всего): {}",
                i.comments, i.html_url
            )),
            _ => None,
        }
    });
    News { release, issue }
}

pub fn remember(seen: &mut Seen, release: Option<&Release>, issue: Option<&Issue>) {
    if let Some(r) = release {
        seen.release = Some(r.tag.clone());
    }
    if let Some(i) = issue {
        seen.issue_state = Some(i.state.clone());
        seen.issue_comments = Some(i.comments);
    }
}

pub struct GitHub {
    http: reqwest::Client,
    api: String,
    raw: String,
    repo: String,
}

#[derive(Deserialize)]
struct LatestRelease {
    tag_name: String,
    html_url: String,
}

impl GitHub {
    pub fn new(api: &str, raw: &str, repo: &str) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("soundiya-tg/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| e.without_url().to_string())?;
        Ok(GitHub {
            http,
            api: api.trim_end_matches('/').to_owned(),
            raw: raw.trim_end_matches('/').to_owned(),
            repo: repo.to_owned(),
        })
    }

    async fn get(&self, url: &str) -> Result<String, String> {
        let response = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| e.without_url().to_string())?;
        if !response.status().is_success() {
            return Err(format!("GitHub {}", response.status()));
        }
        let body = crate::telegram::read_limited(response, MAX_BODY).await?;
        String::from_utf8(body).map_err(|_| "GitHub: not UTF-8".to_owned())
    }

    pub async fn latest(&self) -> Result<Release, String> {
        let text = self
            .get(&format!("{}/repos/{}/releases/latest", self.api, self.repo))
            .await?;
        let latest: LatestRelease = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        if !latest
            .tag_name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
        {
            return Err("unexpected release tag".into());
        }
        // The tag's source tells whether the bot keeps working after the update.
        let control = self
            .get(&format!(
                "{}/{}/{}/src/control.rs",
                self.raw, self.repo, latest.tag_name
            ))
            .await;
        Ok(Release {
            has_state: control.ok().map(|text| text.contains("\"STATE\"")),
            tag: latest.tag_name,
            url: latest.html_url,
        })
    }

    pub async fn issue(&self, number: u64) -> Result<Issue, String> {
        let text = self
            .get(&format!("{}/repos/{}/issues/{number}", self.api, self.repo))
            .await?;
        serde_json::from_str(&text).map_err(|e| e.to_string())
    }
}

/// Runs a fixed shell command (no user input in it) and returns its exit status and output tail.
async fn run(script: &str, wait: Duration) -> Result<String, String> {
    let child = tokio::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(script)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let output = tokio::time::timeout(wait, child)
        .await
        .map_err(|_| "не уложилось во время".to_owned())?
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);
    let tail: Vec<&str> = text.lines().rev().take(6).collect();
    let tail = tail.into_iter().rev().collect::<Vec<_>>().join("\n");
    if output.status.success() {
        Ok(tail)
    } else {
        Err(tail)
    }
}

/// Installs the latest official release with the station's autoupdater: it downloads, checks the
/// checksums, keeps the current build in `prev/`, and rolls back by itself if the service or the
/// sound check fails.
pub async fn install_latest() -> Result<String, String> {
    if !Path::new(AUTOUPDATE).exists() {
        return Err(format!("нет {AUTOUPDATE}"));
    }
    let script = format!(
        "SU_AUTO_INSTALL=1 sh {AUTOUPDATE} once >/dev/null 2>&1; \
         tail -n 3 {AUTOUPDATE_DIR}/update.log; \
         grep -q ' / ubifs rw' /proc/mounts && mount -o remount,ro / ; true"
    );
    run(&script, Duration::from_secs(300)).await
}

/// Puts back the build the autoupdater saved before the last install.
pub async fn rollback() -> Result<String, String> {
    let script = format!(
        "[ -f {AUTOUPDATE_DIR}/prev/soundiya ] || {{ echo 'нет сохранённой сборки'; exit 1; }}; \
         {APP}/stop.sh >/dev/null 2>&1; \
         for f in soundiya install.sh run.sh start.sh stop.sh; do \
           [ -f {AUTOUPDATE_DIR}/prev/$f ] && cp {AUTOUPDATE_DIR}/prev/$f {APP}/$f.new && mv {APP}/$f.new {APP}/$f; \
         done; \
         {APP}/start.sh >/dev/null 2>&1; sha256sum {APP}/soundiya | cut -c1-12"
    );
    run(&script, Duration::from_secs(60)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str) -> Release {
        Release {
            tag: tag.into(),
            url: format!("https://x/{tag}"),
            has_state: Some(true),
        }
    }
    fn issue(state: &str, comments: u64) -> Issue {
        Issue {
            state: state.into(),
            comments,
            html_url: "https://x/5".into(),
        }
    }

    #[test]
    fn a_new_release_is_news_once_and_a_skipped_one_never() {
        let mut seen = Seen {
            release: Some("v0.1.10".into()),
            ..Seen::default()
        };
        assert_eq!(
            news(&seen, Some(&release("v0.1.10")), None, 5),
            News::default()
        );
        let n = news(&seen, Some(&release("v0.1.11")), None, 5);
        assert_eq!(n.release.unwrap().tag, "v0.1.11");
        remember(&mut seen, Some(&release("v0.1.11")), None);
        assert_eq!(
            news(&seen, Some(&release("v0.1.11")), None, 5),
            News::default()
        );
        seen.skipped = Some("v0.1.12".into());
        assert!(
            news(&seen, Some(&release("v0.1.12")), None, 5)
                .release
                .is_none()
        );
    }

    #[test]
    fn issue_news_is_a_comment_or_closing() {
        let mut seen = Seen::default();
        // The first look only records the issue.
        assert!(
            news(&seen, None, Some(&issue("open", 0)), 5)
                .issue
                .is_none()
        );
        remember(&mut seen, None, Some(&issue("open", 0)));
        assert!(
            news(&seen, None, Some(&issue("open", 0)), 5)
                .issue
                .is_none()
        );
        assert!(
            news(&seen, None, Some(&issue("open", 1)), 5)
                .issue
                .unwrap()
                .contains("комментарий")
        );
        assert!(
            news(&seen, None, Some(&issue("closed", 1)), 5)
                .issue
                .unwrap()
                .contains("закрыт")
        );
        remember(&mut seen, None, Some(&issue("closed", 1)));
        assert!(
            news(&seen, None, Some(&issue("closed", 1)), 5)
                .issue
                .is_none()
        );
    }

    #[test]
    fn seen_survives_a_save() {
        let path =
            std::env::temp_dir().join(format!("soundiya-tg-seen-{}.json", std::process::id()));
        let seen = Seen {
            release: Some("v1".into()),
            skipped: None,
            issue_state: Some("open".into()),
            issue_comments: Some(2),
        };
        seen.save(&path);
        assert_eq!(Seen::load(&path), Some(seen));
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let _ = fs::remove_file(&path);
    }
}
