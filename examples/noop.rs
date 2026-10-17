//! Integrity check for the bundled code database against a real save folder.
//!
//! This lists every supported code whose dry-run produces zero differing bytes
//! against a real save folder, so the two reasons a code changes nothing can be
//! told apart:
//!
//! * the cheat is already in effect (the save already holds those values), and
//! * the code could not locate what it patches (a `search` pattern that is not
//!   in this save, reported as a warning).
//!
//! Anything else would be a code that silently does nothing.
//!
//! ```text
//! cargo run --release --example noop -- <save-folder> [SERIAL]
//! ```

use rpcs3_save_hex::apply::resolve_files;
use rpcs3_save_hex::db::Database;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn main() {
    let root: PathBuf = std::env::args()
        .nth(1)
        .expect("usage: noop <save-folder> [SERIAL]")
        .into();
    let serial_filter = std::env::args().nth(2).map(|s| s.to_ascii_uppercase());

    let db = Database::load_embedded().unwrap();
    let mut cache: HashMap<(String, String), Vec<PathBuf>> = HashMap::new();

    let mut total = 0usize;
    let mut noop: Vec<(String, String, String, String, PathBuf, Vec<String>)> = Vec::new();
    let mut errors: Vec<(String, String, String)> = Vec::new();

    for game in &db.games {
        if let Some(want) = &serial_filter {
            if !game.serial.eq_ignore_ascii_case(want) {
                continue;
            }
        }
        for code in &game.codes {
            if code.unsupported.is_some() || code.lines.iter().all(|l| l.trim().is_empty()) {
                continue;
            }
            let files = cache
                .entry((game.serial.clone(), code.file.clone()))
                .or_insert_with(|| resolve_files(&root, &game.serial, &code.file))
                .clone();
            for path in files.iter() {
                total += 1;
                match rpcs3_save_hex::apply::plan_code(path, code) {
                    Ok(plan) if plan.changed == 0 => noop.push((
                        game.serial.clone(),
                        code.name.clone(),
                        code.group.clone(),
                        code.file.clone(),
                        path.clone(),
                        plan.warnings.clone(),
                    )),
                    Ok(_) => {}
                    Err(e) if !e.starts_with("search pattern not found") => {
                        errors.push((game.serial.clone(), code.name.clone(), e))
                    }
                    Err(_) => {}
                }
            }
        }
    }

    println!("planned {total} code/file combinations");
    println!(
        "\n== zero-change codes ({}):",
        noop.len()
    );
    for (serial, name, group, file, path, warnings) in &noop {
        println!(
            "  {serial}  {name:?}  group={group:?}  file={file:?}  -> {}",
            path.display()
        );
        for w in warnings {
            println!("      warn: {w}");
        }
    }

    println!("\n== other errors ({}):", errors.len());
    for (serial, name, e) in &errors {
        println!("  {serial}  {name:?}  {e}");
    }
}

#[allow(dead_code)]
fn _keep_path_import(_p: &Path) {}
