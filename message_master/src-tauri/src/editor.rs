use crate::{aup3, dsp};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serde::Serialize;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter};

#[derive(Debug, Clone, Serialize)]
pub struct WaveformSummary {
    pub duration_s: f64,
    pub sample_rate: u32,
    pub peaks: Vec<(f32, f32)>,
    pub has_clipboard: bool,
    pub can_undo: bool,
}

#[derive(Debug, Clone, Serialize)]
struct PlaybackEvent {
    current_s: f64,
    playing: bool,
}

struct EditSession {
    samples: Vec<f32>,
    sr: u32,
    clipboard: Vec<f32>,
    undo_stack: Vec<Vec<f32>>,
}

pub struct EditorState {
    session: Mutex<Option<EditSession>>,
    stop_play: Arc<AtomicBool>,
}

impl EditorState {
    pub fn new() -> Self {
        Self {
            session: Mutex::new(None),
            stop_play: Arc::new(AtomicBool::new(true)),
        }
    }
}

fn compute_peaks(slice: &[f32], buckets: usize) -> Vec<(f32, f32)> {
    let n = slice.len();
    let buckets = buckets.max(1);
    let mut peaks = Vec::with_capacity(buckets);
    if n == 0 {
        return peaks;
    }
    let step = (n as f64 / buckets as f64).max(1.0);
    for b in 0..buckets {
        let s0 = (b as f64 * step) as usize;
        let s1 = (((b + 1) as f64 * step) as usize).min(n);
        if s0 >= s1 {
            peaks.push((0.0, 0.0));
            continue;
        }
        let mut mn = 0.0f32;
        let mut mx = 0.0f32;
        for &v in &slice[s0..s1] {
            if v < mn { mn = v; }
            if v > mx { mx = v; }
        }
        peaks.push((mn, mx));
    }
    peaks
}

fn build_summary(sess: &EditSession) -> WaveformSummary {
    let n = sess.samples.len();
    let peaks = compute_peaks(&sess.samples, 1600);
    WaveformSummary {
        duration_s: n as f64 / sess.sr as f64,
        sample_rate: sess.sr,
        peaks,
        has_clipboard: !sess.clipboard.is_empty(),
        can_undo: !sess.undo_stack.is_empty(),
    }
}

pub fn get_peaks_range(
    state: &EditorState,
    start_s: f64,
    end_s: f64,
    buckets: usize,
) -> Result<Vec<(f32, f32)>, String> {
    let guard = state.session.lock().map_err(|e| e.to_string())?;
    let sess = guard.as_ref().ok_or("No audio loaded in editor.")?;
    let n = sess.samples.len();
    if n == 0 {
        return Ok(Vec::new());
    }
    let sr = sess.sr as f64;
    let i0 = ((start_s.max(0.0) * sr) as usize).min(n.saturating_sub(1));
    let i1 = ((end_s.max(0.0) * sr) as usize).clamp(i0 + 1, n);
    Ok(compute_peaks(&sess.samples[i0..i1], buckets.clamp(64, 4000)))
}

pub fn load_file(
    state: &EditorState,
    ffmpeg_path: &Path,
    input_path: &Path,
) -> Result<WaveformSummary, String> {
    stop_playback(state);
    let ext = input_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    let (samples, sr) = if ext == "aup3" {
        let (raw, native_sr) = aup3::read_aup3_mono(input_path)?;
        if native_sr != 44100 {
            (dsp::resample_via_ffmpeg(ffmpeg_path, &raw, native_sr, 44100)?, 44100)
        } else {
            (raw, native_sr)
        }
    } else {
        dsp::decode_via_ffmpeg(ffmpeg_path, input_path)?
    };

    let mut guard = state.session.lock().map_err(|e| e.to_string())?;
    let sess = EditSession {
        samples,
        sr,
        clipboard: Vec::new(),
        undo_stack: Vec::new(),
    };
    let sum = build_summary(&sess);
    *guard = Some(sess);
    Ok(sum)
}

fn time_to_idx(t: f64, sr: u32, len: usize) -> usize {
    ((t.max(0.0) * sr as f64).round() as usize).min(len)
}

fn push_undo(sess: &mut EditSession) {
    if sess.undo_stack.len() >= 10 {
        sess.undo_stack.remove(0);
    }
    sess.undo_stack.push(sess.samples.clone());
}

