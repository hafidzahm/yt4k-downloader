//! Engine download.
//!
//! Keputusan arsitektur (Nov 2026): extractor YouTube murni-Rust (`rusty_ytdl`
//! 0.7.4, rilis terakhir Agustus 2024) tidak punya jalur JS-runtime / PO-token
//! sehingga gampang patah setiap YouTube mengganti player. Karena itu aplikasi
//! Rust ini mengelola `yt-dlp` sebagai sidecar (di samping .exe, di folder bin
//! aplikasi, atau di PATH) — pola yang sama yang dipakai crate Rust `yt-dlp`
//! (boul2gom): Rust yang mengatur UI, antrean, progress, log, dan merge
//! FFmpeg; yt-dlp yang mengekstrak format. Semua stdout/stderr yt-dlp dan
//! FFmpeg diteruskan ke Logger supaya bisa didiagnosis.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::sync::mpsc::Sender;

use crate::log::{Level, Logger};

// ---------- URL / ID ----------

/// Ambil video ID (11 karakter) dari berbagai bentuk input:
/// URL watch, youtu.be, shorts, live, embed, atau ID mentah.
pub fn extract_video_id(input: &str) -> Option<String> {
    let s = input.trim();
    let is_id = |t: &str| {
        t.len() == 11
            && t.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    };
    if is_id(s) {
        return Some(s.to_string());
    }
    for marker in ["watch?v=", "youtu.be/", "/shorts/", "/live/", "/embed/", "&v="] {
        if let Some(idx) = s.find(marker) {
            let rest = &s[idx + marker.len()..];
            let id: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                .collect();
            if is_id(&id) {
                return Some(id);
            }
        }
    }
    None
}

pub fn canonical_url(input: &str) -> Option<String> {
    extract_video_id(input).map(|id| format!("https://www.youtube.com/watch?v={id}"))
}

// ---------- Kualitas ----------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Quality {
    Q2160,
    Q1440,
    Q1080,
    Q720,
    AudioOnly,
}

impl Quality {
    pub fn all() -> [Quality; 5] {
        [
            Quality::Q2160,
            Quality::Q1440,
            Quality::Q1080,
            Quality::Q720,
            Quality::AudioOnly,
        ]
    }
    pub fn label(&self) -> &'static str {
        match self {
            Quality::Q2160 => "2160p (4K)",
            Quality::Q1440 => "1440p (2K)",
            Quality::Q1080 => "1080p",
            Quality::Q720 => "720p",
            Quality::AudioOnly => "Audio saja",
        }
    }
    pub fn max_height(&self) -> Option<u32> {
        match self {
            Quality::Q2160 => Some(2160),
            Quality::Q1440 => Some(1440),
            Quality::Q1080 => Some(1080),
            Quality::Q720 => Some(720),
            Quality::AudioOnly => None,
        }
    }
    /// Format selector yt-dlp. Untuk >720p YouTube hanya menyediakan stream
    /// adaptif (video dan audio terpisah), jadi selector selalu meminta
    /// bestvideo+ bestaudio dan membiarkan yt-dlp + FFmpeg menggabungkannya.
    pub fn format_selector(&self) -> String {
        match self.max_height() {
            Some(h) => format!("bv*[height<={h}]+ba/b[height<={h}]/bv*+ba/b"),
            None => "ba/b".to_string(),
        }
    }
}

// ---------- Rate limiter (Mode Aman) ----------

/// Preset "Mode Aman" (v0.1.1): pacing request ala rekomendasi komunitas
/// yt-dlp untuk menekan risiko HTTP 429 / cek bot.
/// - Jeda antar-request ekstraksi (`--sleep-requests`)
/// - Jeda acak antar-download (`--sleep-interval` s.d. `--max-sleep-interval`)
///   — acak penting; jeda tetap yang identik bisa jadi fingerprint.
/// - Fragment paralel tetap 1 (default yt-dlp; jangan dinaikkan).
/// - Retry TERBATAS dengan backoff eksponensial — bukan retry tanpa batas,
///   karena menembak ulang saat kena 429 justru memperpanjang blokir.
/// - Cap bandwidth OPSIONAL dan default mati: efek perlindungannya paling
///   lemah dan file 4K jadi sangat lama. Diisi dari pilihan UI ("5M" dsb).
#[derive(Clone, Debug, PartialEq)]
pub struct RateLimitConfig {
    pub enabled: bool,
    pub sleep_requests_secs: f32,
    pub sleep_interval_secs: u32,
    pub max_sleep_interval_secs: u32,
    pub concurrent_fragments: u32,
    pub retries: u32,
    pub fragment_retries: u32,
    pub limit_rate: Option<String>,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            sleep_requests_secs: 1.5,
            sleep_interval_secs: 5,
            max_sleep_interval_secs: 15,
            concurrent_fragments: 1,
            retries: 5,
            fragment_retries: 10,
            limit_rate: None,
        }
    }
}

