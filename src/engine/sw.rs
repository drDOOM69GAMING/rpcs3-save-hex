//! Interpreter for hex code lines, ported from the reference patch
//! interpreter.
//!
//! Lines look like `XXXXXXXX YYYYYYYY`: the first nibble is the code type, the
//! second nibble is a size / pointer / branch selector, the next six hex digits
//! are an address, and the last eight are a value or search pattern.

use super::{range_in_bounds, read_uint, read_u32_be, read_u32_le, ApplyError, Endian};

/// Minimum length of a well-formed hex code line.
const SW_CODE_LINE_LEN: usize = 17;

/// Parse the leading hex digits of `b` (mirrors `sscanf("%x")`).
fn hx(b: &[u8]) -> u64 {
    let mut v = 0u64;
    for &c in b {
        let d = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => break,
        };
        v = (v << 4) | d as u64;
    }
    v
}

/// `sw_val_bytes`: byte width encoded in the low two bits of the size nibble.
fn sw_val_bytes(t: u8) -> usize {
    let n = if t <= b'9' { t - b'0' } else { t - b'A' + 10 };
    1usize << (n & 3)
}

/// Opcodes whose address is relative to the current pointer.
const POINTER_TYPES: &[u8] = b"89ABCDEF";
/// Type-4 wrapper opcodes where the repeat count starts at column 0.
const WRAPPER_TYPES: &[u8] = b"456CDE";

