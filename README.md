# YT 4K Downloader — Rust (Windows)

Aplikasi desktop Windows untuk download video YouTube sampai **2160p/4K**,
ditulis murni dengan **Rust** (GUI: `egui`/`eframe` — satu file `.exe`,
tanpa Electron, tanpa WebView).

## Kenapa bukan youtubei.js / rusty_ytdl murni?

- youtubei.js itu JavaScript; padanan murni-Rust-nya, `rusty_ytdl` 0.7.4
  (rilis terakhir Agustus 2024), tidak punya jalur JS-runtime / PO-token
  sehingga gampang patah setiap YouTube mengganti player.
- Karena itu aplikasi Rust ini mengelola **yt-dlp sebagai sidecar**:
  Rust yang mengatur UI, pilihan kualitas, progress, merge FFmpeg, dan log;
  yt-dlp yang mengekstrak format — pola yang sama dengan crate Rust
  `yt-dlp` (boul2gom), dan alasan yt-dlp dipilih adalah komunitasnya yang
  menambal perubahan YouTube dalam hitungan hari.
- Di atas 720p, YouTube hanya menyediakan stream adaptif (video dan audio
  terpisah). Aplikasi mengunduh keduanya lalu menggabungkannya dengan
  FFmpeg (`-c copy` via yt-dlp) menjadi **satu file `.mkv`**.

## Fitur

- Tempel URL (watch / youtu.be / shorts / live) atau ID video
- **Cek Info**: judul, channel, durasi, jumlah format, resolusi tertinggi —
  kalau video maksimal 1080p dan kamu memilih 2160p, aplikasi memberi tahu
  akan fallback (bukan upscale)
- Kualitas: 2160p (4K) / 1440p / 1080p / 720p / audio saja
- Progress bar dari output yt-dlp, tombol Batalkan
- Pilihan folder output
- **Tombol "Siapkan Tools"**: mengunduh `yt-dlp.exe` resmi (GitHub) dan
  FFmpeg essentials (gyan.dev) ke folder bin aplikasi — PC tujuan tidak
  perlu install apa pun secara manual

## Mode Aman — Rate Limiter (v0.1.1)

Checkbox **Mode Aman** aktif secara default. Isinya:

- `--sleep-requests 1.5` — jeda antar-request ekstraksi yt-dlp
- Jeda acak **5–15 detik** antar-download (`--sleep-interval` /
  `--max-sleep-interval`) — acak, bukan tetap, supaya tidak jadi fingerprint
- `--concurrent-fragments 1`, `--retries 5`, `--fragment-retries 10`,
  dan `--retry-sleep` backoff eksponensial (bukan retry tanpa batas)
- **Cooldown level aplikasi** 5–15 detik (acak) setelah tiap download,
  sebelum job berikutnya bisa dimulai
- **Lockout 15 menit** bila log mendeteksi HTTP 429 / "Too Many Requests":
  tombol Cek Info & Download terkunci dengan hitung mundur. Menembak ulang
  saat kena 429 hanya memperpanjang blokir, jadi aplikasi mencegahnya.
- Cap kecepatan (`--limit-rate`) opsional: Mati (default) / 5M / 10M / 20M.
  Perlindungannya paling lemah dan file 4K jadi jauh lebih lama, jadi
  sengaja tidak dipaksakan.

Rate limiter **menekan** risiko 429/cek bot, bukan menghapusnya: gate
PO Token/SABR dan reputasi IP (mis. IP datacenter) berada di luar jangkauan
pacing.

### Putus di tengah jalan?

Bisa terjadi (jaringan putus, laptop sleep, koneksi direset) — untuk file
200 MB risikonya kecil, untuk file 4K bergiga-giga risikonya lebih nyata.
Penanganannya:

- Argumen `--continue` selalu dipasang: file `.part` tidak dibuang.
  Klik **Download** lagi dengan kualitas & folder yang sama, unduhan
  **melanjutkan dari byte terakhir**, bukan dari nol.
- Fragment yang gagal di-retry sampai 10x dengan backoff di dalam sesi
  yang sama.
- File final `.mkv` hanya tercipta setelah semua stream lengkap dan
  merge FFmpeg selesai — file hasil tidak pernah setengah/korup.

## Log Debug

- Panel log di bagian bawah aplikasi: filter level
  (Semua / INFO+ / WARN+ / ERROR), toggle **Verbose**, auto-scroll
- File log per hari:
  `%APPDATA%\YT4KDownloader\logs\main-YYYY-MM-DD.log`
- Isi log: versi aplikasi & tool, daftar format (itag, codec, resolusi,
  bitrate/ukuran), format yang dipilih, perintah yt-dlp & FFmpeg lengkap,
  progress, exit code, dan stack error
- Tombol **Copy Log**, **Export .log**, **Buka Folder Log**
- **Redaksi otomatis**: nilai PO Token (`pot`), signature (`sig`/`lsig`),
  cookie, dan Authorization tidak pernah ditulis polos — hanya `***`.
  Log aman dikirim untuk diagnosa.

Folder data aplikasi (Windows):
- Log: `%APPDATA%\YT4KDownloader\logs\`
- Tools: `%APPDATA%\YT4KDownloader\bin\` (`yt-dlp.exe`, `ffmpeg.exe`)

## Build di Windows

Cara termudah: buka folder ini di GitHub — workflow
`.github/workflows/windows-build.yml` membangun paket portable `.zip`
(berisi `.exe` + `yt-dlp.exe` + `ffmpeg.exe`) di runner Windows resmi
setiap ada push / lewat tombol *Run workflow*.

Build lokal di PC Windows:

```bat
build-windows.bat
```

atau manual:

```powershell
rustup default stable
cargo build --release
:: hasil: target\release\yt4k-downloader.exe
```

Syarat build lokal: Rust stable (rustup) — tidak perlu Visual Studio
untuk crate yang dipakai versi ini (egui memakai backend glow/OpenGL).

## Batasan yang jujur

- Video harus memang tersedia di 2160p di YouTube; aplikasi tidak
  meng-upscale.
- Video privat / age-restricted / berbayar bisa butuh cookies — versi ini
  belum menyediakan UI cookies.
- YouTube bisa sewaktu-waktu mengubah gate (PO Token / SABR). Kalau
  extractor yt-dlp tertinggal, memperbarui `yt-dlp.exe` dari tombol
  Siapkan Tools / rilis terbaru biasanya cukup.
- Build `.exe` Windows diverifikasi lewat GitHub Actions (runner Windows);
  pengembangan & unit test logika inti dilakukan di Linux
  (`cargo test --lib`).
