//! Hash and checksum implementations ported from the reference patch
//! interpreter.
//!
//! Only the algorithms that actually occur in the bundled database are
//! implemented. Anything else is reported as unsupported rather than guessed
//! at, so a cheat never silently writes a wrong checksum.

/// Reflect the low `bits` bits of `v`.
fn reflect(mut v: u64, bits: u8) -> u64 {
    let mut r = 0u64;
    for _ in 0..bits {
        r = (r << 1) | (v & 1);
        v >>= 1;
    }
    r
}

/// Generic bit-by-bit CRC, parameterised by width.
pub fn crc_generic(
    width: u8,
    data: &[u8],
    poly: u64,
    init: u64,
    xorout: u64,
    refin: bool,
    refout: bool,
) -> u64 {
    let full = if width >= 64 { u64::MAX } else { (1u64 << width) - 1 };
    let top = 1u64 << (width - 1);
    let mut crc = init & full;

    for &b in data {
        let byte = if refin {
            reflect(b as u64, 8) as u8
        } else {
            b
        };
        crc ^= (byte as u64) << (width - 8);
        for _ in 0..8 {
            if crc & top != 0 {
                crc = ((crc << 1) ^ poly) & full;
            } else {
                crc = (crc << 1) & full;
            }
        }
    }

    if refout {
        crc = reflect(crc, width);
    }
    (crc ^ xorout) & full
}

/// Standard CRC-32 (zlib / `crc32`, `crc-32`, `crc32little`).
pub fn crc32(data: &[u8]) -> u32 {
    crc_generic(32, data, 0x04C11DB7, 0xFFFF_FFFF, 0xFFFF_FFFF, true, true) as u32
}

/// CRC-32/BZIP2 (`crc32big`): same polynomial, no reflection.
pub fn crc32_big(data: &[u8]) -> u32 {
    crc_generic(32, data, 0x04C11DB7, 0xFFFFFFFF, 0xFFFFFFFF, false, false) as u32
}

/// CRC-16/XMODEM.
pub fn crc16(data: &[u8]) -> u16 {
    crc_generic(16, data, 0x1021, 0x0000, 0x0000, false, false) as u16
}

pub fn crc64_ecma(data: &[u8]) -> u64 {
    crc_generic(
        64,
        data,
        0x42F0_E1EB_A9EA3693,
        0,
        0,
        false,
        false,
    )
}

pub fn crc64_iso(data: &[u8]) -> u64 {
    crc_generic(
        64,
        data,
        0x0000_0000_0000_001B,
        u64::MAX,
        u64::MAX,
        true,
        true,
    )
}

/// zlib adler32 (`adler32(init, data)`).
pub fn adler32(init: u32, data: &[u8]) -> u32 {
    const MOD: u32 = 65521;
    let mut a = init & 0xffff;
    let mut b = (init >> 16) & 0xffff;
    for &byte in data {
        a = (a + byte as u32) % MOD;
        b = (b + a) % MOD;
    }
    (b << 16) | a
}

/// Byte-wise sum (8-bit accumulator effectively, wrapping).
pub fn add(data: &[u8]) -> u32 {
    let mut sum = 0u32;
    for &b in data {
        sum = sum.wrapping_add(b as u32);
    }
    sum
}

/// 16-bit word sum. `is_le` selects little- or big-endian words.
pub fn wadd(data: &[u8], is_le: bool) -> u32 {
    let mut sum = 0u32;
    for c in data.chunks_exact(2) {
        let w = if is_le {
            u16::from_le_bytes([c[0], c[1]])
        } else {
            u16::from_be_bytes([c[0], c[1]])
        };
        sum = sum.wrapping_add(w as u32);
    }
    sum
}

/// 32-bit dword sum.
pub fn dwadd(data: &[u8], is_le: bool) -> u32 {
    let mut sum = 0u32;
    for c in data.chunks_exact(4) {
        let w = if is_le {
            u32::from_le_bytes([c[0], c[1], c[2], c[3]])
        } else {
            u32::from_be_bytes([c[0], c[1], c[2], c[3]])
        };
        sum = sum.wrapping_add(w);
    }
    sum
}

