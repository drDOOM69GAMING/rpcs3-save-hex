//! Minimal reader for the `PARAM.SFO` metadata file that sits beside every
//! RPCS3 save. RPCS3 writes the real game title into `TITLE`, so reading it
//! lets us show a proper name for *every* save on disk, even games that have no
//! entry in the cheat database.
//!
//! The format is a small header, an index of key/value records, a table of
//! NUL-terminated key names and a table of values (see the PS3 Developer wiki,
//! "PARAM.SFO"). We only care about the UTF-8 string fields.

use std::path::Path;

const MAGIC: &[u8; 4] = b"\0PSF";
const HEADER_LEN: usize = 20;
const ENTRY_LEN: usize = 16;

/// Read every key/value pair out of a `PARAM.SFO`. Returns an empty vector if
/// the bytes are not a valid SFO. Values are decoded as UTF-8 (lossily).
pub fn read_pairs(bytes: &[u8]) -> Vec<(String, String)> {
    if bytes.len() < HEADER_LEN || &bytes[0..4] != MAGIC {
        return Vec::new();
    }

    let u32_at = |off: usize| -> Option<u32> {
        bytes
            .get(off..off + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };

    let key_table_start = u32_at(8).unwrap_or(0) as usize;
    let data_table_start = u32_at(12).unwrap_or(0) as usize;
    let entry_count = u32_at(16).unwrap_or(0) as usize;

    let mut pairs = Vec::with_capacity(entry_count);
    for i in 0..entry_count {
        let off = HEADER_LEN + i * ENTRY_LEN;
        let Some(entry) = bytes.get(off..off + ENTRY_LEN) else {
            break;
        };
        let key_offset = u16::from_le_bytes([entry[0], entry[1]]) as usize;
        let data_fmt = u16::from_le_bytes([entry[2], entry[3]]);
        let data_len = u32::from_le_bytes([entry[4], entry[5], entry[6], entry[7]]) as usize;
        let data_offset = u32::from_le_bytes([entry[12], entry[13], entry[14], entry[15]]) as usize;

        // Key: NUL-terminated string in the key table.
        let key_start = key_table_start + key_offset;
        let key = match bytes.get(key_start..) {
            Some(rest) => match rest.iter().position(|&b| b == 0) {
                Some(end) => String::from_utf8_lossy(&rest[..end]).into_owned(),
                None => break,
            },
            None => break,
        };

        // Value: only the string formats are interesting here. Format 0x0404 is
        // a little-endian u32.
        let value_start = data_table_start + data_offset;
        let value = match bytes.get(value_start..value_start + data_len) {
            Some(raw) => match data_fmt {
                0x0404 if raw.len() >= 4 => {
                    u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]).to_string()
                }
                _ => {
                    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
                    String::from_utf8_lossy(&raw[..end]).into_owned()
                }
            },
            None => break,
        };

        pairs.push((key, value));
    }
    pairs
}

/// The `TITLE` value of a `PARAM.SFO`, cleaned up for display.
pub fn title_from_bytes(bytes: &[u8]) -> Option<String> {
    let raw = read_pairs(bytes)
        .into_iter()
        .find_map(|(k, v)| (k == "TITLE").then_some(v))?;
    let cleaned = clean_title(&raw);
    (!cleaned.is_empty()).then_some(cleaned)
}

/// The `TITLE_ID` value of a `PARAM.SFO`, if present.
pub fn title_id_from_bytes(bytes: &[u8]) -> Option<String> {
    read_pairs(bytes)
        .into_iter()
        .find_map(|(k, v)| (k == "TITLE_ID").then_some(v))
        .filter(|v| !v.trim().is_empty())
}

/// Read the title from a `PARAM.SFO` on disk.
pub fn title_of(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    title_from_bytes(&bytes)
}

/// Tidy a raw SFO title into something worth showing: drop trademark marks and
/// control characters and collapse runs of whitespace (some titles embed a
/// newline, e.g. `METAL GEAR SOLID 4\nGUNS OF THE PATRIOTS`).
pub fn clean_title(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut pending_space = false;
    for ch in raw.chars() {
        match ch {
            '\u{2122}' | '\u{00AE}' | '\u{00A9}' => {} // ™ ® ©
            c if c.is_whitespace() || c.is_control() => pending_space = true,
            c => {
                if pending_space && !out.is_empty() {
                    out.push(' ');
                }
                pending_space = false;
                out.push(c);
            }
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a valid `PARAM.SFO` holding the given key/value pairs.
    fn build_sfo(pairs: &[(&str, &str)]) -> Vec<u8> {
        let mut key_table = Vec::new();
        let mut key_offsets = Vec::new();
        for (k, _) in pairs {
            key_offsets.push(key_table.len());
            key_table.extend_from_slice(k.as_bytes());
            key_table.push(0);
        }

        let mut data_table = Vec::new();
        let mut data_len = Vec::new();
        let mut data_offsets = Vec::new();
        for (_, v) in pairs {
            data_offsets.push(data_table.len());
            data_table.extend_from_slice(v.as_bytes());
            data_table.push(0);
            data_len.push(v.len() + 1);
        }

        let key_table_start = HEADER_LEN + pairs.len() * ENTRY_LEN;
        let data_table_start = key_table_start + key_table.len();

        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&0x0000_0101u32.to_le_bytes());
        out.extend_from_slice(&(key_table_start as u32).to_le_bytes());
        out.extend_from_slice(&(data_table_start as u32).to_le_bytes());
        out.extend_from_slice(&(pairs.len() as u32).to_le_bytes());

        for i in 0..pairs.len() {
            out.extend_from_slice(&(key_offsets[i] as u16).to_le_bytes());
            out.extend_from_slice(&0x0204u16.to_le_bytes());
            out.extend_from_slice(&(data_len[i] as u32).to_le_bytes());
            out.extend_from_slice(&(data_len[i] as u32).to_le_bytes());
            out.extend_from_slice(&(data_offsets[i] as u32).to_le_bytes());
        }
        out.extend_from_slice(&key_table);
        out.extend_from_slice(&data_table);
        out
    }

    #[test]
    fn reads_title_and_title_id() {
        let sfo = build_sfo(&[("TITLE", "Heavenly Sword"), ("TITLE_ID", "BLES00078")]);
        assert_eq!(title_from_bytes(&sfo).as_deref(), Some("Heavenly Sword"));
        assert_eq!(title_id_from_bytes(&sfo).as_deref(), Some("BLES00078"));
    }

    #[test]
    fn cleans_newlines_and_trademarks() {
        let sfo = build_sfo(&[("TITLE", "METAL GEAR SOLID 4\nGUNS OF THE PATRIOTS")]);
        assert_eq!(
            title_from_bytes(&sfo).unwrap(),
            "METAL GEAR SOLID 4 GUNS OF THE PATRIOTS"
        );
        assert_eq!(clean_title("The Last of Us\u{2122}"), "The Last of Us");
        assert_eq!(clean_title("Killzone\u{00AE}3"), "Killzone3");
        assert_eq!(clean_title("  spaced   out  "), "spaced out");
    }

    #[test]
    fn rejects_non_sfo_bytes() {
        assert!(read_pairs(b"not an sfo at all").is_empty());
        assert!(title_from_bytes(b"\0PSF").is_none());
    }
}
