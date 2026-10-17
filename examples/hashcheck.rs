// Prints the same lines as the C harness in ref.c, from the Rust port, so the
// two can be diffed byte for byte.

use rpcs3_save_hex::engine::hash as h;

fn fill() -> Vec<u8> {
    (0..70_000u32)
        .map(|i| ((i.wrapping_mul(1103515245).wrapping_add(12345)) >> 16) as u8)
        .collect()
}

fn main() {
    let buf = fill();

    for len in 1..=300usize {
        // kh25 reads 8 bytes, skips 4, then the rest: the reference is only
        // ever called with a buffer longer than that.
        if len >= 16 {
            println!("kh25 {len} {:08X}", h::kh25(&buf[16..16 + len]));
        }
        println!("khcom {len} {:08X}", h::khcom(&buf[..len]));
        println!("ffx {len} {:04X}", h::ffx(&buf[..len]));
        println!("tiara2 {len} {:08X}", h::tiara2(&buf[..len]));
        // The reference reads Bytes[i+1] unconditionally, so only even lengths
        // are in bounds for it.
        if len % 2 == 0 {
            println!("castlevania {len} {:08X}", h::castlevania(&buf[..len]));
        }
        let (pc, pb) = h::lookup3_little2(&buf[..len], 0x1234_5678, 0x9ABC_DEF0);
        println!("lookup3 {len} {pc:08X} {pb:08X}");
    }

    for jp in 0..2u8 {
        let mut big: Vec<u8> = (0..400_000u32)
            .map(|i| ((i.wrapping_mul(2654435761)) >> 13) as u8)
            .collect();
        if jp == 1 {
            big[0x58298..0x5829C].copy_from_slice(&[1, 2, 3, 4]);
        }
        let (crcs, is_jp) = h::sw4(&big);
        println!(
            "sw4 {jp} {} {:08X} {:08X} {:08X} {:08X}",
            is_jp as u8, crcs[0], crcs[1], crcs[2], crcs[3]
        );
        let (small, _) = h::sw4(&big[..1024]);
        println!(
            "sw4small {jp} {:08X} {:08X} {:08X} {:08X}",
            small[0], small[1], small[2], small[3]
        );
    }
}