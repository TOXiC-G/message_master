use quick_xml::events::Event;
use quick_xml::Reader;
use rusqlite::{Connection, OpenFlags};
use std::collections::HashMap;
use std::path::Path;

/// Reads an Audacity 3.x `.aup3` SQLite project file and returns mono `f32` samples at `44100 Hz`
/// (or the project's native rate, resampled if needed by the caller).
pub fn read_aup3_mono(path: &Path) -> Result<(Vec<f32>, u32), String> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("Failed to open .aup3 database: {e}"))?;

    // Audacity stores the project XML in `project.doc`
    let doc_bytes: Vec<u8> = conn
        .query_row("SELECT doc FROM project LIMIT 1", [], |row| row.get(0))
        .map_err(|e| format!("Failed to read project XML from .aup3: {e}"))?;

    let doc_str = String::from_utf8_lossy(&doc_bytes);
    let (sample_rate, track_blocks) = parse_project_xml(&doc_str)?;

    if track_blocks.is_empty() {
        return Err("No audio tracks or waveblocks found in .aup3 project.".into());
    }

    // Load referenced sampleblocks
    let mut stmt = conn
        .prepare("SELECT blockid, sampleformat, samples FROM sampleblocks")
        .map_err(|e| format!("Failed to query sampleblocks: {e}"))?;

    let mut block_map: HashMap<i64, (i64, Vec<u8>)> = HashMap::new();
    let rows = stmt
        .query_map([], |row| {
            let id: i64 = row.get(0)?;
            let fmt: i64 = row.get(1)?;
            let data: Vec<u8> = row.get(2)?;
            Ok((id, fmt, data))
        })
        .map_err(|e| format!("Failed to iterate sampleblocks: {e}"))?;

    for r in rows {
        let (id, fmt, data) = r.map_err(|e| format!("Failed to decode sampleblock row: {e}"))?;
        block_map.insert(id, (fmt, data));
    }

    // Assemble each channel/track and mix down to mono
    let mut channels: Vec<Vec<f32>> = Vec::new();
    for blocks in &track_blocks {
        let mut ch_samples = Vec::new();
        for &bid in blocks {
            if let Some((fmt, raw)) = block_map.get(&bid) {
                decode_block(*fmt, raw, &mut ch_samples);
            }
        }
        if !ch_samples.is_empty() {
            channels.push(ch_samples);
        }
    }

    if channels.is_empty() {
        return Err("Audio blocks in .aup3 were empty.".into());
    }

    let max_len = channels.iter().map(|c| c.len()).max().unwrap_or(0);
    let n_ch = channels.len() as f32;
    let mut mono = vec![0.0f32; max_len];
    for ch in &channels {
        for (i, &s) in ch.iter().enumerate() {
            mono[i] += s / n_ch;
        }
    }

    Ok((mono, sample_rate))
}

fn parse_project_xml(xml: &str) -> Result<(u32, Vec<Vec<i64>>), String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut sample_rate: u32 = 44100;
    let mut tracks: Vec<Vec<i64>> = Vec::new();
    let mut current_blocks: Vec<i64> = Vec::new();
    let mut in_sequence = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                match name.as_str() {
                    "project" | "wavetrack" => {
                        for attr in e.attributes().flatten() {
                            let key = String::from_utf8_lossy(attr.key.as_ref());
                            if key == "rate" {
                                if let Ok(v) = String::from_utf8_lossy(&attr.value).parse::<f64>() {
                                    sample_rate = v.round() as u32;
                                }
                            }
                        }
                    }
                    "sequence" => {
                        in_sequence = true;
                        current_blocks.clear();
                    }
                    "waveblock" if in_sequence => {
                        for attr in e.attributes().flatten() {
                            let key = String::from_utf8_lossy(attr.key.as_ref());
                            if key == "blockid" {
                                if let Ok(id) = String::from_utf8_lossy(&attr.value).parse::<i64>() {
                                    if id > 0 {
                                        current_blocks.push(id);
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::End(ref e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if name == "sequence" && in_sequence {
                    in_sequence = false;
                    if !current_blocks.is_empty() {
                        tracks.push(current_blocks.clone());
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("XML parse error in .aup3: {e}")),
            _ => {}
        }
    }

    Ok((sample_rate, tracks))
}

fn decode_block(sample_format: i64, raw: &[u8], out: &mut Vec<f32>) {
    // Audacity sampleformat enums:
    // 131073 (0x20001) = int16Sample
    // 262145 (0x40001) = int24Sample (stored as 32-bit int or 24-bit)
    // 262159 (0x4000F) = floatSample (32-bit float LE)
    match sample_format {
        131073 => {
            for chunk in raw.chunks_exact(2) {
                let s = i16::from_le_bytes([chunk[0], chunk[1]]);
                out.push(s as f32 / 32768.0);
            }
        }
        262145 => {
            // 24-bit stored in 32-bit words in Audacity SQLite
            for chunk in raw.chunks_exact(4) {
                let s = i32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                out.push(s as f32 / 8388608.0);
            }
        }
        _ => {
            // Default float32 LE (262159)
            for chunk in raw.chunks_exact(4) {
                let s = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                out.push(if s.is_finite() { s } else { 0.0 });
            }
        }
    }
}
