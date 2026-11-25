# MessageMaster

Minimal-footprint **Rust + Tauri v2** desktop application for cleaning, mastering, compressing (< 30 MB), and sharing sermon/message recordings directly from Audacity (`.aup3`) or audio files (`.mp3`, `.wav`, `.mpeg`, `.m4a`).

## Features
- **Direct `.aup3` & Audio Support**: Reads Audacity 3.x SQLite projects directly or decodes standard audio via FFmpeg sidecar.
- **Dual-Pass Surgical De-Clicker**: Removes both micro mouth-clicks (`< 0.5 ms`) and wider isolated mic thumps (`0.5–15 ms`).
- **Vocal EQ & Electronic Whine Removal**: `80 Hz` HPF (`50 Hz` hum + rumble), `11.5 kHz` LPF (electronic whine), `-3 dB @ 350 Hz` de-box, `+2 dB @ 3.2 kHz` clarity.
- **True Spectral Noise Reduction (`n_fft = 4096`)**: Auto-extracts room tone without a noise gate, keeping quiet congregation responses intact.
- **RMS-Smoothed Silence Truncation**: Trims pauses `> 5.5s` down to `2.0s` with smooth `50 ms` crossfades.
- **Broadcast Mastering & Under-30 MB Export**: Normalizes to `-16.0 LUFS` (`-1.5 dBFS` ceiling) and encodes at the highest ABR MP3 bitrate that fits strictly under `30 MB`.
- **Google Drive Folder Browser & Upload**: Browse your Drive directories, create new folders inline, upload, and get a shareable link.
- **Editable Message Name & WhatsApp Share**: Pre-fills the file name (editable) and formats:
  ```text
  [Message Name]
  [Google Drive Link]
  ```
  with 1-click Copy and **Send via WhatsApp**.
- **Unobtrusive Background Update Check**: Quietly checks GitHub Releases on startup and shows a title-bar badge if an update is available.

## Building from Source
1. Install [Rust](https://rustup.rs/) and place `ffmpeg.exe` as `message_master/src-tauri/binaries/ffmpeg-x86_64-pc-windows-msvc.exe`.
2. Build release binary:
   ```powershell
   cd message_master/src-tauri
   cargo build --release
   ```
3. Also included: [`Sermon_Mastering_Chain.txt`](Sermon_Mastering_Chain.txt) (Audacity 3.7+ Macro) and [`process_sermon.py`](process_sermon.py) (standalone Python CLI fallback).
