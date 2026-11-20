//! Logger aplikasi: menulis ke file log + menyimpan ring-buffer untuk panel
//! log di UI. Semua baris melewati `redact_line` terlebih dahulu supaya token
//! (PO token, signature URL googlevideo, cookie, Authorization) tidak pernah
//! tertulis polos ke file maupun panel — yang dicatat hanya ada/tidaknya.

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub const MAX_RECORDS: usize = 3000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    pub fn label(&self) -> &'static str {
        match self {
            Level::Debug => "DEBUG",
            Level::Info => "INFO",
            Level::Warn => "WARN",
            Level::Error => "ERROR",
        }
    }
    /// Urutan keparahan untuk filter UI (tampilkan >= ambang).
    pub fn rank(&self) -> u8 {
        match self {
            Level::Debug => 0,
            Level::Info => 1,
            Level::Warn => 2,
            Level::Error => 3,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Record {
    pub time: String,
    pub level: Level,
    pub target: String,
    pub message: String,
}

impl Record {
    pub fn to_line(&self) -> String {
        format!(
            "[{}] [{:<5}] [{}] {}",
            self.time,
            self.level.label(),
            self.target,
            self.message
        )
    }
}

/// Ganti nilai satu `key` di semua kemunculan pola `key=value` / `key: value`.
/// Pemindaian berjalan satu arah (offset selalu maju), jadi tidak ada loop.
fn redact_key(input: &str, key: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    loop {
        let lower = rest.to_lowercase();
        let Some(kidx) = lower.find(key) else {
            out.push_str(rest);
            break;
        };
        let after_key = &rest[kidx + key.len()..];
        // Pemisah wajib: rangkaian spasi lalu tepat satu '=' atau ':' lalu spasi.
        let mut sep_chars = 0usize;
        let mut seen_sep = false;
        for c in after_key.chars() {
            if c == ' ' && !seen_sep {
                sep_chars += 1;
            } else if (c == '=' || c == ':') && !seen_sep {
                seen_sep = true;
                sep_chars += 1;
            } else {
                break;
            }
        }
        if !seen_sep {
            // Bukan pola key=value — salin sampai sesudah key ini, lanjutkan.
            out.push_str(&rest[..kidx + key.len()]);
            rest = after_key;
            continue;
        }
        let vstart_rel = kidx + key.len() + sep_chars;
        let value_part = &rest[vstart_rel..];
        let vlen = value_part
            .find(|c: char| {
                c == '&'
                    || c.is_whitespace()
                    || c == '"'
                    || c == '\''
                    || c == ';'
                    || c == ','
                    || c == ')'
                    || c == ']'
            })
            .unwrap_or(value_part.len());
        out.push_str(&rest[..vstart_rel]);
        out.push_str("***");
        rest = &value_part[vlen..];
    }
    out
}

/// Ganti nilai parameter sensitif dengan `***`.
/// Menangani bentuk `key=value`, `key: value`, dan header `Bearer <token>`.
pub fn redact_line(input: &str) -> String {
    const KEYS: [&str; 7] = [
        "pot",
        "sig",
        "lsig",
        "signature",
        "cookie",
        "authorization",
        "visitor_data",
    ];
    let mut out = input.to_string();

    // Bearer token (satu penggantian, tanpa loop)
    if let Some(idx) = out.to_lowercase().find("bearer ") {
        let start = idx + "bearer ".len();
        if start < out.len() {
            let end = out[start..]
                .find(|c: char| c.is_whitespace() || c == '"' || c == '\'')
                .map(|i| start + i)
                .unwrap_or(out.len());
            if end > start {
                out.replace_range(start..end, "***");
            }
        }
    }

    for key in KEYS {
        out = redact_key(&out, key);
    }
    out
}

struct Inner {
    file: Option<File>,
    records: VecDeque<Record>,
}

pub struct Logger {
    inner: Mutex<Inner>,
    pub file_path: PathBuf,
    pub verbose: AtomicBool,
}

impl Logger {
    pub fn new(file_path: PathBuf) -> Arc<Self> {
        if let Some(parent) = file_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_path)
            .ok();
        Arc::new(Self {
            inner: Mutex::new(Inner {
                file,
                records: VecDeque::new(),
            }),
            file_path,
            verbose: AtomicBool::new(false),
        })
    }

    pub fn log(&self, level: Level, target: &str, message: &str) {
        let record = Record {
            time: chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
            level,
            target: target.to_string(),
            message: redact_line(message),
        };
        let line = record.to_line();
        if let Ok(mut inner) = self.inner.lock() {
            if let Some(f) = inner.file.as_mut() {
                let _ = writeln!(f, "{line}");
                let _ = f.flush();
            }
            if inner.records.len() >= MAX_RECORDS {
                inner.records.pop_front();
            }
            inner.records.push_back(record);
        }
    }

    pub fn debug(&self, target: &str, message: &str) {
        // DEBUG selalu masuk file (untuk diagnosa belakangan), panel UI
        // menyaringnya lewat filter level + toggle verbose.
        self.log(Level::Debug, target, message);
    }
    pub fn info(&self, target: &str, message: &str) {
        self.log(Level::Info, target, message);
    }
    pub fn warn(&self, target: &str, message: &str) {
        self.log(Level::Warn, target, message);
    }
    pub fn error(&self, target: &str, message: &str) {
        self.log(Level::Error, target, message);
    }

    pub fn is_verbose(&self) -> bool {
        self.verbose.load(Ordering::Relaxed)
    }
    pub fn set_verbose(&self, v: bool) {
        self.verbose.store(v, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> Vec<Record> {
        self.inner
            .lock()
            .map(|i| i.records.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Seluruh log sebagai teks (untuk Copy / Export).
    pub fn export_text(&self) -> String {
        self.snapshot()
            .iter()
            .map(|r| r.to_line())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_pot_and_sig_in_url() {
        let line = "GET https://rr.googlevideo.com/videoplayback?expire=1&pot=SECRETTOKEN&sig=ABC123&itag=313";
        let out = redact_line(line);
        assert!(!out.contains("SECRETTOKEN"), "pot bocor: {out}");
        assert!(!out.contains("ABC123"), "sig bocor: {out}");
        assert!(out.contains("itag=313"), "itag harus tetap: {out}");
    }

    #[test]
    fn redact_bearer() {
        let out = redact_line("Authorization: Bearer xyz987 header lain");
        assert!(!out.contains("xyz987"), "{out}");
    }

    #[test]
    fn redact_is_idempotent_and_terminates() {
        // Regresi: versi pertama redact_line loop tak berujung pada nilai
        // yang sudah menjadi "***" (diganti dengan dirinya sendiri).
        let once = redact_line("pot=abc&sig=def");
        assert_eq!(redact_line(&once), once);
        assert_eq!(
            redact_line("kata signature di tengah kalimat biasa"),
            "kata signature di tengah kalimat biasa"
        );
    }

    #[test]
    fn normal_line_unchanged() {
        let line = "Download selesai: 1.2 GiB dalam 40 detik";
        assert_eq!(redact_line(line), line);
    }
}
