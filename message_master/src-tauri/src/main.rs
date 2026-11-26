#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod aup3;
mod catalog;
mod config;
mod dsp;
mod editor;
mod gdrive;
mod recorder;
mod updater;

use catalog::MessageCatalog;
use chrono::Datelike;
use config::AppConfig;
use editor::{EditorState, WaveformSummary};
use gdrive::DriveFolder;
use recorder::{AudioInputDevice, RecorderState};
use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::{
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, State, WebviewBuilder, WebviewUrl,
};

#[derive(Debug, Clone, Serialize)]
struct ProgressEvent {
    step: u8,
    label: String,
}

#[derive(Debug, Clone, Serialize)]
struct ProcessResult {
    display_name: String,
    output_path: String,
    output_size_mb: f64,
    bitrate_kbps: u32,
    clicks_repaired: usize,
    silences_trimmed: usize,
    seconds_trimmed: f64,
    original_duration_s: f64,
    mastered_duration_s: f64,
    initial_lufs: f64,
    final_lufs: f64,
    drive_link: Option<String>,
    drive_error: Option<String>,
    share_message: String,
    whatsapp_url: String,
}

fn emit_progress(app: &AppHandle, step: u8, label: &str) {
    let _ = app.emit(
        "mastering-progress",
        ProgressEvent {
            step,
            label: label.to_string(),
        },
    );
}

fn resolve_ffmpeg(app: &AppHandle) -> Result<PathBuf, String> {
    if let Ok(exe_dir) = std::env::current_exe() {
        if let Some(dir) = exe_dir.parent() {
            for name in ["ffmpeg.exe", "ffmpeg-x86_64-pc-windows-msvc.exe", "ffmpeg"] {
                let p = dir.join(name);
                if p.exists() {
                    return Ok(p);
                }
            }
        }
    }
    if let Ok(res_dir) = app.path().resource_dir() {
        for name in [
            "binaries/ffmpeg.exe",
            "binaries/ffmpeg-x86_64-pc-windows-msvc.exe",
            "ffmpeg.exe",
        ] {
            let p = res_dir.join(name);
            if p.exists() {
                return Ok(p);
            }
        }
    }
    let dev_bin = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("binaries")
        .join("ffmpeg-x86_64-pc-windows-msvc.exe");
    if dev_bin.exists() {
        return Ok(dev_bin);
    }
    Err("Bundled FFmpeg sidecar not found.".into())
}

#[tauri::command]
fn get_config() -> AppConfig {
    AppConfig::load()
}

#[tauri::command]
fn save_config(cfg: AppConfig) -> Result<(), String> {
    cfg.save()
}

#[tauri::command]
fn get_app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[tauri::command]
async fn check_updates_now(app: AppHandle) -> Option<updater::UpdateInfo> {
    let res = updater::check_and_download_update().await;
    if let Some(ref info) = res {
        let _ = app.emit("update-available", info.clone());
    }
    res
}

#[tauri::command]
fn install_update_and_restart(app: AppHandle, installer_path: String) -> Result<(), String> {
    updater::run_installer_and_restart(&installer_path)?;
    app.exit(0);
    Ok(())
}

#[tauri::command]
async fn connect_google_drive(cfg: AppConfig) -> Result<String, String> {
    let mut updated = cfg;
    updated.save()?;
    gdrive::connect_oauth(&mut updated).await
}

#[tauri::command]
async fn list_drive_folders(parent_id: Option<String>) -> Result<Vec<DriveFolder>, String> {
    let cfg = AppConfig::load();
    gdrive::list_folders(&cfg, parent_id.as_deref()).await
}

#[tauri::command]
async fn create_drive_folder(
    parent_id: Option<String>,
    name: String,
) -> Result<DriveFolder, String> {
    let cfg = AppConfig::load();
    gdrive::create_folder(&cfg, parent_id.as_deref(), &name).await
}

#[tauri::command]
async fn rename_drive_folder(folder_id: String, new_name: String) -> Result<DriveFolder, String> {
    let cfg = AppConfig::load();
    gdrive::rename_folder(&cfg, &folder_id, &new_name).await
}

#[tauri::command]
async fn delete_drive_folder(folder_id: String) -> Result<(), String> {
    let cfg = AppConfig::load();
    gdrive::delete_folder(&cfg, &folder_id).await
}

