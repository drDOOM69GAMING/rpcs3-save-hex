#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! RPCS3 Save Editor GUI.
//!
//! Pick the folder that holds your save data (the folder of `BCUSxxxxx...`
//! directories), choose a game, choose a quick code, review the exact byte
//! changes, then apply them. The original file is always backed up first.

use eframe::egui;
use rpcs3_save_hex::apply as workflow;
use rpcs3_save_hex::db;
use rpcs3_save_hex::scan;
use rpcs3_save_hex::sound;
use std::path::PathBuf;
use std::sync::Arc;

/// The same colour with a 0..=1 opacity multiplier applied.
fn with_alpha(colour: egui::Color32, fade: f32) -> egui::Color32 {
    let alpha = (fade.clamp(0.0, 1.0) * 255.0).round() as u8;
    egui::Color32::from_rgba_unmultiplied(colour.r(), colour.g(), colour.b(), alpha)
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 800.0])
            .with_title("RPCS3 Save Editor"),
        ..Default::default()
    };

    // The code database is baked into the binary.
    let db = Arc::new(db::Database::load_embedded().unwrap_or_else(|_| db::Database {
        source: String::new(),
        games: Vec::new(),
        total_games: 0,
        total_codes: 0,
        titles: Vec::new(),
    }));

    eframe::run_native(
        "RPCS3 Save Editor",
        options,
        Box::new(|_cc| Ok(Box::new(App::new(db)))),
    )
}

#[derive(Clone, Debug, PartialEq)]
enum View {
    MainMenu,
    QuickCodes,
    Advanced,
}

/// The dry-run plan (or the reason there is none) for one target file.
struct FilePlan {
    path: PathBuf,
    outcome: Result<workflow::Plan, String>,
}

/// One node of the collapsible quick-code tree. Code lists record groups as
/// backslash-separated paths (`CHARACTERS\SLOT2 Ogony\CLASS`), so each path
/// component becomes a collapsible section and `codes` holds the entries that
/// live exactly at this path. This is what makes per-save-slot codes readable:
/// every slot is its own section instead of a wall of identically named rows.
#[derive(Default)]
struct GroupNode {
    codes: Vec<usize>,
    children: Vec<(String, GroupNode)>,
}

impl GroupNode {
    fn insert(&mut self, path: &[String], idx: usize) {
        match path.split_first() {
            None => self.codes.push(idx),
            Some((head, tail)) => {
                let pos = match self.children.iter().position(|(n, _)| n == head) {
                    Some(p) => p,
                    None => {
                        self.children.push((head.clone(), GroupNode::default()));
                        self.children.len() - 1
                    }
                };
                self.children[pos].1.insert(tail, idx);
            }
        }
    }

    /// How many codes this node and its whole subtree hold.
    fn total_codes(&self) -> usize {
        self.codes.len()
            + self
                .children
                .iter()
                .map(|(_, child)| child.total_codes())
                .sum::<usize>()
    }
}

/// A short-lived popup shown after an action, so a write that happens without
/// the confirmation dialog still tells you it happened.
struct Toast {
    text: String,
    kind: ToastKind,
    /// `egui` time (seconds) when the toast appeared.
    shown_at: f64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ToastKind {
    Ok,
    Info,
    Error,
}

/// What applying one slot's code would do to its file, for the multi-slot
/// summary. `NotFound` means the code's search pattern was absent, so the slot
/// is left untouched for a reason other than already holding the value.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SlotState {
    WillChange,
    Already,
    NotFound,
}

struct App {
    db: Arc<db::Database>,
    view: View,

    save_root: Option<PathBuf>,
    games: Vec<scan::ScannedGame>,
    selected_game: Option<usize>,

    search_code: String,
    selected_code: Option<usize>,

    /// The code index(es) the current preview was built from. Most of the time
    /// this is one code; the "apply to all save slots" shortcut fills it with
    /// every slot's sibling so they patch the file together.
    planned: Vec<usize>,

    /// Whether to save a `.bak` copy of each file before writing to it.
    backup_enabled: bool,

    /// For multi-slot plans, each planned slot's label and what applying it
    /// would do to the file. A slot that already holds the target values shows
    /// as "already applied" instead of silently doing nothing.
    slot_status: Vec<(String, SlotState)>,

    /// The value the user typed for each cheat whose listing leaves it blank,
    /// keyed by that cheat's index.
    choice_values: std::collections::HashMap<usize, String>,

    /// The save file the user picked for cheats whose listing never names one,
    /// keyed by that cheat's index.
    picked_files: std::collections::HashMap<usize, String>,

    /// Preview for the currently selected code, one entry per matching file.
    file_plans: Vec<FilePlan>,
    error: Option<String>,
    status: Option<String>,

    /// The transient "applied" popup, cleared a few seconds after it appears.
    toast: Option<Toast>,

    /// The confirmation dialog is open for the selected code.
    confirm_open: bool,

    // Advanced view
    save_file: Option<PathBuf>,
    hex_view_data: Vec<u8>,
}

