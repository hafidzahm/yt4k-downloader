use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;

use yt4k_downloader::engine::{
    self, ProcEvent, Quality, RateLimitConfig, RunningChild, VideoInfo,
};
use yt4k_downloader::log::{Level, Logger};

pub enum JobKind {
    Info,
    Download,
    SetupTools,
}

pub struct JobDone {
    pub kind: JobKind,
    pub result: Result<String, String>,
}

pub struct App {
    logger: Arc<Logger>,
    app_bin_dir: PathBuf,
    log_dir: PathBuf,

    url_input: String,
    quality_idx: usize,
    output_dir: String,

    info: Option<VideoInfo>,
    status: String,
    progress: Option<f64>,
    busy: bool,
    job_kind: Option<JobKind>,
    running: Arc<RunningChild>,

    ytdlp_path: Option<PathBuf>,
    ffmpeg_path: Option<PathBuf>,

    // Mode Aman (rate limiter) v0.1.1
    safe_mode: bool,
    limit_rate_idx: usize, // 0=Mati, 1=5M, 2=10M, 3=20M
    cooldown_until: Option<Instant>,
    lockout_until: Option<Instant>,
    hit_429: Arc<AtomicBool>,

    log_filter_idx: usize, // 0=Semua, 1=INFO+, 2=WARN+, 3=ERROR saja
    verbose_ui: bool,
    autoscroll: bool,

    progress_rx: Option<Receiver<f64>>,
    done_rx: Option<Receiver<JobDone>>,
}