fn resolve_target_dir(base_dir: &Path, mode: &str, save_date: Option<&str>) -> PathBuf {
    if mode.eq_ignore_ascii_case("base") {
        let parsed_date = save_date
            .and_then(|s| chrono::NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok())
            .unwrap_or_else(|| chrono::Local::now().date_naive());
        let year_str = parsed_date.format("%Y").to_string();
        let month_str = format!("{}. {}", parsed_date.month(), parsed_date.format("%B"));
        base_dir.join(year_str).join(month_str)
    } else {
        base_dir.to_path_buf()
    }
}

fn default_backup_root() -> PathBuf {
    dirs::document_dir()
        .or_else(dirs::data_local_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("MessageMaster")
        .join("Backups")
}

fn save_original_backup(
    src_path: &Path,
    safe_base_name: &str,
    cfg: &AppConfig,
    save_date: Option<&str>,
) -> Result<PathBuf, String> {
    if !src_path.exists() {
        return Err("Original source file no longer exists for backup.".into());
    }
    let root = if cfg.backup_dir.trim().is_empty() {
        default_backup_root()
    } else {
        PathBuf::from(cfg.backup_dir.trim())
    };
    let target_dir = resolve_target_dir(&root, &cfg.backup_save_mode, save_date);
    if !target_dir.exists() {
        std::fs::create_dir_all(&target_dir)
            .map_err(|e| format!("Failed to create backup folder: {e}"))?;
    }
    let ext = src_path
        .extension()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("wav");
    let mut dest = target_dir.join(format!("{safe_base_name} (Original).{ext}"));
    if dest.exists() {
        let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
        dest = target_dir.join(format!("{safe_base_name} (Original)_{ts}.{ext}"));
    }
    std::fs::copy(src_path, &dest)
        .map_err(|e| format!("Failed to copy original recording backup: {e}"))?;
    Ok(dest)
}

#[tauri::command]
async fn process_audio(
    app: AppHandle,
    input_path: String,
    custom_name: Option<String>,
    output_dir: Option<String>,
    save_mode: Option<String>,
    save_date: Option<String>,
    original_backup_source: Option<String>,
) -> Result<ProcessResult, String> {
    let in_path = PathBuf::from(&input_path);
    if !in_path.exists() {
        return Err("Selected file does not exist.".into());
    }

    let cfg = AppConfig::load();
    let ffmpeg = resolve_ffmpeg(&app)?;

    let raw_stem = in_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Message Audio")
        .to_string();
    let default_name = raw_stem
        .strip_suffix(".mp3")
        .unwrap_or(&raw_stem)
        .trim()
        .to_string();

    let display_name = custom_name
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or(default_name);

    let safe_filename: String = display_name
        .chars()
        .map(|c| if "<>:\"/\\|?*".contains(c) { '-' } else { c })
        .collect();

    let chosen_dir = output_dir
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            let c = cfg.local_output_dir.trim();
            if c.is_empty() { None } else { Some(c.to_string()) }
        });

    let base_out_dir = match chosen_dir {
        Some(d) => PathBuf::from(d),
        None => in_path.parent().unwrap_or_else(|| Path::new(".")).to_path_buf(),
    };

    let mode = save_mode
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| cfg.save_mode.to_lowercase());

    let out_dir = resolve_target_dir(&base_out_dir, &mode, save_date.as_deref());

    if !out_dir.exists() {
        std::fs::create_dir_all(&out_dir)
            .map_err(|e| format!("Failed to create output folder: {e}"))?;
    }
    let out_path = out_dir.join(format!("{safe_filename}.mp3"));

    let app_clone = app.clone();
    let in_path_clone = in_path.clone();
    let out_path_clone = out_path.clone();
    let ffmpeg_clone = ffmpeg.clone();
    let max_mb = cfg.max_size_mb;
    let target_lufs = cfg.target_lufs;

    let stats = tokio::task::spawn_blocking(move || -> Result<dsp::MasteringStats, String> {
        emit_progress(&app_clone, 1, "Reading audio track...");
        let ext = in_path_clone
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();

        let (mut samples, sr) = if ext == "aup3" {
            let (raw, native_sr) = aup3::read_aup3_mono(&in_path_clone)?;
            if native_sr != 44100 {
                (dsp::resample_via_ffmpeg(&ffmpeg_clone, &raw, native_sr, 44100)?, 44100)
            } else {
                (raw, native_sr)
            }
        } else {
            dsp::decode_via_ffmpeg(&ffmpeg_clone, &in_path_clone)?
        };

        let original_duration_s = samples.len() as f64 / sr as f64;
        let initial_lufs = dsp::measure_lufs(&samples, sr as f32);

        emit_progress(&app_clone, 2, "Removing clicks & shaping vocal EQ...");
        let clicks_repaired = dsp::dual_pass_declick(&mut samples, sr);
        dsp::apply_sermon_eq(&mut samples, sr as f32);

        emit_progress(&app_clone, 3, "Reducing noise & electronic whine...");
        dsp::apply_spectral_noise_reduction(&mut samples, sr, 0.85);

        emit_progress(&app_clone, 4, "Truncating long silences (> 5.5s -> 2.0s)...");
        let (mut samples, silences_trimmed, seconds_trimmed) =
            dsp::truncate_long_silences(samples, sr, -49.0, 5.5, 2.0);

        emit_progress(&app_clone, 5, "Mastering dynamics & exporting under 30 MB...");
        dsp::apply_compressor(&mut samples, sr as f32);
        let (final_lufs, final_peak_db) =
            dsp::normalize_and_limit(&mut samples, sr as f32, target_lufs, -1.5);

        let (bytes, bitrate_kbps) =
            dsp::encode_under_max_mb(&ffmpeg_clone, &samples, sr, &out_path_clone, max_mb)?;

        Ok(dsp::MasteringStats {
            original_duration_s,
            mastered_duration_s: samples.len() as f64 / sr as f64,
            clicks_repaired,
            silences_trimmed,
            seconds_trimmed,
            initial_lufs,
            final_lufs,
            final_peak_db,
            output_path: out_path_clone.to_string_lossy().to_string(),
            output_size_mb: bytes as f64 / (1000.0 * 1000.0),
            bitrate_kbps,
        })
    })
    .await
    .map_err(|e| format!("Processing thread error: {e}"))??;

    if cfg.backup_enabled {
        let backup_src = original_backup_source
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .filter(|p| p.exists())
            .unwrap_or_else(|| in_path.clone());
        let _ = save_original_backup(&backup_src, &safe_filename, &cfg, save_date.as_deref());
    }

    let share_message = format!("{display_name}\n{}", stats.output_path);
    let whatsapp_url = format!(
        "https://wa.me/?text={}",
        urlencoding::encode(&share_message)
    );

    Ok(ProcessResult {
        display_name,
        output_path: stats.output_path,
        output_size_mb: stats.output_size_mb,
        bitrate_kbps: stats.bitrate_kbps,
        clicks_repaired: stats.clicks_repaired,
        silences_trimmed: stats.silences_trimmed,
        seconds_trimmed: stats.seconds_trimmed,
        original_duration_s: stats.original_duration_s,
        mastered_duration_s: stats.mastered_duration_s,
        initial_lufs: stats.initial_lufs,
        final_lufs: stats.final_lufs,
        drive_link: None,
        drive_error: None,
        share_message,
        whatsapp_url,
    })
}

