use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serde::Serialize;
use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};

#[derive(Debug, Clone, Serialize)]
pub struct AudioInputDevice {
    pub name: String,
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize)]
struct LevelEvent {
    rms_db: f32,
    elapsed_s: f64,
    paused: bool,
    monitoring: bool,
}

struct ActiveRec {
    stop_flag: Arc<AtomicBool>,
    paused_flag: Arc<AtomicBool>,
    output_path: Option<PathBuf>,
    is_monitor: bool,
    join_handle: Option<std::thread::JoinHandle<Result<(), String>>>,
}

pub struct RecorderState {
    active: Mutex<Option<ActiveRec>>,
    gain_bits: Arc<AtomicU32>,
}

impl RecorderState {
    pub fn new(initial_gain: f32) -> Self {
        Self {
            active: Mutex::new(None),
            gain_bits: Arc::new(AtomicU32::new(initial_gain.clamp(0.0, 3.0).to_bits())),
        }
    }

    pub fn set_gain(&self, gain: f32) {
        self.gain_bits
            .store(gain.clamp(0.0, 3.0).to_bits(), Ordering::Relaxed);
    }
}

pub fn list_input_devices() -> Result<Vec<AudioInputDevice>, String> {
    let host = cpal::default_host();
    let default_name = host
        .default_input_device()
        .and_then(|d| d.description().ok().map(|desc| desc.name().to_string()));

    let devices = host
        .input_devices()
        .map_err(|e| format!("Failed to enumerate audio input devices: {e}"))?;

    let mut list = Vec::new();
    for dev in devices {
        if let Ok(desc) = dev.description() {
            let name = desc.name().to_string();
            let is_default = default_name.as_deref() == Some(name.as_str());
            list.push(AudioInputDevice { name, is_default });
        }
    }
    Ok(list)
}

fn find_input_device(device_name: Option<String>) -> Result<cpal::Device, String> {
    let host = cpal::default_host();
    if let Some(ref target) = device_name {
        let mut found = None;
        if let Ok(devs) = host.input_devices() {
            for d in devs {
                if let Ok(desc) = d.description() {
                    if desc.name() == target {
                        found = Some(d);
                        break;
                    }
                }
            }
        }
        found.or_else(|| host.default_input_device())
    } else {
        host.default_input_device()
    }
    .ok_or_else(|| "No microphone input device found.".to_string())
}

pub fn start_monitor(
    state: &RecorderState,
    app: AppHandle,
    device_name: Option<String>,
) -> Result<(), String> {
    let mut guard = state.active.lock().map_err(|e| e.to_string())?;
    if let Some(ref existing) = *guard {
        if !existing.is_monitor {
            // Do not interrupt an active recording
            return Ok(());
        }
    }
    if let Some(mut old) = guard.take() {
        old.stop_flag.store(true, Ordering::Relaxed);
        if let Some(jh) = old.join_handle.take() {
            let _ = jh.join();
        }
    }

    let device = find_input_device(device_name)?;
    let config = device
        .default_input_config()
        .map_err(|e| format!("Failed to get default mic config: {e}"))?;

    let channels = config.channels() as usize;
    let sample_format = config.sample_format();
    let stream_cfg: cpal::StreamConfig = config.into();

    let stop_flag = Arc::new(AtomicBool::new(false));
    let paused_flag = Arc::new(AtomicBool::new(false));
    let stop_c = stop_flag.clone();
    let gain_c = state.gain_bits.clone();

    let handle = std::thread::spawn(move || -> Result<(), String> {
        let latest_rms = Arc::new(Mutex::new(-90.0f32));
        let rms_c = latest_rms.clone();
        let err_fn = |err| eprintln!("Monitor stream error: {err}");

        let stream = match sample_format {
            cpal::SampleFormat::F32 => {
                let g = gain_c.clone();
                let r = rms_c.clone();
                device.build_input_stream(
                    stream_cfg,
                    move |data: &[f32], _| {
                        let gain = f32::from_bits(g.load(Ordering::Relaxed));
                        measure_rms_f32(data, channels, gain, &r);
                    },
                    err_fn,
                    None,
                )
            }
            cpal::SampleFormat::I16 => {
                let g = gain_c.clone();
                let r = rms_c.clone();
                device.build_input_stream(
                    stream_cfg,
                    move |data: &[i16], _| {
                        let gain = f32::from_bits(g.load(Ordering::Relaxed));
                        let f32_buf: Vec<f32> = data.iter().map(|&s| s as f32 / 32768.0).collect();
                        measure_rms_f32(&f32_buf, channels, gain, &r);
                    },
                    err_fn,
                    None,
                )
            }
            _ => return Err("Unsupported microphone sample format.".into()),
        }
        .map_err(|e| format!("Failed to open mic monitor stream: {e}"))?;

        stream.play().map_err(|e| format!("Failed to start mic monitor: {e}"))?;

        while !stop_c.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(65));
            let rms_db = *latest_rms.lock().unwrap_or_else(|e| e.into_inner());
            let _ = app.emit(
                "rec-level",
                LevelEvent {
                    rms_db,
                    elapsed_s: 0.0,
                    paused: false,
                    monitoring: true,
                },
            );
        }
        drop(stream);
        Ok(())
    });

    *guard = Some(ActiveRec {
        stop_flag,
        paused_flag,
        output_path: None,
        is_monitor: true,
        join_handle: Some(handle),
    });

    Ok(())
}

