//! Voice messages: Telegram sends OGG/Opus; the station's own GStreamer (inside Alice's `maind`)
//! turns it into raw PCM, and soundiya's `VOICE` plays that over the paused music.
use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::process::Command;

/// The folder soundiya takes voice files from (`VOICE_DIR` there). Root's files in it are
/// private; `nobody` may only pass through to the work folder of the message being decoded.
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
/// How often the decoder's output size is checked; at most ~0.3 MB more gets written meanwhile.
const WATCH_EVERY: Duration = Duration::from_millis(50);
/// `/voicevol` range, percent of the automatic level.
pub const VOLUME_MIN: u16 = 50;
pub const VOLUME_MAX: u16 = 400;
/// Speech is brought to this RMS (about -11 dBFS), measured only where there is sound:
/// loud enough next to music mastered as loud as SoundCloud's.
const TARGET_RMS: f64 = 9000.0;
/// ...but a near-silent recording is not raised more than this (+24 dB; more is all noise).
const MAX_BOOST: f64 = 16.0;
/// 20 ms windows quieter than this (about -50 dBFS) are pauses and do not count.
const GATE_RMS: f64 = 100.0;
/// Above this share of full scale peaks are rounded off smoothly instead of clipping.
const KNEE: f64 = 0.7;

/// The RMS of the speech: 20 ms windows above the gate only. None for silence.
pub fn speech_rms(samples: &[i16]) -> Option<f64> {
    let window = RATE as usize / 50;
    let (mut power, mut windows) = (0.0, 0u32);
    for chunk in samples.chunks(window) {
        let mean = chunk.iter().map(|&s| f64::from(s).powi(2)).sum::<f64>() / chunk.len() as f64;
        if mean.sqrt() > GATE_RMS {
            power += mean;
            windows += 1;
        }
    }
    (windows > 0).then(|| (power / f64::from(windows)).sqrt())
}

/// Brings speech to one loudness, times `percent` / 100, without hard clipping.
/// Returns the gain applied.
pub fn level(samples: &mut [i16], percent: u16) -> f64 {
    let auto = speech_rms(samples).map_or(1.0, |rms| (TARGET_RMS / rms).min(MAX_BOOST));
    let gain = auto * f64::from(percent.clamp(VOLUME_MIN, VOLUME_MAX)) / 100.0;
    for sample in samples.iter_mut() {
        let x = f64::from(*sample) * gain / 32768.0;
        let y = if x.abs() > KNEE {
            x.signum() * (KNEE + (1.0 - KNEE) * ((x.abs() - KNEE) / (1.0 - KNEE)).tanh())
        } else {
            x
        };
        *sample = (y * 32767.0).round().clamp(-32768.0, 32767.0) as i16;
    }
    gain
}

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
    match fs::DirBuilder::new().mode(0o711).create(DIR) {
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        other => other?,
    }
    // Searchable, not listable: `nobody` reaches only the work folder it is given.
    fs::set_permissions(DIR, fs::Permissions::from_mode(0o711))
}

/// The unprivileged user the decoder runs as, from /etc/passwd; None when this process is
/// not root (tests, a manual run), so there is nothing to drop.
fn nobody() -> Option<(u32, u32)> {
    // /proc/self belongs to this process's user.
    if fs::metadata("/proc/self").map_or(true, |m| m.uid() != 0) {
        return None;
    }
    let passwd = fs::read_to_string("/etc/passwd").ok();
    let entry = passwd.as_deref().and_then(|text| {
        text.lines().find_map(|line| {
            let mut fields = line.split(':');
            (fields.next() == Some("nobody")).then(|| {
                let _password = fields.next();
                Some((fields.next()?.parse().ok()?, fields.next()?.parse().ok()?))
            })?
        })
    });
    Some(entry.unwrap_or((65534, 65534)))
}

/// The files of one message; all are deleted when this goes out of scope, whatever happens.
/// `ogg` and the decoder's output live in `work`, a folder owned by the decoder's user;
/// `pcm`, what soundiya gets, is root's.
pub struct Files {
    work: PathBuf,
    pub ogg: PathBuf,
    decoded: PathBuf,
    pub pcm: PathBuf,
    /// The decoder's output, in root's folder where `nobody` cannot swap it for a link.
    log: PathBuf,
}
impl Files {
    /// `id` names the files: letters, digits and `-` only (soundiya checks the name too).
    pub fn new(id: &str) -> Self {
        let id: String = id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        let dir = Path::new(DIR);
        let work = dir.join(format!("{id}.d"));
        Files {
            ogg: work.join("in.oga"),
            decoded: work.join("out.pcm"),
            pcm: dir.join(format!("{id}.pcm")),
            log: dir.join(format!("{id}.log")),
            work,
        }
    }