impl App {
    fn new(db: Arc<db::Database>) -> Self {
        Self {
            db,
            view: View::MainMenu,
            save_root: None,
            games: Vec::new(),
            selected_game: None,
            search_code: String::new(),
            selected_code: None,
            planned: Vec::new(),
            backup_enabled: true,
            slot_status: Vec::new(),
            choice_values: std::collections::HashMap::new(),
            picked_files: std::collections::HashMap::new(),
            file_plans: Vec::new(),
            error: None,
            status: None,
            toast: None,
            confirm_open: false,
            save_file: None,
            hex_view_data: Vec::new(),
        }
    }

    /// Walk the selected folder and group save directories by game serial.
    fn scan_save_folder(&mut self, folder: &std::path::Path) {
        self.games = scan::scan(&self.db, folder);
    }

    fn load_game(&mut self, idx: usize) {
        self.selected_game = Some(idx);
        self.selected_code = None;
        self.file_plans.clear();
        self.error = None;
        self.status = None;
        self.confirm_open = false;
    }

    fn selected_game(&self) -> Option<&scan::ScannedGame> {
        self.selected_game.and_then(|i| self.games.get(i))
    }

    /// The cheat to plan for, with the user's chosen value filled in.
    ///
    /// A cheat that needs a value is planned from the filled-in copy, so the
    /// preview and the apply use the same numbers. Without a value the reason
    /// the cheat is held back is returned instead.
    fn code_for_plan(&self, code: &db::Code, idx: usize) -> Result<db::Code, String> {
        let value = self
            .choice_values
            .get(&idx)
            .map(|v| v.trim())
            .filter(|v| !v.is_empty());
        match value {
            Some(v) => workflow::resolve_choice(code, v),
            None => match workflow::code_error(code) {
                Some(e) => Err(e),
                None => Ok(code.clone()),
            },
        }
    }

    fn codes_for_selected(&self) -> Vec<db::Code> {
        self.selected_game()
            .and_then(|g| self.db.find_by_serial(&g.serial))
            .map(|g| g.codes.clone())
            .unwrap_or_default()
    }

    /// Recompute the dry-run preview for a single code.
    fn select_code(&mut self, idx: usize) {
        self.selected_code = Some(idx);
        self.plan_many(&[idx]);
    }

    /// Recompute the dry-run preview for one or more codes.
    ///
    /// Codes that share a target file are applied together, in list order, so
    /// selecting every save slot patches them all in one pass instead of the
    /// last one overwriting the earlier ones.
    fn plan_many(&mut self, idxs: &[usize]) {
        self.planned = idxs.to_vec();
        self.file_plans.clear();
        self.error = None;
        self.status = None;

        let Some(serial) = self.selected_game().map(|g| g.serial.clone()) else {
            return;
        };
        let root = self.save_root.clone().unwrap_or_default();
        let codes = self
            .db
            .find_by_serial(&serial)
            .map(|g| g.codes.clone())
            .unwrap_or_default();

        // Collect, per target file, the codes that write to it.
        let mut order: Vec<PathBuf> = Vec::new();
        let mut by_path: std::collections::HashMap<PathBuf, Vec<db::Code>> =
            std::collections::HashMap::new();
        // A multi-file cheat has to run as one unit over its whole save folder,
        // so it is planned on its own and not bundled per file.
        let mut standalone: Vec<FilePlan> = Vec::new();
        let mut standalone_state: std::collections::HashMap<usize, SlotState> =
            std::collections::HashMap::new();

        for &idx in idxs {
            let Some(raw) = codes.get(idx).cloned() else {
                continue;
            };
            // Refuse placeholder or unsupported codes up front with a clear
            // message, filling in the user's chosen value first.
            let code = match self.code_for_plan(&raw, idx) {
                Ok(c) => c,
                Err(e) => {
                    self.error = Some(e);
                    continue;
                }
            };
            // A listing that never says which file it patches gets the one the
            // user picked in its card; without a choice there is nothing to do.
            let mut code = code;
            if code.file.trim().is_empty() {
                match self.picked_files.get(&idx) {
                    Some(f) if !f.trim().is_empty() => code.file = f.clone(),
                    _ => {
                        self.error = Some(
                            "This quick code does not say which save file it patches. \
                             Choose one for it in the list first."
                                .to_string(),
                        );
                        continue;
                    }
                }
            }

            if !code.steps.is_empty() {
                match workflow::plan_code_in(&root, &serial, &code) {
                    Ok(plans) => {
                        let mut any = false;
                        let mut miss = false;
                        for p in plans {
                            any |= p.changed > 0;
                            miss |= p.search_missed();
                            standalone.push(FilePlan {
                                path: p.target.clone(),
                                outcome: Ok(p),
                            });
                        }
                        standalone_state.insert(
                            idx,
                            if any {
                                SlotState::WillChange
                            } else if miss {
                                SlotState::NotFound
                            } else {
                                SlotState::Already
                            },
                        );
                    }
                    Err(e) => self.error = Some(e),
                }
                continue;
            }

            let files = workflow::resolve_files(&root, &serial, &code.file);
            if files.is_empty() {
                self.error = Some(format!(
                    "No save file matched this code's target '{}'. Make sure the game's save folder is under the selected folder.",
                    code.file
                ));
                continue;
            }

            for path in files {
                let bundle = by_path.entry(path.clone()).or_default();
                if bundle.is_empty() {
                    order.push(path);
                }
                bundle.push(code.clone());
            }
        }

        self.slot_status.clear();
        for path in order {
            let bundle = by_path.get(&path).cloned().unwrap_or_default();
            let outcome = workflow::plan_codes(&path, &bundle);
            self.file_plans.push(FilePlan { path, outcome });
        }
        for p in standalone {
            if !self.file_plans.iter().any(|fp| fp.path == p.path) {
                self.file_plans.push(p);
            }
        }

        // When several slots are planned, work out which of them still need the
        // change. This is what stops a slot that is already at the target value
        // from looking like a failed apply.
        if idxs.len() > 1 {
            let mut status = Vec::new();
            for &idx in idxs {
                let Some(code) = codes.get(idx) else { continue };
                let label = if !code.group.trim().is_empty() {
                    code.group.trim().to_string()
                } else if !code.name.trim().is_empty() {
                    code.name.clone()
                } else {
                    format!("code #{idx}")
                };
                let state = if let Some(s) = standalone_state.get(&idx) {
                    *s
                } else {
                    let mut changed = false;
                    let mut not_found = false;
                    for fp in &self.file_plans {
                        if let Ok(plan) = workflow::plan_code(&fp.path, code) {
                            if plan.changed > 0 {
                                changed = true;
                            } else if plan.search_missed() {
                                not_found = true;
                            }
                        }
                    }
                    if changed {
                        SlotState::WillChange
                    } else if not_found {
                        SlotState::NotFound
                    } else {
                        SlotState::Already
                    }
                };
                status.push((label, state));
            }
            self.slot_status = status;
        }
    }

