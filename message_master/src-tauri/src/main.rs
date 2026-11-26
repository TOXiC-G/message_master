#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod aup3;
mod config;
mod dsp;
mod gdrive;
mod updater;

use config::AppConfig;
use chrono::Datelike;
use gdrive::DriveFolder;
use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager};

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

#[tauri::command]
async fn process_audio(
    app: AppHandle,
    input_path: String,
    custom_name: Option<String>,
    output_dir: Option<String>,
    save_mode: Option<String>,
    save_date: Option<String>,
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

    let out_dir = if mode == "base" {
        let parsed_date = save_date
            .as_deref()
            .and_then(|s| chrono::NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok())
            .unwrap_or_else(|| chrono::Local::now().date_naive());
        let year_str = parsed_date.format("%Y").to_string();
        let month_str = format!("{}. {}", parsed_date.month(), parsed_date.format("%B")); // e.g. "11. November"
        base_out_dir.join(year_str).join(month_str)
    } else {
        base_out_dir
    };

    if !out_dir.exists() {
        std::fs::create_dir_all(&out_dir)
            .map_err(|e| format!("Failed to create output folder: {e}"))?;
    }
    let out_path = out_dir.join(format!("{safe_filename}_MASTERED.mp3"));

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

fn main() {
    tauri::Builder::default()
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
            upload_mastered_file
        ])
        .run(tauri::generate_context!())
        .expect("error while running MessageMaster");
}