/// 64-bit qword sum, adding the high dword of every big-endian qword.
pub fn qwadd(data: &[u8]) -> u32 {
    let mut sum = 0u32;
    let mut i = 0usize;
    // The C version skips the lower dword, so it sums bytes 4..8, 12..16, ...
    while i + 8 <= data.len() {
        sum = sum.wrapping_add(u32::from_be_bytes([
            data[i + 4],
            data[i + 5],
            data[i + 6],
            data[i + 7],
        ]));
        i += 8;
    }
    sum
}

/// 16-bit word subtraction.
pub fn wsub(data: &[u8]) -> u32 {
    let mut sum = 0u32;
    for c in data.chunks_exact(2) {
        sum = sum.wrapping_sub(u16::from_be_bytes([c[0], c[1]]) as u32);
    }
    sum
}

/// Sum of signed bytes.
pub fn checksum32(data: &[u8]) -> u32 {
    let mut sum = 0i32;
    for &b in data {
        sum = sum.wrapping_add(b as i8 as i32);
    }
    sum as u32
}

pub fn djb2(data: &[u8]) -> u32 {
    let mut hash = 5381u32;
    for &b in data {
        hash = hash.wrapping_mul(33).wrapping_add(b as u32);
    }
    hash
}

pub fn sdbm(data: &[u8], init: u32) -> u32 {
    let mut h = init;
    for &b in data {
        h = h.wrapping_mul(0x1003f).wrapping_add(b as u32);
    }
    h
}

pub fn fnv1(data: &[u8], init: u32) -> u32 {
    let mut h = init;
    for &b in data {
        h = h.wrapping_mul(0x1000193) ^ b as u32;
    }
    h
}

/// EA/MC02 checksum used by many EA PS3 games (`eachecksum`).
pub fn mc02(pb: &[u8]) -> u32 {
    let table = crc32_table();
    if pb.len() < 4 {
        return 0;
    }

    let rotated_third = ((pb[2] as u32) << 8) | ((pb[2] as u32) >> 24);
    let ored_first_pair = ((pb[0] as u32) << 24)
        | ((pb[0] as u32) >> 8)
        | ((pb[1] as u32) << 16)
        | ((pb[1] as u32) >> 16);
    let mut seed = !((rotated_third | ored_first_pair) | pb[3] as u32);

    for &b in &pb[4..] {
        let looked_up = table[((seed >> 22) & 0x3FC) as usize >> 2];
        let inserted = b as u32 | seed.wrapping_shl(8);
        seed = looked_up ^ inserted;
    }

    !seed
}

// ---------------------------------------------------------------------------
// Checksums the ported interpreter implements under a game-specific name.
//
// Each is a direct port of the reference's own routine. They are separate from
// the generic CRC above because they do not share its shape: several fold the
// stored checksum bytes into the input, fix a table entry, or sum into 32-bit
// accumulators and reduce only at the end.
// ---------------------------------------------------------------------------

/// `ffx_checksum`: Final Fantasy X / X-2 HD Remaster.
///
/// CRC-16/CCITT over the data, but with the last table entry forced to zero —
/// the table the game itself uses is built that way, and using a textbook table
/// gives a different answer.
pub fn ffx(data: &[u8]) -> u16 {
    let mut table = [0u16; 256];
    for (i, slot) in table.iter_mut().enumerate() {
        let mut r = (i as u16) << 8;
        for _ in 0..8 {
            r = if r & 0x8000 != 0 {
                (r << 1) ^ 0x1021
            } else {
                r << 1
            };
        }
        *slot = r;
    }
    table[0xFF] = 0;

    let mut crc: u16 = 0xFFFF;
    for &b in data {
        crc = (crc << 8) ^ table[(((crc >> 8) ^ b as u16) & 0xFF) as usize];
    }
    !crc
}

