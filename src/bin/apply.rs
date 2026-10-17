//! Headless companion to the GUI: apply a quick code to a save file (or a code
//! from the bundled database) and print the same dry-run diff the GUI shows.
//!
//! Usage:
//!   apply db    <serial> [search]                 list a game's codes
//!   apply names <root>                            name every save folder found
//!   apply files <root> <serial> <file-pattern>    show which files a code hits
//!   apply preview <file> <line> [<line> ...]      dry run against one file
//!   apply apply   <file> <line> [<line> ...]      same, but write + back up
//!   apply apply-code --root <dir> --serial <s> --code <n> [--yes] [--no-backup] [--all-slots] [--value HEX]
//!   apply verify <root>                           dry-run every code for every game found
//!
//! Lines are the raw cheat lines, one per argument (a hex code line such as
//! `20000A78 0000001F`, or a script command such as `write at 0x0:FF`).

use rpcs3_save_hex::apply as workflow;
use rpcs3_save_hex::db::{self, Database};
use std::path::{Path, PathBuf};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        None | Some("help") | Some("-h") | Some("--help") => {
            usage();
            0
        }
        Some("db") => cmd_db(&args[1..]),
        Some("names") => cmd_names(&args[1..]),
        Some("files") => cmd_files(&args[1..]),
        Some("preview") => cmd_apply(&args[1..], false),
        Some("apply") => cmd_apply(&args[1..], true),
        Some("apply-code") => cmd_apply_code(&args[1..]),
        Some("verify") => cmd_verify(&args[1..]),
        Some(other) => {
            eprintln!("unknown command: {other}\n");
            usage();
            2
        }
    };
    std::process::exit(code);
}

fn usage() {
    eprintln!(
        "usage:\n  \
         apply db <serial> [search]\n  \
         apply names <root>\n  \
         apply files <root> <serial> <file-pattern>\n  \
         apply preview <file> <line> [<line> ...]\n  \
         apply apply <file> <line> [<line> ...]\n  \
         apply apply-code --root <dir> --serial <serial> --code <n> [--yes] [--no-backup] [--value HEX]\n  \
         apply verify <root>"
    );
}

fn cmd_db(args: &[String]) -> i32 {
    let Some(serial) = args.get(0) else {
        eprintln!("usage: apply db <serial> [search]");
        return 2;
    };
    let db = match Database::load_embedded() {
        Ok(db) => db,
        Err(e) => {
            eprintln!("could not load database: {e}");
            return 1;
        }
    };
    let Some(game) = db.find_by_serial(serial) else {
        eprintln!("no game with serial {serial} in the database");
        return 1;
    };
    let search = args.get(1).map(|s| s.to_lowercase()).unwrap_or_default();
    println!(
        "{} - {} ({} codes)  [search matches name, slot and file]",
        game.serial,
        game.name,
        game.codes.len()
    );
    for (i, c) in game.codes.iter().enumerate() {
        if !search.is_empty()
            && !c.name.to_lowercase().contains(&search)
            && !c.group.to_lowercase().contains(&search)
            && !c.file.to_lowercase().contains(&search)
        {
            continue;
        }
        let slot = if c.group.trim().is_empty() {
            String::new()
        } else {
            format!("[{}]", c.group.trim())
        };
        println!(
            "{:5}  {:<16} {:<44}  {}",
            i,
            slot,
            truncate(&c.name, 44),
            c.file
        );
    }
    0
}

fn cmd_names(args: &[String]) -> i32 {
    let Some(root) = args.first() else {
        eprintln!("usage: apply names <root>");
        return 2;
    };
    let db = match Database::load_embedded() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };
    let games = rpcs3_save_hex::scan::scan(&db, Path::new(root));
    if games.is_empty() {
        eprintln!("no save folders found under {root}");
        return 1;
    }
    for g in &games {
        let codes = db
            .find_by_serial(&g.serial)
            .map(|game| game.codes.len())
            .unwrap_or(0);
        let source = if db.find_by_serial(&g.serial).is_some() {
            "cheat database"
        } else if db.title_for_serial(&g.serial).is_some() {
            "title list"
        } else if g.sfo_title.is_some() {
            "PARAM.SFO"
        } else {
            "serial only"
        };
        println!(
            "{} ({})  [{} folder(s), {} code(s); name from {}]",
            g.name,
            g.serial,
            g.dirs.len(),
            codes,
            source
        );
    }
    println!("\n{} game(s)", games.len());
    0
}

fn cmd_files(args: &[String]) -> i32 {
    if args.len() < 3 {
        eprintln!("usage: apply files <root> <serial> <file-pattern>");
        return 2;
    }
    let root = Path::new(&args[0]);
    let serial = &args[1];
    let pattern = &args[2];
    let files = workflow::resolve_files(root, serial, pattern);
    if files.is_empty() {
        eprintln!("no files matched '{pattern}' for {serial} under {}", root.display());
        return 1;
    }
    for f in files {
        println!("{}", f.display());
    }
    0
}