impl RateLimitConfig {
    /// Argumen pacing untuk proses download.
    pub fn download_args(&self) -> Vec<String> {
        if !self.enabled {
            return Vec::new();
        }
        let mut a: Vec<String> = vec![
            "--sleep-requests".into(),
            format!("{}", self.sleep_requests_secs),
            "--sleep-interval".into(),
            self.sleep_interval_secs.to_string(),
            "--max-sleep-interval".into(),
            self.max_sleep_interval_secs.to_string(),
            "--concurrent-fragments".into(),
            self.concurrent_fragments.to_string(),
            "--retries".into(),
            self.retries.to_string(),
            "--fragment-retries".into(),
            self.fragment_retries.to_string(),
            // Backoff eksponensial: jeda retry membesar (1s -> maks 30s).
            "--retry-sleep".into(),
            "http:exp=1:30".into(),
            "--retry-sleep".into(),
            "fragment:exp=1:20".into(),
        ];
        if let Some(rate) = &self.limit_rate {
            a.push("--limit-rate".into());
            a.push(rate.clone());
        }
        a
    }

    /// Argumen pacing untuk proses cek info (hanya jeda antar-request).
    pub fn info_args(&self) -> Vec<String> {
        if !self.enabled {
            return Vec::new();
        }
        vec![
            "--sleep-requests".into(),
            format!("{}", self.sleep_requests_secs),
        ]
    }

    pub fn describe(&self) -> String {
        if !self.enabled {
            return "Mode Aman: MATI".to_string();
        }
        format!(
            "Mode Aman: AKTIF (sleep-requests {}s, jeda download {}-{}s acak, fragment {}, retries {}/{}, retry backoff exp, limit-rate {})",
            self.sleep_requests_secs,
            self.sleep_interval_secs,
            self.max_sleep_interval_secs,
            self.concurrent_fragments,
            self.retries,
            self.fragment_retries,
            self.limit_rate.as_deref().unwrap_or("mati")
        )
    }
}

/// Deteksi baris log / pesan error yang menandakan rate limit YouTube.
pub fn is_rate_limit_text(text: &str) -> bool {
    text.contains("429") || text.to_lowercase().contains("too many requests")
}

/// Durasi cooldown antar job (detik), diacak 5..=15 dari jam sistem —
/// cukup untuk pacing level aplikasi tanpa crate rand.
pub fn jitter_cooldown_secs() -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(7);
    5 + (nanos % 11)
}

/// Lama lockout setelah 429 terdeteksi (detik): 15 menit.
pub const RATE_LIMIT_LOCKOUT_SECS: u64 = 15 * 60;

// ---------- Argumen yt-dlp ----------

pub fn build_info_args(url: &str, rate: &RateLimitConfig) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "--dump-single-json".into(),
        "--no-playlist".into(),
        "--no-warnings".into(),
    ];
    args.extend(rate.info_args());
    args.push(url.into());
    args
}