/// Apply a run of hex code lines to `data`, in place.
///
/// Returns warnings for the parts that could not be applied. A `search` that
/// finds nothing makes the interpreter skip the lines that depend on it (the
/// reference behaves the same way), so the caller is told rather than being left
/// to believe the code matched when it did not.
pub fn apply(data: &mut Vec<u8>, lines: &[String], endian: Endian) -> Result<Vec<String>, ApplyError> {
    let code: Vec<String> = lines.iter().map(|l| l.trim_start().to_string()).collect();
    let mut pointer: i64 = 0;
    let mut end_pointer: i64 = 0;
    let mut ptr_value: u32 = 0;
    let mut i = 0usize;
    let mut warnings: Vec<String> = Vec::new();

    while i < code.len() {
        let cur = code[i].clone();
        i += 1;
        if cur.len() < SW_CODE_LINE_LEN {
            continue;
        }
        let b = cur.as_bytes();
        let dsize = data.len();

        match b[0] {
            // ---- direct writes (8/16/32-bit) ----
            b'0' | b'1' | b'2' => {
                let bytes = 1usize << (b[0] - b'0');
                let mut off = hx(&b[2..8]) as i64;
                if b[1] == b'8' {
                    off += pointer;
                }
                let val = hx(&b[9..17]);
                if range_in_bounds(dsize, off, bytes) {
                    super::write_uint(&mut data[off as usize..off as usize + bytes], val, bytes, endian);
                }
            }

            // ---- add / subtract write ----
            b'3' => {
                let t = b[1];
                let mut off = hx(&b[2..8]) as i64;
                if POINTER_TYPES.contains(&t) {
                    off += pointer;
                }
                let val = hx(&b[9..17]) as u32;
                let size = sw_val_bytes(t);
                if range_in_bounds(dsize, off, size) {
                    let o = off as usize;
                    let dst = &mut data[o..o + size];
                    match t {
                        b'0' | b'8' => dst[0] = dst[0].wrapping_add((val & 0xff) as u8),
                        b'1' | b'9' => {
                            let v = read_uint(dst, 2, endian) as u16;
                            super::write_uint(dst, v.wrapping_add(val as u16) as u64, 2, endian);
                        }
                        b'2' | b'A' => {
                            let v = read_uint(dst, 4, endian) as u32;
                            super::write_uint(dst, v.wrapping_add(val) as u64, 4, endian);
                        }
                        b'3' | b'B' => {
                            let v = read_uint(dst, 8, endian);
                            super::write_uint(dst, v.wrapping_add(val as u64), 8, endian);
                        }
                        b'4' | b'C' => dst[0] = dst[0].wrapping_sub((val & 0xff) as u8),
                        b'5' | b'D' => {
                            let v = read_uint(dst, 2, endian) as u16;
                            super::write_uint(dst, v.wrapping_sub(val as u16) as u64, 2, endian);
                        }
                        b'6' | b'E' => {
                            let v = read_uint(dst, 4, endian) as u32;
                            super::write_uint(dst, v.wrapping_sub(val) as u64, 4, endian);
                        }
                        b'7' | b'F' => {
                            let v = read_uint(dst, 8, endian);
                            super::write_uint(dst, v.wrapping_sub(val as u64), 8, endian);
                        }
                        _ => {}
                    }
                }
            }

            // ---- multi-write ----
            b'4' => {
                let t = b[1];
                let mut off = hx(&b[2..8]) as i64;
                if matches!(t, b'8' | b'9' | b'A' | b'C' | b'D' | b'E') {
                    off += pointer;
                }
                let mut val = hx(&b[9..17]) as u32;

                let nb = take_strict(&code, &mut i)?;
                let nb = nb.as_bytes();
                let n = if WRAPPER_TYPES.contains(&t) {
                    hx(&nb[0..4])
                } else {
                    hx(&nb[1..4])
                };
                let incoff = hx(&nb[4..8]) as i64;
                let incval = hx(&nb[9..17]) as u32;
                let size = sw_val_bytes(t);

                for k in 0..n {
                    let o = off + incoff * k as i64;
                    if range_in_bounds(dsize, o, size) {
                        let dst = &mut data[o as usize..o as usize + size];
                        match t {
                            b'0' | b'8' | b'4' | b'C' => dst[0] = (val & 0xff) as u8,
                            b'1' | b'9' | b'5' | b'D' => {
                                super::write_uint(dst, (val & 0xffff) as u64, 2, endian)
                            }
                            b'2' | b'A' | b'6' | b'E' => super::write_uint(dst, val as u64, 4, endian),
                            _ => {}
                        }
                    }
                    val = val.wrapping_add(incval);
                }
            }

            // ---- copy bytes ----
            b'5' => {
                let t = b[1];
                let src = hx(&b[2..8]) as i64 + if t == b'8' { pointer } else { 0 };
                let len = hx(&b[9..17]) as usize;

                let nb = take_strict(&code, &mut i)
                    .map_err(|_| ApplyError::new("copy code is truncated"))?;
                let nb = nb.as_bytes();
                let dst = hx(&nb[2..8]) as i64 + if nb[1] == b'8' { pointer } else { 0 };

                if range_in_bounds(dsize, src, len) && range_in_bounds(dsize, dst, len) {
                    let (s, d) = (src as usize, dst as usize);
                    if s != d {
                        let tmp = data[s..s + len].to_vec();
                        data[d..d + len].copy_from_slice(&tmp);
                    }
                }
            }

            // ---- pointer read / move / write ----
            b'6' => {
                let t = b[1];
                let w = b[2];
                let x = b[3];
                let y = b[5];
                let z = b[7];
                let mut val = hx(&b[9..17]) as u32;
                let off = if matches!(t, b'8' | b'9' | b'A') {
                    pointer
                } else {
                    0
                };
                let size = sw_val_bytes(t);

                match w {
                    b'0' => {
                        if x == b'1' {
                            val = val.wrapping_add(ptr_value);
                        }
                        let addr = val as i64 + off;
                        if y == b'1' {
                            pointer = val as i64;
                        }
                        if range_in_bounds(dsize, addr, size) {
                            let a = addr as usize;
                            ptr_value = match size {
                                1 => data[a] as u32,
                                2 => read_uint(&data[a..a + 2], 2, endian) as u32,
                                _ => read_uint(&data[a..a + 4], 4, endian) as u32,
                            };
                        }
                    }
                    b'1' => {
                        match x {
                            b'0' => ptr_value = ptr_value.wrapping_add(val),
                            b'1' => ptr_value = ptr_value.wrapping_sub(val),
                            b'2' => ptr_value = ptr_value.wrapping_mul(val),
                            _ => {}
                        }
                        if z == b'1' {
                            ptr_value = ptr_value.wrapping_add(pointer as u32);
                        }
                        pointer = ptr_value as i64;
                    }
                    b'2' => {
                        match x {
                            b'0' => pointer += val as i64,
                            b'1' => pointer -= val as i64,
                            b'2' => pointer *= val as i64,
                            _ => {}
                        }
                        if y == b'1' {
                            ptr_value = pointer as u32;
                        }
                    }
                    b'4' => {
                        if range_in_bounds(dsize, pointer, size) {
                            let p = pointer as usize;
                            let dst = &mut data[p..p + size];
                            match size {
                                1 => dst[0] = (val & 0xff) as u8,
                                2 => super::write_uint(dst, (val & 0xffff) as u64, 2, endian),
                                _ => super::write_uint(dst, val as u64, 4, endian),
                            }
                        }
                    }
                    _ => {}
                }
            }

            // ---- conditional (min/max) write ----
            b'7' => {
                let t = b[1];
                let mut off = hx(&b[2..8]) as i64;
                if matches!(t, b'8' | b'9' | b'A' | b'C' | b'D' | b'E') {
                    off += pointer;
                }
                let mut val = hx(&b[9..17]) as u32;
                let size = sw_val_bytes(t);
                if range_in_bounds(dsize, off, size) {
                    let o = off as usize;
                    let dst = &mut data[o..o + size];
                    match t {
                        b'0' | b'8' => {
                            val &= 0xff;
                            let v = dst[0] as u32;
                            dst[0] = v.max(val) as u8;
                        }
                        b'1' | b'9' => {
                            val &= 0xffff;
                            let v = read_uint(dst, 2, endian) as u32;
                            super::write_uint(dst, v.max(val) as u64, 2, endian);
                        }
                        b'2' | b'A' => {
                            let v = read_uint(dst, 4, endian) as u32;
                            super::write_uint(dst, v.max(val) as u64, 4, endian);
                        }
                        b'4' | b'C' => {
                            val &= 0xff;
                            let v = dst[0] as u32;
                            dst[0] = v.min(val) as u8;
                        }
                        b'5' | b'D' => {
                            val &= 0xffff;
                            let v = read_uint(dst, 2, endian) as u32;
                            super::write_uint(dst, v.min(val) as u64, 2, endian);
                        }
                        b'6' | b'E' => {
                            let v = read_uint(dst, 4, endian) as u32;
                            super::write_uint(dst, v.min(val) as u64, 4, endian);
                        }
                        _ => {}
                    }
                }
            }

            // ---- search forward ----
            b'8' => {
                let t = b[1];
                let mut cnt = hx(&b[2..4]) as usize;
                let len = hx(&b[4..8]) as usize;
                if len == 0 {
                    return Err(ApplyError::new("empty search pattern"));
                }
                if cnt == 0 {
                    cnt = 1;
                }
                let pat = read_pattern(&code, &mut i, b, len)?;
                let start = if t == b'8' { pointer.max(0) as usize } else { 0 };
                match super::search_data(data, dsize, start, &pat, cnt) {
                    Some(p) => pointer = p as i64,
                    None => {
                        pointer = 0;
                        push_search_miss(&mut warnings, &pat);
                        skip_to_next_search(&code, &mut i);
                    }
                }
            }

            // ---- pointer manipulator ----
            b'9' => {
                let off = hx(&b[9..17]) as i64;
                match b[1] {
                    b'0' => {
                        if range_in_bounds(dsize, off, 4) {
                            pointer = read_u32_be(&data[off as usize..off as usize + 4]) as i64;
                        }
                    }
                    b'1' => {
                        if range_in_bounds(dsize, off, 4) {
                            pointer = read_u32_le(&data[off as usize..off as usize + 4]) as i64;
                        }
                    }
                    b'2' => pointer += off,
                    b'3' => pointer -= off,
                    b'4' => pointer = dsize as i64 - off,
                    b'5' => pointer = off,
                    b'D' => end_pointer = off,
                    b'E' => end_pointer = pointer + off,
                    _ => {}
                }
            }

            // ---- block write ----
            b'A' => {
                let t = b[1];
                let mut off = hx(&b[2..8]) as i64;
                if t == b'8' {
                    off += pointer;
                }
                let size = hx(&b[9..17]) as usize;
                // The loop below always copies a full 4-byte word per half-line,
                // so the last iteration can run past `size`. The reference rounds
                // the buffer up to a multiple of four bytes for exactly this case.
                let mut buf = vec![0u8; if size == 0 { 1 } else { (size + 3) & !3 }];
                let mut j = 0usize;
                while j < size {
                    let nb = take_strict(&code, &mut i)
                        .map_err(|_| ApplyError::new("block-write data is truncated"))?;
                    let nb = nb.as_bytes();
                    let v0 = (hx(&nb[0..8]) as u32).to_be_bytes();
                    buf[j..j + 4].copy_from_slice(&v0);
                    let v1 = (hx(&nb[9..17]) as u32).to_be_bytes();
                    if j + 4 < size {
                        buf[j + 4..j + 8].copy_from_slice(&v1);
                    }
                    j += 8;
                }
                if range_in_bounds(dsize, off, size) {
                    data[off as usize..off as usize + size].copy_from_slice(&buf[..size]);
                }
            }

            // ---- backward search ----
            b'B' => {
                let t = b[1];
                let cnt = {
                    let c = hx(&b[2..4]) as usize;
                    if c == 0 { 1 } else { c }
                };
                let len = hx(&b[4..8]) as usize;
                if len == 0 {
                    return Err(ApplyError::new("empty search pattern"));
                }
                if end_pointer == 0 {
                    end_pointer = dsize as i64 - 1;
                }
                let pat = read_pattern(&code, &mut i, b, len)?;
                let start = if t == b'8' { pointer } else { end_pointer };
                match super::reverse_search_data(data, dsize, start, &pat, cnt) {
                    Some(p) => pointer = p as i64,
                    None => {
                        pointer = 0;
                        push_search_miss(&mut warnings, &pat);
                        skip_to_next_search(&code, &mut i);
                    }
                }
            }

            // ---- address search ----
            b'C' => {
                let t = b[1];
                let cnt = {
                    let c = hx(&b[2..4]) as usize;
                    if c == 0 { 1 } else { c }
                };
                let len = hx(&b[4..8]) as usize;
                let addr = hx(&b[9..17]) as i64
                    + if t == b'8' || t == b'C' { pointer } else { 0 };
                if !range_in_bounds(dsize, addr, len) {
                    pointer = 0;
                    skip_to_next_search(&code, &mut i);
                    continue;
                }
                let a = addr as usize;
                let pat = data[a..a + len].to_vec();
                let found = if t == b'4' || t == b'C' {
                    super::search_data(data, a + len, 0, &pat, cnt)
                } else {
                    super::search_data(data, dsize, a + len, &pat, cnt)
                };
                match found {
                    Some(p) => pointer = p as i64,
                    None => {
                        pointer = 0;
                        push_search_miss(&mut warnings, &pat);
                        skip_to_next_search(&code, &mut i);
                    }
                }
            }

            // ---- byte test / skip ----
            b'D' => {
                let t = b[1];
                let op = b[12];
                let bit = b[11];
                let mut off = hx(&b[2..8]) as i64;
                if t == b'8' {
                    off += pointer;
                }
                let l = hx(&b[9..11]) as i64;
                let mut val = hx(&b[13..17]) as u32;

                let mut src = 0u32;
                let need = if bit == b'1' { 1 } else { 2 };
                if range_in_bounds(dsize, off, need) {
                    let o = off as usize;
                    match bit {
                        b'0' => src = ((data[o] as u32) << 8) | data[o + 1] as u32,
                        b'1' => {
                            val &= 0xff;
                            src = data[o] as u32;
                        }
                        b'2' => src = ((data[o + 1] as u32) << 8) | data[o] as u32,
                        _ => {}
                    }
                }

                let pass = match op {
                    b'0' => src == val,
                    b'1' => src != val,
                    b'2' => src > val,
                    b'3' => src < val,
                    _ => true,
                };

                if !pass {
                    i += l.max(0) as usize;
                }
            }

            _ => {}
        }
    }

    Ok(warnings)
}

