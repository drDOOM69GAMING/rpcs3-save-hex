//! The cheat interpreter.
//!
//! This is a Rust port of the reference interpreter the bundled codes were
//! written for: the same address modes, checksums and cipher handling, so a code
//! behaves here the way it does in the tool it came from. The attribution and
//! licence that travel with the ported code are in `THIRD-PARTY-NOTICES.md`.

use std::fmt;

pub mod bsd;
pub mod crypto;
pub mod hash;
pub mod sw;
pub mod wildcard;

pub use bsd::apply_files;

/// Byte order used when the interpreter converts a value to/from the save.
///
/// PS3 saves are big-endian, which is what this tool always uses.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Endian {
    Big,
    Little,
}

/// One file's bytes, named so a script's `:NAME` lines can switch to it.
///
/// Multi-file cheats (a checksum stored in one file but computed from another)
/// are expressed as one script that switches between these buffers; the
/// interpreter keeps its variables across a switch.
#[derive(Debug, Clone)]
pub struct NamedBuf {
    pub name: String,
    pub data: Vec<u8>,
}

/// A cheat could not be applied. The message is written for the user, not the
/// developer, so the GUI can show it verbatim.
#[derive(Debug, Clone)]
pub struct ApplyError(pub String);

impl ApplyError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

impl fmt::Display for ApplyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ApplyError {}

/// The outcome of applying a cheat to a save.
#[derive(Debug)]
pub struct ApplyResult {
    /// The modified save data (length can change for a script `insert`/`delete`).
    pub data: Vec<u8>,
    /// Human-readable trace of every action taken.
    pub log: Vec<String>,
    /// Lines the interpreter recognised as neither a command nor a code.
    /// A non-empty list means the cheat may not be fully applied.
    pub warnings: Vec<String>,
}

/// Read `len` bytes at `src` as an unsigned integer in the given byte order.
pub(crate) fn read_uint(src: &[u8], len: usize, endian: Endian) -> u64 {
    let mut v = 0u64;
    for i in 0..len {
        let shift = match endian {
            Endian::Big => 8 * (len - 1 - i),
            Endian::Little => 8 * i,
        };
        v |= (src[i] as u64) << shift;
    }
    v
}

/// Write the low `len` bytes of `value` to `dst` in the given byte order.
pub(crate) fn write_uint(dst: &mut [u8], value: u64, len: usize, endian: Endian) {
    for i in 0..len {
        let shift = match endian {
            Endian::Big => 8 * (len - 1 - i),
            Endian::Little => 8 * i,
        };
        dst[i] = ((value >> shift) & 0xff) as u8;
    }
}

/// Port of `_range_in_bounds`: `off` may be negative, and a zero-length read at
/// `off == dsize` is in bounds.
pub(crate) fn range_in_bounds(dsize: usize, off: i64, len: usize) -> bool {
    if off < 0 || off as usize > dsize {
        return false;
    }
    len <= dsize - off as usize
}

pub(crate) fn read_u32_be(src: &[u8]) -> u32 {
    u32::from_be_bytes([src[0], src[1], src[2], src[3]])
}

pub(crate) fn read_u32_le(src: &[u8]) -> u32 {
    u32::from_le_bytes([src[0], src[1], src[2], src[3]])
}

/// Port of `search_data`: find the `count`-th (1-based) occurrence of `pat`
/// within `data[..size]`, scanning forward from `start`.
pub(crate) fn search_data(
    data: &[u8],
    size: usize,
    start: usize,
    pat: &[u8],
    count: usize,
) -> Option<usize> {
    let size = size.min(data.len());
    if pat.is_empty() || size < pat.len() {
        return None;
    }
    let last = size - pat.len();
    let mut k = 1usize;
    let mut i = start;
    while i <= last {
        if &data[i..i + pat.len()] == pat {
            if k == count {
                return Some(i);
            }
            k += 1;
        }
        i += 1;
    }
    None
}

/// Port of `reverse_search_data`: find the `count`-th occurrence scanning
/// backward from `start`.
pub(crate) fn reverse_search_data(
    data: &[u8],
    size: usize,
    start: i64,
    pat: &[u8],
    count: usize,
) -> Option<usize> {
    let size = size.min(data.len());
    if pat.is_empty() || size < pat.len() {
        return None;
    }
    let last = size - pat.len();
    let mut k = 1usize;
    let mut i = start;
    while i >= 0 {
        let iu = i as usize;
        if iu <= last && &data[iu..iu + pat.len()] == pat {
            if k == count {
                return Some(iu);
            }
            k += 1;
        }
        i -= 1;
    }
    None
}

/// A well-formed hex code line is 17 bytes with a space at index 8 and hex
/// everywhere else. Leading spaces are allowed.
pub(crate) fn is_sw_line(s: &str) -> bool {
    let s = s.trim_start_matches(' ');
    let b = s.as_bytes();
    if b.len() != 17 || b[8] != b' ' {
        return false;
    }
    b.iter()
        .enumerate()
        .all(|(i, &c)| i == 8 || c.is_ascii_hexdigit())
}

/// Apply a cheat body to a save.
///
/// A body whose every non-empty line is a hex code line is run by the hex code
/// interpreter; anything else is treated as a script (which may itself contain
/// runs of hex code lines).
pub fn apply_code(
    input: &[u8],
    lines: &[String],
    endian: Endian,
) -> Result<ApplyResult, ApplyError> {
    let cleaned: Vec<String> = lines
        .iter()
        .map(|l| l.trim_end_matches(['\r', '\n']).to_string())
        .collect();

    let all_sw = {
        let mut any = false;
        let mut all = true;
        for l in &cleaned {
            let t = l.trim();
            if t.is_empty() {
                continue;
            }
            any = true;
            if !is_sw_line(t) {
                all = false;
                break;
            }
        }
        any && all
    };

    if all_sw {
        let sw_lines: Vec<String> = cleaned
            .iter()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.trim().to_string())
            .collect();
        let mut data = input.to_vec();
        let warnings = sw::apply(&mut data, &sw_lines, endian)?;
        Ok(ApplyResult {
            data,
            log: vec![format!("Applied {} hex code line(s)", sw_lines.len())],
            warnings,
        })
    } else {
        bsd::apply(input.to_vec(), &cleaned, endian)
    }
}