/// The CRC-32 table Kingdom Hearts 2.5 uses: built with 0xFE rather than 0xFF
/// inner iterations, which leaves the low entries one step short of the usual
/// table.
fn kh25_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    for (x, slot) in table.iter_mut().enumerate() {
        let mut r = (x as u32) << 24;
        for _ in 0..0xFF {
            r = if r & 0x8000_0000 != 0 {
                (r << 1) ^ 0x04C1_1DB7
            } else {
                r << 1
            };
        }
        *slot = r;
    }
    table
}

/// `kh25_checksum`: CRC-32 that skips the four checksum bytes at offset 8.
pub fn kh25(data: &[u8]) -> u32 {
    let table = kh25_table();
    let mut crc: u32 = 0xFFFF_FFFF;
    // The first eight bytes are always hashed, then the stored checksum is
    // stepped over, then the rest.
    let head = data.len().min(8);
    for &b in &data[..head] {
        crc = (crc << 8) ^ table[(((crc >> 24) ^ b as u32) & 0xFF) as usize];
    }
    let rest = &data[(head + 4).min(data.len())..];
    for &b in rest {
        crc = (crc << 8) ^ table[(((crc >> 24) ^ b as u32) & 0xFF) as usize];
    }
    !crc
}

/// `khcom_checksum`: Kingdom Hearts 1.5 / Re:Com.
///
/// A CRC-32 whose data is shifted into the top of the accumulator and stepped
/// one bit per byte, not one bit per byte-bit.
pub fn khcom(data: &[u8]) -> u32 {
    let mut csum: u32 = 0xFFFF_FFFF;
    for &b in data {
        csum ^= (b as u32) << 31;
        csum = if csum & 0x8000_0000 != 0 {
            (csum << 1) ^ 0x04C1_1DB7
        } else {
            csum << 1
        };
    }
    !csum
}

/// `tiara2_checksum`: Tears to Tiara.
///
/// A CRC-shaped loop with no polynomial: each byte grows a running addend, and
/// the accumulator is multiplied by that addend.
pub fn tiara2(data: &[u8]) -> u32 {
    let mut crc: u32 = 1;
    let mut add: u32 = 0x3428;
    for &b in data {
        add = add.wrapping_add(b as u32);
        crc = crc.wrapping_mul(add).wrapping_add(add);
    }
    crc
}

/// `toz_checksum`: Tales of Zestiria.
///
/// SHA-1 over the data with a fixed key appended, then 100 further SHA-1 rounds
/// whose key cycles through eight constants.
pub fn toz(data: &[u8]) -> [u8; 20] {
    const KEYS: [&[u8]; 8] = [b"SRA", b"ROS", b"MIC", b"LAI", b"EDN", b"DEZ", b"ZAB", b"ALI"];

    let mut hash = sha1_with_suffix(data, b"TO12");
    for i in 0..100 {
        hash = sha1_with_suffix(&hash, KEYS[i % 8]);
    }
    hash
}

/// SHA-1 of `data` followed by `key`.
fn sha1_with_suffix(data: &[u8], key: &[u8]) -> [u8; 20] {
    let mut input = Vec::with_capacity(data.len() + key.len());
    input.extend_from_slice(data);
    input.extend_from_slice(key);
    sha1(&input)
}

/// `castlevania_checksum`: Castlevania: Lords of Shadow.
///
/// Two running sums, each adding the byte XOR its own index.
pub fn castlevania(data: &[u8]) -> u32 {
    let mut num: u32 = 0;
    let mut num2: u32 = 0;
    // The index XORed in is the byte offset, so the first sum takes every even
    // offset and the second every odd one.
    for (i, pair) in data.chunks(2).enumerate() {
        let at = (i * 2) as u32;
        num = num.wrapping_add((pair[0] as u32) ^ (at & 255));
        num2 = num2.wrapping_add((*pair.get(1).unwrap_or(&0) as u32) ^ ((at + 1) & 255));
    }
    num.wrapping_add(num2)
}