fn cmd_apply(args: &[String], write: bool) -> i32 {
    if args.len() < 2 {
        eprintln!("usage: apply {} <file> <line> [<line> ...]", if write { "apply" } else { "preview" });
        return 2;
    }
    let file = Path::new(&args[0]);
    let lines: Vec<String> = args[1..].to_vec();

    let plan = match workflow::plan(file, &lines) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };
    print_plan(&plan);

    if write {
        if plan.changed == 0 {
            if plan.search_missed() {
                println!("\nnot applied: this code's search pattern was not found in the file.");
            } else {
                println!("\nno changes to write.");
            }
            return 0;
        }
        match workflow::commit(&plan, true) {
            Ok(Some(bak)) => println!("\nwrote {} (backup: {})", plan.target.display(), bak.display()),
            Ok(None) => println!("\nwrote {}", plan.target.display()),
            Err(e) => {
                eprintln!("error: {e}");
                return 1;
            }
        }
    }
    0
}

fn cmd_apply_code(args: &[String]) -> i32 {
    let mut root: Option<PathBuf> = None;
    let mut serial = String::new();
    let mut code_idx: Option<usize> = None;
    let mut yes = false;
    let mut backup = true;
    let mut all_slots = false;
    let mut value: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--root" => {
                i += 1;
                root = args.get(i).map(PathBuf::from);
            }
            "--serial" => {
                i += 1;
                serial = args.get(i).cloned().unwrap_or_default();
            }
            "--code" => {
                i += 1;
                code_idx = args.get(i).and_then(|s| s.parse().ok());
            }
            "--yes" | "-y" => yes = true,
            "--no-backup" => backup = false,
            "--all-slots" => all_slots = true,
            "--value" => {
                i += 1;
                value = args.get(i).cloned();
            }
            other => {
                eprintln!("unknown argument: {other}");
                return 2;
            }
        }
        i += 1;
    }

    let (Some(root), Some(code_idx)) = (root, code_idx) else {
        eprintln!("usage: apply apply-code --root <dir> --serial <s> --code <n> [--yes] [--no-backup] [--all-slots] [--value HEX]");
        return 2;
    };

    let db = match Database::load_embedded() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };
    let Some(game) = db.find_by_serial(&serial) else {
        if let Some(title) = db.title_for_serial(&serial) {
            println!("{serial} - {title} (no quick codes in the database)");
            return 0;
        }
        eprintln!("serial {serial} is not in the database");
        return 1;
    };
    let Some(code) = game.codes.get(code_idx) else {
        eprintln!("{serial} has no code #{code_idx} (it has {})", game.codes.len());
        return 1;
    };

    println!("code: {} ({})", code.name, serial);

    // A cheat whose listing leaves a value blank needs one; `--value` supplies it.
    let code = match value.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
        Some(v) => match workflow::resolve_choice(code, v) {
            Ok(filled) => Box::leak(Box::new(filled)),
            Err(e) => {
                eprintln!("refusing: {e}");
                return 1;
            }
        },
        None => {
            if let Some(e) = workflow::code_error(code) {
                eprintln!("refusing: {e}");
                return 1;
            }
            code
        }
    };

    if code.file.trim().is_empty() {
        eprintln!("refusing: this code does not name a save file, so it cannot be applied automatically");
        return 1;
    }

    // `--all-slots` patches every save slot that shares this code's name, so the
    // change lands whichever slot the game actually reads.
    let codes: Vec<db::Code> = if all_slots {
        let siblings: Vec<db::Code> = db::slot_siblings(&game.codes, code)
            .into_iter()
            .cloned()
            .collect();
        if siblings.len() > 1 {
            println!("applying to all {} save slots", siblings.len());
        }
        siblings
    } else {
        vec![code.clone()]
    };

    // A multi-file cheat runs as one unit over its save folder; a single-file one
    // can still be bundled per slot with `--all-slots`.
    let plans: Vec<workflow::Plan> = if code.steps.is_empty() {
        let files = workflow::resolve_files(&root, &serial, &code.file);
        if files.is_empty() {
            eprintln!(
                "no save files matched '{}' for {} under {}",
                code.file,
                serial,
                root.display()
            );
            return 1;
        }
        let mut out = Vec::new();
        for file in &files {
            match workflow::plan_codes(file, &codes) {
                Ok(p) => out.push(p),
                Err(e) => {
                    eprintln!("error: {e}");
                    return 1;
                }
            }
        }
        out
    } else {
        match workflow::plan_code_in(&root, &serial, code) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("error: {e}");
                return 1;
            }
        }
    };

    for plan in &plans {
        println!("\n=== {} ===", plan.target.display());
        print_plan(plan);
        if yes {
            if plan.changed == 0 {
                if plan.search_missed() {
                    println!("not applied: this code's search pattern was not found.");
                } else {
                    println!("no changes to write.");
                }
                continue;
            }
            match workflow::commit(plan, backup) {
                Ok(Some(bak)) => println!(
                    "wrote {} (backup: {})",
                    plan.target.display(),
                    bak.display()
                ),
                Ok(None) => println!("wrote {}", plan.target.display()),
                Err(e) => {
                    eprintln!("error: {e}");
                    return 1;
                }
            }
        }
    }

    if !yes {
        println!("\n(dry run; pass --yes to write)");
    }
    0
}