impl App {
    pub fn new(logger: Arc<Logger>, app_bin_dir: PathBuf, log_dir: PathBuf) -> Self {
        let ytdlp_path = engine::find_tool("yt-dlp", &app_bin_dir);
        let ffmpeg_path = engine::find_tool("ffmpeg", &app_bin_dir);
        logger.info(
            "app",
            &format!(
                "YT 4K Downloader v{} mulai. OS: {} / arch: {}",
                env!("CARGO_PKG_VERSION"),
                std::env::consts::OS,
                std::env::consts::ARCH
            ),
        );
        logger.info(
            "app",
            &format!(
                "yt-dlp: {}",
                ytdlp_path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "TIDAK DITEMUKAN".into())
            ),
        );
        logger.info(
            "app",
            &format!(
                "ffmpeg: {}",
                ffmpeg_path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "TIDAK DITEMUKAN".into())
            ),
        );
        let output_dir = dirs::download_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| ".".into());
        Self {
            logger,
            app_bin_dir,
            log_dir,
            url_input: String::new(),
            quality_idx: 0,
            output_dir,
            info: None,
            status: "Siap. Tempel URL video, lalu Cek Info atau langsung Download.".into(),
            progress: None,
            busy: false,
            job_kind: None,
            running: Arc::new(RunningChild::new()),
            ytdlp_path,
            ffmpeg_path,
            safe_mode: true,
            limit_rate_idx: 0,
            cooldown_until: None,
            lockout_until: None,
            hit_429: Arc::new(AtomicBool::new(false)),
            log_filter_idx: 0,
            verbose_ui: false,
            autoscroll: true,
            progress_rx: None,
            done_rx: None,
        }
    }

    fn rate_config(&self) -> RateLimitConfig {
        RateLimitConfig {
            enabled: self.safe_mode,
            limit_rate: match self.limit_rate_idx {
                1 => Some("5M".to_string()),
                2 => Some("10M".to_string()),
                3 => Some("20M".to_string()),
                _ => None,
            },
            ..Default::default()
        }
    }

    fn lockout_remaining(&self) -> Option<Duration> {
        self.lockout_until
            .and_then(|t| t.checked_duration_since(Instant::now()))
            .filter(|d| !d.is_zero())
    }

    fn cooldown_remaining(&self) -> Option<Duration> {
        self.cooldown_until
            .and_then(|t| t.checked_duration_since(Instant::now()))
            .filter(|d| !d.is_zero())
    }

    /// True bila job baru harus ditolak karena lockout 429 / cooldown.
    fn guard_blocked(&mut self) -> bool {
        if let Some(d) = self.lockout_remaining() {
            self.status = format!(
                "LOCKOUT rate limit (429): tunggu {} menit {} detik lagi sebelum mencoba. Menembak ulang sekarang hanya memperpanjang blokir.",
                d.as_secs() / 60,
                d.as_secs() % 60
            );
            self.logger.warn("ratelimit", &self.status.clone());
            return true;
        }
        if let Some(d) = self.cooldown_remaining() {
            self.status = format!(
                "Cooldown Mode Aman: job berikutnya bisa dimulai dalam {} detik.",
                d.as_secs() + 1
            );
            return true;
        }
        false
    }

    fn arm_cooldown_after_download(&mut self) {
        if self.hit_429.load(Ordering::SeqCst) {
            self.lockout_until =
                Some(Instant::now() + Duration::from_secs(engine::RATE_LIMIT_LOCKOUT_SECS));
            self.cooldown_until = None;
            self.logger.error(
                "ratelimit",
                "429 terdeteksi selama download — lockout 15 menit diaktifkan. Jangan retry sebelum lockout selesai.",
            );
            self.status =
                "Kena rate limit YouTube (429). Lockout 15 menit — tunggu, jangan retry dulu."
                    .into();
        } else if self.safe_mode {
            let secs = engine::jitter_cooldown_secs();
            self.cooldown_until = Some(Instant::now() + Duration::from_secs(secs));
            self.logger.info(
                "ratelimit",
                &format!("Cooldown Mode Aman: {secs} detik sebelum job berikutnya."),
            );
        }
    }

    fn refresh_tools(&mut self) {
        self.ytdlp_path = engine::find_tool("yt-dlp", &self.app_bin_dir);
        self.ffmpeg_path = engine::find_tool("ffmpeg", &self.app_bin_dir);
    }

    fn spawn_info(&mut self) {
        if self.guard_blocked() {
            return;
        }
        let Some(url) = engine::canonical_url(&self.url_input) else {
            self.status = "URL / ID video tidak dikenali. Periksa lagi tempelannya.".into();
            self.logger.warn("ui", "Input tidak dikenali sebagai URL/ID YouTube.");
            return;
        };
        let Some(ytdlp) = self.ytdlp_path.clone() else {
            self.status = "yt-dlp belum ada. Klik 'Siapkan Tools' dulu.".into();
            return;
        };
        self.logger.info("info", &format!("Cek info: {url}"));
        let (done_tx, done_rx) = std::sync::mpsc::channel::<JobDone>();
        let (prog_tx, prog_rx) = std::sync::mpsc::channel::<f64>();
        let _ = prog_tx; // cek info tidak memakai progress bar
        self.done_rx = Some(done_rx);
        self.progress_rx = None;
        self.busy = true;
        self.job_kind = Some(JobKind::Info);
        self.status = "Mengambil info video...".into();
        let logger = self.logger.clone();
        let running = Arc::new(RunningChild::new());
        self.running = running.clone();
        let rate = self.rate_config();
        std::thread::spawn(move || {
            let args = engine::build_info_args(&url, &rate);
            // Untuk info, stdout adalah JSON utuh: tangkap terpisah dari run_logged.
            let result = run_info_capture(&ytdlp, &args, &logger, &running);
            let _ = done_tx.send(JobDone {
                kind: JobKind::Info,
                result,
            });
            drop(prog_rx);
        });
    }

    fn spawn_download(&mut self) {
        if self.guard_blocked() {
            return;
        }
        let Some(url) = engine::canonical_url(&self.url_input) else {
            self.status = "URL / ID video tidak dikenali.".into();
            return;
        };
        let Some(ytdlp) = self.ytdlp_path.clone() else {
            self.status = "yt-dlp belum ada. Klik 'Siapkan Tools' dulu.".into();
            return;
        };
        if self.ffmpeg_path.is_none() {
            self.logger.warn(
                "download",
                "ffmpeg tidak ditemukan: stream 4K akan terdownload terpisah TANPA merge. Siapkan tools dulu untuk hasil 1 file.",
            );
        }
        let quality = Quality::all()[self.quality_idx];
        let out_dir = PathBuf::from(self.output_dir.trim());
        let _ = std::fs::create_dir_all(&out_dir);
        let template = out_dir
            .join("%(title)s [%(id)s].%(ext)s")
            .display()
            .to_string();
        self.logger.info(
            "download",
            &format!(
                "Mulai download {} kualitas {} -> {}",
                url,
                quality.label(),
                out_dir.display()
            ),
        );
        let (done_tx, done_rx) = std::sync::mpsc::channel::<JobDone>();
        let (prog_tx, prog_rx) = std::sync::mpsc::channel::<f64>();
        self.done_rx = Some(done_rx);
        self.progress_rx = Some(prog_rx);
        self.busy = true;
        self.job_kind = Some(JobKind::Download);
        self.progress = Some(0.0);
        self.status = format!("Mengunduh {} ...", quality.label());
        let logger = self.logger.clone();
        let running = Arc::new(RunningChild::new());
        self.running = running.clone();
        let ffmpeg = self.ffmpeg_path.clone();
        let verbose = self.logger.is_verbose();
        let rate = self.rate_config();
        self.logger.info("ratelimit", &rate.describe());
        let hit_flag = Arc::new(AtomicBool::new(false));
        self.hit_429 = hit_flag.clone();
        std::thread::spawn(move || {
            let args = engine::build_download_args(
                &url,
                quality,
                &template,
                ffmpeg.as_deref(),
                verbose,
                &rate,
            );
            let result = engine::run_logged(
                &ytdlp,
                &args,
                &logger,
                "yt-dlp",
                Some(prog_tx),
                &running,
                Some(hit_flag),
            );
            let _ = done_tx.send(JobDone {
                kind: JobKind::Download,
                result,
            });
        });
    }

    fn spawn_setup_tools(&mut self) {
        self.busy = true;
        self.job_kind = Some(JobKind::SetupTools);
        self.status = "Menyiapkan yt-dlp & ffmpeg ke folder bin aplikasi...".into();
        let (done_tx, done_rx) = std::sync::mpsc::channel::<JobDone>();
        self.done_rx = Some(done_rx);
        self.progress_rx = None;
        let logger = self.logger.clone();
        let bin_dir = self.app_bin_dir.clone();
        let running = Arc::new(RunningChild::new());
        self.running = running.clone();
        std::thread::spawn(move || {
            let result = setup_tools(&bin_dir, &logger, &running);
            let _ = done_tx.send(JobDone {
                kind: JobKind::SetupTools,
                result,
            });
        });
    }

    fn poll_channels(&mut self) {
        if let Some(rx) = &self.progress_rx {
            while let Ok(p) = rx.try_recv() {
                self.progress = Some(p);
            }
        }
        let mut finished: Option<JobDone> = None;
        if let Some(rx) = &self.done_rx {
            if let Ok(done) = rx.try_recv() {
                finished = Some(done);
            }
        }
        if let Some(done) = finished {
            self.busy = false;
            self.job_kind = None;
            self.done_rx = None;
            self.progress_rx = None;
            match done.kind {
                JobKind::Info => match done.result {
                    Ok(json) => match engine::parse_info_json(&json) {
                        Ok(info) => {
                            self.logger.info(
                                "info",
                                &format!(
                                    "Judul: {} | durasi: {} | format: {} | maks: {}",
                                    info.title,
                                    info.duration_secs
                                        .map(engine::format_duration)
                                        .unwrap_or_else(|| "?".into()),
                                    info.formats.len(),
                                    info.max_height
                                        .map(|h| format!("{h}p"))
                                        .unwrap_or_else(|| "audio".into())
                                ),
                            );
                            for f in &info.formats {
                                self.logger.debug(
                                    "info",
                                    &format!(
                                        "format {} ext={} h={:?} fps={:?} vcodec={} acodec={} size={}",
                                        f.format_id,
                                        f.ext,
                                        f.height,
                                        f.fps,
                                        f.vcodec,
                                        f.acodec,
                                        f.filesize
                                            .map(engine::format_filesize)
                                            .unwrap_or_else(|| "?".into())
                                    ),
                                );
                            }
                            self.status = format!("Info OK: {}", info.title);
                            self.info = Some(info);
                        }
                        Err(e) => {
                            self.logger.error("info", &e);
                            self.status = format!("Gagal membaca info: {e}");
                        }
                    },
                    Err(e) => {
                        self.logger.error("info", &e);
                        if engine::is_rate_limit_text(&e) {
                            self.lockout_until = Some(
                                Instant::now()
                                    + Duration::from_secs(engine::RATE_LIMIT_LOCKOUT_SECS),
                            );
                            self.logger.error(
                                "ratelimit",
                                "429 saat cek info — lockout 15 menit diaktifkan.",
                            );
                        }
                        self.status = format!("Cek info gagal: {e}");
                    }
                },
                JobKind::Download => {
                    match done.result {
                        Ok(path) => {
                            self.progress = Some(100.0);
                            self.status = if path.is_empty() {
                                "Download selesai. Cek folder output.".into()
                            } else {
                                format!("Download selesai: {path}")
                            };
                            self.logger.info("download", &self.status.clone());
                        }
                        Err(e) => {
                            self.logger.error("download", &e);
                            if engine::is_rate_limit_text(&e) {
                                self.hit_429.store(true, Ordering::SeqCst);
                            }
                            self.status = format!("Download gagal/dibatalkan: {e}");
                        }
                    }
                    // Setelah status akhir tersimpan: aktifkan cooldown /
                    // lockout. Bila 429, arm akan menimpa status dengan
                    // pesan lockout.
                    self.arm_cooldown_after_download();
                }
                JobKind::SetupTools => {
                    self.refresh_tools();
                    match done.result {
                        Ok(msg) => self.status = msg,
                        Err(e) => {
                            self.logger.error("setup", &e);
                            self.status = format!("Setup tools gagal: {e}");
                        }
                    }
                }
            }
        }
    }
}

