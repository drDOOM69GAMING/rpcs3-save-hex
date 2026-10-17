use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

/// One file's part of a multi-file cheat: the target pattern and the lines to
/// run while that file is active. A code whose work spans several files (a
/// checksum computed in one file and stored in another) carries these in order.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CodeStep {
    /// Save path this step runs against, e.g. `USERDATA.BIN`. May be empty to
    /// mean the code's own `file`.
    #[serde(default)]
    pub file: String,
    /// The script lines to run for this file.
    #[serde(default)]
    pub lines: Vec<String>,
}

/// A single quick code / cheat entry.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Code {
    /// Cheat name, e.g. "99 lifes"
    #[serde(default)]
    pub name: String,
    /// Group the cheat belongs to (may be empty)
    #[serde(default)]
    pub group: String,
    /// First code offset, e.g. "0x000019E3" (may be empty for CRC/patch style codes)
    #[serde(default)]
    pub offset: String,
    /// First code value, e.g. "0x00000063"
    #[serde(default)]
    pub value: String,
    /// Save path this code targets, as recorded by the code list, e.g.
    /// `BCUS98123_NDI_UNCHARTED2_BT_P\USR-DATA`. Components may contain glob
    /// wildcards and either `\` or `/` separators. May be empty.
    #[serde(default)]
    pub file: String,
    /// Every raw line that makes up the code (multi-line codes, CRC updates, etc.)
    #[serde(default)]
    pub lines: Vec<String>,
    /// Ordered per-file steps for a multi-file cheat. Empty for the common
    /// single-file code, which uses `file`/`lines` directly. When set, `lines`
    /// still holds the cheat's own visible writes so the UI can show them.
    #[serde(default)]
    pub steps: Vec<CodeStep>,
    /// If set, this code cannot be applied by this tool and the string explains
    /// why (for example, it comes from a patch that needs a decryption step we
    /// do not implement). The cheat is still listed so it can be explained.
    #[serde(default)]
    pub unsupported: Option<String>,
}

/// True when a code's group names a numbered save slot anywhere in its path,
/// e.g. `Slot 3`, `CHARACTERS\SLOT2 Ogony` or `Franklin\4 Port Garage (Slot 1)`.
/// Codes in such groups come as a set of one per slot, all with the same name,
/// which is why the slot has to be shown to tell them apart.
pub fn is_slot_group(group: &str) -> bool {
    slot_key(group).is_some()
}

/// Split a cheat's group into its path components. The code lists record groups
/// as backslash-separated paths, e.g. `CHARACTERS\SLOT2 Ogony\CLASS`, so this
/// returns `["CHARACTERS", "SLOT2 Ogony", "CLASS"]`. Empty components are
/// dropped, so an empty group yields an empty list.
pub fn group_path(group: &str) -> Vec<String> {
    group
        .split(|c| c == '\\' || c == '/')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// If any component of a group path names a save slot, return a signature that
/// is identical for every sibling slot. For example `CHARACTERS\SLOT1 Backup`
/// and `CHARACTERS\SLOT2 Ogony` both give the same key, so the codes that patch
/// the same field in different slots can be found. Returns `None` when the group
/// names no slot at all.
pub fn slot_key(group: &str) -> Option<String> {
    let parts = group_path(group);
    let at = parts.iter().position(|p| slot_number_in(p).is_some())?;
    let mut out: Vec<String> = parts[..at].to_vec();
    out.push("\u{1}slot".to_string());
    out.extend_from_slice(&parts[at + 1..]);
    Some(out.join("\u{1}"))
}

/// The slot number mentioned inside a single group component, if any. Handles
/// `Slot 1`, `SLOT2 Ogony`, `SAVESLOT 3` and `4 Port Garage (Slot 1)`.
fn slot_number_in(part: &str) -> Option<u32> {
    let lower = part.to_ascii_lowercase();
    let at = lower.find("slot")?;
    let digits: String = lower[at + 4..]
        .chars()
        .skip_while(|c| *c == ' ')
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        None
    } else {
        digits.parse().ok()
    }
}

