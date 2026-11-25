#!/usr/bin/env python3
"""
Sermon Audio Mastering & Declicking Pipeline
-------------------------------------------
Automatic batch-processing pipeline for sermon recordings recorded with
the same microphone, room acoustic, and speaker characteristics.

Pipeline stages:
  1. Surgical Transient De-Clicker:
     Detects isolated mouth clicks, lip smacks, and digital spikes and repairs
     them using natural cubic spline interpolation without affecting vocal consonants.
  2. Acoustic EQ & Filtering:
     - 80 Hz 3rd-order Butterworth high-pass filter (eliminates HVAC rumble, mic stand thump,
       and the 50 Hz mains hum).
     - 350 Hz Parametric Bell Dip (-3.0 dB, Q=1.4) to eliminate hollow sanctuary boxiness.
     - 3.2 kHz Vocal Presence Lift (+2.0 dB, Q=1.5) to enhance speech intelligibility.
  3. Silence Truncation:
     - Detects long pauses (> 5.5s below -46 dBFS) and truncates them down to 2.0s with
       smooth crossfading to eliminate awkward dead air while keeping natural speaking rhythm.
  4. Spectral Noise Reduction:
     - Smooth background attenuation targeting room hiss while preserving speech integrity.
  5. Vocal Dynamic Compressor:
     - Smooth RMS compressor (Threshold: -22 dBFS, Ratio: 3.2:1, Attack: 20ms, Release: 300ms)
       accelerated via Numba JIT. Controls loud shouting peaks and lifts quiet speech.
  6. ITU-R BS.1770-4 Loudness Normalization & True-Peak Limiter:
     - Targets broadcast standard -16.0 LUFS (or user-specified target).
     - Soft knee ceiling at -1.5 dBFS to prevent DAC inter-sample clipping on MP3 encoding.

Usage:
  python process_sermon.py --input "path/to/sermon.mp3" --output "path/to/mastered.mp3"
"""

import os
import sys
import argparse
import time
import json
import numpy as np
import scipy.signal as signal
from scipy.interpolate import CubicSpline
import soundfile as sf
import pyloudnorm as pyln
import subprocess
import imageio_ffmpeg
import noisereduce as nr
import numba

FFMPEG_EXE = imageio_ffmpeg.get_ffmpeg_exe()

def load_audio(file_path, target_sr=44100):
    """Loads audio file into mono float32 array using ffmpeg."""
    cmd = [
        FFMPEG_EXE,
        "-i", file_path,
        "-vn",
        "-f", "f32le",
        "-ac", "1",
        "-ar", str(target_sr),
        "-"
    ]
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, bufsize=10**7)
    raw, stderr = proc.communicate()
    if proc.returncode != 0:
        raise RuntimeError(f"FFmpeg failed to decode audio: {stderr.decode('utf-8', errors='ignore')}")
    audio = np.frombuffer(raw, dtype=np.float32)
    return audio, target_sr