const SW4_OFF_1: usize = 0x00004;
const SW4_OFF_2: usize = 0x000A8;
const SW4_OFF_3: usize = 0x00C16;
const SW4_OFF_JP: usize = 0x58298;

/// `sw4_checksum`: Samurai Warriors 4.
///
/// Four checksums computed from byte sums over fixed regions of the save, with
/// different region sizes for the Japanese and Western releases. The Japanese
/// build is recognised by a non-zero marker at `SW4_OFF_JP`; every other release
/// falls through with all-zero checksums when the file is too small.
///
/// Returns `(crcs, is_japanese)`.
pub fn sw4(data: &[u8]) -> ([u32; 4], bool) {
    let mut crcs = [0u32; 4];
    if data.len() < SW4_OFF_JP {
        return (crcs, false);
    }
    let is_jp = data[SW4_OFF_JP..SW4_OFF_JP + 4] != [0, 0, 0, 0];

    // The reference reads fixed offsets and lengths without bounds checks
    // beyond the size test above, so a slice is taken only where it is in range.
    let sum = |start: usize, len: usize| -> u32 {
        let end = (start + len).min(data.len());
        let s = if start < end { &data[start..end] } else { &[][..] };
        add(s)
    };

    let num2 = sum(SW4_OFF_1 + 4, SW4_OFF_2 - (SW4_OFF_1 + 4));
    let num3 = sum(SW4_OFF_2 + 4, 2284 - (SW4_OFF_2 + 4));

    let (num1, num4, num5, num6) = if is_jp {
        (
            sum(SW4_OFF_3 + 4, 30294 - (SW4_OFF_3 + 4)),
            sum(33630, 361106 - 33630),
            sum(30294, 33631 - 30294),
            sum(SW4_OFF_1 + 4, 30294 - (SW4_OFF_1 + 4)),
        )
    } else {
        (
            sum(SW4_OFF_3 + 4, 30934 - (SW4_OFF_3 + 4)),
            sum(34270, 361778 - 34270),
            sum(30934, 34271 - 30934),
            sum(SW4_OFF_1 + 4, 30934 - (SW4_OFF_1 + 4)),
        )
    };

    crcs[0] = 0x7FFF_FFFF & num5.wrapping_add((num1.wrapping_add(num2)).wrapping_mul(num3));
    crcs[1] = 0x7FFF_FFFF & num2.wrapping_mul(
        num1.wrapping_add(num3).wrapping_add(num4),
    );
    crcs[2] = num6;
    crcs[3] = 0x7FFF_FFFF & crcs[0].wrapping_add(crcs[1]).wrapping_add(num6);
    (crcs, is_jp)
}

/// `lookup3_little2`: Bob Jenkins' `hashlittle2`, returning the two 32-bit
/// values the patch writes out (`primary` first).
fn rot(x: u32, k: u32) -> u32 {
    x.rotate_left(k)
}

fn lookup3_mix(a: &mut u32, b: &mut u32, c: &mut u32) {
    *a = a.wrapping_sub(*c);
    *a ^= rot(*c, 4);
    *c = c.wrapping_add(*b);
    *b = b.wrapping_sub(*a);
    *b ^= rot(*a, 6);
    *a = a.wrapping_add(*c);
    *c = c.wrapping_sub(*b);
    *c ^= rot(*b, 8);
    *b = b.wrapping_add(*a);
    *a = a.wrapping_sub(*c);
    *a ^= rot(*c, 16);
    *c = c.wrapping_add(*b);
    *b = b.wrapping_sub(*a);
    *b ^= rot(*a, 19);
    *a = a.wrapping_add(*c);
    *c = c.wrapping_sub(*b);
    *c ^= rot(*b, 4);
    *b = b.wrapping_add(*a);
}