pub fn cut_region(state: &EditorState, start_s: f64, end_s: f64) -> Result<WaveformSummary, String> {
    stop_playback(state);
    let mut guard = state.session.lock().map_err(|e| e.to_string())?;
    let sess = guard.as_mut().ok_or("No audio loaded in editor.")?;
    let a = time_to_idx(start_s.min(end_s), sess.sr, sess.samples.len());
    let b = time_to_idx(start_s.max(end_s), sess.sr, sess.samples.len());
    if b <= a {
        return Err("Select a region first to cut.".into());
    }
    push_undo(sess);
    sess.clipboard = sess.samples[a..b].to_vec();
    sess.samples.drain(a..b);
    Ok(build_summary(sess))
}

pub fn copy_region(state: &EditorState, start_s: f64, end_s: f64) -> Result<WaveformSummary, String> {
    let mut guard = state.session.lock().map_err(|e| e.to_string())?;
    let sess = guard.as_mut().ok_or("No audio loaded in editor.")?;
    let a = time_to_idx(start_s.min(end_s), sess.sr, sess.samples.len());
    let b = time_to_idx(start_s.max(end_s), sess.sr, sess.samples.len());
    if b <= a {
        return Err("Select a region first to copy.".into());
    }
    sess.clipboard = sess.samples[a..b].to_vec();
    Ok(build_summary(sess))
}

pub fn delete_region(state: &EditorState, start_s: f64, end_s: f64) -> Result<WaveformSummary, String> {
    stop_playback(state);
    let mut guard = state.session.lock().map_err(|e| e.to_string())?;
    let sess = guard.as_mut().ok_or("No audio loaded in editor.")?;
    let a = time_to_idx(start_s.min(end_s), sess.sr, sess.samples.len());
    let b = time_to_idx(start_s.max(end_s), sess.sr, sess.samples.len());
    if b <= a {
        return Err("Select a region first to delete.".into());
    }
    push_undo(sess);
    sess.samples.drain(a..b);
    Ok(build_summary(sess))
}

pub fn paste_at(state: &EditorState, at_s: f64) -> Result<WaveformSummary, String> {
    stop_playback(state);
    let mut guard = state.session.lock().map_err(|e| e.to_string())?;
    let sess = guard.as_mut().ok_or("No audio loaded in editor.")?;
    if sess.clipboard.is_empty() {
        return Err("Clipboard is empty. Copy or cut a selection first.".into());
    }
    push_undo(sess);
    let idx = time_to_idx(at_s, sess.sr, sess.samples.len());
    let clip = sess.clipboard.clone();
    sess.samples.splice(idx..idx, clip);
    Ok(build_summary(sess))
}

pub fn undo(state: &EditorState) -> Result<WaveformSummary, String> {
    stop_playback(state);
    let mut guard = state.session.lock().map_err(|e| e.to_string())?;
    let sess = guard.as_mut().ok_or("No audio loaded in editor.")?;
    let prev = sess.undo_stack.pop().ok_or("Nothing to undo.")?;
    sess.samples = prev;
    Ok(build_summary(sess))
}

pub fn export_temp_wav(state: &EditorState) -> Result<String, String> {
    let guard = state.session.lock().map_err(|e| e.to_string())?;
    let sess = guard.as_ref().ok_or("No audio loaded in editor.")?;
    let out_path = std::env::temp_dir().join("MessageMaster_edited.wav");
    let file = File::create(&out_path).map_err(|e| e.to_string())?;
    let mut w = BufWriter::new(file);
    let data_bytes = (sess.samples.len() * 2) as u32;
    let riff_len = 36u32.saturating_add(data_bytes);
    let byte_rate = sess.sr * 2;
    w.write_all(b"RIFF").map_err(|e| e.to_string())?;
    w.write_all(&riff_len.to_le_bytes()).map_err(|e| e.to_string())?;
    w.write_all(b"WAVEfmt ").map_err(|e| e.to_string())?;
    w.write_all(&16u32.to_le_bytes()).map_err(|e| e.to_string())?;
    w.write_all(&1u16.to_le_bytes()).map_err(|e| e.to_string())?;
    w.write_all(&1u16.to_le_bytes()).map_err(|e| e.to_string())?;
    w.write_all(&sess.sr.to_le_bytes()).map_err(|e| e.to_string())?;
    w.write_all(&byte_rate.to_le_bytes()).map_err(|e| e.to_string())?;
    w.write_all(&2u16.to_le_bytes()).map_err(|e| e.to_string())?;
    w.write_all(&16u16.to_le_bytes()).map_err(|e| e.to_string())?;
    w.write_all(b"data").map_err(|e| e.to_string())?;
    w.write_all(&data_bytes.to_le_bytes()).map_err(|e| e.to_string())?;
    let mut pcm = Vec::with_capacity(sess.samples.len() * 2);
    for &s in &sess.samples {
        let s16 = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        pcm.extend_from_slice(&s16.to_le_bytes());
    }
    w.write_all(&pcm).map_err(|e| e.to_string())?;
    w.flush().map_err(|e| e.to_string())?;
    Ok(out_path.to_string_lossy().to_string())
}

