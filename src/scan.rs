//! Finding a user's RPCS3 saves and working out which game each one belongs to.
//!
//! A save folder is named after the game's title id (e.g.
//! `BCUS98123_NDI_UNCHARTED2_BT_P`), and almost every save carries a
//! `PARAM.SFO` whose `TITLE` field is the game's real name. We combine three
//! sources so that *every* save gets a readable name, even games with no cheat
//! codes: the cheat database, the bundled fallback title list, and the save's
//! own `PARAM.SFO`.

use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::db::Database;
use crate::sfo;

/// A game found on disk, with the save folders discovered for it.
#[derive(Debug, Clone)]
pub struct ScannedGame {
    pub serial: String,
    pub name: String,
    /// Title taken from a save's own `PARAM.SFO`, when one was present.
    pub sfo_title: Option<String>,
    pub dirs: Vec<PathBuf>,
}

/// Work out which game serial a save-directory name belongs to. Every RPCS3
/// save directory starts with its title id. Prefer a serial the cheat database
/// knows; otherwise take the first nine characters (the length of a PS3 title
/// id) so the save still shows up.
pub fn serial_for_dir(db: &Database, dir_name: &str) -> Option<String> {
    let upper = dir_name.to_ascii_uppercase();
    for len in [10usize, 9] {
        if upper.len() < len {
            continue;
        }
        let candidate = &upper[..len];
        if db.find_by_serial(candidate).is_some() {
            return Some(candidate.to_string());
        }
    }
    (upper.len() >= 9).then(|| upper[..9].to_string())
}

/// Look for a `PARAM.SFO` inside a save directory and return its title.
pub fn save_title(dir: &Path) -> Option<String> {
    // Most saves put PARAM.SFO at the root; a few nest it one level down.
    if let Some(title) = sfo::title_of(&dir.join("PARAM.SFO")) {
        return Some(title);
    }
    WalkDir::new(dir)
        .max_depth(2)
        .into_iter()
        .flatten()
        .filter(|e| e.file_type().is_file())
        .find(|e| {
            e.file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case("PARAM.SFO")
        })
        .and_then(|e| sfo::title_of(e.path()))
}

/// Best available display name for a save, in order of preference: the cheat
/// database, the bundled title list, the save's own `PARAM.SFO`, the serial.
pub fn resolve_name(db: &Database, serial: &str, sfo_title: Option<&str>) -> String {
    if let Some(game) = db.find_by_serial(serial) {
        if !game.name.trim().is_empty() {
            return game.name.clone();
        }
    }
    if let Some(title) = db.title_for_serial(serial) {
        return title.to_string();
    }
    if let Some(title) = sfo_title {
        if !title.trim().is_empty() {
            return title.to_string();
        }
    }
    serial.to_string()
}

/// Walk `folder`, group its save directories by game, and give every game a
/// name. The result is sorted by name.
pub fn scan(db: &Database, folder: &Path) -> Vec<ScannedGame> {
    let mut games: Vec<ScannedGame> = Vec::new();

    for entry in WalkDir::new(folder).max_depth(1).min_depth(1).into_iter().flatten() {
        if !entry.file_type().is_dir() {
            continue;
        }
        let dir_name = entry.file_name().to_string_lossy().to_string();
        let Some(serial) = serial_for_dir(db, &dir_name) else {
            continue;
        };
        let path = entry.path().to_path_buf();
        let sfo_title = save_title(&path);
        match games.iter_mut().find(|g| g.serial == serial) {
            Some(game) => {
                game.dirs.push(path);
                if game.sfo_title.is_none() {
                    game.sfo_title = sfo_title;
                }
            }
            None => games.push(ScannedGame {
                serial,
                name: String::new(),
                sfo_title,
                dirs: vec![path],
            }),
        }
    }

    for game in &mut games {
        game.name = resolve_name(db, &game.serial, game.sfo_title.as_deref());
    }
    games.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    games
}

#[cfg(test)]
mod tests {
    use super::*;

    fn embedded() -> Database {
        Database::load_embedded().unwrap()
    }

    #[test]
    fn serials_are_taken_from_the_directory_name() {
        let db = embedded();
        assert_eq!(
            serial_for_dir(&db, "BCUS98123_NDI_UNCHARTED2_BT_P").as_deref(),
            Some("BCUS98123")
        );
        assert_eq!(
            serial_for_dir(&db, "NPUB31392-GAMES-QUICKSAVE").as_deref(),
            Some("NPUB31392")
        );
        assert_eq!(serial_for_dir(&db, "BC").as_deref(), None);
    }

    #[test]
    fn unknown_games_are_still_named() {
        let db = embedded();
        // Not in the cheat database, but present in the bundled title list.
        assert_eq!(resolve_name(&db, "BCUS98181", None), "Starhawk");
        // Not in either database: the save's own PARAM.SFO title wins.
        assert_eq!(
            resolve_name(&db, "NPUB30163", Some("Outland")),
            "Outland"
        );
        // Nothing known at all: fall back to the serial.
        assert_eq!(resolve_name(&db, "ZZZZ99999", None), "ZZZZ99999");
    }

    #[test]
    fn reads_title_from_a_param_sfo_on_disk() {
        let dir = std::env::temp_dir().join(format!(
            "rpcs3-scan-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("PARAM.SFO"), mini_sfo("The Evil Within")).unwrap();
        assert_eq!(save_title(&dir).as_deref(), Some("The Evil Within"));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A one-key `PARAM.SFO` holding a `TITLE`.
    fn mini_sfo(title: &str) -> Vec<u8> {
        let key_table = b"TITLE\0";
        let mut data_table = title.as_bytes().to_vec();
        data_table.push(0);
        let key_table_start = 20 + 16;
        let data_table_start = key_table_start + key_table.len();

        let mut out = Vec::new();
        out.extend_from_slice(b"\0PSF");
        out.extend_from_slice(&0x0000_0101u32.to_le_bytes());
        out.extend_from_slice(&(key_table_start as u32).to_le_bytes());
        out.extend_from_slice(&(data_table_start as u32).to_le_bytes());
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // key offset
        out.extend_from_slice(&0x0204u16.to_le_bytes()); // utf-8
        out.extend_from_slice(&(data_table.len() as u32).to_le_bytes());
        out.extend_from_slice(&(data_table.len() as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes()); // data offset
        out.extend_from_slice(key_table);
        out.extend_from_slice(&data_table);
        out
    }
}