    /// Select a code and open the apply-confirmation dialog for it.
    fn open_apply(&mut self, idx: usize) {
        self.select_code(idx);
        self.confirm_open = true;
    }

    /// Select a code and every sibling that targets a different save slot, then
    /// open the confirmation dialog. This patches all slots at once so the right
    /// one is hit without having to know which slot the save lives in.
    fn open_apply_slots(&mut self, idx: usize) {
        self.selected_code = Some(idx);
        self.plan_slots(idx);
        self.confirm_open = true;
    }

    /// Plan the selected code plus every sibling that patches the same field in
    /// a different save slot, without opening the confirmation dialog.
    fn plan_slots(&mut self, idx: usize) {
        let Some(serial) = self.selected_game().map(|g| g.serial.clone()) else {
            return;
        };
        let codes = self
            .db
            .find_by_serial(&serial)
            .map(|g| g.codes.clone())
            .unwrap_or_default();
        let Some(code) = codes.get(idx) else {
            return;
        };
        let key = db::slot_key(&code.group);
        let siblings: Vec<usize> = codes
            .iter()
            .enumerate()
            .filter(|(_, c)| {
                c.name == code.name && c.file == code.file && db::slot_key(&c.group) == key
            })
            .map(|(i, _)| i)
            .collect();

        self.selected_code = Some(idx);
        self.plan_many(&siblings);
    }

    /// True when the selected code has at least one file that would change.
    fn has_changes(&self) -> bool {
        self.file_plans
            .iter()
            .any(|fp| matches!(&fp.outcome, Ok(p) if p.changed > 0))
    }

    fn selected_code_name(&self) -> String {
        let Some(game) = self.selected_game() else {
            return String::new();
        };
        let Some(idx) = self.selected_code else {
            return String::new();
        };
        let Some(code) = self.db.find_by_serial(&game.serial).and_then(|g| g.codes.get(idx))
        else {
            return String::new();
        };
        let base = if code.name.trim().is_empty() {
            "(unnamed code)".to_string()
        } else {
            code.name.clone()
        };
        let slot = if code.group.trim().is_empty() {
            String::new()
        } else {
            format!(" [{}]", code.group.trim())
        };
        if self.planned.len() > 1 {
            format!("{base}{slot} - all {} save slots", self.planned.len())
        } else {
            format!("{base}{slot}")
        }
    }

