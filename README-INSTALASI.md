# YT 4K Downloader v0.1.1 — Panduan Instalasi Windows

Aplikasi download video YouTube sampai 2160p/4K, ditulis dengan Rust.
Satu paket ini berisi source code lengkap di folder `app\`.
File `.exe` Windows dibuat dengan salah satu dari dua cara di bawah —
pilih yang paling cocok untukmu.

---

## Isi Paket

```
YT4KDownloader-v0.1.1\
  README-INSTALASI.md   <- file ini
  README-INSTALASI.txt  <- versi teks polos (Notepad)
  app\
    src\                <- source code Rust
    Cargo.toml
    build-windows.bat   <- script build sekali klik (Cara B)
    .github\workflows\  <- workflow build otomatis (Cara A)
    README.md           <- dokumentasi teknis aplikasi
```

Persyaratan: Windows 10/11 64-bit dan koneksi internet.

---

## Cara A — Paling Mudah: Build Otomatis via GitHub Actions

Tidak perlu install Rust di PC-mu. GitHub yang membangunkan `.exe`-nya
di server Windows mereka.

1. Buat repository baru di GitHub (boleh Private).
2. Upload **isi folder `app\`** ke repository itu
   (termasuk folder tersembunyi `.github` — ini yang paling penting,
   di situlah workflow build-nya).
3. Di halaman repository, buka tab **Actions**.
4. Pilih workflow **"Build Windows"** di sisi kiri, klik
   **Run workflow** → **Run workflow**.
5. Tunggu ±5–10 menit sampai tanda centang hijau.
6. Klik run yang selesai → di bagian **Artifacts** unduh
   **YT4KDownloader-Windows-x64**.
7. Ekstrak zip hasil unduhan itu. Isinya:
   `yt4k-downloader.exe` + `yt-dlp.exe` + `ffmpeg.exe`.
   Simpan bertiga dalam **satu folder yang sama**.
8. Jalankan `yt4k-downloader.exe`. Selesai — lanjut ke bagian
   **Mengatasi Windows SmartScreen** dan **Pertama Kali Jalan** di bawah.

---

## Cara B — Build Sendiri di PC Windows

Kelebihan cara ini: file `.exe` hasil build lokal **tidak** membawa
tanda "file unduhan internet", jadi biasanya **tidak memicu
SmartScreen sama sekali** (penjelasan lengkap di bagian SmartScreen).

1. Install Rust:
   - Buka https://rustup.rs lalu unduh `rustup-init.exe`, jalankan,
     pilih opsi default (tekan Enter).
   - Tutup dan buka lagi terminalmu, lalu cek dengan perintah:
     `cargo --version` — harus muncul nomor versi.
2. Buka folder `app\` dari paket ini.
3. Klik dua kali **`build-windows.bat`**
   (atau buka terminal di folder itu dan jalankan `cargo build --release`).
4. Tunggu build pertama selesai (±5–15 menit; build berikutnya jauh
   lebih cepat).
5. File hasil ada di: `app\target\release\yt4k-downloader.exe`
6. Jalankan exe itu. Lanjut ke **Pertama Kali Jalan**.

---

## Mengatasi Windows SmartScreen

Saat menjalankan exe hasil unduhan (Cara A), Windows mungkin menampilkan
layar biru: **"Windows protected your PC"**.

Itu **bukan** vonis virus. SmartScreen memblokir sementara semua program
baru yang **tidak ditandatangani sertifikat code-signing berbayar** dan
belum punya reputasi unduhan. Aplikasi ini tidak ditandatangani, jadi
wajar kena — downloader resmi pun yang tidak bertanda tangan akan sama.

### Cara melewatinya (untuk file yang kamu percaya — aplikasi ini)

1. Di layar biru itu, klik **More info**.
2. Klik tombol **Run anyway** yang muncul di bawah.
3. Aplikasi berjalan normal. Biasanya prompt ini hanya muncul di
   eksekusi pertama.

### Alternatif: Unblock file sebelum dijalankan

1. Klik kanan `yt4k-downloader.exe` → **Properties**.
2. Di tab **General**, paling bawah ada tulisan
   "This file came from another computer..." — centang **Unblock**.
3. Klik **Apply** → **OK**, lalu jalankan seperti biasa.

Lewat PowerShell (untuk ketiga file sekaligus, dari foldernya):

```powershell
Get-ChildItem . | Unblock-File
```

### Cara agar tidak muncul lagi selamanya

- **Build sendiri (Cara B)** — cara gratis dan paling bersih. File yang
  dikompilasi di PC-mu sendiri tidak membawa tanda unduhan internet
  (Mark of the Web), jadi reputasi SmartScreen tidak diterapkan padanya.
- **Code signing** — cara resmi untuk distribusi publik: sertifikat
  OV/EV atau Azure Trusted Signing (berbayar, ±US$10/bulan untuk Trusted
  Signing). Dengan tanda tangan + reputasi, prompt hilang untuk semua
  pengguna. Berlebihan untuk pemakaian pribadi.

### Yang TIDAK disarankan

Jangan mematikan SmartScreen dari Settings demi aplikasi ini.
Itu menurunkan perlindungan untuk **semua** program di PC-mu.
"Run anyway" / Unblock hanya berlaku untuk file spesifik ini saja —
jauh lebih aman.

---

## Pertama Kali Jalan

1. Jalankan `yt4k-downloader.exe`.
2. Lihat kanan atas: indikator `yt-dlp` dan `ffmpeg`.
   - Cara A: keduanya hijau (OK) karena file exe-nya sudah satu folder.
   - Cara B / bila ada yang merah: klik tombol **Siapkan Tools** —
     aplikasi mengunduh sendiri `yt-dlp.exe` resmi (dari GitHub yt-dlp)
     dan `ffmpeg.exe` (build essentials gyan.dev) ke folder
     `%APPDATA%\YT4KDownloader\bin\`. Tunggu sampai selesai.
3. Tempel URL video YouTube → klik **Cek Info** untuk melihat judul,
   durasi, dan resolusi tertinggi yang tersedia.
4. Pilih kualitas (default 2160p/4K), atur folder output, klik
   **Download**.
5. Hasil akhir adalah satu file `.mkv` di folder output pilihanmu.

---

## Catatan Pemakaian

- **Mode Aman (rate limiter)** aktif secara default: jeda antar-request,
  jeda acak 5–15 detik antar-download, cooldown setelah tiap download,
  dan **lockout otomatis 15 menit** bila YouTube membalas 429
  (Too Many Requests). Jangan matikan kecuali kamu tahu alasannya.
- **Download putus di tengah?** Klik Download lagi dengan kualitas dan
  folder yang sama — unduhan **dilanjutkan** dari file `.part` yang
  sudah ada, bukan dari nol. File hasil tidak pernah korup setengah:
  `.mkv` final hanya dibuat setelah unduhan lengkap dan tergabung.
- **Video maksimal 1080p tapi memilih 2160p?** Aplikasi memberi
  peringatan dan mengunduh 1080p (resolusi asli tertinggi) — bukan
  upscale.
- **Log debug** ada di panel bawah aplikasi dan tersimpan sebagai file di
  `%APPDATA%\YT4KDownloader\logs\`. Token/cookie di log otomatis
  diredaksi. Kalau ada error, kirim file log itu untuk didiagnosis.

## Menghapus Aplikasi (Uninstall)

Tidak ada installer, tidak ada registry yang diubah. Cukup:

1. Hapus folder berisi `yt4k-downloader.exe`.
2. Hapus folder `%APPDATA%\YT4KDownloader` (berisi tools + log).

## Troubleshooting Cepat

| Gejala | Penyebab & Solusi |
|---|---|
| SmartScreen biru | Lihat bagian SmartScreen di atas: More info → Run anyway |
| yt-dlp / ffmpeg merah | Klik **Siapkan Tools**, pastikan internet menyala |
| Error 429 / lockout | Rate limit YouTube. Tunggu lockout selesai, jangan retry paksa |
| Download berhenti di tengah | Klik Download lagi (kualitas + folder sama) → otomatis resume |
| `cargo` tidak dikenal (Cara B) | Rust belum terinstall / terminal belum dibuka ulang setelah install |
| Video tidak ada 4K | Sumber videonya memang tidak menyediakan 2160p; aplikasi fallback ke tertinggi yang ada |