#[derive(Debug, Clone, Serialize)]
struct UploadResult {
    drive_link: String,
    share_message: String,
    whatsapp_url: String,
}

#[tauri::command]
async fn upload_mastered_file(
    file_path: String,
    display_name: String,
    drive_name: Option<String>,
    drive_mode: Option<String>,
    save_date: Option<String>,
) -> Result<UploadResult, String> {
    let cfg = AppConfig::load();
    let path = PathBuf::from(&file_path);
    if !path.exists() {
        return Err("Mastered file not found on disk.".into());
    }
    let final_name = drive_name
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| display_name.trim().to_string());

    let safe_filename: String = final_name
        .chars()
        .map(|c| if "<>:\"/\\|?*".contains(c) { '-' } else { c })
        .collect();
    let upload_filename = format!("{safe_filename}.mp3");
    let link = gdrive::upload_file(
        &cfg,
        &path,
        &upload_filename,
        drive_mode.as_deref(),
        save_date.as_deref(),
    )
    .await?;
    let share_message = format!("{final_name}\n{link}");
    let whatsapp_url = format!(
        "https://wa.me/?text={}",
        urlencoding::encode(&share_message)
    );
    Ok(UploadResult {
        drive_link: link,
        share_message,
        whatsapp_url,
    })
}

#[tauri::command]
fn get_catalog() -> MessageCatalog {
    MessageCatalog::load()
}

#[tauri::command]
fn save_catalog_entry(title: String, part: u32, preacher: String, language: String) {
    let mut cat = MessageCatalog::load();
    cat.record_usage(&title, part, &preacher, &language);
}

#[tauri::command]
fn list_audio_inputs() -> Result<Vec<AudioInputDevice>, String> {
    recorder::list_input_devices()
}

#[tauri::command]
fn rec_monitor_start(
    app: AppHandle,
    state: State<'_, RecorderState>,
    device_name: Option<String>,
) -> Result<(), String> {
    recorder::start_monitor(&state, app, device_name)
}