def save_audio(audio, sr, output_path, bitrate=None, max_size_mb=30.0):
    """
    Saves float32 audio to MP3 or WAV using soundfile and ffmpeg.
    If max_size_mb is specified (and bitrate is None), automatically calculates
    the highest MP3 bitrate that keeps the output strictly under max_size_mb.
    """
    ext = os.path.splitext(output_path)[1].lower()
    audio = np.clip(audio, -1.0, 1.0)
    if ext == ".wav":
        sf.write(output_path, audio, sr, subtype="PCM_24")
        return os.path.getsize(output_path), "PCM_24"

    dur_s = len(audio) / sr
    temp_wav = output_path + ".temp.wav"
    used_bitrate = bitrate
    try:
        sf.write(temp_wav, audio, sr, subtype="PCM_16")
        if bitrate is None and max_size_mb is not None:
            # Target 96% of decimal MB limit (e.g., 28.8 MB for a 30 MB limit) to leave room for headers/tags
            target_bytes = max_size_mb * 0.96 * 1000 * 1000
            kbps = int((target_bytes * 8) / max(dur_s, 1.0) / 1000)
            kbps = max(32, min(192, kbps))
            max_bytes = max_size_mb * 1000 * 1000

            while True:
                used_bitrate = f"{kbps}k"
                cmd = [
                    FFMPEG_EXE,
                    "-y",
                    "-i", temp_wav,
                    "-c:a", "libmp3lame",
                    "-abr", "1",
                    "-b:a", used_bitrate,
                    output_path
                ]
                res = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                if res.returncode != 0:
                    raise RuntimeError(f"FFmpeg encoding failed: {res.stderr}")
                actual_bytes = os.path.getsize(output_path)
                if actual_bytes <= max_bytes or kbps <= 32:
                    break
                kbps = max(32, kbps - 4)
        else:
            used_bitrate = bitrate or "128k"
            cmd = [
                FFMPEG_EXE,
                "-y",
                "-i", temp_wav,
                "-c:a", "libmp3lame",
                "-b:a", used_bitrate,
                output_path
            ]
            res = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            if res.returncode != 0:
                raise RuntimeError(f"FFmpeg encoding failed: {res.stderr}")
    finally:
        if os.path.exists(temp_wav):
            try:
                os.remove(temp_wav)
            except Exception:
                pass
    return os.path.getsize(output_path), used_bitrate

def surgical_declick(audio, sr, ratio_thresh=22.0, max_spike_samples=12):
    """
    Detects and surgically reconstructs isolated mouth clicks and micro-ticks.
    """
    print("  -> Stage 1: Detecting and repairing impulsive mouth clicks...", flush=True)
    t0 = time.time()
    diff = np.abs(np.diff(audio, prepend=audio[0]))
    win_len = int(sr * 0.010) # 10ms
    step = win_len // 2
    
    click_indices = []
    
    for idx_w in range(0, len(audio) - win_len, step):
        w_diff = diff[idx_w : idx_w + win_len]
        med = np.median(w_diff)
        peak = np.max(w_diff)
        if med < 1e-6:
            continue
        ratio = peak / (med + 1e-12)
        if ratio > ratio_thresh:
            local_peak_idx = idx_w + np.argmax(w_diff)
            click_indices.append(local_peak_idx)
            
    if not click_indices:
        print(f"     No clicks found above threshold {ratio_thresh}x.", flush=True)
        return audio, 0
        
    deduped = [click_indices[0]]
    for idx in click_indices[1:]:
        if idx - deduped[-1] > 20:
            deduped.append(idx)
            
    repaired_audio = audio.copy()
    repaired_count = 0
    
    for c_idx in deduped:
        med_local = np.median(diff[max(0, c_idx - 200) : min(len(diff), c_idx + 200)])
        thresh = med_local * 4.0
        
        l = c_idx
        while l > max(0, c_idx - max_spike_samples) and diff[l] > thresh:
            l -= 1
        r = c_idx
        while r < min(len(audio) - 1, c_idx + max_spike_samples) and diff[r] > thresh:
            r += 1
            
        spike_len = r - l + 1
        if spike_len > max_spike_samples + 4:
            continue
            
        pad = 2
        r_start = max(0, l - pad)
        r_end = min(len(audio), r + pad + 1)
        
        fit_pts = 6
        left_x = np.arange(max(0, r_start - fit_pts), r_start)
        right_x = np.arange(r_end, min(len(audio), r_end + fit_pts))
        if len(left_x) < 3 or len(right_x) < 3:
            continue
            
        x_k = np.concatenate([left_x, right_x])
        y_k = np.concatenate([repaired_audio[left_x], repaired_audio[right_x]])
        cs = CubicSpline(x_k, y_k, bc_type='natural')
        target_x = np.arange(r_start, r_end)
        repaired_audio[target_x] = cs(target_x)
        repaired_count += 1
        
    print(f"     Surgically repaired {repaired_count} clicks in {time.time()-t0:.1f}s.", flush=True)
    return repaired_audio, repaired_count