    /// Write every plan that has changes, backing each file up first.
    ///
    /// Sets the bottom-bar status and pops a short toast (with a chime) so an
    /// apply that skipped the dialog still tells you it happened.
    fn apply_all(&mut self, ctx: &egui::Context) {
        let mut written = 0usize;
        let mut backups = 0usize;
        let mut missed = 0usize;
        let mut failed = Vec::new();
        let backup = self.backup_enabled;

        for fp in &self.file_plans {
            match &fp.outcome {
                Ok(plan) if plan.changed == 0 => {
                    if plan.search_missed() {
                        missed += 1;
                    }
                    continue;
                }
                Ok(plan) => match workflow::commit(plan, backup) {
                    Ok(Some(_)) => {
                        backups += 1;
                        written += 1;
                    }
                    Ok(None) => written += 1,
                    Err(e) => failed.push(e),
                },
                Err(e) => failed.push(e.clone()),
            }
        }

        let error = if !failed.is_empty() {
            Some(failed.join("\n"))
        } else if written == 0 && missed > 0 {
            Some("Not applied: the code's search pattern was not found in this save.".to_string())
        } else {
            None
        };
        let status = if error.is_none() {
            if backup {
                Some(format!(
                    "Applied to {written} file(s); wrote {backups} backup file(s)."
                ))
            } else {
                Some(format!(
                    "Applied to {written} file(s); backups were turned off."
                ))
            }
        } else {
            None
        };

        // Recompute so the shown diff reflects the file on disk now.
        let planned = self.planned.clone();
        if !planned.is_empty() {
            self.plan_many(&planned);
        }
        self.status = status;
        self.error = error;

        self.toast = Some(self.apply_toast(ctx, written, backup));
    }

    /// The message a finished apply should pop up, and the chime that goes with
    /// it. Errors do not chime.
    fn apply_toast(&self, ctx: &egui::Context, written: usize, backup: bool) -> Toast {
        let now = ctx.input(|i| i.time);
        if let Some(err) = &self.error {
            let first = err.lines().next().unwrap_or("unknown error").to_string();
            return Toast {
                text: format!("Could not apply: {first}"),
                kind: ToastKind::Error,
                shown_at: now,
            };
        }
        if written == 0 {
            return Toast {
                text: "Nothing to write, already applied".to_string(),
                kind: ToastKind::Info,
                shown_at: now,
            };
        }
        sound::play_done();
        let what = if written == 1 {
            "1 file".to_string()
        } else {
            format!("{written} files")
        };
        Toast {
            text: if backup {
                format!("Applied to {what}, backup saved")
            } else {
                format!("Applied to {what}")
            },
            kind: ToastKind::Ok,
            shown_at: now,
        }
    }

    /// Draw the toast, if one is showing, and clear it when its time is up.
    fn show_toast(&mut self, ctx: &egui::Context) {
        let now = ctx.input(|i| i.time);
        let Some(toast) = &self.toast else {
            return;
        };
        let shown_at = toast.shown_at;
        let kind = toast.kind;
        let text = toast.text.clone();

        let life = if kind == ToastKind::Error { 6.0 } else { 3.2 };
        let elapsed = now - shown_at;
        if elapsed >= life {
            self.toast = None;
            return;
        }

        let (fill, stroke, ink) = match kind {
            ToastKind::Ok => (
                egui::Color32::from_rgb(30, 86, 46),
                egui::Color32::from_rgb(80, 180, 110),
                egui::Color32::from_rgb(235, 255, 240),
            ),
            ToastKind::Info => (
                egui::Color32::from_rgb(48, 60, 78),
                egui::Color32::from_rgb(120, 150, 190),
                egui::Color32::from_rgb(235, 245, 255),
            ),
            ToastKind::Error => (
                egui::Color32::from_rgb(96, 40, 40),
                egui::Color32::from_rgb(200, 90, 90),
                egui::Color32::from_rgb(255, 225, 225),
            ),
        };

        // Hold at full strength, then fade out over the last sixth of a second.
        let fade = ((life - elapsed) / 0.6).clamp(0.0, 1.0) as f32;

        egui::Area::new(egui::Id::new("apply_toast"))
            .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 56.0))
            .interactable(false)
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::none()
                    .fill(with_alpha(fill, fade))
                    .stroke(egui::Stroke::new(1.0_f32, with_alpha(stroke, fade)))
                    .rounding(egui::Rounding::same(8.0))
                    .inner_margin(egui::Margin::symmetric(16.0, 9.0))
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(&text)
                                .color(with_alpha(ink, fade))
                                .size(15.0)
                                .strong(),
                        );
                    });
            });
        // Keep repainting while the toast is up so it disappears on time.
        ctx.request_repaint_after(std::time::Duration::from_millis(60));
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("menu").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Home").clicked() {
                    self.view = View::MainMenu;
                }
                if ui.button("Quick Codes").clicked() {
                    self.view = View::QuickCodes;
                }
                if ui.button("Advanced Hex Editor").clicked() {
                    self.view = View::Advanced;
                }
                ui.separator();
                ui.checkbox(&mut self.backup_enabled, "Back up before writing")
                    .on_hover_text(
                        "Save a .bak copy of each file before it is changed. \
                         Turn this off to write in place without a backup.",
                    );
                ui.separator();
                match &self.save_root {
                    Some(f) => ui.label(format!("Save folder: {}", f.display())),
                    None => ui.label("No save folder selected"),
                };
            });
        });

        match self.view {
            View::MainMenu => self.show_main_menu(ctx),
            View::QuickCodes => self.show_quick_codes(ctx),
            View::Advanced => self.show_advanced(ctx),
        }

        self.show_toast(ctx);
    }
}