fn print_plan(plan: &workflow::Plan) {
    println!("target:      {}", plan.target.display());
    println!("size:        {} -> {} bytes", plan.before_len, plan.after_len());
    println!("changed:     {} byte(s)", plan.changed);

    for change in &plan.changes {
        let before = change.before.map(|b| format!("{b:02X}")).unwrap_or_else(|| "--".to_string());
        let after = change.after.map(|b| format!("{b:02X}")).unwrap_or_else(|| "--".to_string());
        println!("  0x{:08X}: {} -> {}", change.offset, before, after);
    }
    if plan.changed > plan.changes.len() {
        println!("  ... {} more change(s)", plan.changed - plan.changes.len());
    }

    for line in &plan.log {
        println!("  . {line}");
    }
    for warning in &plan.warnings {
        println!("  ! {warning}");
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n.saturating_sub(3)).collect::<String>() + "..."
    }
}

/// Dry-run every code of every game found under `root`. This is a smoke test
/// for the interpreter against real saves: it reports how many codes apply
/// cleanly, how many are placeholders, how many have no matching file, and
/// lists (once each) any interpretation errors.
fn cmd_verify(args: &[String]) -> i32 {
    let Some(root) = args.get(0) else {
        eprintln!("usage: apply verify <root>");
        return 2;
    };
    let root = Path::new(root);
    let db = match Database::load_embedded() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };

    // Group the save directories under `root` by serial.
    let mut serials: Vec<String> = Vec::new();
    for dir in list_subdirs(root) {
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().to_ascii_uppercase())
            .unwrap_or_default();
        for len in [10usize, 9] {
            if name.len() >= len && db.find_by_serial(&name[..len]).is_some() {
                let s = name[..len].to_string();
                if !serials.contains(&s) {
                    serials.push(s);
                }
                break;
            }
        }
    }
    serials.sort();

    let mut games = 0usize;
    let mut codes = 0usize;
    let mut applied = 0usize;
    let mut not_found = 0usize;
    let mut placeholder = 0usize;
    let mut unsupported = 0usize;
    let mut no_target = 0usize;
    let mut changed_files = 0usize;
    let mut errors: Vec<String> = Vec::new();

    for serial in &serials {
        let Some(game) = db.find_by_serial(serial) else {
            continue;
        };
        games += 1;
        let mut game_errors = 0usize;
        for code in &game.codes {
            codes += 1;
            if code.unsupported.is_some() {
                unsupported += 1;
                continue;
            }
            if workflow::runnable_lines(&code.lines).is_err() {
                placeholder += 1;
                continue;
            }
            let plans = if code.steps.is_empty() {
                let files = workflow::resolve_files(root, serial, &code.file);
                if files.is_empty() {
                    no_target += 1;
                    continue;
                }
                let mut out = Vec::new();
                for file in &files {
                    match workflow::plan_code(file, code) {
                        Ok(p) => out.push(p),
                        Err(e) => {
                            game_errors += 1;
                            let msg = format!("{serial} '{}': {e}", code.name);
                            if !errors.contains(&msg) {
                                errors.push(msg);
                            }
                        }
                    }
                }
                out
            } else {
                match workflow::plan_code_in(root, serial, code) {
                    Ok(p) => p,
                    Err(e) => {
                        game_errors += 1;
                        let msg = format!("{serial} '{}': {e}", code.name);
                        if !errors.contains(&msg) {
                            errors.push(msg);
                        }
                        continue;
                    }
                }
            };
            for plan in plans {
                applied += 1;
                if plan.changed > 0 {
                    changed_files += 1;
                } else if plan.search_missed() {
                    not_found += 1;
                }
            }
        }
        println!(
            "{} - {}: {} code(s), {} error(s)",
            serial,
            game.name,
            game.codes.len(),
            game_errors
        );
    }

    println!("\n--- summary ---");
    println!("games:            {games} (of {} in the database)", db.total_games);
    println!("codes examined:   {codes}");
    println!("code/file pairs applied cleanly: {applied} ({changed_files} with changes)");
    println!("code/file pairs where the search pattern was not found: {not_found}");
    println!("placeholder codes refused: {placeholder}");
    println!("codes marked not supported in the database: {unsupported}");
    println!("codes with no matching save file: {no_target}");
    println!("errors:           {}", errors.len());
    for e in errors.iter().take(40) {
        println!("  ! {e}");
    }
    0
}

fn list_subdirs(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(root).into_iter().flatten().flatten() {
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            out.push(entry.path());
        }
    }
    out
}