    pub fn write_ogg(&self, data: &[u8]) -> Result<(), String> {
        ensure_dir().map_err(|e| format!("{DIR}: {e}"))?;
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&self.work)
            .map_err(|e| format!("{}: {e}", self.work.display()))?;
        if let Some((uid, gid)) = nobody() {
            std::os::unix::fs::chown(&self.work, Some(uid), Some(gid))
                .map_err(|e| format!("{}: {e}", self.work.display()))?;
        }
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&self.ogg)
            .map_err(|e| format!("{}: {e}", self.ogg.display()))?;
        // The bot runs with umask 077 (run.sh): the decoder can read its input only as its owner.
        if let Some((uid, gid)) = nobody() {
            std::os::unix::fs::chown(&self.ogg, Some(uid), Some(gid))
                .map_err(|e| format!("{}: {e}", self.ogg.display()))?;
        }
        file.write_all(data).map_err(|e| e.to_string())
    }

    /// Applies [`level`] to the decoded file in place; returns a line for the log (numbers only).
    pub fn level(&self, percent: u16) -> Result<String, String> {
        let bytes = fs::read(&self.pcm).map_err(|e| e.to_string())?;
        let mut samples: Vec<i16> = bytes
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        let before = speech_rms(&samples);
        let gain = level(&mut samples, percent);
        let out: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        fs::write(&self.pcm, out).map_err(|e| e.to_string())?;
        let db = |rms: f64| 20.0 * (rms / 32768.0).log10();
        Ok(match (before, speech_rms(&samples)) {
            (Some(a), Some(b)) => format!(
                "voice: speech {:.0} dBFS -> {:.0} dBFS (gain x{gain:.1}, /voicevol {percent} %)",
                db(a),
                db(b)
            ),
            _ => format!("voice: silence (/voicevol {percent} %)"),
        })
    }

    /// OGG/Opus -> PCM with `maind --gst-launch`, run as `nobody`: a crafted file that breaks
    /// the old GStreamer gets no root. The result is copied out of the work folder (checked,
    /// see [`take_decoded`]) and the work folder with the OGG is deleted right after.
    pub async fn decode(&self, gst: &str) -> Result<(), String> {
        let result = match decode(gst, &self.ogg, &self.decoded, nobody(), &self.log).await {
            Ok(()) => take_decoded(&self.decoded, &self.pcm),
            Err(e) => {
                log_decoder_errors(&self.log);
                Err(e)
            }
        };
        let _ = fs::remove_file(&self.log);
        let _ = fs::remove_dir_all(&self.work);
        result
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.work);
        let _ = fs::remove_file(&self.pcm);
        let _ = fs::remove_file(&self.log);
    }
}

/// GStreamer's own complaints, so a failure says why (paths and errors only, no audio).
fn log_decoder_errors(log: &Path) {
    let Ok(text) = fs::read(log) else {
        return eprintln!("voice: the decoder wrote no log");
    };
    let text = String::from_utf8_lossy(&text);
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| {
            let l = l.to_ascii_lowercase();
            ["error", "denied", "permission", "fail", "no such", "warn"]
                .iter()
                .any(|w| l.contains(w))
        })
        .collect();
    for line in &lines[lines.len().saturating_sub(5)..] {
        eprintln!("voice: gst: {}", line.chars().take(200).collect::<String>());
    }
}

/// Copies the decoder's output into root's file. The work folder belongs to the decoder's
/// user, so its output is not trusted: it must be a plain file (no link to something of
/// root's), the same one that was checked, and at most a minute long.
fn take_decoded(from: &Path, to: &Path) -> Result<(), String> {
    let checked = fs::symlink_metadata(from).map_err(|_| "нет результата раскодирования")?;
    if !checked.file_type().is_file() {
        return Err("результат раскодирования — не обычный файл".into());
    }
    let file = fs::File::open(from).map_err(|e| e.to_string())?;
    let opened = file.metadata().map_err(|e| e.to_string())?;
    if (opened.dev(), opened.ino()) != (checked.dev(), checked.ino()) {
        return Err("результат раскодирования подменён".into());
    }
    if opened.len() > MAX_PCM {
        return Err("голосовое длиннее 1 минуты".into());
    }
    let mut data = Vec::new();
    file.take(MAX_PCM + 1)
        .read_to_end(&mut data)
        .map_err(|e| e.to_string())?;
    if data.is_empty() {
        return Err("не удалось раскодировать голосовое".into());
    }
    if data.len() as u64 > MAX_PCM {
        return Err("голосовое длиннее 1 минуты".into());
    }
    let mut out = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(to)
        .map_err(|e| format!("{}: {e}", to.display()))?;
    out.write_all(&data).map_err(|e| e.to_string())
}