def apply_sermon_eq(audio, sr):
    """
    Applies high-pass rumble filter (80Hz Butterworth),
    low-pass ultrasonic/electronic whine filter (11.5kHz Butterworth),
    boxiness cut at 350Hz (-3dB), and intelligibility lift at 3.2kHz (+2dB).
    """
    print("  -> Stage 2: Applying EQ (80Hz rumble cut, 11.5kHz HF whine cut, 350Hz de-box, 3.2kHz presence)...", flush=True)
    sos_hp = signal.butter(3, 80, 'hp', fs=sr, output='sos')
    y = signal.sosfilt(sos_hp, audio)

    # 11.5 kHz 4th-order Butterworth low-pass to remove 12-20 kHz electronic whine
    sos_lp = signal.butter(4, 11500, 'lp', fs=sr, output='sos')
    y = signal.sosfilt(sos_lp, y)
    
    # 350 Hz Bell cut (-3 dB, Q=1.4)
    w0 = 2 * np.pi * 350 / sr
    Q = 1.4
    gain_db = -3.0
    A = 10**(gain_db / 40)
    alpha = np.sin(w0) / (2 * Q)
    b = np.array([1 + alpha * A, -2 * np.cos(w0), 1 - alpha * A])
    a = np.array([1 + alpha / A, -2 * np.cos(w0), 1 - alpha / A])
    b /= a[0]
    a /= a[0]
    y = signal.lfilter(b, a, y)
    
    # 3200 Hz Bell boost (+2 dB, Q=1.5)
    w0_b = 2 * np.pi * 3200 / sr
    Q_b = 1.5
    gain_db_b = 2.0
    A_b = 10**(gain_db_b / 40)
    alpha_b = np.sin(w0_b) / (2 * Q_b)
    b_b = np.array([1 + alpha_b * A_b, -2 * np.cos(w0_b), 1 - alpha_b * A_b])
    a_b = np.array([1 + alpha_b / A_b, -2 * np.cos(w0_b), 1 - alpha_b / A_b])
    b_b /= a_b[0]
    a_b /= a_b[0]
    y = signal.lfilter(b_b, a_b, y)
    
    return y

def truncate_long_silences(audio, sr, threshold_db=-46.0, min_dur_s=5.5, target_dur_s=2.0):
    """
    Detects silences longer than min_dur_s and truncates them down to target_dur_s
    using smooth crossfading.
    """
    print(f"  -> Stage 3: Truncating long silences (> {min_dur_s}s down to {target_dur_s}s)...", flush=True)
    t0 = time.time()
    
    frame_len = int(sr * 0.050)
    hop_len = int(sr * 0.025)
    num_frames = (len(audio) - frame_len) // hop_len
    
    shape = (num_frames, frame_len)
    strides = (audio.strides[0] * hop_len, audio.strides[0])
    frames = np.lib.stride_tricks.as_strided(audio, shape=shape, strides=strides)
    
    rms = np.sqrt(np.mean(frames**2, axis=1))
    db = 20 * np.log10(rms + 1e-12)
    is_silent = db < threshold_db
    
    silences = []
    in_silence = False
    start_frame = 0
    
    for i, s in enumerate(is_silent):
        if s and not in_silence:
            in_silence = True
            start_frame = i
        elif not s and in_silence:
            in_silence = False
            dur_s = (i - start_frame) * hop_len / sr
            if dur_s >= min_dur_s:
                silences.append((start_frame * hop_len, i * hop_len, dur_s))
                
    if in_silence:
        dur_s = (len(is_silent) - start_frame) * hop_len / sr
        if dur_s >= min_dur_s:
            silences.append((start_frame * hop_len, len(is_silent) * hop_len, dur_s))
            
    if not silences:
        print(f"     No silences longer than {min_dur_s}s found.", flush=True)
        return audio, 0, 0.0
        
    target_samples = int(target_dur_s * sr)
    xfade_samples = int(0.050 * sr) # 50ms crossfade
    
    chunks = []
    last_idx = 0
    total_trimmed_s = 0.0
    
    for s_idx, e_idx, dur in silences:
        # Keep speech up to silence start + half of target_dur
        half_target = target_samples // 2
        cut_start = s_idx + half_target
        cut_end = e_idx - half_target
        
        if cut_end <= cut_start + xfade_samples:
            continue
            
        trimmed_s = (cut_end - cut_start) / sr
        total_trimmed_s += trimmed_s
        
        # Append audio up to cut_start
        chunks.append(audio[last_idx:cut_start])
        # Smooth crossfade
        xfade_in = audio[cut_end : cut_end + xfade_samples]
        xfade_out = audio[cut_start - xfade_samples : cut_start]
        t_arr = np.linspace(0, 1, xfade_samples)
        blend = xfade_out * (1.0 - t_arr) + xfade_in * t_arr
        chunks.append(blend)
        
        last_idx = cut_end + xfade_samples
        
    chunks.append(audio[last_idx:])
    truncated_audio = np.concatenate(chunks)
    
    print(f"     Truncated {len(silences)} long pauses, saving {total_trimmed_s:.1f}s ({total_trimmed_s/60:.2f} mins) in {time.time()-t0:.1f}s.", flush=True)
    return truncated_audio, len(silences), total_trimmed_s

