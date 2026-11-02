//! Built-in file picker (SPEC section 7, rule 2).
//!
//! Drawn with egui itself instead of a system-dialog library: on Linux the
//! usual libraries reach the desktop portal over D-Bus, which would bring
//! network-capable code into the binary. The picker only lists folders and
//! returns the paths the user explicitly chose; nothing is opened here.
//!
//! Names are shown with control and bidi characters replaced, so a file
//! called "photo\u{202e}gpj.exe" can't pose as "photoexe.jpg".

use eframe::egui;
use std::path::{Path, PathBuf};

/// Most entries listed in one folder (huge folders stay responsive).
const MAX_ENTRIES: usize = 5_000;
/// Most directory entries read per folder (most may be non-photos).
const MAX_SCANNED: usize = 50_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Pick one or more photos.
    Photos,
    /// Pick a project (a `.printnest` folder or its project.json).
    Project,
    /// Choose a folder and a name to save a new project into.
    Save,
    /// Choose a folder (exports go into a new folder inside it).
    Folder,
}

#[derive(Debug)]
pub enum Outcome {
    Pending,
    Cancelled,
    Picked(Vec<PathBuf>),
    SaveTo { parent: PathBuf, name: String },
}

#[derive(Debug, Clone)]
struct Entry {
    name: String,
    display: String,
    path: PathBuf,
    is_dir: bool,
    is_project: bool,
}

#[derive(Debug)]
pub struct Picker {
    mode: Mode,
    dir: PathBuf,
    entries: Vec<Entry>,
    truncated: bool,
    error: Option<String>,
    selected: Vec<PathBuf>,
    name: String,
    path_text: String,
    show_hidden: bool,
    places: Vec<(String, PathBuf)>,
}

/// Replaces characters that could make a name display misleadingly.
#[must_use]
pub fn display_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            let invisible = c.is_control() || printnest_model::safe_path::is_deceptive(c);
            if invisible { '\u{FFFD}' } else { c }
        })
        .collect()
}

fn is_photo(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    [".jpg", ".jpeg", ".png", ".webp", ".tif", ".tiff"]
        .iter()
        .any(|e| lower.ends_with(e))
}

/// The user's home folder, if the OS says where it is.
#[must_use]
pub fn home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
}

impl Picker {
    #[must_use]
    pub fn new(mode: Mode, start: Option<PathBuf>) -> Self {
        let dir = start
            .filter(|p| p.is_dir())
            .or_else(|| {
                home().map(|h| {
                    let pics = h.join("Pictures");
                    if mode == Mode::Photos && pics.is_dir() {
                        pics
                    } else {
                        h
                    }
                })
            })
            .unwrap_or_else(|| PathBuf::from("/"));
        let mut p = Picker {
            mode,
            dir: dir.clone(),
            entries: Vec::new(),
            truncated: false,
            error: None,
            selected: Vec::new(),
            name: String::from("My project"),
            path_text: dir.display().to_string(),
            show_hidden: false,
            places: Self::compute_places(),
        };
        p.refresh();
        p
    }

    fn go(&mut self, dir: PathBuf) {
        self.dir = dir;
        self.path_text = self.dir.display().to_string();
        self.selected.clear();
        self.refresh();
    }

    fn refresh(&mut self) {
        self.entries.clear();
        let mut scanned = 0usize;
        self.truncated = false;
        self.error = None;
        let read = match std::fs::read_dir(&self.dir) {
            Ok(r) => r,
            Err(e) => {
                self.error = Some(format!("Can't open this folder: {e}"));
                return;
            }
        };
        for item in read.flatten() {
            scanned += 1;
            if self.entries.len() >= MAX_ENTRIES || scanned > MAX_SCANNED {
                self.truncated = true;
                break;
            }
            let name = item.file_name().to_string_lossy().into_owned();
            if !self.show_hidden && name.starts_with('.') {
                continue;
            }
            let path = item.path();
            // metadata() follows symlinks: a link to a folder lists as a folder.
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            let is_dir = meta.is_dir();
            let is_project = is_dir && name.to_ascii_lowercase().ends_with(".printnest");
            let keep = match self.mode {
                Mode::Photos => is_dir || (meta.is_file() && is_photo(&name)),
                Mode::Project => is_dir || (meta.is_file() && name == "project.json"),
                Mode::Save | Mode::Folder => is_dir,
            };
            if keep {
                self.entries.push(Entry {
                    display: display_name(&name),
                    name,
                    path,
                    is_dir,
                    is_project,
                });
            }
        }
        self.entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
    }