async fn decode(
    gst: &str,
    ogg: &Path,
    pcm: &Path,
    user: Option<(u32, u32)>,
    log: &Path,
) -> Result<(), String> {
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
    // The decoder works in the message's folder: the bot's own working directory may be gone
    // (the installer's) or closed to `nobody`.
    let work = ogg.parent().unwrap_or(Path::new("/tmp"));
    let log = fs::File::create(log).map_err(|e| format!("GStreamer log: {e}"))?;
    let log_err = log.try_clone().map_err(|e| e.to_string())?;
    let mut command = Command::new(gst);
    if let Some((uid, gid)) = user {
        // Supplementary groups are dropped too when the uid changes from root.
        command.uid(uid).gid(gid);
    }
    let mut child = command
        .current_dir(work)
        .env("HOME", work)
        .env("LD_LIBRARY_PATH", GST_LIBS)
        .arg("--gst-launch")
        .args(pipeline)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err))
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("GStreamer: {e}"))?;
    // The duration in the message comes from the sender's app and can be a lie: 2 MB of
    // low-bitrate Opus would decode to tens of MB in the station's RAM. Watch the output and
    // stop at a minute's worth.
    let started = std::time::Instant::now();
    let size = || fs::symlink_metadata(pcm).map_or(0, |m| m.len());
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(e) => return Err(format!("GStreamer: {e}")),
        }
        if size() > MAX_PCM {
            let _ = child.kill().await;
            return Err("голосовое длиннее 1 минуты".into());
        }
        if started.elapsed() > DECODE_TIMEOUT {
            let _ = child.kill().await;
            return Err("GStreamer не ответил за 30 с".into());
        }
        tokio::time::sleep(WATCH_EVERY).await;
    }
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

    fn tone(amplitude: f64, seconds: usize) -> Vec<i16> {
        (0..RATE as usize * seconds)
            .map(|n| {
                (amplitude * (n as f64 * 440.0 * std::f64::consts::TAU / 16_000.0).sin()) as i16
            })
            .collect()
    }
    fn rms(samples: &[i16]) -> f64 {
        (samples.iter().map(|&s| f64::from(s).powi(2)).sum::<f64>() / samples.len() as f64).sqrt()
    }

    #[test]
    fn quiet_and_loud_speech_end_up_alike() {
        let mut quiet = tone(1500.0, 1);
        let mut loud = tone(20_000.0, 1);
        level(&mut quiet, 100);
        level(&mut loud, 100);
        assert!((rms(&quiet) - TARGET_RMS).abs() < 200.0, "{}", rms(&quiet));
        assert!((rms(&loud) - TARGET_RMS).abs() < 200.0, "{}", rms(&loud));
    }

    #[test]
    fn pauses_do_not_count_and_near_silence_is_not_blown_up() {
        // Half a second of speech, then a second of silence: leveled as the speech alone.
        let mut speech = tone(1500.0, 1)[..8000].to_vec();
        speech.extend(std::iter::repeat_n(0, 16_000));
        level(&mut speech, 100);
        assert!((rms(&speech[..8000]) - TARGET_RMS).abs() < 200.0);
        assert!(speech[8000..].iter().all(|&s| s == 0));
        // A faint hiss is raised at most MAX_BOOST times.
        let mut hiss = tone(400.0, 1);
        let before = rms(&hiss);
        assert_eq!(level(&mut hiss, 100), MAX_BOOST);
        assert!((rms(&hiss) / before - MAX_BOOST).abs() < 0.05);
        let mut silence = vec![0i16; 1000];
        assert_eq!(level(&mut silence, 150), 1.5);
        assert!(silence.iter().all(|&s| s == 0));
    }

    #[test]
    fn the_owner_level_scales_and_peaks_never_clip() {
        let mut normal = tone(1500.0, 1);
        let mut double = tone(1500.0, 1);
        let g1 = level(&mut normal, 100);
        let g2 = level(&mut double, 200);
        assert!((g2 / g1 - 2.0).abs() < 1e-9);
        // Speech has sharp peaks: at 200 % they pass the knee and are rounded off, never wrapped.
        let mut spiky = tone(1000.0, 1);
        for s in spiky.iter_mut().step_by(100) {
            *s = 30_000;
        }
        level(&mut spiky, 200);
        let knee = (KNEE * 32767.0) as i16;
        assert!(spiky.iter().step_by(100).all(|&s| s > knee));
        assert!(spiky.iter().all(|&s| s > -32767));
        // Outside 50-400 is held to the range.
        let (mut a, mut b) = (tone(1500.0, 1), tone(1500.0, 1));
        assert_eq!(level(&mut a, 500), level(&mut b, 400));
    }

    /// A stand-in decoder (`sh -c`) and its output file in a test folder.
    fn fake_gst(name: &str, body: &str) -> (String, PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "soundiya-tg-voice-test-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let script = dir.join("gst.sh");
        fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let (ogg, pcm) = (dir.join("v.oga"), dir.join("v.pcm"));
        fs::write(&ogg, b"OggS").unwrap();
        (script.to_str().unwrap().to_owned(), ogg, pcm)
    }
    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    #[test]
    fn a_lying_duration_cannot_fill_the_ram() {
        // Writes without end, as a long recording with a short claimed duration would.
        let (gst, ogg, pcm) = fake_gst(
            "endless",
            &format!(
                "while :; do head -c 65536 /dev/zero; done > {}",
                "\"$(echo \"$@\" | sed 's/.*location=//')\""
            ),
        );
        let started = std::time::Instant::now();
        let result = runtime().block_on(decode(&gst, &ogg, &pcm, None, &pcm.with_extension("log")));
        assert_eq!(result.unwrap_err(), "голосовое длиннее 1 минуты");
        assert!(started.elapsed() < Duration::from_secs(10));
        // Stopped soon after the limit, not long after.
        assert!(fs::metadata(&pcm).unwrap().len() < MAX_PCM + (8 << 20));
        let _ = fs::remove_dir_all(pcm.parent().unwrap());
    }

    #[test]
    fn a_normal_message_decodes_and_an_empty_one_fails() {
        let (gst, ogg, pcm) = fake_gst(
            "ok",
            "head -c 64000 /dev/zero > \"$(echo \"$@\" | sed 's/.*location=//')\"",
        );
        assert!(
            runtime()
                .block_on(decode(&gst, &ogg, &pcm, None, &pcm.with_extension("log")))
                .is_ok()
        );
        assert_eq!(fs::metadata(&pcm).unwrap().len(), 64000);
        let (gst, ogg, pcm) = fake_gst("empty", "exit 0");
        assert!(
            runtime()
                .block_on(decode(&gst, &ogg, &pcm, None, &pcm.with_extension("log")))
                .is_err()
        );
        for dir in ["ok", "empty"] {
            let _ = fs::remove_dir_all(std::env::temp_dir().join(format!(
                "soundiya-tg-voice-test-{}-{dir}",
                std::process::id()
            )));
        }
    }

    #[test]
    fn the_decoders_output_is_taken_only_as_a_plain_file() {
        let dir = std::env::temp_dir().join(format!("soundiya-tg-take-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let (from, to) = (dir.join("out.pcm"), dir.join("v.pcm"));
        // A link to a secret is refused, and the secret is not copied.
        let secret = dir.join("secret");
        fs::write(&secret, b"root only").unwrap();
        std::os::unix::fs::symlink(&secret, &from).unwrap();
        assert!(
            take_decoded(&from, &to)
                .unwrap_err()
                .contains("не обычный файл")
        );
        assert!(!to.exists());
        fs::remove_file(&from).unwrap();
        // Too long is refused; a normal one is copied, private to root.
        fs::write(&from, vec![0u8; MAX_PCM as usize + 2]).unwrap();
        assert!(take_decoded(&from, &to).unwrap_err().contains("1 минуты"));
        fs::write(&from, [1u8, 2, 3, 4]).unwrap();
        take_decoded(&from, &to).unwrap();
        assert_eq!(fs::read(&to).unwrap(), [1, 2, 3, 4]);
        assert_eq!(
            fs::metadata(&to).unwrap().permissions().mode() & 0o777,
            0o600
        );
        // The target is never overwritten (create_new): a planted file stops it.
        assert!(take_decoded(&from, &to).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_names_keep_only_safe_characters() {
        let files = Files::new("v42-7/../x y");
        assert_eq!(files.ogg, Path::new(DIR).join("v42-7xy.d/in.oga"));
        assert_eq!(files.pcm, Path::new(DIR).join("v42-7xy.pcm"));
    }
}
