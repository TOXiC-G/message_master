use quick_xml::events::Event;
use quick_xml::Reader;
use rusqlite::{Connection, OpenFlags};
use std::collections::HashMap;
use std::path::Path;

/// Reads an Audacity 3.x `.aup3` SQLite project file (both binary XML and plain XML)
/// and returns mono `f32` samples at the project's native sample rate.
pub fn read_aup3_mono(path: &Path) -> Result<(Vec<f32>, u32), String> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("Failed to open .aup3 database: {e}"))?;

    let (dict_bytes, doc_bytes): (Vec<u8>, Vec<u8>) = conn
        .query_row("SELECT dict, doc FROM project LIMIT 1", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .map_err(|e| format!("Failed to read project table from .aup3: {e}"))?;

    // Audacity 3.x stores project.doc in binary XML when dict is non-empty
    let (sample_rate, track_blocks) = if !dict_bytes.is_empty() && !doc_bytes.starts_with(b"<") {
        parse_binary_xml(&dict_bytes, &doc_bytes)?
    } else {
        let doc_str = String::from_utf8_lossy(&doc_bytes);
        parse_plain_xml(&doc_str)?
    };

    if track_blocks.is_empty() {
        return Err("No audio tracks or waveblocks found in .aup3 project.".into());
    }

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

fn decode_utf16le(bytes: &[u8]) -> String {
    let u16s: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16_lossy(&u16s)
}

/// Parses Audacity 3.x binary XML (`project.dict` + `project.doc`).
fn parse_binary_xml(dict: &[u8], doc: &[u8]) -> Result<(u32, Vec<Vec<i64>>), String> {
    // 1. Parse string dictionary
    let mut names: HashMap<u16, String> = HashMap::new();
    let mut i = if dict.len() >= 2 && dict[0] == 0 && dict[1] == 2 {
        2usize
    } else {
        0usize
    };
    while i < dict.len() {
        let op = dict[i];
        i += 1;
        if op == 0x0f && i + 4 <= dict.len() {
            let tid = u16::from_le_bytes([dict[i], dict[i + 1]]);
            let blen = u16::from_le_bytes([dict[i + 2], dict[i + 3]]) as usize;
            i += 4;
            if i + blen <= dict.len() {
                names.insert(tid, decode_utf16le(&dict[i..i + blen]));
                i += blen;
            } else {
                break;
            }
        } else {
            break;
        }
    }

    // 2. Walk binary XML opcodes
    let mut p = 0usize;
    let mut sample_rate: u32 = 44100;
    let mut tracks: Vec<Vec<i64>> = Vec::new();
    let mut cur_blocks: Vec<i64> = Vec::new();
    let mut in_sequence = false;

    while p < doc.len() {
        let op = doc[p];
        p += 1;
        match op {
            0x01 => {
                // StartTag (u16 tag_id)
                if p + 2 > doc.len() { break; }
                let tid = u16::from_le_bytes([doc[p], doc[p + 1]]);
                p += 2;
                if names.get(&tid).map(|s| s.as_str()) == Some("sequence") {
                    in_sequence = true;
                    cur_blocks.clear();
                }
            }
            0x02 => {
                // EndTag (u16 tag_id)
                if p + 2 > doc.len() { break; }
                let tid = u16::from_le_bytes([doc[p], doc[p + 1]]);
                p += 2;
                if names.get(&tid).map(|s| s.as_str()) == Some("sequence") && in_sequence {
                    in_sequence = false;
                    if !cur_blocks.is_empty() {
                        tracks.push(cur_blocks.clone());
                    }
                }
            }
            0x03 => {
                // StringAttr (u16 attr_id + u32 byte_len + UTF-16LE)
                if p + 6 > doc.len() { break; }
                let blen = u32::from_le_bytes([doc[p + 2], doc[p + 3], doc[p + 4], doc[p + 5]]) as usize;
                p += 6 + blen;
            }
            0x0c | 0x0d => {
                // Raw / Data (u32 byte_len + UTF-16LE)
                if p + 4 > doc.len() { break; }
                let blen = u32::from_le_bytes([doc[p], doc[p + 1], doc[p + 2], doc[p + 3]]) as usize;
                p += 4 + blen;
            }
            0x04 | 0x06 | 0x08 => {
                // 32-bit int / long / size_t attr (u16 attr_id + 4 bytes)
                p += 6;
            }
            0x05 | 0x0b => {
                // Bool attr (u16 attr_id + 1 byte)
                p += 3;
            }
            0x07 => {
                // 64-bit longlong attr (u16 attr_id + i64) -> used by `blockid`
                if p + 10 > doc.len() { break; }
                let aid = u16::from_le_bytes([doc[p], doc[p + 1]]);
                let val = i64::from_le_bytes([
                    doc[p + 2], doc[p + 3], doc[p + 4], doc[p + 5],
                    doc[p + 6], doc[p + 7], doc[p + 8], doc[p + 9],
                ]);
                p += 10;
                if in_sequence && names.get(&aid).map(|s| s.as_str()) == Some("blockid") && val > 0 {
                    cur_blocks.push(val);
                }
            }
            0x09 => {
                // Float attr (u16 attr_id + f32 + i32 digits)
                p += 10;
            }
            0x0a => {
                // Double attr (u16 attr_id + f64 + i32 digits)
                if p + 14 > doc.len() { break; }
                let aid = u16::from_le_bytes([doc[p], doc[p + 1]]);
                let val = f64::from_le_bytes([
                    doc[p + 2], doc[p + 3], doc[p + 4], doc[p + 5],
                    doc[p + 6], doc[p + 7], doc[p + 8], doc[p + 9],
                ]);
                p += 14;
                if names.get(&aid).map(|s| s.as_str()) == Some("rate") && val > 1000.0 {
                    sample_rate = val.round() as u32;
                }
            }
            _ => {
                return Err(format!("Unsupported binary XML opcode 0x{op:02x} in .aup3"));
            }
        }
    }

    Ok((sample_rate, tracks))
}

fn parse_plain_xml(xml: &str) -> Result<(u32, Vec<Vec<i64>>), String> {
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
    match sample_format {
        131073 => {
            for chunk in raw.chunks_exact(2) {
                let s = i16::from_le_bytes([chunk[0], chunk[1]]);
                out.push(s as f32 / 32768.0);
            }
        }
        262145 => {
            for chunk in raw.chunks_exact(4) {
                let s = i32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                out.push(s as f32 / 8388608.0);
            }
        }
        _ => {
            for chunk in raw.chunks_exact(4) {
                let s = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                out.push(if s.is_finite() { s } else { 0.0 });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_real_aup3() {
        let p = Path::new(r"z:\Dev\message_analysis\archive\The Rapture Messenger And His Message - Part  3 - English  - 24.11.2026.aup3");
        if !p.exists() { return; }
        let (samples, sr) = read_aup3_mono(p).expect("read .aup3");
        println!("Decoded .aup3: {} samples @ {} Hz ({:.2} min)", samples.len(), sr, samples.len() as f64 / sr as f64 / 60.0);
        assert_eq!(sr, 44100);
        assert!(samples.len() > 100_000_000);
    }
}