    /// Shortcut folders, worked out once per dialog (checking drive
    /// letters can be slow on Windows, e.g. a disconnected network drive).
    fn compute_places() -> Vec<(String, PathBuf)> {
        let mut out = Vec::new();
        if let Some(h) = home() {
            out.push(("Home".to_owned(), h.clone()));
            for sub in ["Desktop", "Pictures", "Documents", "Downloads"] {
                let p = h.join(sub);
                if p.is_dir() {
                    out.push((sub.to_owned(), p));
                }
            }
        }
        if cfg!(windows) {
            for letter in 'C'..='Z' {
                let p = PathBuf::from(format!("{letter}:\\"));
                if p.is_dir() {
                    out.push((format!("{letter}:"), p));
                }
            }
        } else {
            out.push(("Computer".to_owned(), PathBuf::from("/")));
        }
        out
    }

    /// Draws the picker. Call every frame until it returns something other
    /// than [`Outcome::Pending`].
    pub fn show(&mut self, ctx: &egui::Context) -> Outcome {
        let mut outcome = Outcome::Pending;
        let title = match self.mode {
            Mode::Photos => "Add photos",
            Mode::Project => "Open project",
            Mode::Save => "Save project",
            Mode::Folder => "Choose where to export",
        };
        let modal = egui::Modal::new(egui::Id::unique("printnest-picker")).show(ctx, |ui| {
            ui.set_width(660.0);
            ui.horizontal(|ui| {
                ui.heading(title);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Cancel").clicked() {
                        outcome = Outcome::Cancelled;
                    }
                });
            });
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Up").clicked()
                    && let Some(parent) = self.dir.parent().map(Path::to_path_buf)
                {
                    self.go(parent);
                }
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut self.path_text)
                        .desired_width(ui.available_width() - 90.0),
                );
                if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    let p = PathBuf::from(self.path_text.trim());
                    if p.is_dir() {
                        self.go(p);
                    } else {
                        self.error = Some("That folder doesn't exist.".into());
                    }
                }
                if ui.checkbox(&mut self.show_hidden, "Hidden").changed() {
                    self.refresh();
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                ui.set_height(300.0);
                ui.vertical(|ui| {
                    ui.set_width(110.0);
                    for (label, path) in self.places.clone() {
                        if ui.selectable_label(self.dir == path, label).clicked() {
                            self.go(path);
                        }
                    }
                });
                ui.separator();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .max_height(300.0)
                    .show(ui, |ui| {
                        ui.vertical(|ui| {
                            if let Some(e) = &self.error {
                                ui.colored_label(egui::Color32::from_rgb(210, 60, 50), e);
                            }
                            let mut go_to = None;
                            let mut finish = None;
                            for entry in &self.entries {
                                let label = if entry.is_project {
                                    format!("{}   (project)", entry.display)
                                } else if entry.is_dir {
                                    format!("{}/", entry.display)
                                } else {
                                    entry.display.clone()
                                };
                                let selected = self.selected.contains(&entry.path);
                                let r = ui.selectable_label(selected, label);
                                if r.double_clicked() {
                                    if entry.is_project && self.mode == Mode::Project {
                                        finish = Some(vec![entry.path.clone()]);
                                    } else if entry.is_dir {
                                        go_to = Some(entry.path.clone());
                                    } else {
                                        finish = Some(vec![entry.path.clone()]);
                                    }
                                } else if r.clicked() && (!entry.is_dir || entry.is_project) {
                                    // Photos: each click toggles (no modifier keys
                                    // needed). Projects: single choice.
                                    if self.mode == Mode::Photos {
                                        if selected {
                                            self.selected.retain(|p| p != &entry.path);
                                        } else {
                                            self.selected.push(entry.path.clone());
                                        }
                                    } else {
                                        self.selected = vec![entry.path.clone()];
                                    }
                                }
                            }
                            if self.truncated {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "Showing the first {MAX_ENTRIES} items."
                                    ))
                                    .weak(),
                                );
                            }
                            if self.entries.is_empty() && self.error.is_none() {
                                ui.label(egui::RichText::new("Nothing here to pick.").weak());
                            }
                            if let Some(d) = go_to {
                                self.go(d);
                            }
                            if let Some(f) = finish {
                                outcome = Outcome::Picked(f);
                            }
                        })
                    });
            });
            ui.separator();
            ui.horizontal(|ui| match self.mode {
                Mode::Save => {
                    ui.label("Project name:");
                    ui.add(egui::TextEdit::singleline(&mut self.name).desired_width(220.0));
                    ui.label(egui::RichText::new(".printnest").weak());
                    if ui.button("Save here").clicked() {
                        outcome = Outcome::SaveTo {
                            parent: self.dir.clone(),
                            name: self.name.clone(),
                        };
                    }
                }
                Mode::Folder => {
                    ui.label(
                        egui::RichText::new(
                            "A new folder is created here; nothing is overwritten.",
                        )
                        .weak(),
                    );
                    if ui.button("Export here").clicked() {
                        outcome = Outcome::Picked(vec![self.dir.clone()]);
                    }
                }
                Mode::Photos => {
                    ui.label(
                        egui::RichText::new("Click photos to tick them. JPEG, PNG, WebP, TIFF.")
                            .weak(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let label = format!("Add {} photo(s)", self.selected.len());
                        if ui
                            .add_enabled(!self.selected.is_empty(), egui::Button::new(label))
                            .clicked()
                        {
                            outcome = Outcome::Picked(self.selected.clone());
                        }
                        if ui.button("Select all").clicked() {
                            self.selected = self
                                .entries
                                .iter()
                                .filter(|e| !e.is_dir)
                                .map(|e| e.path.clone())
                                .collect();
                        }
                    });
                }
                Mode::Project => {
                    ui.label(egui::RichText::new("Pick a .printnest project.").weak());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(!self.selected.is_empty(), egui::Button::new("Open"))
                            .clicked()
                        {
                            outcome = Outcome::Picked(self.selected.clone());
                        }
                    });
                }
            });
        });
        if modal.should_close() {
            return Outcome::Cancelled;
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_names_neutralise_tricks() {
        assert_eq!(display_name("photo\u{202e}gpj.exe"), "photo\u{FFFD}gpj.exe");
        assert_eq!(display_name("a\u{200b}b\nc"), "a\u{FFFD}b\u{FFFD}c");
        assert_eq!(display_name("Été 2026.jpg"), "Été 2026.jpg");
    }

    #[test]
    fn only_photo_extensions_listed() {
        for ok in ["a.jpg", "B.JPEG", "c.png", "d.webp", "e.tif", "f.TIFF"] {
            assert!(is_photo(ok), "{ok}");
        }
        for bad in ["a.gif", "b.exe", "c.jpg.exe", "photo", "svg.svg"] {
            assert!(!is_photo(bad), "{bad}");
        }
    }

    #[test]
    fn lists_a_folder_with_filters() {
        // nosemgrep: rust.lang.security.temp-dir.temp-dir -- test-only scratch folder
        let d = std::env::temp_dir().join(format!("printnest-picker-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("sub")).unwrap();
        std::fs::create_dir_all(d.join("Book.printnest")).unwrap();
        std::fs::write(d.join("a.jpg"), b"x").unwrap();
        std::fs::write(d.join("notes.txt"), b"x").unwrap();
        std::fs::write(d.join(".hidden.jpg"), b"x").unwrap();
        let p = Picker::new(Mode::Photos, Some(d.clone()));
        let names: Vec<&str> = p.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["Book.printnest", "sub", "a.jpg"]);
        let p = Picker::new(Mode::Save, Some(d.clone()));
        assert!(p.entries.iter().all(|e| e.is_dir));
        let p = Picker::new(Mode::Project, Some(d.clone()));
        assert!(p.entries.iter().any(|e| e.is_project));
        let _ = std::fs::remove_dir_all(&d);
    }
}