fn lookup3_final(a: &mut u32, b: &mut u32, c: &mut u32) {
    *c ^= *b;
    *c = c.wrapping_sub(rot(*b, 14));
    *a ^= *c;
    *a = a.wrapping_sub(rot(*c, 11));
    *b ^= *a;
    *b = b.wrapping_sub(rot(*a, 25));
    *c ^= *b;
    *c = c.wrapping_sub(rot(*b, 16));
    *a ^= *c;
    *a = a.wrapping_sub(rot(*c, 4));
    *b ^= *a;
    *b = b.wrapping_sub(rot(*a, 14));
    *c ^= *b;
    *c = c.wrapping_sub(rot(*b, 24));
}

/// Port of `apollo_hash_lookup3_little2`.
pub fn lookup3_little2(k: &[u8], pc: u32, pb: u32) -> (u32, u32) {
    let mut length = k.len();
    let (mut a, mut b, mut c) = {
        let seed = 0xDEAD_BEEFu32
            .wrapping_add(length as u32)
            .wrapping_add(pc);
        (seed, seed, seed.wrapping_add(pb))
    };

    let mut i = 0usize;
    while length > 12 {
        a = a.wrapping_add(u32::from_le_bytes([k[i], k[i + 1], k[i + 2], k[i + 3]]));
        b = b.wrapping_add(u32::from_le_bytes([k[i + 4], k[i + 5], k[i + 6], k[i + 7]]));
        c = c.wrapping_add(u32::from_le_bytes([k[i + 8], k[i + 9], k[i + 10], k[i + 11]]));
        lookup3_mix(&mut a, &mut b, &mut c);
        length -= 12;
        i += 12;
    }

    // The reference's switch falls through from the highest case, so the bytes
    // still left are added in the same order here.
    let rest = &k[i..i + length];
    if length >= 12 {
        c = c.wrapping_add((rest[11] as u32) << 24);
    }
    if length >= 11 {
        c = c.wrapping_add((rest[10] as u32) << 16);
    }
    if length >= 10 {
        c = c.wrapping_add((rest[9] as u32) << 8);
    }
    if length >= 9 {
        c = c.wrapping_add(rest[8] as u32);
    }
    if length >= 8 {
        b = b.wrapping_add((rest[7] as u32) << 24);
    }
    if length >= 7 {
        b = b.wrapping_add((rest[6] as u32) << 16);
    }
    if length >= 6 {
        b = b.wrapping_add((rest[5] as u32) << 8);
    }
    if length >= 5 {
        b = b.wrapping_add(rest[4] as u32);
    }
    if length >= 4 {
        a = a.wrapping_add((rest[3] as u32) << 24);
    }
    if length >= 3 {
        a = a.wrapping_add((rest[2] as u32) << 16);
    }
    if length >= 2 {
        a = a.wrapping_add((rest[1] as u32) << 8);
    }
    if length >= 1 {
        a = a.wrapping_add(rest[0] as u32);
    } else {
        // A zero-length key needs no mixing at all.
        return (c, b);
    }

    lookup3_final(&mut a, &mut b, &mut c);
    (c, b)
}

fn crc32_table() -> [u32; 256] {
    let mut t = [0u32; 256];
    for (i, entry) in t.iter_mut().enumerate() {
        let mut c = (i as u32) << 24;
        for _ in 0..8 {
            c = if c & 0x8000_0000 != 0 {
                (c << 1) ^ 0x04C11DB7
            } else {
                c << 1
            };
        }
        *entry = c;
    }
    t
}