pub fn stop_playback(state: &EditorState) {
    state.stop_play.store(true, Ordering::Relaxed);
}

pub fn play_from(state: &EditorState, app: AppHandle, start_s: f64) -> Result<(), String> {
    stop_playback(state);
    std::thread::sleep(std::time::Duration::from_millis(30));

    let (samples, sr) = {
        let guard = state.session.lock().map_err(|e| e.to_string())?;
        let sess = guard.as_ref().ok_or("No audio loaded in editor.")?;
        (Arc::new(sess.samples.clone()), sess.sr)
    };

    let start_idx = time_to_idx(start_s, sr, samples.len());
    if start_idx >= samples.len() {
        return Ok(());
    }

    state.stop_play.store(false, Ordering::Relaxed);
    let stop_flag = state.stop_play.clone();

    std::thread::spawn(move || {
        let host = cpal::default_host();
        let Some(device) = host.default_output_device() else { return };
        let Ok(config) = device.default_output_config() else { return };
        let out_sr = config.sample_rate();
        let out_ch = config.channels() as usize;
        let fmt = config.sample_format();
        let stream_cfg: cpal::StreamConfig = config.into();

        let pos = Arc::new(AtomicUsize::new(start_idx));
        let pos_c = pos.clone();
        let stop_c = stop_flag.clone();
        let samples_c = samples.clone();
        let ratio = sr as f64 / out_sr as f64;
        let frac = Arc::new(Mutex::new(start_idx as f64));

        let next_sample = move || -> f32 {
            let mut f = frac.lock().unwrap_or_else(|e| e.into_inner());
            let idx = *f as usize;
            if idx >= samples_c.len() {
                stop_c.store(true, Ordering::Relaxed);
                return 0.0;
            }
            pos_c.store(idx, Ordering::Relaxed);
            *f += ratio;
            samples_c[idx]
        };

        let err_fn = |err| eprintln!("Playback error: {err}");
        let stream = match fmt {
            cpal::SampleFormat::F32 => {
                let ns = next_sample;
                device.build_output_stream(
                    stream_cfg,
                    move |out: &mut [f32], _| {
                        for frame in out.chunks_mut(out_ch) {
                            let v = ns();
                            for s in frame.iter_mut() { *s = v; }
                        }
                    },
                    err_fn,
                    None,
                )
            }
            cpal::SampleFormat::I16 => {
                let ns = next_sample;
                device.build_output_stream(
                    stream_cfg,
                    move |out: &mut [i16], _| {
                        for frame in out.chunks_mut(out_ch) {
                            let v = (ns().clamp(-1.0, 1.0) * 32767.0) as i16;
                            for s in frame.iter_mut() { *s = v; }
                        }
                    },
                    err_fn,
                    None,
                )
            }
            _ => return,
        };

        if let Ok(st) = stream {
            let _ = st.play();
            while !stop_flag.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(50));
                let cur = pos.load(Ordering::Relaxed) as f64 / sr as f64;
                let _ = app.emit("playback-pos", PlaybackEvent { current_s: cur, playing: true });
            }
        }
        let cur = pos.load(Ordering::Relaxed) as f64 / sr as f64;
        let _ = app.emit("playback-pos", PlaybackEvent { current_s: cur, playing: false });
    });

    Ok(())
}