/// Every code in `codes` that is the same cheat as `code` aimed at a different
/// save slot: same name, same target file, and the same group path once the slot
/// number is masked out. Always contains `code` itself. Returns just `code` when
/// it is not part of a save-slot set.
pub fn slot_siblings<'a>(codes: &'a [Code], code: &'a Code) -> Vec<&'a Code> {
    let Some(key) = slot_key(&code.group) else {
        return vec![code];
    };
    codes
        .iter()
        .filter(|c| c.name == code.name && c.file == code.file && slot_key(&c.group).as_deref() == Some(key.as_str()))
        .collect()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Game {    pub serial: String,
    pub title_id: String,
    /// Real game name (e.g. "Heavenly Sword"); falls back to the serial if unknown.
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub codes: Vec<Code>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Database {
    pub source: String,
    pub games: Vec<Game>,
    pub total_games: usize,
    pub total_codes: usize,
    /// Fallback serial -> title list covering far more games than have cheat
    /// codes. Loaded from the bundled `assets/titles.tsv`, so it is skipped
    /// when (de)serializing the JSON cheat database itself.
    #[serde(skip)]
    pub titles: Vec<(String, String)>,
}

impl Database {
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> anyhow::Result<Self> {
        let content = fs::read_to_string(path)?;
        let mut db: Self = serde_json::from_str(&content)?;
        db.titles = load_titles();
        Ok(db)
    }

    /// The database is compiled into the binary so it always ships with the app.
    pub fn load_embedded() -> anyhow::Result<Self> {
        let content = include_str!("../assets/codes.json");
        let mut db: Self = serde_json::from_str(content)?;
        db.titles = load_titles();
        Ok(db)
    }

    /// Games that actually have at least one quick code.
    pub fn games_with_codes(&self) -> Vec<&Game> {
        self.games.iter().filter(|g| !g.codes.is_empty()).collect()
    }

    pub fn find_by_serial(&self, serial: &str) -> Option<&Game> {
        self.games
            .iter()
            .find(|g| g.serial.eq_ignore_ascii_case(serial))
    }

    /// Title from the fallback list for a serial that has no cheat codes.
    pub fn title_for_serial(&self, serial: &str) -> Option<&str> {
        let key = serial.to_ascii_uppercase();
        self.titles
            .binary_search_by(|(k, _)| k.as_str().cmp(key.as_str()))
            .ok()
            .map(|i| self.titles[i].1.as_str())
    }

    /// Human-readable name for a serial. Prefers the cheat database's own name,
    /// then the bundled fallback titles, and finally the serial itself.
    pub fn name_for_serial(&self, serial: &str) -> String {
        if let Some(game) = self.find_by_serial(serial) {
            let name = game.name.trim();
            if !name.is_empty() {
                return name.to_string();
            }
        }
        if let Some(title) = self.title_for_serial(serial) {
            return title.to_string();
        }
        serial.to_string()
    }
}

/// Parse the bundled `assets/titles.tsv` into a sorted, deduplicated list.
fn load_titles() -> Vec<(String, String)> {
    let raw = include_str!("../assets/titles.tsv");
    let mut titles: Vec<(String, String)> = raw
        .lines()
        .filter_map(|line| {
            let (serial, title) = line.split_once('\t')?;
            let serial = serial.trim().to_ascii_uppercase();
            let title = title.trim();
            (!serial.is_empty() && !title.is_empty()).then(|| (serial, title.to_string()))
        })
        .collect();
    titles.sort_by(|a, b| a.0.cmp(&b.0));
    titles.dedup_by(|a, b| a.0 == b.0);
    titles
}

#[cfg(test)]
mod tests {
    use super::*;

    fn embedded() -> Database {
        Database::load_embedded().expect("embedded database must parse")
    }

    #[test]
    fn slot_groups_are_recognised() {
        for g in [
            "Slot 1",
            "slot 8",
            "SAVESLOT 2",
            "Save Slot 10",
            " Slot  3 ",
            // Slots that sit inside a longer path, as used by several games.
            "CHARACTERS\\SLOT2 Ogony",
            "CHARACTERS\\SLOT1 Backup\\CLASS",
            "Franklin\\4 Port Garage (Slot 1)",
            "Final Fantasy X\\Party Members\\Slot 7",
        ] {
            assert!(is_slot_group(g), "{g:?} should be a save slot");
        }
        for g in ["", "General", "Slot", "Slot A", "Slots 1", "Cheats", "Items\\Important Items"] {
            assert!(!is_slot_group(g), "{g:?} should not be a save slot");
        }
    }

    #[test]
    fn group_paths_split_on_backslash() {
        assert_eq!(group_path(""), Vec::<String>::new());
        assert_eq!(group_path("Items"), vec!["Items"]);
        assert_eq!(
            group_path("CHARACTERS\\SLOT2 Ogony\\CLASS"),
            vec!["CHARACTERS", "SLOT2 Ogony", "CLASS"]
        );
    }

    #[test]
    fn sibling_slots_share_one_key() {
        // The slot component is masked, so different slot names collapse together
        // while deeper sections still tell fields apart.
        assert_eq!(
            slot_key("CHARACTERS\\SLOT1 Backup\\CLASS"),
            slot_key("CHARACTERS\\SLOT2 Ogony\\CLASS")
        );
        assert_ne!(
            slot_key("CHARACTERS\\SLOT1 Backup\\CLASS"),
            slot_key("CHARACTERS\\SLOT1 Backup\\RACE")
        );
        assert_ne!(slot_key("Slot 1"), slot_key("Slot 2\\Extra"));
        assert_eq!(slot_key("Items"), None);
    }

    #[test]
    fn siblings_are_the_same_cheat_for_every_slot() {
        let mk = |name: &str, group: &str| Code {
            name: name.to_string(),
            group: group.to_string(),
            file: "DATA0.DAT".to_string(),
            ..Default::default()
        };
        let codes = vec![
            mk("Max ammo", "Slot 1"),
            mk("Max ammo", "Slot 2"),
            mk("Max ammo", "Slot 8"),
            mk("Max money", "Slot 1"),
            mk("Max money", "Slot 2"),
            mk("Loose code", ""),
        ];
        let sibs = slot_siblings(&codes, &codes[0]);
        assert_eq!(sibs.len(), 3, "three Max ammo slots");
        assert!(sibs.iter().all(|c| c.name == "Max ammo"));

        let money = slot_siblings(&codes, &codes[3]);
        assert_eq!(money.len(), 2, "two Max money slots");

        let loose = slot_siblings(&codes, &codes[5]);
        assert_eq!(loose.len(), 1, "a non-slot code is its own sibling");
    }

    #[test]
    fn game_names_are_extracted() {
        let db = embedded();
        assert!(db.games.len() > 900, "expected many games, got {}", db.games.len());
        assert_eq!(db.name_for_serial("BCES00078"), "Heavenly Sword");
        assert_eq!(db.name_for_serial("BCES00052"), "Ratchet & Clank Future: Tools Of Destruction");
        assert_eq!(db.name_for_serial("BCES00001"), "Resistance: Fall of Man");

        let unnamed: Vec<&str> = db
            .games
            .iter()
            .filter(|g| g.name.trim().is_empty() || g.name.eq_ignore_ascii_case(&g.serial))
            .map(|g| g.serial.as_str())
            .collect();
        assert!(unnamed.is_empty(), "games missing a real name: {unnamed:?}");
    }

    #[test]
    fn no_name_is_a_website() {
        fn website_ish(name: &str) -> bool {
            const BAD: [&str; 8] = [
                "save-editor",
                "save editor",
                "http://",
                "https://",
                "www.",
                ".com",
                ".net",
                ".org",
            ];
            let lower = name.to_lowercase();
            BAD.iter().any(|bad| lower.contains(bad))
        }

        let db = embedded();
        for game in &db.games {
            assert!(
                !website_ish(&game.name),
                "game {} has a website-like name: {:?}",
                game.serial,
                game.name
            );
        }
        for (serial, title) in &db.titles {
            assert!(
                !website_ish(title),
                "fallback title {serial} looks like a website: {title:?}"
            );
        }
    }

    #[test]
    fn fallback_titles_cover_games_without_codes() {
        let db = embedded();
        // BCUS98181 has no cheat codes, so the name must come from titles.tsv.
        assert!(db.find_by_serial("BCUS98181").is_none());
        assert_eq!(db.name_for_serial("BCUS98181"), "Starhawk");
        assert_eq!(db.name_for_serial("BLUS30554"), "L.A. Noire");
        // Unknown serials still fall back to the serial itself.
        assert_eq!(db.name_for_serial("ZZZZ99999"), "ZZZZ99999");
    }

    #[test]
    fn cheat_names_are_extracted() {
        let db = embedded();
        let total: usize = db.games.iter().map(|g| g.codes.len()).sum();
        let named: usize = db
            .games
            .iter()
            .flat_map(|g| g.codes.iter())
            .filter(|c| !c.name.is_empty())
            .count();
        assert!(total > 40000, "expected many codes, got {total}");
        assert!(named > 40000, "expected cheat names, got {named}");

        let sly = db.find_by_serial("BCAS20149").unwrap();
        assert!(sly.codes.iter().any(|c| c.name == "99 lifes"));
    }
}