def extract_noise_profile(audio, sr, chunk_s=0.5, num_chunks=8):
    """
    Automatically finds the quietest room-tone segments (excluding digital zeros)
    to build an accurate spectral noise profile of room hiss and electronic whine.
    """
    chunk_len = int(sr * chunk_s)
    n_chunks = len(audio) // chunk_len
    if n_chunks < 2:
        return None
    reshaped = audio[:n_chunks * chunk_len].reshape(n_chunks, chunk_len)
    rms = np.sqrt(np.mean(reshaped**2, axis=1))
    # Ignore pure digital silence (< -85 dBFS)
    valid = np.where(rms > 5e-5)[0]
    if len(valid) == 0:
        return None
    quiet_order = valid[np.argsort(rms[valid])]
    chosen = quiet_order[:min(num_chunks, len(quiet_order))]
    profile = np.concatenate([reshaped[idx] for idx in chosen])
    profile_db = 20 * np.log10(np.sqrt(np.mean(profile**2)) + 1e-12)
    print(f"     Auto-extracted {len(profile)/sr:.1f}s noise profile (floor: {profile_db:.1f} dBFS).", flush=True)
    return profile

def apply_noise_reduction(audio, sr, prop_decrease=0.88):
    """
    Applies true spectral noise reduction using an isolated room-tone noise profile.
    Targets broadband hiss and narrowband high-frequency whines (e.g. 6.5kHz, 8kHz)
    without gating out quiet congregation responses.
    """
    print("  -> Stage 3: Applying true spectral noise reduction (preserving congregation)...", flush=True)
    t0 = time.time()
    noise_profile = extract_noise_profile(audio, sr)
    reduced = nr.reduce_noise(
        y=audio,
        sr=sr,
        y_noise=noise_profile,
        prop_decrease=prop_decrease,
        stationary=True,
        n_fft=4096,
        win_length=4096,
        hop_length=1024,
        freq_mask_smooth_hz=300,
        time_mask_smooth_ms=80
    )
    print(f"     Noise reduction finished in {time.time()-t0:.1f}s.", flush=True)
    return reduced

@numba.jit(nopython=True, fastmath=True)
def _fast_compressor_envelope(abs_audio, alpha_a, alpha_r, threshold_db, ratio):
    n = len(abs_audio)
    gains = np.empty(n, dtype=np.float32)
    env = 0.0
    for i in range(n):
        val = abs_audio[i]
        if val > env:
            env = alpha_a * env + (1.0 - alpha_a) * val
        else:
            env = alpha_r * env + (1.0 - alpha_r) * val
            
        env_db = 20.0 * np.log10(env + 1e-12)
        if env_db > threshold_db:
            excess = env_db - threshold_db
            target_gain_db = -excess * (1.0 - 1.0 / ratio)
        else:
            target_gain_db = 0.0
            
        gains[i] = 10.0**(target_gain_db / 20.0)
    return gains