pub fn stop_monitor(state: &RecorderState) -> Result<(), String> {
    let mut guard = state.active.lock().map_err(|e| e.to_string())?;
    if let Some(ref existing) = *guard {
        if existing.is_monitor {
            if let Some(mut old) = guard.take() {
                old.stop_flag.store(true, Ordering::Relaxed);
                if let Some(jh) = old.join_handle.take() {
                    let _ = jh.join();
                }
            }
        }
    }
    Ok(())
}

pub fn start_recording(
    state: &RecorderState,
    app: AppHandle,
    device_name: Option<String>,
) -> Result<String, String> {
    let mut guard = state.active.lock().map_err(|e| e.to_string())?;
    if let Some(ref existing) = *guard {
        if !existing.is_monitor {
            return Err("Recording is already in progress.".into());
        }
    }
    // Stop active monitor if running
    if let Some(mut mon) = guard.take() {
        mon.stop_flag.store(true, Ordering::Relaxed);
        if let Some(jh) = mon.join_handle.take() {
            let _ = jh.join();
        }
    }

    let device = find_input_device(device_name)?;
    let config = device
        .default_input_config()
        .map_err(|e| format!("Failed to get default mic config: {e}"))?;

    let sr = config.sample_rate();
    let channels = config.channels() as usize;
    let sample_format = config.sample_format();
    let stream_cfg: cpal::StreamConfig = config.into();

    let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let out_path = std::env::temp_dir().join(format!("MessageMaster_rec_{ts}.wav"));
    let out_path_clone = out_path.clone();

    let stop_flag = Arc::new(AtomicBool::new(false));
    let paused_flag = Arc::new(AtomicBool::new(false));
    let stop_c = stop_flag.clone();
    let paused_c = paused_flag.clone();
    let gain_c = state.gain_bits.clone();

    let handle = std::thread::spawn(move || -> Result<(), String> {
        let file = File::create(&out_path_clone)
            .map_err(|e| format!("Failed to create recording WAV: {e}"))?;
        let writer = Arc::new(Mutex::new(BufWriter::new(file)));
        {
            let mut w = writer.lock().map_err(|e| e.to_string())?;
            write_wav_header(&mut *w, sr, 0)?;
        }

        let frames_written = Arc::new(AtomicU64::new(0));
        let fw_c = frames_written.clone();
        let w_c = writer.clone();
        let p_c = paused_c.clone();
        let latest_rms = Arc::new(Mutex::new(-90.0f32));
        let rms_c = latest_rms.clone();

        let err_fn = |err| eprintln!("Recording stream error: {err}");

        let stream = match sample_format {
            cpal::SampleFormat::F32 => {
                let g = gain_c.clone();
                device.build_input_stream(
                    stream_cfg,
                    move |data: &[f32], _| {
                        let gain = f32::from_bits(g.load(Ordering::Relaxed));
                        handle_input_f32(data, channels, gain, &p_c, &w_c, &fw_c, &rms_c);
                    },
                    err_fn,
                    None,
                )
            }
            cpal::SampleFormat::I16 => {
                let g = gain_c.clone();
                device.build_input_stream(
                    stream_cfg,
                    move |data: &[i16], _| {
                        let gain = f32::from_bits(g.load(Ordering::Relaxed));
                        let f32_buf: Vec<f32> = data.iter().map(|&s| s as f32 / 32768.0).collect();
                        handle_input_f32(&f32_buf, channels, gain, &p_c, &w_c, &fw_c, &rms_c);
                    },
                    err_fn,
                    None,
                )
            }
            _ => return Err("Unsupported microphone sample format.".into()),
        }
        .map_err(|e| format!("Failed to open microphone stream: {e}"))?;

        stream.play().map_err(|e| format!("Failed to start mic stream: {e}"))?;

        while !stop_c.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(65));
            let rms_db = *latest_rms.lock().unwrap_or_else(|e| e.into_inner());
            let elapsed_s = frames_written.load(Ordering::Relaxed) as f64 / sr as f64;
            let _ = app.emit(
                "rec-level",
                LevelEvent {
                    rms_db,
                    elapsed_s,
                    paused: paused_c.load(Ordering::Relaxed),
                    monitoring: false,
                },
            );
        }

        drop(stream);

        // Finalize WAV header sizes
        let total_frames = frames_written.load(Ordering::Relaxed);
        let data_bytes = (total_frames * 2) as u32;
        let mut w = writer.lock().map_err(|e| e.to_string())?;
        w.flush().map_err(|e| e.to_string())?;
        let inner = w.get_mut();
        inner.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
        write_wav_header(inner, sr, data_bytes)?;
        inner.flush().map_err(|e| e.to_string())?;
        Ok(())
    });

    *guard = Some(ActiveRec {
        stop_flag,
        paused_flag,
        output_path: Some(out_path.clone()),
        is_monitor: false,
        join_handle: Some(handle),
    });

    Ok(out_path.to_string_lossy().to_string())
}

