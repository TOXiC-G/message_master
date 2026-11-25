use rustfft::{num_complex::Complex, FftPlanner};
use serde::Serialize;
use std::f32::consts::PI;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Serialize)]
pub struct MasteringStats {
    pub original_duration_s: f64,
    pub mastered_duration_s: f64,
    pub clicks_repaired: usize,
    pub silences_trimmed: usize,
    pub seconds_trimmed: f64,
    pub initial_lufs: f64,
    pub final_lufs: f64,
    pub final_peak_db: f64,
    pub output_path: String,
    pub output_size_mb: f64,
    pub bitrate_kbps: u32,
}

#[derive(Debug, Clone)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    fn highpass(sr: f32, freq: f32, q: f32) -> Self {
        let w0 = 2.0 * PI * freq / sr;
        let alpha = w0.sin() / (2.0 * q);
        let cos_w0 = w0.cos();
        let a0 = 1.0 + alpha;
        Self {
            b0: ((1.0 + cos_w0) / 2.0) / a0,
            b1: -(1.0 + cos_w0) / a0,
            b2: ((1.0 + cos_w0) / 2.0) / a0,
            a1: (-2.0 * cos_w0) / a0,
            a2: (1.0 - alpha) / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    fn lowpass(sr: f32, freq: f32, q: f32) -> Self {
        let w0 = 2.0 * PI * freq / sr;
        let alpha = w0.sin() / (2.0 * q);
        let cos_w0 = w0.cos();
        let a0 = 1.0 + alpha;
        Self {
            b0: ((1.0 - cos_w0) / 2.0) / a0,
            b1: (1.0 - cos_w0) / a0,
            b2: ((1.0 - cos_w0) / 2.0) / a0,
            a1: (-2.0 * cos_w0) / a0,
            a2: (1.0 - alpha) / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    fn peaking(sr: f32, freq: f32, q: f32, gain_db: f32) -> Self {
        let w0 = 2.0 * PI * freq / sr;
        let a = 10.0f32.powf(gain_db / 40.0);
        let alpha = w0.sin() / (2.0 * q);
        let cos_w0 = w0.cos();
        let a0 = 1.0 + alpha / a;
        Self {
            b0: (1.0 + alpha * a) / a0,
            b1: (-2.0 * cos_w0) / a0,
            b2: (1.0 - alpha * a) / a0,
            a1: (-2.0 * cos_w0) / a0,
            a2: (1.0 - alpha / a) / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    fn high_shelf(sr: f32, freq: f32, q: f32, gain_db: f32) -> Self {
        let w0 = 2.0 * PI * freq / sr;
        let a = 10.0f32.powf(gain_db / 40.0);
        let alpha = w0.sin() / (2.0 * q);
        let cos_w0 = w0.cos();
        let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;
        let a0 = (a + 1.0) - (a - 1.0) * cos_w0 + two_sqrt_a_alpha;
        Self {
            b0: (a * ((a + 1.0) + (a - 1.0) * cos_w0 + two_sqrt_a_alpha)) / a0,
            b1: (-2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w0)) / a0,
            b2: (a * ((a + 1.0) + (a - 1.0) * cos_w0 - two_sqrt_a_alpha)) / a0,
            a1: (2.0 * ((a - 1.0) - (a + 1.0) * cos_w0)) / a0,
            a2: ((a + 1.0) - (a - 1.0) * cos_w0 - two_sqrt_a_alpha) / a0,
            z1: 0.0,
            z2: 0.0,
        }
    }

    #[inline]
    fn process_sample(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }

    fn process_in_place(&mut self, buf: &mut [f32]) {
        for s in buf.iter_mut() {
            *s = self.process_sample(*s);
        }
    }
}

/// Decodes any audio file via FFmpeg sidecar into mono `44100 Hz` `f32` samples.
pub fn decode_via_ffmpeg(ffmpeg_path: &Path, input_path: &Path) -> Result<(Vec<f32>, u32), String> {
    let output = Command::new(ffmpeg_path)
        .args([
            "-i",
            input_path.to_str().ok_or("Invalid input path")?,
            "-vn",
            "-f",
            "f32le",
            "-ac",
            "1",
            "-ar",
            "44100",
            "-",
        ])
        .output()
        .map_err(|e| format!("Failed to launch FFmpeg: {e}"))?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(format!("FFmpeg decode error: {err}"));
    }

    let mut samples = Vec::with_capacity(output.stdout.len() / 4);
    for chunk in output.stdout.chunks_exact(4) {
        let v = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        samples.push(if v.is_finite() { v } else { 0.0 });
    }
    Ok((samples, 44100))
}

/// Resamples audio to 44100 Hz via FFmpeg if an `.aup3` project was recorded at another rate.
pub fn resample_via_ffmpeg(
    ffmpeg_path: &Path,
    samples: &[f32],
    from_sr: u32,
    to_sr: u32,
) -> Result<Vec<f32>, String> {
    if from_sr == to_sr {
        return Ok(samples.to_vec());
    }
    let temp_in = std::env::temp_dir().join("sermonmaster_resample_in.wav");
    write_wav_16(&temp_in, samples, from_sr)?;
    let res = Command::new(ffmpeg_path)
        .args([
            "-y",
            "-i",
            temp_in.to_str().unwrap_or_default(),
            "-f",
            "f32le",
            "-ac",
            "1",
            "-ar",
            &to_sr.to_string(),
            "-",
        ])
        .output()
        .map_err(|e| format!("FFmpeg resample failed: {e}"))?;
    let _ = fs::remove_file(&temp_in);
    if !res.status.success() {
        return Err(String::from_utf8_lossy(&res.stderr).to_string());
    }
    let mut out = Vec::with_capacity(res.stdout.len() / 4);
    for chunk in res.stdout.chunks_exact(4) {
        out.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }
    Ok(out)
}

/// Dual-pass surgical de-clicker:
/// - Pass 1: Micro-clicks (1–20 samples / < 0.5 ms) via local derivative MAD ratio + linear interpolation.
/// - Pass 2: Macro-clicks / isolated mic thumps (0.5–15 ms, e.g. 100–650 samples) surrounded by quiet flanks.
pub fn dual_pass_declick(audio: &mut [f32], sr: u32) -> usize {
    let n = audio.len();
    if n < 1024 {
        return 0;
    }

    let mut repaired = 0usize;

    // --- Pass 1: Micro-clicks (2-20 samples) ---
    let mut diffs = vec![0.0f32; n];
    for i in 1..n {
        diffs[i] = (audio[i] - audio[i - 1]).abs();
    }

    let win = (sr as usize) / 100; // 10ms
    let step = win / 2;
    let mut candidates = Vec::new();

    let mut w_buf = Vec::with_capacity(win);
    let mut pos = 0;
    while pos + win < n {
        w_buf.clear();
        w_buf.extend_from_slice(&diffs[pos..pos + win]);
        let peak = w_buf.iter().copied().fold(0.0f32, f32::max);
        let peak_rel = w_buf
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(idx, _)| idx)
            .unwrap_or(0);
        w_buf.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let med = w_buf[w_buf.len() / 2];
        if med > 1e-6 && (peak / med) > 18.0 {
            candidates.push(pos + peak_rel);
        }
        pos += step;
    }

    candidates.sort_unstable();
    let mut last_c = 0usize;
    for c in candidates {
        if last_c > 0 && c.saturating_sub(last_c) <= 24 {
            continue;
        }
        last_c = c;
        let l_bound = c.saturating_sub(200);
        let r_bound = (c + 200).min(n);
        let mut loc: Vec<f32> = diffs[l_bound..r_bound].to_vec();
        loc.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let thresh = loc[loc.len() / 2] * 3.5;

        let mut l = c;
        while l > c.saturating_sub(14) && diffs[l] > thresh {
            l -= 1;
        }
        let mut r = c;
        while r + 1 < (c + 14).min(n) && diffs[r] > thresh {
            r += 1;
        }
        if r - l + 1 <= 18 {
            let rs = l.saturating_sub(2);
            let re = (r + 3).min(n - 1);
            if rs > 0 && re > rs {
                let y0 = audio[rs - 1];
                let y1 = audio[re];
                let span = (re - rs + 1) as f32;
                for k in rs..re {
                    let t = (k - rs + 1) as f32 / span;
                    audio[k] = y0 * (1.0 - t) + y1 * t;
                }
                repaired += 1;
            }
        }
    }

    // --- Pass 2: Macro-clicks / isolated impulses (0.5 ms to 15 ms, e.g. 452.067s & 471.080s) ---
    let f_len = (sr as usize) / 200; // 5 ms frames
    let n_frames = n / f_len;
    if n_frames > 20 {
        let mut f_peak = vec![0.0f32; n_frames];
        for fi in 0..n_frames {
            let slice = &audio[fi * f_len..(fi + 1) * f_len];
            f_peak[fi] = slice.iter().map(|x| x.abs()).fold(0.0f32, f32::max);
        }

        let mut fi = 4;
        while fi + 5 < n_frames {
            // Check 1 to 3 frame burst (5ms to 15ms)
            let left_flank = f_peak[fi - 4..fi]
                .iter()
                .copied()
                .fold(0.0f32, f32::max);
            if f_peak[fi] > 0.02 && left_flank < f_peak[fi] * 0.25 {
                // Find end of short burst within 3 frames (15ms)
                let mut end_fi = fi + 1;
                while end_fi < (fi + 4).min(n_frames - 4) && f_peak[end_fi] > left_flank * 2.0 {
                    end_fi += 1;
                }
                if end_fi - fi <= 3 {
                    let right_flank = f_peak[end_fi..end_fi + 4]
                        .iter()
                        .copied()
                        .fold(0.0f32, f32::max);
                    let burst_peak = f_peak[fi..end_fi]
                        .iter()
                        .copied()
                        .fold(0.0f32, f32::max);
                    if right_flank < burst_peak * 0.25 {
                        // Isolated impulse click! Crossfade smoothly from left edge to right edge
                        let s0 = fi * f_len;
                        let s1 = (end_fi * f_len).min(n - 1);
                        let y0 = audio[s0.saturating_sub(1)];
                        let y1 = audio[s1];
                        let len = (s1 - s0 + 1) as f32;
                        for k in s0..s1 {
                            let t = (k - s0 + 1) as f32 / len;
                            audio[k] = y0 * (1.0 - t) + y1 * t;
                        }
                        repaired += 1;
                        fi = end_fi + 2;
                        continue;
                    }
                }
            }
            fi += 1;
        }
    }

    repaired
}

/// Applies 80 Hz HPF (4th order), 11.5 kHz LPF (4th order), 350 Hz -3 dB dip, and 3.2 kHz +2 dB lift.
pub fn apply_sermon_eq(audio: &mut [f32], sr: f32) {
    let mut hp1 = Biquad::highpass(sr, 80.0, 0.5412);
    let mut hp2 = Biquad::highpass(sr, 80.0, 1.3065);
    let mut lp1 = Biquad::lowpass(sr, 11500.0, 0.5412);
    let mut lp2 = Biquad::lowpass(sr, 11500.0, 1.3065);
    let mut debox = Biquad::peaking(sr, 350.0, 1.4, -3.0);
    let mut presence = Biquad::peaking(sr, 3200.0, 1.5, 2.0);

    hp1.process_in_place(audio);
    hp2.process_in_place(audio);
    lp1.process_in_place(audio);
    lp2.process_in_place(audio);
    debox.process_in_place(audio);
    presence.process_in_place(audio);
}

/// Pure-Rust STFT spectral noise reduction (`n_fft = 4096`) using an auto-extracted 4s room-tone profile.
/// Preserves quiet congregation responses (no Noise Gate).
pub fn apply_spectral_noise_reduction(audio: &mut [f32], sr: u32, prop_decrease: f32) {
    let n_fft = 4096usize;
    let hop = 1024usize;
    if audio.len() < n_fft * 2 {
        return;
    }

    // 1. Auto-extract quietest 4.0s (8 x 0.5s chunks above digital zero)
    let chunk_len = (sr as usize) / 2;
    let n_chunks = audio.len() / chunk_len;
    let mut chunk_rms: Vec<(f32, usize)> = Vec::new();
    for ci in 0..n_chunks {
        let sl = &audio[ci * chunk_len..(ci + 1) * chunk_len];
        let rms = (sl.iter().map(|x| x * x).sum::<f32>() / chunk_len as f32).sqrt();
        if rms > 5e-5 {
            chunk_rms.push((rms, ci));
        }
    }
    if chunk_rms.is_empty() {
        return;
    }
    chunk_rms.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let n_prof = chunk_rms.len().min(8);

    let mut planner = FftPlanner::<f32>::new();
    let fft_fwd = planner.plan_fft_forward(n_fft);
    let fft_inv = planner.plan_fft_inverse(n_fft);

    let hann: Vec<f32> = (0..n_fft)
        .map(|i| 0.5 * (1.0 - (2.0 * PI * i as f32 / n_fft as f32).cos()))
        .collect();

    // 2. Compute noise magnitude profile (mean + 1.5 * std per bin)
    let mut bin_sum = vec![0.0f32; n_fft / 2 + 1];
    let mut bin_sq = vec![0.0f32; n_fft / 2 + 1];
    let mut prof_frames = 0usize;
    let mut buf = vec![Complex::new(0.0f32, 0.0f32); n_fft];

    for &(_, ci) in &chunk_rms[..n_prof] {
        let sl = &audio[ci * chunk_len..(ci + 1) * chunk_len];
        let mut p = 0;
        while p + n_fft <= sl.len() {
            for i in 0..n_fft {
                buf[i] = Complex::new(sl[p + i] * hann[i], 0.0);
            }
            fft_fwd.process(&mut buf);
            for k in 0..=n_fft / 2 {
                let mag = buf[k].norm();
                bin_sum[k] += mag;
                bin_sq[k] += mag * mag;
            }
            prof_frames += 1;
            p += hop;
        }
    }
    if prof_frames == 0 {
        return;
    }

    let mut noise_thresh = vec![0.0f32; n_fft / 2 + 1];
    for k in 0..=n_fft / 2 {
        let mean = bin_sum[k] / prof_frames as f32;
        let var = (bin_sq[k] / prof_frames as f32 - mean * mean).max(0.0);
        noise_thresh[k] = mean + 1.5 * var.sqrt();
    }

    // 3. Overlap-add spectral gating with frequency smoothing
    let n = audio.len();
    let mut out = vec![0.0f32; n];
    let mut norm = vec![0.0f32; n];
    let mut prev_mask = vec![1.0f32; n_fft / 2 + 1];
    let mut raw_mask = vec![1.0f32; n_fft / 2 + 1];
    let floor_gain = 1.0 - prop_decrease;

    let mut pos = 0;
    while pos + n_fft <= n {
        for i in 0..n_fft {
            buf[i] = Complex::new(audio[pos + i] * hann[i], 0.0);
        }
        fft_fwd.process(&mut buf);

        for k in 0..=n_fft / 2 {
            let mag = buf[k].norm();
            let th = noise_thresh[k];
            let g = if mag <= th {
                floor_gain
            } else if mag < th * 2.0 {
                let t = (mag - th) / th.max(1e-9);
                floor_gain + (1.0 - floor_gain) * t
            } else {
                1.0
            };
            // Temporal smoothing
            raw_mask[k] = 0.55 * prev_mask[k] + 0.45 * g;
            prev_mask[k] = raw_mask[k];
        }

        // Light 3-bin frequency smoothing + apply mask symmetrically
        for k in 0..=n_fft / 2 {
            let g = if k > 0 && k < n_fft / 2 {
                0.25 * raw_mask[k - 1] + 0.5 * raw_mask[k] + 0.25 * raw_mask[k + 1]
            } else {
                raw_mask[k]
            };
            buf[k] *= g;
            if k > 0 && k < n_fft / 2 {
                buf[n_fft - k] *= g;
            }
        }

        fft_inv.process(&mut buf);
        let scale = 1.0 / n_fft as f32;
        for i in 0..n_fft {
            let w = hann[i];
            out[pos + i] += buf[i].re * scale * w;
            norm[pos + i] += w * w;
        }
        pos += hop;
    }

    for i in 0..n {
        if norm[i] > 1e-6 {
            audio[i] = out[i] / norm[i];
        }
    }
}

/// RMS-smoothed silence truncation: any silence > `min_dur_s` (below `threshold_db`)
/// is truncated down to `target_dur_s` with a 50ms crossfade. Ignores isolated < 250ms blips.
pub fn truncate_long_silences(
    audio: Vec<f32>,
    sr: u32,
    threshold_db: f32,
    min_dur_s: f64,
    target_dur_s: f64,
) -> (Vec<f32>, usize, f64) {
    let f_len = (sr as usize) / 10; // 100 ms frames
    let n_frames = audio.len() / f_len;
    if n_frames < 10 {
        return (audio, 0, 0.0);
    }

    let mut is_silent = vec![false; n_frames];
    for fi in 0..n_frames {
        let sl = &audio[fi * f_len..(fi + 1) * f_len];
        let rms = (sl.iter().map(|x| x * x).sum::<f32>() / f_len as f32).sqrt();
        let db = 20.0 * (rms + 1e-12).log10();
        is_silent[fi] = db < threshold_db;
    }

    // Bridge tiny 1-2 frame (< 200ms) non-silent blips surrounded by silence
    for fi in 1..n_frames - 2 {
        if is_silent[fi - 1] && !is_silent[fi] && is_silent[fi + 1] {
            is_silent[fi] = true;
        } else if is_silent[fi - 1] && !is_silent[fi] && !is_silent[fi + 1] && is_silent[fi + 2] {
            is_silent[fi] = true;
            is_silent[fi + 1] = true;
        }
    }

    let mut silences: Vec<(usize, usize)> = Vec::new();
    let mut in_sil = false;
    let mut start_f = 0usize;
    for (fi, &s) in is_silent.iter().enumerate() {
        if s && !in_sil {
            in_sil = true;
            start_f = fi;
        } else if !s && in_sil {
            in_sil = false;
            let dur = (fi - start_f) as f64 * 0.1;
            if dur >= min_dur_s {
                silences.push((start_f * f_len, fi * f_len));
            }
        }
    }
    if in_sil {
        let dur = (n_frames - start_f) as f64 * 0.1;
        if dur >= min_dur_s {
            silences.push((start_f * f_len, n_frames * f_len));
        }
    }

    if silences.is_empty() {
        return (audio, 0, 0.0);
    }

    let target_samples = (target_dur_s * sr as f64) as usize;
    let half_target = target_samples / 2;
    let xfade = (sr as usize) / 20; // 50ms

    let mut out = Vec::with_capacity(audio.len());
    let mut last_idx = 0usize;
    let mut trimmed_samples = 0usize;
    let mut count = 0usize;

    for (s_idx, e_idx) in silences {
        let cut_start = s_idx + half_target;
        let cut_end = e_idx.saturating_sub(half_target);
        if cut_end <= cut_start + xfade || cut_start < xfade || cut_end + xfade >= audio.len() {
            continue;
        }
        out.extend_from_slice(&audio[last_idx..cut_start]);
        for i in 0..xfade {
            let t = i as f32 / xfade as f32;
            let a = audio[cut_start - xfade + i];
            let b = audio[cut_end + i];
            out.push(a * (1.0 - t) + b * t);
        }
        trimmed_samples += cut_end - cut_start;
        count += 1;
        last_idx = cut_end + xfade;
    }
    out.extend_from_slice(&audio[last_idx..]);

    (out, count, trimmed_samples as f64 / sr as f64)
}

/// Smooth RMS feed-forward compressor (Threshold: -22 dB, Ratio: 3.2:1, Attack: 20ms, Release: 300ms).
pub fn apply_compressor(audio: &mut [f32], sr: f32) {
    let threshold_db = -22.0f32;
    let ratio = 3.2f32;
    let alpha_a = (-1.0 / (0.020 * sr)).exp();
    let alpha_r = (-1.0 / (0.300 * sr)).exp();
    let mut env = 0.0f32;

    for s in audio.iter_mut() {
        let val = s.abs();
        if val > env {
            env = alpha_a * env + (1.0 - alpha_a) * val;
        } else {
            env = alpha_r * env + (1.0 - alpha_r) * val;
        }
        let env_db = 20.0 * (env + 1e-12).log10();
        if env_db > threshold_db {
            let gain_db = -(env_db - threshold_db) * (1.0 - 1.0 / ratio);
            *s *= 10.0f32.powf(gain_db / 20.0);
        }
    }
}

/// ITU-R BS.1770-4 K-weighted integrated loudness estimate (LUFS).
pub fn measure_lufs(audio: &[f32], sr: f32) -> f64 {
    let mut shelf = Biquad::high_shelf(sr, 1681.97, 0.7071, 4.0);
    let mut hp = Biquad::highpass(sr, 38.13, 0.5003);

    let block_len = (0.4 * sr) as usize; // 400ms
    let hop = (0.1 * sr) as usize; // 100ms
    if audio.len() < block_len {
        return -23.0;
    }

    let mut filtered = audio.to_vec();
    shelf.process_in_place(&mut filtered);
    hp.process_in_place(&mut filtered);

    let mut block_energies = Vec::new();
    let mut pos = 0;
    while pos + block_len <= filtered.len() {
        let mean_sq = filtered[pos..pos + block_len]
            .iter()
            .map(|x| (*x as f64) * (*x as f64))
            .sum::<f64>()
            / block_len as f64;
        let lufs = -0.691 + 10.0 * (mean_sq + 1e-12).log10();
        if lufs > -70.0 {
            block_energies.push((mean_sq, lufs));
        }
        pos += hop;
    }

    if block_energies.is_empty() {
        return -70.0;
    }
    let avg_sq = block_energies.iter().map(|x| x.0).sum::<f64>() / block_energies.len() as f64;
    let rel_thresh = -0.691 + 10.0 * (avg_sq + 1e-12).log10() - 10.0;
    let gated: Vec<f64> = block_energies
        .iter()
        .filter(|x| x.1 > rel_thresh)
        .map(|x| x.0)
        .collect();
    if gated.is_empty() {
        return -0.691 + 10.0 * (avg_sq + 1e-12).log10();
    }
    let final_sq = gated.iter().sum::<f64>() / gated.len() as f64;
    -0.691 + 10.0 * (final_sq + 1e-12).log10()
}

/// Normalizes to `target_lufs` (-16.0) and applies `tanh` soft-knee peak limiting at `ceiling_db` (-1.5).
pub fn normalize_and_limit(audio: &mut [f32], sr: f32, target_lufs: f64, ceiling_db: f32) -> (f64, f64) {
    let cur_lufs = measure_lufs(audio, sr);
    let gain = 10.0f32.powf(((target_lufs - cur_lufs) as f32) / 20.0);
    let ceiling_lin = 10.0f32.powf(ceiling_db / 20.0);
    let knee_lin = 10.0f32.powf((ceiling_db - 2.0) / 20.0);

    let mut max_peak = 0.0f32;
    for s in audio.iter_mut() {
        let mut v = *s * gain;
        let mag = v.abs();
        if mag > knee_lin {
            let comp = knee_lin + (ceiling_lin - knee_lin) * ((mag - knee_lin) / (ceiling_lin - knee_lin)).tanh();
            v = v.signum() * comp;
        }
        v = v.clamp(-1.0, 1.0);
        if v.abs() > max_peak {
            max_peak = v.abs();
        }
        *s = v;
    }

    let final_lufs = measure_lufs(audio, sr);
    let final_peak_db = 20.0 * (max_peak + 1e-12).log10() as f64;
    (final_lufs, final_peak_db)
}

fn write_wav_16(path: &Path, audio: &[f32], sr: u32) -> Result<(), String> {
    let mut f = fs::File::create(path).map_err(|e| format!("Failed to create temp WAV: {e}"))?;
    let data_len = (audio.len() * 2) as u32;
    let riff_len = 36 + data_len;
    let byte_rate = sr * 2;
    f.write_all(b"RIFF").map_err(|e| e.to_string())?;
    f.write_all(&riff_len.to_le_bytes()).map_err(|e| e.to_string())?;
    f.write_all(b"WAVEfmt ").map_err(|e| e.to_string())?;
    f.write_all(&16u32.to_le_bytes()).map_err(|e| e.to_string())?;
    f.write_all(&1u16.to_le_bytes()).map_err(|e| e.to_string())?; // PCM
    f.write_all(&1u16.to_le_bytes()).map_err(|e| e.to_string())?; // Mono
    f.write_all(&sr.to_le_bytes()).map_err(|e| e.to_string())?;
    f.write_all(&byte_rate.to_le_bytes()).map_err(|e| e.to_string())?;
    f.write_all(&2u16.to_le_bytes()).map_err(|e| e.to_string())?; // Block align
    f.write_all(&16u16.to_le_bytes()).map_err(|e| e.to_string())?; // Bits/sample
    f.write_all(b"data").map_err(|e| e.to_string())?;
    f.write_all(&data_len.to_le_bytes()).map_err(|e| e.to_string())?;

    let mut pcm_bytes = Vec::with_capacity(audio.len() * 2);
    for &s in audio {
        let clamped = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        pcm_bytes.extend_from_slice(&clamped.to_le_bytes());
    }
    f.write_all(&pcm_bytes).map_err(|e| e.to_string())?;
    Ok(())
}

/// Encodes audio to MP3 strictly under `max_size_mb` using maximum possible ABR bitrate.
pub fn encode_under_max_mb(
    ffmpeg_path: &Path,
    audio: &[f32],
    sr: u32,
    output_path: &Path,
    max_size_mb: f64,
) -> Result<(u64, u32), String> {
    let dur_s = (audio.len() as f64 / sr as f64).max(1.0);
    let temp_wav: PathBuf = std::env::temp_dir().join(format!(
        "sermonmaster_enc_{}.wav",
        std::process::id()
    ));
    write_wav_16(&temp_wav, audio, sr)?;

    let target_bytes = max_size_mb * 0.96 * 1000.0 * 1000.0;
    let max_bytes = (max_size_mb * 1000.0 * 1000.0) as u64;
    let mut kbps = (((target_bytes * 8.0) / dur_s) / 1000.0) as u32;
    kbps = kbps.clamp(32, 192);

    loop {
        let br_arg = format!("{kbps}k");
        let res = Command::new(ffmpeg_path)
            .args([
                "-y",
                "-i",
                temp_wav.to_str().unwrap_or_default(),
                "-c:a",
                "libmp3lame",
                "-abr",
                "1",
                "-b:a",
                &br_arg,
                output_path.to_str().ok_or("Invalid output path")?,
            ])
            .output()
            .map_err(|e| format!("Failed to run FFmpeg encoder: {e}"))?;

        if !res.status.success() {
            let _ = fs::remove_file(&temp_wav);
            return Err(format!(
                "FFmpeg MP3 encode failed: {}",
                String::from_utf8_lossy(&res.stderr)
            ));
        }

        let size = fs::metadata(output_path).map(|m| m.len()).unwrap_or(0);
        if size <= max_bytes || kbps <= 32 {
            let _ = fs::remove_file(&temp_wav);
            return Ok((size, kbps));
        }
        kbps = kbps.saturating_sub(4).max(32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_real_sermon_dsp() {
        let ffmpeg = Path::new(r"z:\Dev\message_analysis\sermon_master\src-tauri\binaries\ffmpeg-x86_64-pc-windows-msvc.exe");
        let input = Path::new(r"z:\Dev\message_analysis\Test Audio  - 24.11.2026.mp3");
        if !input.exists() || !ffmpeg.exists() {
            return;
        }
        let t0 = std::time::Instant::now();
        let (mut samples, sr) = decode_via_ffmpeg(ffmpeg, input).expect("decode");
        let orig_s = samples.len() as f64 / sr as f64;
        let clicks = dual_pass_declick(&mut samples, sr);
        apply_sermon_eq(&mut samples, sr as f32);
        apply_spectral_noise_reduction(&mut samples, sr, 0.85);
        let (mut samples, sil_cnt, sil_sec) = truncate_long_silences(samples, sr, -49.0, 5.5, 2.0);
        apply_compressor(&mut samples, sr as f32);
        let (lufs, peak) = normalize_and_limit(&mut samples, sr as f32, -16.0, -1.5);
        let out = Path::new(r"z:\Dev\message_analysis\Test Audio - 24.11.2026_RUST_MASTERED.mp3");
        let (bytes, kbps) = encode_under_max_mb(ffmpeg, &samples, sr, out, 30.0).expect("encode");
        println!(
            "RUST DSP VERIFIED in {:.1}s | Orig: {:.1}s -> Final: {:.1}s | Clicks: {} | Silences: {} ({:.1}s trimmed) | LUFS: {:.2} | Peak: {:.2} dB | Size: {:.2} MB @ {}k",
            t0.elapsed().as_secs_f64(),
            orig_s,
            samples.len() as f64 / sr as f64,
            clicks,
            sil_cnt,
            sil_sec,
            lufs,
            peak,
            bytes as f64 / 1_000_000.0,
            kbps
        );
        assert!(sil_cnt > 0, "Expected long silences including 7:34 to be truncated!");
        assert!(bytes <= 30_000_000, "Expected output under 30 MB!");
    }
}