/// Record a `search` that matched nothing, so the caller can report that the
/// dependent lines were skipped instead of claiming the file already matches.
fn push_search_miss(warnings: &mut Vec<String>, pat: &[u8]) {
    let hex: Vec<String> = pat.iter().map(|b| format!("{b:02X}")).collect();
    warnings.push(format!("search pattern not found: {}", hex.join(" ")));
}

/// Take the next line, requiring it to be a full 17-character code line.
fn take_strict(code: &[String], i: &mut usize) -> Result<String, ApplyError> {
    let l = code
        .get(*i)
        .ok_or_else(|| ApplyError::new("truncated code"))?
        .clone();
    if l.len() < 17 {
        return Err(ApplyError::new("truncated code"));
    }
    *i += 1;
    Ok(l)
}

/// Build a search pattern from the current line plus any continuation lines.
fn read_pattern(
    code: &[String],
    i: &mut usize,
    b: &[u8],
    len: usize,
) -> Result<Vec<u8>, ApplyError> {
    let mut pat = vec![0u8; (len + 3) & !3];
    pat[0..4].copy_from_slice(&(hx(&b[9..17]) as u32).to_be_bytes());
    let mut j = 4usize;
    while j < len {
        let nb = take_strict(code, i).map_err(|_| ApplyError::new("truncated search pattern"))?;
        let nb = nb.as_bytes();
        pat[j..j + 4].copy_from_slice(&(hx(&nb[0..8]) as u32).to_be_bytes());
        if j + 4 < len {
            pat[j + 4..j + 8].copy_from_slice(&(hx(&nb[9..17]) as u32).to_be_bytes());
        }
        j += 8;
    }
    Ok(pat)
}

