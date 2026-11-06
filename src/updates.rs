//! New soundiya releases and news on the watched issue, from GitHub.
//!
//! Only reads public data. Installing happens only when an owner presses the button: the bot
//! downloads the release files and hands them to the station's own autoupdater script, which
//! checks the checksums, keeps the current build and rolls back by itself.
use crate::config::Config;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::Path,
    time::Duration,
};

pub const AUTOUPDATE: &str = "/data/opt/soundiya-autoupdate/soundiya-autoupdate.sh";
const APP: &str = "/data/opt/soundiya";
/// GitHub answers here are a few KB; anything bigger is refused.
const MAX_BODY: usize = 1 << 20;
/// A release file (the soundiya binary is a few MB).
const MAX_FILE: usize = 32 << 20;
/// The files of a soundiya release, as the autoupdater expects them.
const RELEASE_FILES: [&str; 7] = [
    "SHA256SUMS",
    "soundiya",
    "install.sh",
    "run.sh",
    "start.sh",
    "stop.sh",
    "S99soundiya",
];
const DOWNLOAD_DIR: &str = "/tmp/soundiya-tg-update";

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
    download: String,
    repo: String,
    issue_repo: String,
}

#[derive(Deserialize)]
struct LatestRelease {
    tag_name: String,
    html_url: String,
}

impl GitHub {
    pub fn new(config: &Config) -> Result<Self, String> {
        let trim = |url: &str| url.trim_end_matches('/').to_owned();
        let http = reqwest::Client::builder()
            .user_agent(concat!("soundiya-tg/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            // Release downloads redirect to GitHub's file storage: never down to plain HTTP.
            // Only tests point the endpoints at http://127.0.0.1.
            .https_only(
                [
                    &config.github_api,
                    &config.github_raw,
                    &config.github_download,
                ]
                .iter()
                .all(|url| url.starts_with("https://")),
            )
            .build()
            .map_err(|e| e.without_url().to_string())?;
        Ok(GitHub {
            http,
            api: trim(&config.github_api),
            raw: trim(&config.github_raw),
            download: trim(&config.github_download),
            repo: config.github_repo.clone(),
            issue_repo: config.issue_repo.clone(),
        })
    }

    async fn fetch(&self, url: &str, limit: usize, wait: Duration) -> Result<Vec<u8>, String> {
        let response = self
            .http
            .get(url)
            .timeout(wait)
            .send()
            .await
            .map_err(|e| e.without_url().to_string())?;
        if !response.status().is_success() {
            return Err(format!("GitHub {}", response.status()));
        }
        crate::telegram::read_limited(response, limit).await
    }

    async fn get(&self, url: &str) -> Result<String, String> {
        let body = self.fetch(url, MAX_BODY, Duration::from_secs(30)).await?;
        String::from_utf8(body).map_err(|_| "GitHub: not UTF-8".to_owned())
    }

    pub async fn latest(&self) -> Result<Release, String> {
        let text = self
            .get(&format!("{}/repos/{}/releases/latest", self.api, self.repo))
            .await?;
        let latest: LatestRelease = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        if !valid_tag(&latest.tag_name) {
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
            .get(&format!(
                "{}/repos/{}/issues/{number}",
                self.api, self.issue_repo
            ))
            .await?;
        serde_json::from_str(&text).map_err(|e| e.to_string())
    }

    /// Saves the files of release `tag` into `dir`.
    async fn download(&self, tag: &str, dir: &Path) -> Result<(), String> {
        for name in RELEASE_FILES {
            let url = format!(
                "{}/{}/releases/download/{tag}/{name}",
                self.download, self.repo
            );
            let body = self
                .fetch(&url, MAX_FILE, Duration::from_secs(180))
                .await
                .map_err(|e| format!("{name}: {e}"))?;
            fs::write(dir.join(name), body).map_err(|e| format!("{name}: {e}"))?;
        }
        Ok(())
    }

    /// Installs release `tag` with the station's autoupdater, which checks the checksums, keeps
    /// the current build in `prev/` and rolls back by itself if the service or the sound check
    /// fails. It also waits for a pause in the music.
    pub async fn install(&self, tag: &str, autoupdate: &str) -> Result<String, String> {
        if !valid_tag(tag) {
            return Err("unexpected release tag".into());
        }
        if !Path::new(autoupdate).exists() {
            return Err(format!("нет {autoupdate}"));
        }
        let dir = Path::new(DOWNLOAD_DIR);
        let _ = fs::remove_dir_all(dir);
        fs::DirBuilder::new()
            .mode(0o700)
            .create(dir)
            .map_err(|e| format!("{DOWNLOAD_DIR}: {e}"))?;
        let result = match self.download(tag, dir).await {
            Ok(()) => {
                let log = autoupdate_dir(autoupdate);
                run(
                    &format!(
                        "SU_SOURCE={DOWNLOAD_DIR} SU_AUTO_INSTALL=1 sh {autoupdate} once >/dev/null 2>&1; \
                         tail -n 3 {log}/update.log; \
                         grep -q ' / ubifs rw' /proc/mounts && mount -o remount,ro / ; true"
                    ),
                    Duration::from_secs(300),
                )
                .await
            }
            Err(e) => Err(format!("не скачалось: {e}")),
        };
        let _ = fs::remove_dir_all(dir);
        result
    }
}

/// Release tags go into URLs and callback data.
pub fn valid_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag.len() <= 40
        && tag
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

/// The autoupdater keeps its log and the previous build next to itself.
fn autoupdate_dir(autoupdate: &str) -> &str {
    autoupdate.rsplit_once('/').map_or("/", |(dir, _)| dir)
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

/// Puts back the build the autoupdater saved before the last install.
pub async fn rollback(autoupdate: &str) -> Result<String, String> {
    let prev = format!("{}/prev", autoupdate_dir(autoupdate));
    let script = format!(
        "[ -f {prev}/soundiya ] || {{ echo 'нет сохранённой сборки'; exit 1; }}; \
         {APP}/stop.sh >/dev/null 2>&1; \
         for f in soundiya install.sh run.sh start.sh stop.sh; do \
           [ -f {prev}/$f ] && cp {prev}/$f {APP}/$f.new && mv {APP}/$f.new {APP}/$f; \
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