/// MD5 of `input` (RFC 1321).
pub fn md5(input: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];

    let mut a0 = 0x67452301u32;
    let mut b0 = 0xefcdab89u32;
    let mut c0 = 0x98badcfeu32;
    let mut d0 = 0x10325476u32;

    let mut msg = input.to_vec();
    let bitlen = (input.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bitlen.to_le_bytes());

    for chunk in msg.chunks_exact(64) {
        let mut m = [0u32; 16];
        for (i, w) in m.iter_mut().enumerate() {
            *w = u32::from_le_bytes([
                chunk[4 * i],
                chunk[4 * i + 1],
                chunk[4 * i + 2],
                chunk[4 * i + 3],
            ]);
        }

        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | (!b & d), i),
                16..=31 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let tmp = d;
            d = c;
            c = b;
            let x = a
                .wrapping_add(f)
                .wrapping_add(K[i])
                .wrapping_add(m[g]);
            b = b.wrapping_add(x.rotate_left(S[i]));
            a = tmp;
        }

        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }

    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&a0.to_le_bytes());
    out[4..8].copy_from_slice(&b0.to_le_bytes());
    out[8..12].copy_from_slice(&c0.to_le_bytes());
    out[12..16].copy_from_slice(&d0.to_le_bytes());
    out
}

/// SHA-1 of `input` (FIPS 180-4).
pub fn sha1(input: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];

    let mut msg = input.to_vec();
    let bitlen = (input.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bitlen.to_be_bytes());

    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[4 * i],
                chunk[4 * i + 1],
                chunk[4 * i + 2],
                chunk[4 * i + 3],
            ]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }

        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for i in 0..80 {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(w[i]);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }

        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }

    let mut out = [0u8; 20];
    for (i, v) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

/// SHA-224 / SHA-256 / SHA-384 / SHA-512 of `input` (FIPS 180-4).
pub fn sha224(input: &[u8]) -> [u8; 28] {
    use sha2::Digest;
    let mut out = [0u8; 28];
    out.copy_from_slice(&sha2::Sha224::digest(input));
    out
}

pub fn sha256(input: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    let mut out = [0u8; 32];
    out.copy_from_slice(&sha2::Sha256::digest(input));
    out
}

pub fn sha384(input: &[u8]) -> [u8; 48] {
    use sha2::Digest;
    let mut out = [0u8; 48];
    out.copy_from_slice(&sha2::Sha384::digest(input));
    out
}

pub fn sha512(input: &[u8]) -> [u8; 64] {
    use sha2::Digest;
    let mut out = [0u8; 64];
    out.copy_from_slice(&sha2::Sha512::digest(input));
    out
}

/// HMAC-SHA1, used by the `hmac_sha1("key")` checksum function.
pub fn hmac_sha1(key: &[u8], data: &[u8]) -> [u8; 20] {
    use hmac::{Hmac, Mac};
    type HmacSha1 = Hmac<sha1::Sha1>;
    let mut mac = HmacSha1::new_from_slice(key).expect("HMAC accepts keys of any length");
    mac.update(data);
    let mut out = [0u8; 20];
    out.copy_from_slice(&mac.finalize().into_bytes());
    out
}

/// XOR the four MD5 words together (the `md5_xor` checksum).
///
/// The reference reads the 16-byte digest back as four native words and then
/// `LE32`s the result, which cancels out on a little-endian host, so the value
/// here is the little-endian word view of the digest.
pub fn md5_xor(data: &[u8]) -> u32 {
    let d = md5(data);
    let word = |i: usize| u32::from_le_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]);
    word(0) ^ word(4) ^ word(8) ^ word(12)
}

/// Fold the 20-byte SHA-1 digest into one 64-bit word (the `sha1_xor64` checksum).
///
/// The reference reads the digest as three native little-endian words, XORs
/// them and then converts the result to big-endian (`BE64`). We reproduce that
/// little-endian-host result exactly, because that is what the patches expect.
pub fn sha1_xor64(input: &[u8]) -> u64 {
    let d = sha1(input);
    let mut tail = [0u8; 8];
    tail[..4].copy_from_slice(&d[16..20]);
    let a = u64::from_le_bytes(d[0..8].try_into().unwrap());
    let b = u64::from_le_bytes(d[8..16].try_into().unwrap());
    let c = u64::from_le_bytes(tail);
    (a ^ b ^ c).swap_bytes()
}

