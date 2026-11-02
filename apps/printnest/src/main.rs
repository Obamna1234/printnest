//! PrintNest desktop app.
//!
//! Start screen (pick a product) → editor (design it) → save as a project
//! folder. Photos are decoded only in the sandboxed worker. No network I/O.

#![forbid(unsafe_code)]
// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod editor;
mod export_dialog;
mod library;
mod picker;
mod preview;
mod start;

use editor::{Editor, Request};
use eframe::egui;
use library::Origin;
use picker::{Mode, Outcome, Picker, display_name};
use printnest_document::{DocError, ProjectFolder};
use printnest_model::catalogue::Catalogue;
use printnest_model::project::{Page, Project};
use printnest_model::safe_path::{SafePath, is_deceptive};
use start::{StartAction, StartScreen};

fn main() -> eframe::Result {
    // If started as the sandboxed photo decoder, serve one request and exit
    // before any window or GPU code runs.
    printnest_sandbox::maybe_run_worker();
    let catalogue = match Catalogue::builtin() {
        Ok(c) => c,
        Err(e) => return Err(eframe::Error::AppCreation(Box::new(e))),
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("PrintNest")
            .with_inner_size([1280.0, 820.0])
            .with_min_inner_size([900.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native(
        "PrintNest",
        options,
        Box::new(|_cc| Ok(Box::new(App::new(catalogue)))),
    )
}

/// What to do once the user confirms discarding unsaved changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AfterDiscard {
    New,
    Open,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PickerFor {
    Photos,
    Open,
    SaveAs,
    Export,
}

#[derive(Debug)]
struct App {
    catalogue: Catalogue,
    start: StartScreen,
    editor: Option<Editor>,
    picker: Option<(PickerFor, Picker)>,
    confirm: Option<AfterDiscard>,
    message: Option<String>,
    last_dir: Option<std::path::PathBuf>,
    /// The user confirmed closing with unsaved changes.
    closing: bool,
    export: Option<export_dialog::ExportDialog>,
    /// Autosaved changes offered for restoring after opening a project.
    restore: Option<Project>,
    last_autosave: std::time::Instant,
}

impl App {
    fn new(catalogue: Catalogue) -> Self {
        App {
            start: StartScreen::new(catalogue.clone()),
            catalogue,
            editor: None,
            picker: None,
            confirm: None,
            message: None,
            last_dir: None,
            closing: false,
            export: None,
            restore: None,
            last_autosave: std::time::Instant::now(),
        }
    }

    fn create(&mut self, product: &str, paper: &str, cover: Option<String>, pages: u32) {
        let Some(prod) = self.catalogue.product(product).cloned() else {
            return;
        };
        let built = Project::new_blank(&self.catalogue, product, "Untitled").and_then(|mut p| {
            p.paper = paper.to_owned();
            p.cover = cover;
            p.pages = (0..pages).map(|_| Page::default()).collect();
            p.validate(&self.catalogue).map(|()| p)
        });
        match built {
            Ok(p) => self.editor = Some(Editor::new(p, prod, None)),
            Err(e) => self.message = Some(format!("Couldn't start that project: {e}")),
        }
    }

    fn open_project(&mut self, path: &std::path::Path) {
        match printnest_document::open(path, &self.catalogue) {
            Ok((folder, project)) => {
                let Some(prod) = self.catalogue.product(&project.product).cloned() else {
                    return;
                };
                self.last_dir = folder.root().parent().map(std::path::Path::to_path_buf);
                // Autosaved changes newer than the last save (e.g. after a crash).
                self.restore = folder
                    .recovery(&self.catalogue)
                    .filter(|r| *r != project && r.product == project.product);
                self.editor = Some(Editor::new(project, prod, Some(folder)));
            }
            Err(e) => self.message = Some(format!("Couldn't open that project: {e}")),
        }
    }

    /// Copies the photos the design uses into `folder` (from disk, or from
    /// the project it was opened from), then writes project.json. A photo
    /// counts as saved only once its copy succeeded; failures are shown.
    fn save_into(&mut self, folder: ProjectFolder) {
        let Some(ed) = self.editor.as_mut() else {
            return;
        };
        let mut problems = Vec::new();
        let mut missing = Vec::new();
        let mut taken = ed.library.keys();
        taken.extend(folder.photo_names());
        let used: Vec<SafePath> = ed.project.photo_paths().into_iter().cloned().collect();
        taken.extend(used.iter().map(|k| k.as_str().to_owned()));
        for key in used {
            let source = match ed.library.origin(key.as_str()) {
                Some(Origin::Project(f)) if *f == folder => continue,
                Some(origin) => origin.path(&key),
                None => Err("not found".to_owned()),
            };
            // A photo already missing from the project it came from can't be
            // copied; that's reported but doesn't block a clean save.
            if let Ok(src) = &source
                && std::fs::symlink_metadata(src).is_err()
            {
                missing.push(display_name(key.as_str()));
                continue;
            }
            let result = source.and_then(|src| import_unique(&folder, &key, &src, &mut taken));
            match result {
                Ok(stored) => {
                    if stored != key {
                        ed.retarget_photo(&key, &stored);
                    }
                    ed.library.saved_as(&key, &stored, &folder);
                }
                Err(e) => problems.push(format!(
                    "{}: {}",
                    display_name(key.as_str()),
                    display_name(&e)
                )),
            }
        }
        // Title from the folder name, minus anything that can't be shown.
        let name: String = folder
            .root()
            .file_stem()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
            .chars()
            .filter(|c| !c.is_control() && !is_deceptive(*c))
            .take(200)
            .collect();
        if !name.trim().is_empty() {
            ed.project.title = name;
        }
        match folder.save(&ed.project, &self.catalogue) {
            Ok(()) => {
                ed.folder = Some(folder);
                if problems.is_empty() {
                    ed.dirty = false;
                    ed.status = "Saved.".into();
                    if !missing.is_empty() {
                        self.message = Some(format!(
                            "Saved. These photos were already missing from the project \
                             and still are:\n\n{}",
                            missing.join("\n")
                        ));
                    }
                } else {
                    // Still unsaved: the next save retries the copies.
                    ed.status = "Saved, but some photos are missing.".into();
                    self.message = Some(format!(
                        "The design was saved, but these photos couldn't be copied \
                         into the project and will be missing when it's reopened:\n\n{}",
                        problems.join("\n")
                    ));
                }
            }
            Err(e) => self.message = Some(format!("Couldn't save: {e}")),
        }
    }

    /// Once a minute, while a saved project has unsaved changes and nothing
    /// else is going on, writes them to the project's recovery file. The
    /// user's own `project.json` is only ever written by Save.
    fn autosave(&mut self) {
        const EVERY: std::time::Duration = std::time::Duration::from_secs(60);
        let idle = self.picker.is_none()
            && self.confirm.is_none()
            && self.message.is_none()
            && self.export.is_none();
        let Some(ed) = self.editor.as_ref() else {
            return;
        };
        if !ed.dirty || ed.busy() || !idle || self.last_autosave.elapsed() < EVERY {
            return;
        }
        self.last_autosave = std::time::Instant::now();
        if let Some(folder) = ed.folder.clone() {
            let result = folder.save_recovery(&ed.project, &self.catalogue);
            if let Some(ed) = self.editor.as_mut() {
                // Never a pop-up from autosave: just the status line.
                ed.status = match result {
                    Ok(()) => "Changes autosaved for recovery (not yet saved).".into(),
                    Err(e) => format!("Autosave failed: {e}"),
                };
            }
        }
    }

    /// The user agreed to throw away unsaved changes: forget the recovery
    /// copy too.
    fn forget_recovery(&self) {
        if let Some(f) = self.editor.as_ref().and_then(|e| e.folder.as_ref()) {
            f.discard_recovery();
        }
    }

    /// `discard`: the user already agreed to lose unsaved changes. The
    /// dirty flag is never cleared here, so cancelling the next dialog
    /// keeps the protection.
    fn handle(&mut self, req: Request, discard: bool) {
        let dirty = !discard && self.editor.as_ref().is_some_and(|e| e.dirty);
        match req {
            Request::New if dirty => self.confirm = Some(AfterDiscard::New),
            Request::New => self.editor = None,
            Request::Open if dirty => self.confirm = Some(AfterDiscard::Open),
            Request::Open => {
                self.picker = Some((
                    PickerFor::Open,
                    Picker::new(Mode::Project, self.last_dir.clone()),
                ))
            }
            Request::Save => {
                if let Some(folder) = self.editor.as_ref().and_then(|e| e.folder.clone()) {
                    self.save_into(folder);
                } else {
                    self.picker = Some((
                        PickerFor::SaveAs,
                        Picker::new(Mode::Save, self.last_dir.clone()),
                    ));
                }
            }
            Request::SaveAs => {
                self.picker = Some((
                    PickerFor::SaveAs,
                    Picker::new(Mode::Save, self.last_dir.clone()),
                ));
            }
            Request::Export => {
                if let Some(ed) = self.editor.as_ref() {
                    self.export = Some(export_dialog::ExportDialog::new(
                        ed.project.clone(),
                        ed.product.clone(),
                        ed.photo_sources(),
                    ));
                }
            }
            Request::AddPhotos => {
                self.picker = Some((
                    PickerFor::Photos,
                    Picker::new(Mode::Photos, self.last_dir.clone()),
                ));
            }
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        if self.picker.is_none()
            && let Some(d) = self.export.as_mut()
        {
            match d.show(ctx) {
                export_dialog::Action::None => {}
                export_dialog::Action::Close => self.export = None,
                export_dialog::Action::PickFolder => {
                    self.picker = Some((
                        PickerFor::Export,
                        Picker::new(Mode::Folder, self.last_dir.clone()),
                    ));
                }
            }
        }
        if let Some((purpose, picker)) = self.picker.as_mut() {
            let purpose = *purpose;
            match picker.show(ctx) {
                Outcome::Pending => {}
                Outcome::Cancelled => self.picker = None,
                Outcome::Picked(paths) => {
                    self.picker = None;
                    self.last_dir = paths
                        .first()
                        .and_then(|p| p.parent())
                        .map(std::path::Path::to_path_buf);
                    match purpose {
                        PickerFor::Photos => {
                            if let Some(ed) = self.editor.as_mut() {
                                ed.add_photo_files(paths);
                            }
                        }
                        PickerFor::Open => {
                            if let Some(p) = paths.first() {
                                self.open_project(&p.clone());
                            }
                        }
                        PickerFor::SaveAs => {}
                        PickerFor::Export => {
                            if let (Some(d), Some(dir)) = (self.export.as_mut(), paths.first()) {
                                d.start(dir.clone());
                            }
                        }
                    }
                }
                Outcome::SaveTo { parent, name } => match ProjectFolder::create(&parent, &name) {
                    Ok(folder) => {
                        self.picker = None;
                        self.last_dir = Some(parent);
                        self.save_into(folder);
                    }
                    Err(DocError::Exists) => {
                        self.message = Some(
                            "A project with that name already exists there. Pick another name."
                                .into(),
                        );
                    }
                    Err(e) => {
                        self.message = Some(format!("Couldn't create the project folder: {e}"))
                    }
                },
            }
        }

        if let Some(after) = self.confirm {
            egui::Modal::new(egui::Id::unique("confirm-discard")).show(ctx, |ui| {
                ui.heading("Unsaved changes");
                ui.label("This design has changes that aren't saved. Discard them?");
                ui.horizontal(|ui| {
                    if ui.button("Discard").clicked() {
                        self.confirm = None;
                        self.forget_recovery();
                        match after {
                            AfterDiscard::New => self.handle(Request::New, true),
                            AfterDiscard::Open => self.handle(Request::Open, true),
                            AfterDiscard::Close => {
                                self.closing = true;
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                        }
                    }
                    if ui.button("Keep editing").clicked() {
                        self.confirm = None;
                    }
                });
            });
        }

        if self.restore.is_some() {
            egui::Modal::new(egui::Id::unique("restore")).show(ctx, |ui| {
                ui.heading("Unsaved changes found");
                ui.label(
                    "This project has changes that were autosaved but never saved \
                     (perhaps the app closed unexpectedly). Restore them?",
                );
                ui.horizontal(|ui| {
                    if ui.button("Restore").clicked()
                        && let (Some(p), Some(ed)) = (self.restore.take(), self.editor.as_mut())
                    {
                        ed.project = p;
                        ed.dirty = true;
                        ed.status = "Restored autosaved changes. Save to keep them.".into();
                    }
                    if ui.button("No, use the saved version").clicked() {
                        self.restore = None;
                        self.forget_recovery();
                    }
                });
            });
        }

        if let Some(msg) = self.message.clone() {
            egui::Modal::new(egui::Id::unique("message")).show(ctx, |ui| {
                ui.set_max_width(420.0);
                ui.label(msg);
                if ui.button("OK").clicked() {
                    self.message = None;
                }
            });
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let title = match &self.editor {
            Some(e) => format!(
                "{}{} — PrintNest",
                if e.dirty { "• " } else { "" },
                display_name(&e.project.title)
            ),
            None => "PrintNest".into(),
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));

        // Closing mid-export would leave half the files written.
        let exporting = self
            .export
            .as_ref()
            .is_some_and(export_dialog::ExportDialog::busy);
        if exporting
            && ctx.input(|i| i.viewport().close_requested())
            && let Some(d) = &self.export
        {
            // Closing stops the export; its folder is cleaned up.
            d.cancel();
        }
        // Closing the window with unsaved changes asks first.
        let dirty = self.editor.as_ref().is_some_and(|e| e.dirty);
        if ctx.input(|i| i.viewport().close_requested()) && dirty && !self.closing {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm = Some(AfterDiscard::Close);
        }

        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(
                        "Offline · no network access · photos are opened in a locked-down process",
                    )
                    .small(),
                );
                if let Some(ed) = &self.editor
                    && !ed.status.is_empty()
                {
                    ui.separator();
                    ui.label(egui::RichText::new(&ed.status).small());
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!("PrintNest {}", env!("CARGO_PKG_VERSION")))
                            .small()
                            .weak(),
                    );
                });
            });
        });

        let mut requests = Vec::new();
        if let Some(ed) = self.editor.as_mut() {
            let modal = self.picker.is_some()
                || self.confirm.is_some()
                || self.message.is_some()
                || self.export.is_some()
                || self.restore.is_some();
            requests = ed.ui(ui, &self.catalogue, modal);
        } else if let Some(action) = self.start.ui(ui) {
            match action {
                StartAction::Create {
                    product,
                    paper,
                    cover,
                    pages,
                } => self.create(&product, &paper, cover, pages),
                StartAction::Open => self.handle(Request::Open, false),
            }
        }
        for r in requests {
            self.handle(r, false);
        }
        self.autosave();
        self.dialogs(&ctx);
    }
}

/// Copies `source` into `folder` as `key`, or the next free name if that
/// name is already used on disk (never reusing an existing file).
fn import_unique(
    folder: &ProjectFolder,
    key: &SafePath,
    source: &std::path::Path,
    taken: &mut std::collections::HashSet<String>,
) -> Result<SafePath, String> {
    let file_name = key.components().last().unwrap_or("photo.jpg").to_owned();
    let mut dest = key.clone();
    for _ in 0..50 {
        match folder.import_photo(&dest, source) {
            Ok(()) => {
                taken.insert(dest.as_str().to_owned());
                return Ok(dest);
            }
            Err(DocError::Exists) => {
                taken.insert(dest.as_str().to_owned());
                dest = printnest_layout::edit::photo_path(&file_name, taken)
                    .ok_or_else(|| "no free file name".to_owned())?;
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("no free file name".into())
}
