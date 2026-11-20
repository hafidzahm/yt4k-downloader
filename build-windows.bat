@echo off
REM Build YT 4K Downloader di Windows (lokal)
REM Syarat: Rust stable sudah terinstall (https://rustup.rs)
setlocal
where cargo >nul 2>nul
if errorlevel 1 (
  echo [ERROR] cargo tidak ditemukan. Install Rust dulu dari https://rustup.rs
  pause
  exit /b 1
)
echo Membangun versi release...
cargo build --release
if errorlevel 1 (
  echo [ERROR] Build gagal. Lihat pesan di atas.
  pause
  exit /b 1
)
echo.
echo Selesai: target\release\yt4k-downloader.exe
echo Jalankan exe-nya, lalu klik "Siapkan Tools" di kanan atas
echo untuk mengunduh yt-dlp.exe + ffmpeg.exe secara otomatis.
pause