/// Bob Jenkins' one-at-a-time hash, used by the
/// Rockstar `CHKS` checksum. Bytes are summed sign-extended, as in the C.
pub fn jenkins_oaat(data: &[u8], init: u32) -> u32 {
    let mut hash = init;
    for &b in data {
        hash = hash.wrapping_add(b as i8 as i32 as u32);
        hash = hash.wrapping_add(hash << 10);
        hash ^= hash >> 6;
    }
    hash = hash.wrapping_add(hash << 3);
    hash ^= hash >> 11;
    hash = hash.wrapping_add(hash << 15);
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_crc32() {
        assert_eq!(crc_generic(32, b"123456789", 0x04C11DB7, 0xFFFFFFFF, 0xFFFFFFFF, true, true), 0xCBF43926);
        assert_eq!(crc_generic(32, b"123456789", 0x04C11DB7, 0xFFFFFFFF, 0xFFFFFFFF, false, false), 0xFC891918);
        assert_eq!(crc_generic(16, b"123456789", 0x1021, 0, 0, false, false), 0x31C3);
        assert_eq!(crc_generic(64, b"123456789", 0x42F0E1EBA9EA3693, 0, 0, false, false), 0x6C40DF5F0B497347);
    }

    #[test]
    fn game_checksums_match_the_reference() {
        // `toz` is the only one that needs a digest, so it is checked against a
        // value computed from the reference recipe (SHA-1 of the data plus
        // "TO12", then 100 rounds over eight rotating keys). The rest are
        // checked byte-for-byte against the reference C by `examples/hashcheck.rs`.
        let data: Vec<u8> = (0..64u32).map(|i| ((i * 7 + 3) & 0xFF) as u8).collect();
        assert_eq!(
            toz(&data),
            [
                0x4c, 0x21, 0x29, 0x73, 0x6d, 0xd2, 0x05, 0x85, 0x44, 0x03, 0x19, 0x68, 0x61, 0x4b,
                0xf3, 0x98, 0xaf, 0x76, 0x6d, 0x07
            ]
        );
        // A buffer shorter than the eight bytes `kh25` reads must still be safe.
        let _ = kh25(&data[..1]);
        let _ = sw4(&data[..16]);
    }

    #[test]
    fn known_adler32() {
        assert_eq!(adler32(1, b"Wikipedia"), 0x11E60398);
    }

    #[test]
    fn known_md5_sha1() {
        assert_eq!(
            md5(b""),
            [
                0xd4, 0x1d, 0x8c, 0xd9, 0x8f, 0x00, 0xb2, 0x04, 0xe9, 0x80, 0x09, 0x98, 0xec, 0xf8,
                0x42, 0x7e
            ]
        );
        assert_eq!(
            sha1(b"abc"),
            [
                0xa9, 0x99, 0x3e, 0x36, 0x47, 0x06, 0x81, 0x6a, 0xba, 0x3e, 0x25, 0x71, 0x78, 0x50,
                0xc2, 0x6c, 0x9c, 0xd0, 0xd8, 0x9d
            ]
        );
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn known_sha2_and_hmac() {
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(sha224(b"abc").len(), 28);
        assert_eq!(sha384(b"abc").len(), 48);
        assert_eq!(sha512(b"abc").len(), 64);

        // RFC 2202 HMAC-SHA1 test case 1.
        let key = [0x0bu8; 20];
        assert_eq!(
            hex(&hmac_sha1(&key, b"Hi There")),
            "b617318655057264e28bc0b6fb378c8ef146be00"
        );
    }

    #[test]
    fn known_jenkins_oaat() {
        // Vectors produced by compiling the reference implementation's
        // one-at-a-time hash directly, so the Rockstar CHKS checksum matches.
        assert_eq!(jenkins_oaat(b"", 0x3FAC7125), 0x6AE1_5BB2);
        assert_eq!(jenkins_oaat(b"abc", 0x3FAC7125), 0xB0D3_C031);
        assert_eq!(jenkins_oaat(b"abc", 0), 0xED13_1F5B);
        assert_eq!(jenkins_oaat(&[1, 2, 3, 4, 5], 0x3FAC7125), 0x6777_EA9A);
    }
}