#[tauri::command]
fn rec_monitor_stop(state: State<'_, RecorderState>) -> Result<(), String> {
    recorder::stop_monitor(&state)
}

#[tauri::command]
fn rec_set_gain(state: State<'_, RecorderState>, gain: f32) -> Result<(), String> {
    state.set_gain(gain);
    Ok(())
}

#[tauri::command]
fn rec_start(
    app: AppHandle,
    state: State<'_, RecorderState>,
    device_name: Option<String>,
) -> Result<String, String> {
    recorder::start_recording(&state, app, device_name)
}

#[tauri::command]
fn rec_pause_toggle(state: State<'_, RecorderState>) -> Result<bool, String> {
    recorder::toggle_pause(&state)
}

#[tauri::command]
fn rec_stop(state: State<'_, RecorderState>) -> Result<String, String> {
    recorder::stop_recording(&state)
}

#[tauri::command]
fn rec_clear_and_backup(
    state: State<'_, RecorderState>,
    save_date: Option<String>,
) -> Result<String, String> {
    let temp_wav = recorder::stop_recording(&state)?;
    let cfg = AppConfig::load();
    let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let backup_name = format!("Cleared_Take_{ts}");
    let backed_up = save_original_backup(
        Path::new(&temp_wav),
        &backup_name,
        &cfg,
        save_date.as_deref(),
    )?;
    let _ = std::fs::remove_file(&temp_wav);
    Ok(backed_up.to_string_lossy().to_string())
}

#[tauri::command]
async fn editor_load(
    app: AppHandle,
    state: State<'_, EditorState>,
    input_path: String,
) -> Result<WaveformSummary, String> {
    let ffmpeg = resolve_ffmpeg(&app)?;
    let path = PathBuf::from(&input_path);
    if !path.exists() {
        return Err("Audio file not found.".into());
    }
    editor::load_file(&state, &ffmpeg, &path)
}

#[tauri::command]
fn editor_cut(state: State<'_, EditorState>, start_s: f64, end_s: f64) -> Result<WaveformSummary, String> {
    editor::cut_region(&state, start_s, end_s)
}

#[tauri::command]
fn editor_copy(state: State<'_, EditorState>, start_s: f64, end_s: f64) -> Result<WaveformSummary, String> {
    editor::copy_region(&state, start_s, end_s)
}

#[tauri::command]
fn editor_delete(state: State<'_, EditorState>, start_s: f64, end_s: f64) -> Result<WaveformSummary, String> {
    editor::delete_region(&state, start_s, end_s)
}

#[tauri::command]
fn editor_paste(state: State<'_, EditorState>, at_s: f64) -> Result<WaveformSummary, String> {
    editor::paste_at(&state, at_s)
}

#[tauri::command]
fn editor_undo(state: State<'_, EditorState>) -> Result<WaveformSummary, String> {
    editor::undo(&state)
}

#[tauri::command]
fn editor_play(app: AppHandle, state: State<'_, EditorState>, start_s: f64) -> Result<(), String> {
    editor::play_from(&state, app, start_s)
}

#[tauri::command]
fn editor_stop(state: State<'_, EditorState>) {
    editor::stop_playback(&state);
}

#[tauri::command]
fn editor_export_wav(state: State<'_, EditorState>) -> Result<String, String> {
    editor::export_temp_wav(&state)
}

#[tauri::command]
fn editor_peaks(
    state: State<'_, EditorState>,
    start_s: f64,
    end_s: f64,
    buckets: usize,
) -> Result<Vec<(f32, f32)>, String> {
    editor::get_peaks_range(&state, start_s, end_s, buckets)
}

#[tauri::command]
async fn embed_meet(
    app: AppHandle,
    url: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<(), String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err("Please enter your Google Meet URL first.".into());
    }
    let parsed: tauri::Url = trimmed
        .parse()
        .map_err(|e| format!("Invalid Google Meet URL: {e}"))?;

    if let Some(wv) = app.get_webview("meet-embed") {
        let _ = wv.set_position(LogicalPosition::new(x, y));
        let _ = wv.set_size(LogicalSize::new(width.max(100.0), height.max(100.0)));
        let _ = wv.navigate(parsed);
        let _ = wv.show();
        return Ok(());
    }

    let main_win = app
        .get_window("main")
        .ok_or_else(|| "Main window not found.".to_string())?;

    let builder = WebviewBuilder::new("meet-embed", WebviewUrl::External(parsed));
    main_win
        .add_child(
            builder,
            LogicalPosition::new(x, y),
            LogicalSize::new(width.max(100.0), height.max(100.0)),
        )
        .map_err(|e| format!("Failed to embed Google Meet: {e}"))?;

    Ok(())
}