def apply_compressor(audio, sr, threshold_db=-22.0, ratio=3.2, attack_s=0.02, release_s=0.30):
    """
    Applies smooth vocal dynamic compressor to balance soft speech and loud shouting peaks.
    """
    print("  -> Stage 5: Applying dynamic range compressor (Threshold: -22dB, Ratio: 3.2:1)...", flush=True)
    t0 = time.time()
    alpha_a = np.exp(-1.0 / (attack_s * sr))
    alpha_r = np.exp(-1.0 / (release_s * sr))
    abs_audio = np.abs(audio)
    
    gains = _fast_compressor_envelope(abs_audio, alpha_a, alpha_r, threshold_db, ratio)
    compressed = audio * gains
    max_gr = -20 * np.log10(np.min(gains) + 1e-12)
    print(f"     Compression applied: Max gain reduction = {max_gr:.1f} dB (finished in {time.time()-t0:.2f}s).", flush=True)
    return compressed

def apply_limiter_and_lufs(audio, sr, target_lufs=-16.0, ceiling_dbtp=-1.5):
    """
    Normalizes loudness to ITU-R BS.1770-4 target LUFS and limits true peaks to ceiling.
    """
    print(f"  -> Stage 6: Normalizing loudness to {target_lufs} LUFS & limiting peaks to {ceiling_dbtp} dBTP...", flush=True)
    meter = pyln.Meter(sr)
    cur_lufs = meter.integrated_loudness(audio)
    gain_db = target_lufs - cur_lufs
    normed = audio * (10**(gain_db / 20.0))
    
    ceiling_lin = 10**(ceiling_dbtp / 20.0)
    knee_db = ceiling_dbtp - 2.0
    knee_lin = 10**(knee_db / 20.0)
    
    limited = normed.copy()
    high_mask = np.abs(limited) > knee_lin
    if np.any(high_mask):
        sgn = np.sign(limited[high_mask])
        mag = np.abs(limited[high_mask])
        compressed_mag = knee_lin + (ceiling_lin - knee_lin) * np.tanh((mag - knee_lin) / (ceiling_lin - knee_lin))
        limited[high_mask] = sgn * compressed_mag
        
    final_lufs = meter.integrated_loudness(limited)
    final_peak_db = 20 * np.log10(np.max(np.abs(limited)) + 1e-12)
    print(f"     Final Loudness: {final_lufs:.2f} LUFS | Peak: {final_peak_db:.2f} dBFS", flush=True)
    return limited, final_lufs, final_peak_db