/// Jalankan yt-dlp untuk info: stdout = satu dokumen JSON besar, jadi tangkap
/// utuh (maks ~4 MB) sambil tetap meneruskan stderr ke log.
fn run_info_capture(
    ytdlp: &PathBuf,
    args: &[String],
    logger: &Arc<Logger>,
    running: &RunningChild,
) -> Result<String, String> {
    use std::io::Read;
    logger.debug("info", &format!("Jalankan: {} {}", ytdlp.display(), args.join(" ")));
    let mut cmd = std::process::Command::new(ytdlp);
    cmd.args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Gagal menjalankan yt-dlp: {e}"))?;
    if let Ok(mut g) = running.pid.lock() {
        *g = Some(child.id());
    }
    let mut out = String::new();
    if let Some(mut so) = child.stdout.take() {
        let _ = so.read_to_string(&mut out);
    }
    let mut err = String::new();
    if let Some(mut se) = child.stderr.take() {
        let _ = se.read_to_string(&mut err);
    }
    for line in err.lines() {
        logger.debug("info", line);
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!(
            "yt-dlp exit {:?}. {}",
            status.code(),
            err.lines().last().unwrap_or("")
        ));
    }
    Ok(out)
}

/// Unduh yt-dlp (+ ffmpeg di Windows) resmi ke folder bin aplikasi memakai
/// curl bawaan OS (Windows 10/11 dan Linux sama-sama punya curl & tar).
fn setup_tools(
    bin_dir: &PathBuf,
    logger: &Arc<Logger>,
    running: &RunningChild,
) -> Result<String, String> {
    std::fs::create_dir_all(bin_dir).map_err(|e| e.to_string())?;
    let _ = ProcEvent::Done; // (menjaga referensi tipe tetap terpakai)
    let curl = engine::find_tool("curl", bin_dir)
        .ok_or_else(|| "curl tidak ditemukan di PATH; tidak bisa mengunduh tools.".to_string())?;

    if engine::find_tool("yt-dlp", bin_dir).is_none() {
        let url = if cfg!(windows) {
            "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe"
        } else {
            "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp"
        };
        let dest = bin_dir.join(if cfg!(windows) { "yt-dlp.exe" } else { "yt-dlp" });
        logger.info("setup", &format!("Mengunduh yt-dlp dari rilis resmi GitHub -> {}", dest.display()));
        let args = vec![
            "-L".to_string(),
            "--fail".to_string(),
            "-o".to_string(),
            dest.display().to_string(),
            url.to_string(),
        ];
        engine::run_logged(&curl, &args, logger, "setup", None, running, None)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755));
        }
    } else {
        logger.info("setup", "yt-dlp sudah ada, lewati.");
    }

    if engine::find_tool("ffmpeg", bin_dir).is_none() {
        if cfg!(windows) {
            let tar = engine::find_tool("tar", bin_dir)
                .ok_or_else(|| "tar tidak ditemukan; ekstrak ffmpeg manual.".to_string())?;
            let zip = bin_dir.join("ffmpeg-essentials.zip");
            logger.info("setup", "Mengunduh FFmpeg essentials (gyan.dev)...");
            let args = vec![
                "-L".to_string(),
                "--fail".to_string(),
                "-o".to_string(),
                zip.display().to_string(),
                "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip".to_string(),
            ];
            engine::run_logged(&curl, &args, logger, "setup", None, running, None)?;
            let extract_dir = bin_dir.join("ffmpeg-extract");
            let args = vec![
                "-xf".to_string(),
                zip.display().to_string(),
                "-C".to_string(),
                extract_dir.display().to_string(),
            ];
            let _ = std::fs::create_dir_all(&extract_dir);
            engine::run_logged(&tar, &args, logger, "setup", None, running, None)?;
            // Cari ffmpeg.exe di hasil ekstrak (satu level folder versi).
            let mut found = None;
            if let Ok(entries) = std::fs::read_dir(&extract_dir) {
                for e in entries.flatten() {
                    let cand = e.path().join("bin").join("ffmpeg.exe");
                    if cand.is_file() {
                        found = Some(cand);
                        break;
                    }
                }
            }
            let src = found.ok_or_else(|| "ffmpeg.exe tidak ditemukan di zip.".to_string())?;
            std::fs::copy(&src, bin_dir.join("ffmpeg.exe")).map_err(|e| e.to_string())?;
            let _ = std::fs::remove_file(&zip);
            let _ = std::fs::remove_dir_all(&extract_dir);
            logger.info("setup", "ffmpeg.exe terpasang di folder bin.");
        } else {
            logger.warn(
                "setup",
                "Di OS ini ffmpeg diharapkan dari package manager / PATH; unduhan otomatis ffmpeg hanya untuk Windows.",
            );
        }
    } else {
        logger.info("setup", "ffmpeg sudah ada, lewati.");
    }
    Ok("Tools siap: yt-dlp & ffmpeg terdeteksi.".into())
}