pub fn build_download_args(
    url: &str,
    quality: Quality,
    output_template: &str,
    ffmpeg_location: Option<&Path>,
    verbose: bool,
    rate: &RateLimitConfig,
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "--newline".into(),
        "--progress".into(),
        "--no-playlist".into(),
        "--no-mtime".into(),
        // Resume WAJIB eksplisit: bila download putus di tengah (jaringan,
        // laptop sleep, 403 sesaat), file .part TIDAK dibuang — klik
        // Download lagi dengan kualitas yang sama melanjutkan dari byte
        // terakhir, bukan mengulang dari nol. File final (.mkv) hanya
        // dibuat setelah semua stream lengkap dan merge selesai, jadi
        // file hasil tidak pernah korup gara-gara putus di tengah.
        "--continue".into(),
        "-f".into(),
        quality.format_selector(),
        "-o".into(),
        output_template.into(),
        // Cetak path final setelah merge supaya UI tahu file hasilnya.
        "--print".into(),
        "after_move:filepath".into(),
    ];
    args.extend(rate.download_args());
    if quality.max_height().is_some() {
        args.push("--merge-output-format".into());
        args.push("mkv".into());
    }
    if let Some(ff) = ffmpeg_location {
        args.push("--ffmpeg-location".into());
        args.push(ff.to_string_lossy().to_string());
    }
    if verbose {
        args.push("--verbose".into());
    }
    args.push(url.into());
    args
}

// ---------- Info video ----------

#[derive(Clone, Debug)]
pub struct FormatInfo {
    pub format_id: String,
    pub ext: String,
    pub height: Option<u32>,
    pub fps: Option<f64>,
    pub vcodec: String,
    pub acodec: String,
    pub filesize: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct VideoInfo {
    pub id: String,
    pub title: String,
    pub duration_secs: Option<f64>,
    pub uploader: String,
    pub max_height: Option<u32>,
    pub formats: Vec<FormatInfo>,
}

pub fn parse_info_json(json: &str) -> Result<VideoInfo, String> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("JSON info tidak valid: {e}"))?;
    let mut formats = Vec::new();
    let mut max_height: Option<u32> = None;
    if let Some(arr) = v.get("formats").and_then(|f| f.as_array()) {
        for f in arr {
            let height = f.get("height").and_then(|h| h.as_u64()).map(|h| h as u32);
            if let Some(h) = height {
                max_height = Some(max_height.map_or(h, |m: u32| m.max(h)));
            }
            formats.push(FormatInfo {
                format_id: f
                    .get("format_id")
                    .and_then(|x| x.as_str())
                    .unwrap_or("?")
                    .to_string(),
                ext: f
                    .get("ext")
                    .and_then(|x| x.as_str())
                    .unwrap_or("?")
                    .to_string(),
                height,
                fps: f.get("fps").and_then(|x| x.as_f64()),
                vcodec: f
                    .get("vcodec")
                    .and_then(|x| x.as_str())
                    .unwrap_or("none")
                    .to_string(),
                acodec: f
                    .get("acodec")
                    .and_then(|x| x.as_str())
                    .unwrap_or("none")
                    .to_string(),
                filesize: f
                    .get("filesize")
                    .and_then(|x| x.as_u64())
                    .or_else(|| f.get("filesize_approx").and_then(|x| x.as_u64())),
            });
        }
    }
    Ok(VideoInfo {
        id: v.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        title: v
            .get("title")
            .and_then(|x| x.as_str())
            .unwrap_or("(tanpa judul)")
            .to_string(),
        duration_secs: v.get("duration").and_then(|x| x.as_f64()),
        uploader: v
            .get("uploader")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        max_height,
        formats,
    })
}