impl App {
    fn show_main_menu(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(60.0);
                ui.heading("RPCS3 Save Editor");
                ui.add_space(8.0);
                ui.label("Choose your RPCS3 save-data folder to browse pre-made codes.");
                ui.add_space(20.0);

                if ui
                    .add_sized([280.0, 36.0], egui::Button::new("Choose Save Folder..."))
                    .clicked()
                {
                    if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                        self.scan_save_folder(&folder);
                        self.save_root = Some(folder);
                        self.selected_game = None;
                        self.selected_code = None;
                        self.file_plans.clear();
                        self.confirm_open = false;
                        self.view = View::QuickCodes;
                    }
                }

                ui.add_space(8.0);
                if ui
                    .add_sized([280.0, 36.0], egui::Button::new("Advanced Hex Editor"))
                    .clicked()
                {
                    self.view = View::Advanced;
                }

                ui.add_space(30.0);
                ui.label(format!(
                    "Built-in code database: {} games, {} quick codes",
                    self.db.total_games, self.db.total_codes
                ));
                ui.label(
                    "Codes are applied with a dry-run preview and your save is backed up (as .bak) before any change.",
                );
            });
        });
    }

    fn show_quick_codes(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("games")
            .resizable(true)
            .default_width(340.0)
            .show(ctx, |ui| {
                ui.heading("Your Games");
                if ui.button("Choose Save Folder...").clicked() {
                    if let Some(folder) = rfd::FileDialog::new().pick_folder() {
                        self.scan_save_folder(&folder);
                        self.save_root = Some(folder);
                        self.selected_game = None;
                        self.selected_code = None;
                        self.file_plans.clear();
                        self.confirm_open = false;
                    }
                }
                ui.separator();

                if self.games.is_empty() {
                    ui.label("No saves found. Select your RPCS3 save-data folder.");
                } else {
                    let mut clicked: Option<usize> = None;
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        for (idx, g) in self.games.iter().enumerate() {
                            let label = format!("{} ({})", g.name, g.serial);
                            let selected = self.selected_game == Some(idx);
                            if ui.selectable_label(selected, label).clicked() {
                                clicked = Some(idx);
                            }
                        }
                    });
                    if let Some(idx) = clicked {
                        self.load_game(idx);
                    }
                }
            });

        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                if let Some(err) = &self.error {
                    ui.colored_label(
                        egui::Color32::from_rgb(220, 90, 90),
                        format!("Cannot apply: {err}"),
                    );
                } else if let Some(s) = &self.status {
                    ui.colored_label(egui::Color32::from_rgb(90, 190, 90), s);
                } else {
                    ui.weak(
                        "Pick a quick code and press Apply. Your save is backed up (.bak) before it is changed.",
                    );
                }
            });
            ui.separator();
            ui.label(egui::RichText::new("drDOOM69GAMING").weak());
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            let Some(game) = self.selected_game() else {
                ui.vertical_centered(|ui| {
                    ui.add_space(80.0);
                    ui.label("Select a game on the left.");
                });
                return;
            };

            let serial = game.serial.clone();
            let name = game.name.clone();
            let dir_count = game.dirs.len();

            ui.horizontal(|ui| {
                ui.heading(&name);
                ui.label(format!("({serial})"));
                ui.label(format!("{dir_count} save folder(s)"));
            });

            let codes = self.codes_for_selected();
            ui.label(format!("{} quick codes", codes.len()));
            ui.separator();
            if codes.is_empty() {
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(format!("No quick codes are available for {name} yet."))
                        .strong(),
                );
                ui.weak(
                    "This game has no codes in the bundled database, so there is nothing to \
                     apply. You can still inspect its files in the Advanced Hex Editor.",
                );
                return;
            }
            ui.add(
                egui::TextEdit::singleline(&mut self.search_code)
                    .hint_text("Search quick codes..."),
            );
            ui.add_space(4.0);

            let needle = self.search_code.to_lowercase();

            // Build a collapsible tree from each code's group path. Groups are
            // recorded as paths (`CHARACTERS\SLOT2 Ogony\CLASS`), so every level
            // becomes its own open/close section. Codes with no group are drawn
            // at the top. This keeps identically named per-slot codes apart.
            let mut root = GroupNode::default();
            for (idx, code) in codes.iter().enumerate() {
                if !needle.is_empty()
                    && !code.name.to_lowercase().contains(&needle)
                    && !code.group.to_lowercase().contains(&needle)
                    && !code.offset.to_lowercase().contains(&needle)
                    && !code.value.to_lowercase().contains(&needle)
                {
                    continue;
                }
                root.insert(&db::group_path(&code.group), idx);
            }

            let searching = !needle.is_empty();
            let mut clicked: Option<usize> = None;
            let mut apply_clicked: Option<usize> = None;
            let mut apply_slots: Option<usize> = None;
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.push_id(serial.as_str(), |ui| {
                    self.show_code_tree(
                        ui,
                        &root,
                        &codes,
                        searching,
                        &mut clicked,
                        &mut apply_clicked,
                        &mut apply_slots,
                    );
                });
            });
            if let Some(idx) = clicked {
                self.select_code(idx);
            }
            if let Some(idx) = apply_clicked {
                if self.backup_enabled {
                    self.open_apply(idx);
                } else {
                    // Backups are off, so there is nothing to confirm: apply and
                    // show the toast instead of asking for another click.
                    self.select_code(idx);
                    self.apply_all(ctx);
                }
            }
            if let Some(idx) = apply_slots {
                if self.backup_enabled {
                    self.open_apply_slots(idx);
                } else {
                    self.plan_slots(idx);
                    self.apply_all(ctx);
                }
            }
        });

        self.show_confirm(ctx);
    }

    /// Draw a group subtree: the codes that live directly at this path, then
    /// each deeper group as its own collapsible section. When `expanded` is set
    /// (while a search is active) every section is forced open.
    #[allow(clippy::too_many_arguments)]
    fn show_code_tree(
        &mut self,
        ui: &mut egui::Ui,
        node: &GroupNode,
        all: &[db::Code],
        expanded: bool,
        clicked: &mut Option<usize>,
        apply_clicked: &mut Option<usize>,
        apply_slots: &mut Option<usize>,
    ) {
        for &idx in &node.codes {
            self.code_card(ui, idx, all, clicked, apply_clicked, apply_slots);
        }
        for (name, child) in &node.children {
            let count = child.total_codes();
            let heading = format!(
                "{}  ({} code{})",
                name,
                count,
                if count == 1 { "" } else { "s" }
            );
            let header = egui::CollapsingHeader::new(heading);
            let header = if expanded {
                header.open(Some(true))
            } else {
                header.default_open(false)
            };
            header.show(ui, |ui| {
                self.show_code_tree(ui, child, all, expanded, clicked, apply_clicked, apply_slots);
            });
        }
    }

    /// Draw one quick-code card: name, save-slot label, the raw code lines, an
    /// Apply button, an "All slots" button when the code is one of a save-slot
    /// set, and the inline dry-run preview when it is the selected code.
    fn code_card(
        &mut self,
        ui: &mut egui::Ui,
        idx: usize,
        all: &[db::Code],
        clicked: &mut Option<usize>,
        apply_clicked: &mut Option<usize>,
        apply_slots: &mut Option<usize>,
    ) {
        let Some(code) = all.get(idx) else {
            return;
        };

        // Codes that target one of several save slots come in sets with identical
        // names; offer to patch every matching slot in one go.
        let key = db::slot_key(&code.group);
        let slot_siblings = key.is_some()
            && all
                .iter()
                .filter(|c| {
                    c.name == code.name
                        && c.file == code.file
                        && db::slot_key(&c.group) == key
                })
                .count()
                > 1;

        ui.group(|ui| {
            ui.horizontal(|ui| {
                let title = if code.name.is_empty() {
                    "(unnamed code)"
                } else {
                    code.name.as_str()
                };
                if ui
                    .selectable_label(self.selected_code == Some(idx), title)
                    .clicked()
                {
                    *clicked = Some(idx);
                }
                if !code.group.trim().is_empty() {
                    ui.label(
                        egui::RichText::new(format!("[{}]", code.group.trim()))
                            .color(egui::Color32::from_rgb(120, 170, 230)),
                    );
                }
                if let Some(reason) = &code.unsupported {
                    let label = if workflow::is_choice_reason(reason) {
                        "needs a value"
                    } else {
                        "not supported"
                    };
                    ui.label(
                        egui::RichText::new(label).color(egui::Color32::from_rgb(200, 120, 40)),
                    )
                    .on_hover_text(reason);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (fill, hover) = match &code.unsupported {
                        Some(reason) => (egui::Color32::from_rgb(90, 90, 90), reason.clone()),
                        None => (
                            egui::Color32::from_rgb(40, 120, 60),
                            "Preview the exact changes, then apply".to_string(),
                        ),
                    };
                    if ui
                        .add(egui::Button::new("  Apply  ").fill(fill))
                        .on_hover_text(hover)
                        .clicked()
                    {
                        *apply_clicked = Some(idx);
                    }
                    if slot_siblings
                        && ui
                            .add(egui::Button::new("  All slots  "))
                            .on_hover_text("Apply this code to every save slot at once")
                            .clicked()
                    {
                        *apply_slots = Some(idx);
                    }
                    if !code.file.is_empty() {
                        ui.label(egui::RichText::new(&code.file).weak());
                    }
                });
            });
            for line in &code.lines {
                ui.label(egui::RichText::new(line).monospace());
            }

            // A cheat that never names its file needs one chosen, otherwise
            // there is nothing to write to.
            if code.file.trim().is_empty() {
                let root = self.save_root.clone().unwrap_or_default();
                let serial = self.selected_game().map(|g| g.serial.clone()).unwrap_or_default();
                let files = if serial.is_empty() {
                    Vec::new()
                } else {
                    workflow::candidate_files(&root, &serial)
                };
                ui.horizontal_wrapped(|ui| {
                    ui.label("Save file:");
                    if files.is_empty() {
                        ui.weak("No save files found for this game under the chosen folder.");
                        return;
                    }
                    let current = self.picked_files.get(&idx).cloned().unwrap_or_default();
                    let label = if current.is_empty() {
                        "(choose)".to_string()
                    } else {
                        current
                            .rsplit(['\\', '/'])
                            .next()
                            .unwrap_or(&current)
                            .to_string()
                    };
                    egui::ComboBox::from_id_salt(("pick", idx))
                        .selected_text(label)
                        .show_ui(ui, |ui| {
                            for path in &files {
                                let text = path
                                    .strip_prefix(&root)
                                    .unwrap_or(path)
                                    .display()
                                    .to_string();
                                if ui.selectable_label(current == text, &text).clicked() {
                                    self.picked_files.insert(idx, text.clone());
                                    self.plan_many(&[idx]);
                                }
                            }
                        });
                });
            }

            // A cheat whose listing leaves a value blank needs one typed in
            // before it can be previewed or applied.
            if workflow::needs_value(&workflow::code_lines(code)) {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Value (hex):");
                    let mut value = self.choice_values.get(&idx).cloned().unwrap_or_default();
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut value)
                                .desired_width(160.0)
                                .hint_text("e.g. 63"),
                        )
                        .changed()
                    {
                        if value.trim().is_empty() {
                            self.choice_values.remove(&idx);
                        } else {
                            self.choice_values.insert(idx, value.clone());
                        }
                        if self.selected_code == Some(idx) {
                            self.plan_many(&[idx]);
                        }
                    }
                    match self.choice_values.get(&idx).map(|v| v.trim()) {
                        None | Some("") => {
                            ui.weak("Enter a value to preview this cheat.");
                        }
                        Some(v) => match workflow::resolve_choice(code, v) {
                            Ok(filled) => {
                                let shown = workflow::code_lines(&filled);
                                for line in shown.iter().rev().take(2) {
                                    ui.label(egui::RichText::new(format!("becomes: {line}")).monospace());
                                }
                            }
                            Err(e) => {
                                ui.colored_label(egui::Color32::from_rgb(220, 90, 90), e);
                            }
                        },
                    }
                });
            }

            if self.selected_code == Some(idx) {
                ui.separator();
                self.show_plan_summary(ui, 60, false);
            }
        });
    }

    /// Render the dry-run diff for every target file. Shared by the inline
    /// preview and the confirmation dialog.
    ///
    /// `note_noop` decides whether a file that would not change says so. The
    /// inline preview leaves it out so the list stays quiet; the dialog, where
    /// the user is about to press Apply, includes it.
    fn show_plan_summary(&self, ui: &mut egui::Ui, max_changes: usize, note_noop: bool) {
        if let Some(err) = &self.error {
            ui.colored_label(
                egui::Color32::from_rgb(220, 90, 90),
                format!("Cannot apply: {err}"),
            );
            return;
        }
        if self.file_plans.is_empty() {
            ui.weak("No matching save file.");
            return;
        }

        for fp in &self.file_plans {
            ui.separator();
            ui.label(
                egui::RichText::new(fp.path.display().to_string())
                    .strong()
                    .monospace(),
            );
            match &fp.outcome {
                Err(e) => {
                    ui.colored_label(
                        egui::Color32::from_rgb(200, 120, 60),
                        format!("cannot apply: {e}"),
                    );
                }
                Ok(plan) => {
                    if plan.changed == 0 {
                        if plan.search_missed() {
                            ui.colored_label(
                                egui::Color32::from_rgb(200, 140, 60),
                                "Not applied: this code's search pattern was not found in this save.",
                            );
                        } else if note_noop {
                            ui.colored_label(
                                egui::Color32::from_rgb(150, 170, 200),
                                "Nothing to change: this save already holds the values this \
                                 code writes, so the cheat is already in effect.",
                            );
                        }
                    } else {
                        ui.label(format!(
                            "{} -> {} bytes, {} byte(s) changed",
                            plan.before_len,
                            plan.after_len(),
                            plan.changed
                        ));
                    }
                    for change in plan.changes.iter().take(max_changes) {
                        let before = change
                            .before
                            .map(|b| format!("{b:02X}"))
                            .unwrap_or_else(|| "--".to_string());
                        let after = change
                            .after
                            .map(|b| format!("{b:02X}"))
                            .unwrap_or_else(|| "--".to_string());
                        ui.label(
                            egui::RichText::new(format!(
                                "  0x{:08X}: {} -> {}",
                                change.offset, before, after
                            ))
                            .monospace(),
                        );
                    }
                    if plan.changed > plan.changes.len() {
                        ui.label(format!(
                            "  ... {} more change(s)",
                            plan.changed - plan.changes.len()
                        ));
                    }
                    for warning in &plan.warnings {
                        ui.colored_label(
                            egui::Color32::from_rgb(200, 160, 60),
                            format!("  ! {warning}"),
                        );
                    }
                }
            }
        }
    }

    /// The apply-confirmation dialog. Shows the dry-run diff and only writes
    /// after the user presses the apply button.
    fn show_confirm(&mut self, ctx: &egui::Context) {
        if !self.confirm_open {
            return;
        }

        let title = format!("Apply: {}", self.selected_code_name());
        let mut open = true;
        let mut do_apply = false;
        let mut close = false;

        egui::Window::new(title)
            .collapsible(false)
            .resizable(true)
            .default_width(780.0)
            .open(&mut open)
            .show(ctx, |ui| {
                if let Some(status) = &self.status {
                    ui.colored_label(egui::Color32::from_rgb(90, 190, 90), status);
                }
                if self.error.is_none() {
                    if self.backup_enabled {
                        ui.label(
                            "Review the exact bytes below. The original file is backed up as \
                             <filename>.bak before anything is written.",
                        );
                    } else {
                        ui.colored_label(
                            egui::Color32::from_rgb(210, 150, 60),
                            "Review the exact bytes below. Backups are turned off: the file is \
                             written in place with no .bak copy.",
                        );
                    }
                    ui.checkbox(
                        &mut self.backup_enabled,
                        "Back up the original as <filename>.bak before writing",
                    );
                }

                egui::ScrollArea::vertical()
                    .max_height(380.0)
                    .show(ui, |ui| {
                        self.show_plan_summary(ui, 300, true);
                    });

                if self.slot_status.len() > 1 {
                    let will_change = self
                        .slot_status
                        .iter()
                        .filter(|(_, s)| *s == SlotState::WillChange)
                        .count();
                    let already = self
                        .slot_status
                        .iter()
                        .filter(|(_, s)| *s == SlotState::Already)
                        .count();
                    let not_found = self.slot_status.len() - will_change - already;
                    ui.separator();
                    let mut line = format!(
                        "Save slots: {will_change} will change, {already} already match"
                    );
                    if not_found > 0 {
                        line.push_str(&format!(", {not_found} not found"));
                    }
                    line.push('.');
                    ui.label(line);
                    ui.horizontal_wrapped(|ui| {
                        for (label, state) in &self.slot_status {
                            let (hint, colour) = match state {
                                SlotState::WillChange => (
                                    "this slot will be changed",
                                    egui::Color32::from_rgb(120, 190, 120),
                                ),
                                SlotState::Already => (
                                    "this slot already holds the target values, so it will not change",
                                    egui::Color32::from_gray(140),
                                ),
                                SlotState::NotFound => (
                                    "this code's search pattern was not found in this slot, so it will not change",
                                    egui::Color32::from_rgb(200, 140, 60),
                                ),
                            };
                            ui.label(egui::RichText::new(format!("[{label}]")).color(colour))
                                .on_hover_text(hint);
                        }
                    });
                }

                ui.separator();
                ui.horizontal(|ui| {
                    if self.error.is_none() && self.has_changes() {
                        let label = if self.backup_enabled {
                            "Apply with backup"
                        } else {
                            "Apply without backup"
                        };
                        if ui
                            .add(
                                egui::Button::new(label)
                                    .fill(egui::Color32::from_rgb(40, 130, 70)),
                            )
                            .clicked()
                        {
                            do_apply = true;
                        }
                    }
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
            });

        if do_apply {
            self.apply_all(ctx);
            // A successful apply closes the dialog; the toast reports it.
            if self.error.is_none() {
                self.confirm_open = false;
            }
        }
        if close || !open {
            self.confirm_open = false;
        }
    }

    fn show_advanced(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("advanced_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Open Save File...").clicked() {
                    if let Some(path) = rfd::FileDialog::new().pick_file() {
                        if let Ok(data) = std::fs::read(&path) {
                            self.hex_view_data = data;
                            self.save_file = Some(path);
                        }
                    }
                }
                ui.separator();
                match &self.save_file {
                    Some(p) => ui.label(format!("{}", p.display())),
                    None => ui.label("No file loaded"),
                };
                ui.separator();
                ui.label(format!("{} bytes", self.hex_view_data.len()));
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            if self.hex_view_data.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add_space(80.0);
                    ui.label("Open a save file to view its hex data.");
                });
                return;
            }

            egui::ScrollArea::both().show(ui, |ui| {
                for (i, chunk) in self.hex_view_data.chunks(16).take(20000).enumerate() {
                    let mut line = format!("{:08X} | ", i * 16);
                    for b in chunk {
                        line.push_str(&format!("{:02X} ", b));
                    }
                    while line.len() < 60 {
                        line.push(' ');
                    }
                    line.push_str("| ");
                    for b in chunk {
                        if (32..127).contains(b) {
                            line.push(*b as char);
                        } else {
                            line.push('.');
                        }
                    }
                    ui.label(egui::RichText::new(&line).monospace());
                }
            });
        });
    }
}