// ---------- UI ----------

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_channels();
        if self.busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(120));
        }
        let lock_rem = self.lockout_remaining();
        let cool_rem = self.cooldown_remaining();
        let time_blocked = lock_rem.is_some() || cool_rem.is_some();
        if time_blocked {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }

        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.heading("YT 4K Downloader");
                ui.label(format!("v{}", env!("CARGO_PKG_VERSION")));
                ui.separator();
                let (yt_txt, yt_col) = match &self.ytdlp_path {
                    Some(_) => ("yt-dlp: OK", egui::Color32::from_rgb(60, 170, 80)),
                    None => ("yt-dlp: belum ada", egui::Color32::from_rgb(200, 70, 60)),
                };
                let (ff_txt, ff_col) = match &self.ffmpeg_path {
                    Some(_) => ("ffmpeg: OK", egui::Color32::from_rgb(60, 170, 80)),
                    None => ("ffmpeg: belum ada", egui::Color32::from_rgb(200, 70, 60)),
                };
                ui.colored_label(yt_col, yt_txt);
                ui.colored_label(ff_col, ff_txt);
                if (self.ytdlp_path.is_none() || self.ffmpeg_path.is_none()) && !self.busy {
                    if ui.button("Siapkan Tools").clicked() {
                        self.spawn_setup_tools();
                    }
                }
            });
            ui.add_space(4.0);
        });

        egui::TopBottomPanel::bottom("log_panel")
            .resizable(true)
            .default_height(230.0)
            .min_height(110.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.strong("Log Debug");
                    ui.separator();
                    egui::ComboBox::from_id_salt("log_filter")
                        .selected_text(["Semua", "INFO+", "WARN+", "ERROR"][self.log_filter_idx])
                        .show_ui(ui, |ui| {
                            for (i, name) in ["Semua", "INFO+", "WARN+", "ERROR"].iter().enumerate() {
                                ui.selectable_value(&mut self.log_filter_idx, i, *name);
                            }
                        });
                    if ui
                        .checkbox(&mut self.verbose_ui, "Verbose")
                        .changed()
                    {
                        self.logger.set_verbose(self.verbose_ui);
                        self.logger
                            .info("ui", &format!("Verbose debug: {}", self.verbose_ui));
                    }
                    ui.checkbox(&mut self.autoscroll, "Auto-scroll");
                    if ui.button("Copy Log").clicked() {
                        ui.ctx().copy_text(self.logger.export_text());
                    }
                    if ui.button("Export .log").clicked() {
                        if let Some(path) = rfd::FileDialog::new()
                            .set_file_name("yt4k-log.txt")
                            .save_file()
                        {
                            let _ = std::fs::write(&path, self.logger.export_text());
                            self.logger
                                .info("ui", &format!("Log diekspor ke {}", path.display()));
                        }
                    }
                    if ui.button("Buka Folder Log").clicked() {
                        let dir = self.log_dir.clone();
                        #[cfg(windows)]
                        let _ = std::process::Command::new("explorer").arg(&dir).spawn();
                        #[cfg(not(windows))]
                        let _ = std::process::Command::new("xdg-open").arg(&dir).spawn();
                    }
                    ui.label(format!("File: {}", self.logger.file_path.display()));
                });
                ui.separator();
                let min_rank = [0u8, 1, 2, 3][self.log_filter_idx];
                let records = self.logger.snapshot();
                egui::ScrollArea::vertical()
                    .stick_to_bottom(self.autoscroll)
                    .show(ui, |ui| {
                        for r in &records {
                            if r.level.rank() < min_rank {
                                continue;
                            }
                            if r.level == Level::Debug && !self.verbose_ui && self.log_filter_idx == 0 {
                                // Mode ringkas: DEBUG hanya tampil bila Verbose aktif,
                                // kecuali pengguna memilih filter lain secara eksplisit.
                                continue;
                            }
                            let color = match r.level {
                                Level::Debug => egui::Color32::GRAY,
                                Level::Info => ui.visuals().text_color(),
                                Level::Warn => egui::Color32::from_rgb(210, 150, 40),
                                Level::Error => egui::Color32::from_rgb(220, 70, 60),
                            };
                            ui.colored_label(
                                color,
                                egui::RichText::new(r.to_line()).monospace().size(11.0),
                            );
                        }
                    });
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_space(6.0);
            ui.label("URL / ID Video YouTube");
            ui.horizontal(|ui| {
                let edit = egui::TextEdit::singleline(&mut self.url_input)
                    .desired_width(ui.available_width() - 190.0)
                    .hint_text("https://www.youtube.com/watch?v=...");
                ui.add(edit);
                if ui
                    .add_enabled(!self.busy && !time_blocked, egui::Button::new("Cek Info"))
                    .clicked()
                {
                    self.spawn_info();
                }
                if ui
                    .add_enabled(!self.busy, egui::Button::new("Tempel"))
                    .clicked()
                {
                    if let Some(txt) = ui.ctx().input(|i| {
                        i.events.iter().find_map(|e| match e {
                            egui::Event::Paste(t) => Some(t.clone()),
                            _ => None,
                        })
                    }) {
                        self.url_input = txt;
                    }
                }
            });

            if let Some(info) = &self.info {
                ui.add_space(6.0);
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.strong(&info.title);
                    ui.label(format!(
                        "Channel: {}  •  Durasi: {}  •  Format tersedia: {}  •  Tertinggi: {}",
                        info.uploader,
                        info.duration_secs
                            .map(engine::format_duration)
                            .unwrap_or_else(|| "?".into()),
                        info.formats.len(),
                        info.max_height
                            .map(|h| format!("{h}p"))
                            .unwrap_or_else(|| "audio".into())
                    ));
                    if let Some(sel) = Quality::all()[self.quality_idx].max_height() {
                        if let Some(mh) = info.max_height {
                            if mh < sel {
                                ui.colored_label(
                                    egui::Color32::from_rgb(210, 150, 40),
                                    format!(
                                        "Catatan: video ini maksimal {mh}p, pilihan {sel}p akan fallback ke {mh}p (bukan upscale)."
                                    ),
                                );
                            }
                        }
                    }
                });
            }

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label("Kualitas:");
                egui::ComboBox::from_id_salt("quality")
                    .selected_text(Quality::all()[self.quality_idx].label())
                    .show_ui(ui, |ui| {
                        for (i, q) in Quality::all().iter().enumerate() {
                            ui.selectable_value(&mut self.quality_idx, i, q.label());
                        }
                    });
            });
            ui.horizontal(|ui| {
                let before = self.safe_mode;
                ui.checkbox(&mut self.safe_mode, "Mode Aman (rate limiter)");
                if self.safe_mode != before {
                    self.logger
                        .info("ratelimit", &self.rate_config().describe());
                }
                ui.separator();
                ui.label("Cap kecepatan:");
                egui::ComboBox::from_id_salt("limit_rate")
                    .selected_text(["Mati", "5M", "10M", "20M"][self.limit_rate_idx])
                    .show_ui(ui, |ui| {
                        for (i, name) in ["Mati", "5M", "10M", "20M"].iter().enumerate() {
                            ui.selectable_value(&mut self.limit_rate_idx, i, *name);
                        }
                    });
            });
            ui.horizontal(|ui| {
                ui.label("Folder output:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.output_dir)
                        .desired_width(ui.available_width() - 110.0),
                );
                if ui.button("Pilih...").clicked() {
                    if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                        self.output_dir = dir.display().to_string();
                    }
                }
            });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        !self.busy && !time_blocked,
                        egui::Button::new("⬇ Download").min_size(egui::vec2(150.0, 36.0)),
                    )
                    .clicked()
                {
                    self.spawn_download();
                }
                if self.busy {
                    if ui.button("Batalkan").clicked() {
                        self.running.request_cancel();
                        self.logger.warn("ui", "Pembatalan diminta pengguna.");
                        self.status = "Membatalkan...".into();
                    }
                    ui.spinner();
                }
            });

            if let Some(p) = self.progress {
                ui.add_space(6.0);
                ui.add(
                    egui::ProgressBar::new((p / 100.0) as f32)
                        .show_percentage()
                        .animate(self.busy),
                );
            }
            ui.add_space(6.0);
            ui.label(&self.status);
            if let Some(d) = lock_rem {
                ui.colored_label(
                    egui::Color32::from_rgb(220, 70, 60),
                    format!(
                        "LOCKOUT 429: tersisa {} menit {:02} detik — jangan retry dulu.",
                        d.as_secs() / 60,
                        d.as_secs() % 60
                    ),
                );
            } else if let Some(d) = cool_rem {
                ui.label(format!(
                    "Cooldown Mode Aman: {} detik lagi.",
                    d.as_secs() + 1
                ));
            }
        });
    }
}
