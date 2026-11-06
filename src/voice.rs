//! Voice messages: Telegram sends OGG/Opus; the station's own GStreamer (inside Alice's `maind`)
//! turns it into raw PCM, and soundiya's `VOICE` plays that over the paused music.
use std::{
    fs,
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::process::Command;

/// The folder soundiya takes voice files from (`VOICE_DIR` there); private to root.
pub const DIR: &str = "/tmp/soundiya-tg-voice";
pub const MAX_SECONDS: u64 = 60;
/// The OGG file as Telegram sends it.
pub const MAX_FILE: u64 = 2 << 20;
/// What soundiya plays: signed 16-bit little-endian mono at 16 kHz; a minute is 1.92 MB.
const RATE: u32 = 16_000;
const MAX_PCM: u64 = MAX_SECONDS * RATE as u64 * 2;
pub const DEFAULT_GST: &str = "/system/vendor/quasar/maind";
const GST_LIBS: &str = "/system/vendor/quasar/libs";
const DECODE_TIMEOUT: Duration = Duration::from_secs(30);

/// Refuses a message before it is downloaded, from what Telegram says about it.
pub fn limits(seconds: u64, size: Option<u64>) -> Result<(), &'static str> {
    if seconds > MAX_SECONDS {
        return Err("⛔ Голосовое длиннее 1 минуты — не включаю.");
    }
    if size.is_some_and(|s| s > MAX_FILE) {
        return Err("⛔ Голосовое больше 2 МБ — не включаю.");
    }
    Ok(())
}

/// Removes whatever a crash left behind and recreates the folder.
pub fn reset_dir() {
    let _ = fs::remove_dir_all(DIR);
    if let Err(e) = ensure_dir() {
        eprintln!("voice: {DIR}: {e}");
    }
}

fn ensure_dir() -> std::io::Result<()> {
    match fs::DirBuilder::new().mode(0o700).create(DIR) {
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        other => other,
    }
}

/// The two files of one message; both are deleted when this goes out of scope, whatever happens.
pub struct Files {
    pub ogg: PathBuf,
    pub pcm: PathBuf,
}
impl Files {
    /// `id` names the files: letters, digits and `-` only (soundiya checks the name too).
    pub fn new(id: &str) -> Self {
        let id: String = id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        let dir = Path::new(DIR);
        Files {
            ogg: dir.join(format!("{id}.oga")),
            pcm: dir.join(format!("{id}.pcm")),
        }
    }

    pub fn write_ogg(&self, data: &[u8]) -> Result<(), String> {
        ensure_dir().map_err(|e| format!("{DIR}: {e}"))?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&self.ogg)
            .map_err(|e| format!("{}: {e}", self.ogg.display()))?;
        file.write_all(data).map_err(|e| e.to_string())
    }

    /// OGG/Opus -> PCM with `maind --gst-launch`; the OGG is deleted right after.
    pub async fn decode(&self, gst: &str) -> Result<(), String> {
        let result = decode(gst, &self.ogg, &self.pcm).await;
        let _ = fs::remove_file(&self.ogg);
        result
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.ogg);
        let _ = fs::remove_file(&self.pcm);
    }
}

async fn decode(gst: &str, ogg: &Path, pcm: &Path) -> Result<(), String> {
    let source = format!("location={}", ogg.display());
    let sink = format!("location={}", pcm.display());
    let caps = format!("audio/x-raw,format=S16LE,rate={RATE},channels=1");
    let pipeline = [
        "filesrc",
        &source,
        "!",
        "oggdemux",
        "!",
        "opusdec",
        "!",
        "audioconvert",
        "!",
        "audioresample",
        "!",
        &caps,
        "!",
        "filesink",
        &sink,
    ];
    // No shell: every part is its own argument.
    let mut child = Command::new(gst)
        .env("LD_LIBRARY_PATH", GST_LIBS)
        .arg("--gst-launch")
        .args(pipeline)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("GStreamer: {e}"))?;
    tokio::time::timeout(DECODE_TIMEOUT, child.wait())
        .await
        .map_err(|_| "GStreamer не ответил за 30 с".to_owned())?
        .map_err(|e| format!("GStreamer: {e}"))?;
    // maind's exit code says little; the output does.
    match fs::metadata(pcm).map(|m| m.len()) {
        Ok(0) | Err(_) => Err("не удалось раскодировать голосовое".into()),
        Ok(n) if n > MAX_PCM => Err("голосовое длиннее 1 минуты".into()),
        Ok(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_follow_duration_and_size() {
        assert!(limits(60, Some(MAX_FILE)).is_ok());
        assert!(limits(5, None).is_ok());
        assert!(limits(61, Some(10)).unwrap_err().contains("1 минуты"));
        assert!(limits(5, Some(MAX_FILE + 1)).unwrap_err().contains("2 МБ"));
    }

    #[test]
    fn file_names_keep_only_safe_characters() {
        let files = Files::new("v42-7/../x y");
        assert_eq!(files.ogg, Path::new(DIR).join("v42-7xy.oga"));
        assert_eq!(files.pcm, Path::new(DIR).join("v42-7xy.pcm"));
    }
}