#[tauri::command]
fn set_meet_bounds(
    app: AppHandle,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    visible: bool,
) -> Result<(), String> {
    if let Some(wv) = app.get_webview("meet-embed") {
        if visible && width > 40.0 && height > 40.0 {
            let _ = wv.set_position(LogicalPosition::new(x, y));
            let _ = wv.set_size(LogicalSize::new(width, height));
            let _ = wv.show();
        } else {
            let _ = wv.hide();
        }
    }
    Ok(())
}

#[tauri::command]
fn meet_control(app: AppHandle, action: String) -> Result<(), String> {
    let wv = app
        .get_webview("meet-embed")
        .ok_or_else(|| "Load Google Meet in the embedded panel first.".to_string())?;

    let js = match action.as_str() {
        "toggle_cam" => {
            r#"(function(){
                const btn = document.querySelector('[aria-label*="camera" i]');
                if (btn) btn.click();
            })();"#
        }
        "start" => {
            r#"(function(){
                const camOff = document.querySelector('[aria-label*="Turn on camera" i]');
                if (camOff) camOff.click();
                const join = Array.from(document.querySelectorAll('button')).find(b => /join now|ask to join/i.test(b.innerText));
                if (join) join.click();
                setTimeout(() => {
                    const recQuick = document.querySelector('[aria-label*="Record" i]');
                    if (recQuick) { recQuick.click(); return; }
                    const act = document.querySelector('[aria-label*="Activities" i], [aria-label*="Meeting tools" i]');
                    if (act) {
                        act.click();
                        setTimeout(() => {
                            const recItem = Array.from(document.querySelectorAll('[role="menuitem"], [role="button"], button, li'))
                                .find(el => /recording/i.test(el.innerText || ''));
                            if (recItem) {
                                recItem.click();
                                setTimeout(() => {
                                    const startBtn = Array.from(document.querySelectorAll('button'))
                                        .find(b => /start recording/i.test(b.innerText || ''));
                                    if (startBtn) {
                                        startBtn.click();
                                        setTimeout(() => {
                                            const confirmBtn = Array.from(document.querySelectorAll('button'))
                                                .find(b => /^start$/i.test((b.innerText || '').trim()));
                                            if (confirmBtn) confirmBtn.click();
                                        }, 500);
                                    }
                                }, 600);
                            }
                        }, 500);
                    }
                }, 800);
            })();"#
        }
        "stop" => {
            r#"(function(){
                const stopBtn = Array.from(document.querySelectorAll('button, [role="button"]'))
                    .find(b => /stop recording/i.test(b.innerText || b.getAttribute('aria-label') || ''));
                if (stopBtn) {
                    stopBtn.click();
                    setTimeout(() => {
                        const confirm = Array.from(document.querySelectorAll('button'))
                            .find(b => /stop recording/i.test(b.innerText || ''));
                        if (confirm) confirm.click();
                    }, 500);
                }
            })();"#
        }
        _ => return Err("Unknown Meet control action.".into()),
    };

    wv.eval(js)
        .map_err(|e| format!("Failed to control Meet view: {e}"))
}

fn main() {
    let initial_gain = AppConfig::load().mic_gain;
    tauri::Builder::default()
        .manage(RecorderState::new(initial_gain))
        .manage(EditorState::new())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .setup(|app| {
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Some(info) = updater::check_and_download_update().await {
                    let _ = handle.emit("update-available", info);
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_config,
            save_config,
            get_app_version,
            check_updates_now,
            install_update_and_restart,
            connect_google_drive,
            list_drive_folders,
            create_drive_folder,
            rename_drive_folder,
            delete_drive_folder,
            process_audio,
            upload_mastered_file,
            get_catalog,
            save_catalog_entry,
            list_audio_inputs,
            rec_monitor_start,
            rec_monitor_stop,
            rec_set_gain,
            rec_start,
            rec_pause_toggle,
            rec_stop,
            rec_clear_and_backup,
            editor_load,
            editor_cut,
            editor_copy,
            editor_delete,
            editor_paste,
            editor_undo,
            editor_play,
            editor_stop,
            editor_export_wav,
            editor_peaks,
            embed_meet,
            set_meet_bounds,
            meet_control
        ])
        .run(tauri::generate_context!())
        .expect("error while running MessageMaster");
}