pub fn format_duration(secs: f64) -> String {
    let s = secs as u64;
    format!("{}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
}

pub fn format_filesize(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{v:.1} {}", UNITS[i])
}

// ---------- Progress ----------

/// Parse baris progress yt-dlp: `[download]  45.2% of  120.00MiB at ...`
pub fn parse_progress(line: &str) -> Option<f64> {
    if !line.contains("[download]") {
        return None;
    }
    let pct_idx = line.find('%')?;
    let before = &line[..pct_idx];
    let num: String = before
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    num.parse::<f64>().ok().filter(|p| (0.0..=100.0).contains(p))
}

// ---------- Pencarian tool (yt-dlp / ffmpeg) ----------

fn exe_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// Cari tool: 1) folder bin aplikasi, 2) folder .exe aplikasi, 3) PATH.
pub fn find_tool(name: &str, app_bin_dir: &Path) -> Option<PathBuf> {
    let file = exe_name(name);
    let in_bin = app_bin_dir.join(&file);
    if in_bin.is_file() {
        return Some(in_bin);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let beside = dir.join(&file);
            if beside.is_file() {
                return Some(beside);
            }
        }
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let cand = dir.join(&file);
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

// ---------- Menjalankan proses dengan log + progress ----------

#[derive(Debug)]
pub enum ProcEvent {
    /// Proses selesai; Ok berisi baris stdout terakhir yang tidak kosong
    /// (dipakai untuk menangkap path file hasil dari --print after_move).
    Done(Result<String, String>),
}

pub struct RunningChild {
    pub pid: Arc<Mutex<Option<u32>>>,
    pub cancel: Arc<AtomicBool>,
}

impl RunningChild {
    pub fn new() -> Self {
        Self {
            pid: Arc::new(Mutex::new(None)),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn request_cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        if let Ok(guard) = self.pid.lock() {
            if let Some(pid) = *guard {
                kill_pid(pid);
            }
        }
    }
}

fn kill_pid(pid: u32) {
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output();
    }
    #[cfg(not(windows))]
    {
        let _ = Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .output();
    }
}

/// Jalankan program (yt-dlp) di thread saat ini (blocking), meneruskan semua
/// output ke logger dan progress (0..=100) ke `progress_tx`.
/// Mengembalikan baris stdout terakhir (untuk path hasil download).
pub fn run_logged(
    program: &Path,
    args: &[String],
    logger: &Arc<Logger>,
    target: &str,
    progress_tx: Option<Sender<f64>>,
    handle: &RunningChild,
    rate_limit_hit: Option<Arc<AtomicBool>>,
) -> Result<String, String> {
    logger.info(
        target,
        &format!("Jalankan: {} {}", program.display(), args.join(" ")),
    );
    let mut child: Child = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Gagal menjalankan {}: {e}", program.display()))?;

    if let Ok(mut g) = handle.pid.lock() {
        *g = Some(child.id());
    }

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let (line_tx, line_rx) = std::sync::mpsc::channel::<(bool, String)>();
    let mut readers = Vec::new();
    if let Some(out) = stdout {
        let tx = line_tx.clone();
        readers.push(std::thread::spawn(move || {
            for line in BufReader::new(out).lines() {
                match line {
                    Ok(l) => {
                        let _ = tx.send((false, l));
                    }
                    Err(_) => break,
                }
            }
        }));
    }
    if let Some(err) = stderr {
        let tx = line_tx.clone();
        readers.push(std::thread::spawn(move || {
            for line in BufReader::new(err).lines() {
                match line {
                    Ok(l) => {
                        let _ = tx.send((true, l));
                    }
                    Err(_) => break,
                }
            }
        }));
    }
    drop(line_tx);

    let mut last_stdout_line = String::new();
    for (is_stderr, line) in line_rx {
        if is_rate_limit_text(&line) {
            if let Some(flag) = &rate_limit_hit {
                flag.store(true, Ordering::SeqCst);
            }
            logger.warn(
                target,
                "Rate limit YouTube terdeteksi (429) — aplikasi akan masuk lockout setelah proses ini berhenti.",
            );
        }
        if let Some(p) = parse_progress(&line) {
            if let Some(tx) = &progress_tx {
                let _ = tx.send(p);
            }
        }
        if is_stderr {
            // yt-dlp menulis progress & verbose ke stderr; perlakukan baris
            // ERROR/WARNING sesuai isinya, sisanya DEBUG.
            let lvl = if line.contains("ERROR") {
                Level::Error
            } else if line.contains("WARNING") {
                Level::Warn
            } else {
                Level::Debug
            };
            logger.log(lvl, target, &line);
        } else {
            logger.debug(target, &line);
            let t = line.trim();
            if !t.is_empty() && !t.starts_with('{') {
                last_stdout_line = t.to_string();
            }
        }
    }
    for r in readers {
        let _ = r.join();
    }

    let status = child
        .wait()
        .map_err(|e| format!("Gagal menunggu proses selesai: {e}"))?;
    if let Ok(mut g) = handle.pid.lock() {
        *g = None;
    }
    if handle.cancel.load(Ordering::SeqCst) {
        return Err("Dibatalkan oleh pengguna.".to_string());
    }
    if status.success() {
        logger.info(target, "Proses selesai dengan sukses (exit 0).");
        Ok(last_stdout_line)
    } else {
        Err(format!(
            "Proses gagal dengan kode keluar {:?}. Lihat baris ERROR di log untuk penyebabnya.",
            status.code()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_from_watch_url() {
        assert_eq!(
            extract_video_id("https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=42s").as_deref(),
            Some("dQw4w9WgXcQ")
        );
    }
    #[test]
    fn id_from_short_forms() {
        assert_eq!(
            extract_video_id("https://youtu.be/dQw4w9WgXcQ?si=x").as_deref(),
            Some("dQw4w9WgXcQ")
        );
        assert_eq!(
            extract_video_id("https://www.youtube.com/shorts/dQw4w9WgXcQ").as_deref(),
            Some("dQw4w9WgXcQ")
        );
        assert_eq!(extract_video_id("dQw4w9WgXcQ").as_deref(), Some("dQw4w9WgXcQ"));
        assert_eq!(extract_video_id("bukan url"), None);
    }
    #[test]
    fn selector_2160_uses_adaptive() {
        let s = Quality::Q2160.format_selector();
        assert!(s.contains("height<=2160") && s.contains("+ba"), "{s}");
        assert_eq!(Quality::AudioOnly.format_selector(), "ba/b");
    }
    #[test]
    fn download_args_shape() {
        let args = build_download_args(
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            Quality::Q2160,
            "out/%(title)s.%(ext)s",
            None,
            false,
            &RateLimitConfig::default(),
        );
        assert_eq!(args.last().unwrap(), "https://www.youtube.com/watch?v=dQw4w9WgXcQ");
        assert!(args.windows(2).any(|w| w[0] == "--merge-output-format" && w[1] == "mkv"));
        // Resume harus selalu ada supaya putus di tengah bisa dilanjutkan.
        assert!(args.iter().any(|a| a == "--continue"));
    }
    #[test]
    fn rate_limit_args_on_and_off() {
        let on = RateLimitConfig::default().download_args();
        assert!(on.windows(2).any(|w| w[0] == "--sleep-requests" && w[1] == "1.5"));
        assert!(on.windows(2).any(|w| w[0] == "--max-sleep-interval" && w[1] == "15"));
        assert!(on.windows(2).any(|w| w[0] == "--concurrent-fragments" && w[1] == "1"));
        assert!(on.iter().any(|a| a == "--retry-sleep"));
        // Default: tanpa cap bandwidth.
        assert!(!on.iter().any(|a| a == "--limit-rate"));
        let mut capped = RateLimitConfig::default();
        capped.limit_rate = Some("10M".into());
        assert!(capped
            .download_args()
            .windows(2)
            .any(|w| w[0] == "--limit-rate" && w[1] == "10M"));
        let off = RateLimitConfig {
            enabled: false,
            ..Default::default()
        };
        assert!(off.download_args().is_empty());
        assert!(off.info_args().is_empty());
    }
    #[test]
    fn rate_limit_detection_and_jitter() {
        assert!(is_rate_limit_text("ERROR: HTTP Error 429: Too Many Requests"));
        assert!(is_rate_limit_text("too many requests, slow down"));
        assert!(!is_rate_limit_text("HTTP Error 403: Forbidden"));
        for _ in 0..50 {
            let s = jitter_cooldown_secs();
            assert!((5..=15).contains(&s), "jitter di luar rentang: {s}");
        }
    }
    #[test]
    fn progress_parse() {
        assert_eq!(
            parse_progress("[download]  45.2% of  120.00MiB at  5.00MiB/s ETA 00:13"),
            Some(45.2)
        );
        assert_eq!(parse_progress("[download] Destination: file.mkv"), None);
        assert_eq!(parse_progress("halo 50% dunia"), None);
    }
    #[test]
    fn info_parse() {
        let json = r#"{"id":"dQw4w9WgXcQ","title":"Tes","duration":212,"uploader":"Chan",
          "formats":[{"format_id":"313","ext":"webm","height":2160,"fps":30,"vcodec":"vp9","acodec":"none","filesize_approx":1000},
                     {"format_id":"251","ext":"webm","height":null,"vcodec":"none","acodec":"opus"}]}"#;
        let info = parse_info_json(json).unwrap();
        assert_eq!(info.max_height, Some(2160));
        assert_eq!(info.formats.len(), 2);
        assert_eq!(format_duration(3723.0), "1:02:03");
    }
}
