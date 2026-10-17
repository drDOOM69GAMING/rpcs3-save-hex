//! Integrity check for the bundled code database.
//!
//! Runs every supported cheat through the interpreter against a zeroed buffer
//! and reports anything the engine does not understand:
//!
//! * hex/script lines it cannot parse at all (should be zero),
//! * `crc_output_*` options it does not recognise (should be zero),
//! * codes it refuses for reasons that do not depend on the save contents
//!   (a multi-write missing its second line, a bitwise change to a variable
//!   the code never sets, a malformed search pattern).
//!
//! The last group is what `tools/unsupported_structural.json` records; the
//! merge marks those codes unsupported so the UI can show a plain reason. Run
//! this against a database that does **not** yet carry those overrides to
//! regenerate the list:
//!
//! ```text
//! cargo run --release --example audit
//! ```
//!
//! "search pattern not found" is not reported here: it just means the zeroed
//! buffer does not contain the searched bytes, which is expected.

use rpcs3_save_hex::db::Database;
use rpcs3_save_hex::engine::{self, Endian};
use std::collections::BTreeMap;

fn main() {
    let db = Database::load_embedded().unwrap();
    let buf = vec![0u8; 0x20000];

    let mut unrecognised: BTreeMap<String, (usize, String, String)> = BTreeMap::new();
    let mut crc_opts: BTreeMap<String, usize> = BTreeMap::new();
    let mut err_kinds: BTreeMap<String, (usize, String, String)> = BTreeMap::new();
    let mut codes_with_bad_line = 0usize;
    let mut structural: Vec<(String, String, String, String)> = Vec::new();

    for game in &db.games {
        for code in &game.codes {
            if code.unsupported.is_some() {
                continue;
            }
            if code.lines.iter().all(|l| l.trim().is_empty()) {
                continue;
            }
            match engine::apply_code(&buf, &code.lines, Endian::Big) {
                Ok(res) => {
                    let mut bad = false;
                    for w in &res.warnings {
                        if let Some(line) = w.strip_prefix("unrecognised line ignored: ") {
                            bad = true;
                            let e = unrecognised
                                .entry(line.to_string())
                                .or_insert((0, game.serial.clone(), code.name.clone()));
                            e.0 += 1;
                        } else if w.starts_with("unrecognised crc option") {
                            *crc_opts.entry(w.clone()).or_default() += 1;
                        }
                    }
                    if bad {
                        codes_with_bad_line += 1;
                    }
                }
                Err(e) => {
                    let k = e.0.chars().take(60).collect::<String>();
                    let ent = err_kinds
                        .entry(k.clone())
                        .or_insert((0, game.serial.clone(), code.name.clone()));
                    ent.0 += 1;
                    // A missing search pattern only reflects the empty buffer,
                    // not a bad code, so keep it out of the override report.
                    if !e.0.starts_with("search pattern not found") {
                        structural.push((
                            game.serial.clone(),
                            code.name.clone(),
                            code.file.clone(),
                            e.0.clone(),
                        ));
                    }
                }
            }
        }
    }

    println!("codes with an unrecognised line: {codes_with_bad_line}");
    println!("\n== distinct unrecognised lines ({}):", unrecognised.len());
    let mut v: Vec<_> = unrecognised.into_iter().collect();
    v.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
    for (line, (n, serial, code)) in v {
        println!("  {n:5}  {line:?}   e.g. {serial} / {code:?}");
    }

    println!("\n== unrecognised crc options:");
    for (w, n) in &crc_opts {
        println!("  {n:5}  {w}");
    }

    println!("\n== error kinds:");
    let mut e: Vec<_> = err_kinds.into_iter().collect();
    e.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
    for (k, (n, serial, code)) in e {
        println!("  {n:5}  {k}   e.g. {serial} / {code:?}");
    }

    println!(
        "\n== codes refused regardless of save contents ({}):",
        structural.len()
    );
    for (serial, name, file, reason) in &structural {
        println!("STRUCT\t{serial}\t{name}\t{file}\t{reason}");
    }
}