/// Advance to the next search opcode after a search finds nothing, so the
/// standard write lines that follow it are skipped.
fn skip_to_next_search(code: &[String], i: &mut usize) {
    while *i < code.len() {
        let l = code[*i].trim_start();
        let b = l.as_bytes();
        if b.len() >= 2 && (b[0] == b'8' || b[0] == b'B' || b[0] == b'C') && b[1] != b'8' {
            break;
        }
        *i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(data: &mut Vec<u8>, lines: &[&str]) {
        let lines: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
        apply(data, &lines, Endian::Big).unwrap();
    }

    #[test]
    fn direct_writes() {
        let mut d = vec![0u8; 16];
        run(
            &mut d,
            &[
                "20000004 12345678",
                "10000000 000000AB",
                "0000000C 000000FF",
            ],
        );
        assert_eq!(&d[4..8], &[0x12, 0x34, 0x56, 0x78]);
        assert_eq!(&d[0..2], &[0x00, 0xAB]);
        assert_eq!(d[12], 0xFF);
    }

    #[test]
    fn search_then_pointer_write() {
        // "UTF\0" at offset 6; the type-4 code then writes 0xFFFFFFFF 19 times
        // at pointer+0 in 4-byte steps (the classic SFxT shape).
        let mut d = vec![0u8; 128];
        d[6..10].copy_from_slice(b"UTF\0");
        run(
            &mut d,
            &[
                "80010004 55544600",
                "4A000000 FFFFFFFF",
                "40130004 00000000",
            ],
        );
        assert_eq!(&d[6..10], &[0xFF, 0xFF, 0xFF, 0xFF]);
        let last = 6 + 4 * 18;
        assert_eq!(&d[last..last + 4], &[0xFF, 0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn add_subtract() {
        let mut d = vec![0u8; 8];
        d[2..6].copy_from_slice(&100u32.to_be_bytes());
        run(&mut d, &["32000002 00000005", "36000002 0000000A"]);
        assert_eq!(u32::from_be_bytes([d[2], d[3], d[4], d[5]]), 95);
    }

    #[test]
    fn conditional_no_less_than() {
        let mut d = vec![0u8; 4];
        d[0] = 0x40;
        run(&mut d, &["70000000 00000030"]); // max(0x40, 0x30)
        assert_eq!(d[0], 0x40);
        run(&mut d, &["70000000 00000050"]); // max(0x40, 0x50)
        assert_eq!(d[0], 0x50);
    }

    #[test]
    fn pointer_manipulator_and_read() {
        let mut d = vec![0u8; 32];
        d[0..4].copy_from_slice(&20u32.to_be_bytes());
        run(&mut d, &["90000000 00000000", "28000000 DEADBEEF"]);
        assert_eq!(&d[20..24], &[0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn block_write() {
        let mut d = vec![0u8; 16];
        run(&mut d, &["A0000004 00000008", "11111111 22222222"]);
        assert_eq!(&d[4..12], &[0x11, 0x11, 0x11, 0x11, 0x22, 0x22, 0x22, 0x22]);
    }

    #[test]
    fn out_of_bounds_is_skipped() {
        let mut d = vec![0u8; 4];
        run(&mut d, &["2000FFFF DEADBEEF"]);
        assert_eq!(d, vec![0, 0, 0, 0]);
    }
}