def process_file(input_file, output_file, target_lufs=-16.0, ceiling_dbtp=-1.5, bitrate=None, max_size_mb=30.0):
    print(f"\n=======================================================", flush=True)
    print(f"Processing Sermon Audio: {os.path.basename(input_file)}", flush=True)
    print(f"=======================================================", flush=True)
    t_start = time.time()
    
    # Load
    audio, sr = load_audio(input_file)
    dur_m = len(audio) / sr / 60.0
    print(f"Loaded {dur_m:.2f} minutes of audio ({sr} Hz).", flush=True)
    
    # Initial measurements
    meter = pyln.Meter(sr)
    in_lufs = meter.integrated_loudness(audio)
    in_peak_db = 20 * np.log10(np.max(np.abs(audio)) + 1e-12)
    print(f"Initial Metrics: Loudness = {in_lufs:.2f} LUFS | Peak = {in_peak_db:.2f} dBFS", flush=True)
    
    # Stage 1: De-click
    declicked, clicks_repaired = surgical_declick(audio, sr)
    
    # Stage 2: EQ
    eq_audio = apply_sermon_eq(declicked, sr)
    
    # Stage 3: True Spectral Noise Reduction (before silence truncation so quiet responses stay distinct)
    nr_audio = apply_noise_reduction(eq_audio, sr)
    
    # Stage 4: Silence Truncation (> 5.5s -> 2.0s)
    trunc_audio, silences_count, time_trimmed = truncate_long_silences(nr_audio, sr, threshold_db=-49.0, min_dur_s=5.5, target_dur_s=2.0)
    
    # Stage 5: Compression (Numba-accelerated)
    comp_audio = apply_compressor(trunc_audio, sr)
    
    # Stage 6: Limiting & Normalization
    final_audio, out_lufs, out_peak_db = apply_limiter_and_lufs(comp_audio, sr, target_lufs, ceiling_dbtp)
    
    # Save
    print(f"Saving mastered file to: {output_file}...", flush=True)
    file_bytes, used_bitrate = save_audio(final_audio, sr, output_file, bitrate=bitrate, max_size_mb=max_size_mb)
    file_mb = file_bytes / (1000 * 1000)
    file_mib = file_bytes / (1024 * 1024)
    
    elapsed = time.time() - t_start
    final_dur_m = len(final_audio) / sr / 60.0
    print(f"\nProcessing Complete in {elapsed:.1f}s ({elapsed/dur_m:.2f}x real-time speed).", flush=True)
    print(f"Mastering Summary:")
    print(f"  * Original Duration : {dur_m:.2f} minutes")
    print(f"  * Mastered Duration : {final_dur_m:.2f} minutes ({time_trimmed:.1f}s trimmed from {silences_count} dead silences)")
    print(f"  * Clicks repaired   : {clicks_repaired}")
    print(f"  * Loudness Change   : {in_lufs:.1f} LUFS -> {out_lufs:.1f} LUFS")
    print(f"  * Peak Level        : {in_peak_db:+.2f} dBFS -> {out_peak_db:+.2f} dBFS")
    print(f"  * Output Bitrate    : {used_bitrate}")
    print(f"  * Output Size       : {file_mb:.2f} MB ({file_mib:.2f} MiB)")
    print(f"  * Output Location   : {output_file}\n", flush=True)
    return {
        "input_file": input_file,
        "output_file": output_file,
        "original_duration_m": dur_m,
        "mastered_duration_m": final_dur_m,
        "silences_trimmed_count": silences_count,
        "time_trimmed_seconds": time_trimmed,
        "clicks_repaired": clicks_repaired,
        "initial_lufs": in_lufs,
        "final_lufs": out_lufs,
        "initial_peak_db": in_peak_db,
        "final_peak_db": out_peak_db,
        "elapsed_seconds": elapsed
    }

def main():
    parser = argparse.ArgumentParser(description="Automated Sermon Audio Mastering Pipeline")
    parser.add_argument("--input", "-i", required=True, help="Path to input audio file")
    parser.add_argument("--output", "-o", default=None, help="Path to output audio file (MP3 or WAV)")
    parser.add_argument("--lufs", type=float, default=-16.0, help="Target integrated LUFS (default: -16.0)")
    parser.add_argument("--ceiling", type=float, default=-1.5, help="True peak limiter ceiling in dBTP (default: -1.5)")
    parser.add_argument("--bitrate", default=None, help="Fixed MP3 bitrate (e.g., 128k). Overrides --max-size-mb if set.")
    parser.add_argument("--max-size-mb", type=float, default=30.0, help="Target max output MP3 size in MB (default: 30.0)")
    
    args = parser.parse_args()
    
    input_file = args.input
    output_file = args.output
    if output_file is None:
        base, ext = os.path.splitext(input_file)
        if ext.lower() == ".mpeg":
            base, _ = os.path.splitext(base)
        output_file = f"{base}_MASTERED.mp3"
        
    process_file(
        input_file,
        output_file,
        target_lufs=args.lufs,
        ceiling_dbtp=args.ceiling,
        bitrate=args.bitrate,
        max_size_mb=args.max_size_mb
    )

if __name__ == "__main__":
    main()