pub fn toggle_pause(state: &RecorderState) -> Result<bool, String> {
    let guard = state.active.lock().map_err(|e| e.to_string())?;
    let rec = guard
        .as_ref()
        .filter(|r| !r.is_monitor)
        .ok_or("No active recording.")?;
    let prev = rec.paused_flag.load(Ordering::Relaxed);
    let next = !prev;
    rec.paused_flag.store(next, Ordering::Relaxed);
    Ok(next)
}

pub fn stop_recording(state: &RecorderState) -> Result<String, String> {
    let mut guard = state.active.lock().map_err(|e| e.to_string())?;
    let is_rec = guard.as_ref().map(|r| !r.is_monitor).unwrap_or(false);
    if !is_rec {
        return Err("No active recording.".into());
    }
    let mut rec = guard.take().ok_or("No active recording.")?;
    rec.stop_flag.store(true, Ordering::Relaxed);
    if let Some(jh) = rec.join_handle.take() {
        jh.join().map_err(|_| "Recorder thread panicked".to_string())??;
    }
    let path = rec
        .output_path
        .ok_or_else(|| "Missing recording output path".to_string())?;
    Ok(path.to_string_lossy().to_string())
}

fn measure_rms_f32(data: &[f32], channels: usize, gain: f32, latest_rms: &Mutex<f32>) {
    if data.is_empty() || channels == 0 {
        return;
    }
    let n_frames = data.len() / channels;
    let mut sum_sq = 0.0f32;
    for f in 0..n_frames {
        let mut mono = 0.0f32;
        for ch in 0..channels {
            mono += data[f * channels + ch];
        }
        mono = (mono / channels as f32) * gain;
        sum_sq += mono * mono;
    }
    let rms = (sum_sq / n_frames.max(1) as f32).sqrt();
    let db = 20.0 * (rms + 1e-6).log10();
    if let Ok(mut r) = latest_rms.lock() {
        *r = db.max(-90.0);
    }
}

fn handle_input_f32(
    data: &[f32],
    channels: usize,
    gain: f32,
    paused: &AtomicBool,
    writer: &Mutex<BufWriter<File>>,
    frames_written: &AtomicU64,
    latest_rms: &Mutex<f32>,
) {
    if data.is_empty() || channels == 0 {
        return;
    }
    let n_frames = data.len() / channels;
    let mut sum_sq = 0.0f32;
    let mut pcm_bytes = Vec::with_capacity(n_frames * 2);

    for f in 0..n_frames {
        let mut mono = 0.0f32;
        for ch in 0..channels {
            mono += data[f * channels + ch];
        }
        mono = (mono / channels as f32) * gain;
        sum_sq += mono * mono;
        let s16 = (mono.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        pcm_bytes.extend_from_slice(&s16.to_le_bytes());
    }

    let rms = (sum_sq / n_frames.max(1) as f32).sqrt();
    let db = 20.0 * (rms + 1e-6).log10();
    if let Ok(mut r) = latest_rms.lock() {
        *r = db.max(-90.0);
    }

    if !paused.load(Ordering::Relaxed) {
        if let Ok(mut w) = writer.lock() {
            if w.write_all(&pcm_bytes).is_ok() {
                frames_written.fetch_add(n_frames as u64, Ordering::Relaxed);
            }
        }
    }
}

fn write_wav_header<W: Write>(w: &mut W, sr: u32, data_bytes: u32) -> Result<(), String> {
    let riff_len = 36u32.saturating_add(data_bytes);
    let byte_rate = sr * 2;
    w.write_all(b"RIFF").map_err(|e| e.to_string())?;
    w.write_all(&riff_len.to_le_bytes()).map_err(|e| e.to_string())?;
    w.write_all(b"WAVEfmt ").map_err(|e| e.to_string())?;
    w.write_all(&16u32.to_le_bytes()).map_err(|e| e.to_string())?;
    w.write_all(&1u16.to_le_bytes()).map_err(|e| e.to_string())?;
    w.write_all(&1u16.to_le_bytes()).map_err(|e| e.to_string())?;
    w.write_all(&sr.to_le_bytes()).map_err(|e| e.to_string())?;
    w.write_all(&byte_rate.to_le_bytes()).map_err(|e| e.to_string())?;
    w.write_all(&2u16.to_le_bytes()).map_err(|e| e.to_string())?;
    w.write_all(&16u16.to_le_bytes()).map_err(|e| e.to_string())?;
    w.write_all(b"data").map_err(|e| e.to_string())?;
    w.write_all(&data_bytes.to_le_bytes()).map_err(|e| e.to_string())?;
    Ok(())
}
