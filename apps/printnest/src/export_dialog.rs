//! The Export dialog: pick how you'll print, see what to fix, export.
//!
//! The design check (preflight) and the export both run on a background
//! thread. Photos are read size-capped and decoded only in the sandboxed
//! worker, exactly as in the editor.

use crate::library::Origin;
use eframe::egui;
use printnest_export::{ExportReport, Issue, Paper, Profile, Severity};
use printnest_model::catalogue::{ExportProfile, Product};
use printnest_model::project::Project;
use printnest_model::safe_path::SafePath;
use printnest_render::Photo;
use printnest_sandbox::{Worker, read_capped};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};

const PROFILES: [(Profile, &str, &str); 4] = [
    (
        Profile::Kiosk,
        "Photo kiosk or online photo lab",
        "JPEG files at exactly the right size. Copy them to a USB stick, SD card or phone.",
    ),
    (
        Profile::PrintShop,
        "Print shop",
        "A PDF with bleed and crop marks, ready for professional printing and trimming.",
    ),
    (
        Profile::Home(Paper::A4),
        "Home printer, A4",
        "A PDF to print yourself. Big designs are split over several sheets to tape together.",
    ),
    (
        Profile::Home(Paper::Letter),
        "Home printer, US Letter",
        "As above, on US Letter paper.",
    ),
];

/// Whether `product` can be printed this way (from the catalogue).
fn offered(product: &Product, p: Profile) -> bool {
    let wanted = match p {
        Profile::Kiosk => ExportProfile::KioskJpeg,
        Profile::PrintShop => ExportProfile::PrintPdf,
        Profile::Home(_) => ExportProfile::Home,
    };
    product.exports.contains(&wanted)
}

enum Msg {
    Issues(Vec<Issue>, HashMap<String, (u32, u32)>),
    Progress(usize, usize),
    Done(Result<ExportReport, String>),
}

enum Stage {
    Choosing,
    Exporting { done: usize, total: usize },
    Finished(Result<ExportReport, String>),
}

/// What the dialog wants the app to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    None,
    Close,
    PickFolder,
}

pub struct ExportDialog {
    project: Project,
    product: Product,
    cover: Option<printnest_layout::book::CoverGeometry>,
    /// Where each photo's bytes are; project photos are re-checked on
    /// every read.
    sources: Arc<HashMap<String, Origin>>,
    profile: Profile,
    issues: Option<Vec<Issue>>,
    /// Full-size photo dimensions found by the design check.
    sizes: Arc<HashMap<String, (u32, u32)>>,
    stage: Stage,
    rx: Receiver<Msg>,
    tx: std::sync::mpsc::Sender<Msg>,
    /// Set to stop the export; also set when the dialog is dropped.
    cancel: Arc<AtomicBool>,
}

impl std::fmt::Debug for ExportDialog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExportDialog")
            .field("profile", &self.profile)
            .finish()
    }
}

impl Drop for ExportDialog {
    fn drop(&mut self) {
        // Stops the design check and any export still running.
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn load_bytes(sources: &HashMap<String, Origin>, src: &str) -> Option<Vec<u8>> {
    match sources.get(src)? {
        Origin::External(path) => read_capped(path).ok(),
        Origin::Project(folder) => folder.read_photo(&SafePath::new(src).ok()?).ok(),
    }
}

impl ExportDialog {
    /// Opens the dialog and starts checking the design.
    pub fn new(
        project: Project,
        product: Product,
        cover: Option<printnest_layout::book::CoverGeometry>,
        sources: HashMap<String, Origin>,
    ) -> Self {
        let (tx, rx) = channel();
        let sources = Arc::new(sources);
        let cancel = Arc::new(AtomicBool::new(false));
        {
            let (project, product, sources, tx, stop) = (
                project.clone(),
                product.clone(),
                Arc::clone(&sources),
                tx.clone(),
                Arc::clone(&cancel),
            );
            std::thread::spawn(move || {
                let worker = Worker::current_exe().ok();
                let mut sizes = HashMap::new();
                for src in project.photo_paths() {
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    if let (Some(w), Some(bytes)) =
                        (worker.as_ref(), load_bytes(&sources, src.as_str()))
                        && let Ok(info) = w.probe(&bytes)
                    {
                        sizes.insert(src.as_str().to_owned(), (info.width, info.height));
                    }
                }
                let issues = printnest_export::preflight(&project, &product, cover, &sizes);
                let _ = tx.send(Msg::Issues(issues, sizes));
            });
        }
        let profile = PROFILES
            .iter()
            .map(|(p, _, _)| *p)
            .find(|p| offered(&product, *p))
            .unwrap_or(Profile::PrintShop);
        ExportDialog {
            project,
            product,
            cover,
            sources,
            profile,
            issues: None,
            sizes: Arc::new(HashMap::new()),
            stage: Stage::Choosing,
            rx,
            tx,
            cancel,
        }
    }

    /// Starts exporting into a new folder inside `parent`.
    pub fn start(&mut self, parent: PathBuf) {
        let cover = self.cover;
        let (project, product, sources, tx, cancel, profile, sizes) = (
            self.project.clone(),
            self.product.clone(),
            Arc::clone(&self.sources),
            self.tx.clone(),
            Arc::clone(&self.cancel),
            self.profile,
            Arc::clone(&self.sizes),
        );
        self.stage = Stage::Exporting {
            done: 0,
            total: project.pages.len(),
        };
        std::thread::spawn(move || {
            let worker = Worker::current_exe().ok();
            // Pages often repeat a photo; keep the last few decoded.
            let cache: Mutex<HashMap<(String, u32), Arc<Photo>>> = Mutex::new(HashMap::new());
            let mut loader = |src: &str, edge: u32| {
                if let Ok(c) = cache.lock()
                    && let Some(p) = c.get(&(src.to_owned(), edge))
                {
                    return Some(Arc::clone(p));
                }
                let bytes = load_bytes(&sources, src)?;
                let p = worker.as_ref()?.preview(&bytes, edge).ok()?;
                let photo = Arc::new(Photo {
                    width: p.width,
                    height: p.height,
                    rgba: p.rgba,
                });
                if let Ok(mut c) = cache.lock() {
                    if c.len() >= 4 {
                        c.clear();
                    }
                    c.insert((src.to_owned(), edge), Arc::clone(&photo));
                }
                Some(photo)
            };
            let tx2 = tx.clone();
            let stop = Arc::clone(&cancel);
            let mut progress = |done: usize, total: usize| {
                let _ = tx2.send(Msg::Progress(done, total));
                !stop.load(Ordering::Relaxed)
            };
            let result = printnest_export::export(
                &project,
                &product,
                profile,
                &parent,
                cover,
                &sizes,
                &mut loader,
                &mut progress,
                &|| cancel.load(Ordering::Relaxed),
            )
            .map_err(|e| e.to_string());
            let _ = tx.send(Msg::Done(result));
        });
    }

    /// Stops a running export (its folder is removed).
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Issues(i, sizes) => {
                    self.issues = Some(i);
                    self.sizes = Arc::new(sizes);
                }
                Msg::Progress(done, total) => {
                    self.stage = Stage::Exporting { done, total };
                }
                Msg::Done(r) => self.stage = Stage::Finished(r),
            }
        }
        let busy = self.issues.is_none() || matches!(self.stage, Stage::Exporting { .. });
        if busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    /// Draws the dialog.
    pub fn show(&mut self, ctx: &egui::Context) -> Action {
        self.poll(ctx);
        let mut action = Action::None;
        egui::Modal::new(egui::Id::unique("export-dialog")).show(ctx, |ui| {
            ui.set_width(560.0);
            ui.heading("Export for printing");
            ui.add_space(4.0);
            match &self.stage {
                Stage::Choosing => {
                    ui.label(egui::RichText::new("How will you print it?").strong());
                    let mut any = false;
                    for (p, name, help) in PROFILES {
                        if !offered(&self.product, p) {
                            continue;
                        }
                        any = true;
                        ui.radio_value(&mut self.profile, p, name);
                        ui.label(egui::RichText::new(help).small().weak());
                    }
                    if !any {
                        ui.label(
                            "Print templates for this product (mugs, shirts and other \
                             gifts) arrive in a later version.",
                        );
                    }
                    ui.separator();
                    ui.label(egui::RichText::new("Design check").strong());
                    match &self.issues {
                        None => {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label("Checking your photos…");
                            });
                        }
                        Some(issues) if issues.is_empty() => {
                            ui.label(
                                egui::RichText::new("Everything looks ready to print.")
                                    .color(egui::Color32::from_rgb(46, 160, 67)),
                            );
                        }
                        Some(issues) => {
                            egui::ScrollArea::vertical()
                                .max_height(180.0)
                                .show(ui, |ui| {
                                    for i in issues {
                                        let (tag, colour) = match i.severity {
                                            Severity::Problem => {
                                                ("Problem", egui::Color32::from_rgb(210, 60, 50))
                                            }
                                            Severity::Warning => {
                                                ("Check", egui::Color32::from_rgb(220, 150, 20))
                                            }
                                        };
                                        ui.horizontal_wrapped(|ui| {
                                            ui.label(
                                                egui::RichText::new(if i.page == 0 {
                                                    format!("{tag} · cover")
                                                } else {
                                                    format!("{tag} · page {}", i.page)
                                                })
                                                .color(colour)
                                                .strong(),
                                            );
                                            ui.label(crate::picker::display_name(&i.message));
                                        });
                                    }
                                });
                            ui.label(
                                egui::RichText::new(
                                    "You can still export; these are only suggestions.",
                                )
                                .small()
                                .weak(),
                            );
                        }
                    }
                    ui.separator();
                    ui.horizontal(|ui| {
                        let ready = self.issues.is_some() && offered(&self.product, self.profile);
                        if ui
                            .add_enabled(ready, egui::Button::new("Choose folder and export…"))
                            .clicked()
                        {
                            action = Action::PickFolder;
                        }
                        if ui.button("Cancel").clicked() {
                            action = Action::Close;
                        }
                    });
                }
                Stage::Exporting { done, total } => {
                    let frac = if *total == 0 {
                        0.0
                    } else {
                        *done as f32 / *total as f32
                    };
                    ui.label(format!(
                        "Rendering page {} of {total}…",
                        (*done + 1).min(*total)
                    ));
                    ui.add(egui::ProgressBar::new(frac).show_percentage());
                    if ui.button("Cancel").clicked() {
                        self.cancel();
                    }
                }
                Stage::Finished(Ok(report)) => {
                    ui.label(
                        egui::RichText::new("Done!")
                            .strong()
                            .color(egui::Color32::from_rgb(46, 160, 67)),
                    );
                    ui.label(format!(
                        "Saved at {:.0} ppi in:\n{}",
                        report.ppi,
                        crate::picker::display_name(&report.folder.display().to_string())
                    ));
                    if !report.missing.is_empty() {
                        ui.label(
                            egui::RichText::new(format!(
                                "These photos couldn't be opened and are missing from the export: {}",
                                crate::picker::display_name(&report.missing.join(", "))
                            ))
                            .color(egui::Color32::from_rgb(210, 60, 50)),
                        );
                    }
                    for f in &report.files {
                        let name = f
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        ui.label(format!("• {}", crate::picker::display_name(&name)));
                    }
                    if ui.button("Close").clicked() {
                        action = Action::Close;
                    }
                }
                Stage::Finished(Err(e)) => {
                    ui.label(
                        egui::RichText::new("The export didn't finish")
                            .strong()
                            .color(egui::Color32::from_rgb(210, 60, 50)),
                    );
                    ui.label(crate::picker::display_name(e));
                    if ui.button("Close").clicked() {
                        action = Action::Close;
                    }
                }
            }
        });
        action
    }

    /// True while pages are being written (closing would abandon it).
    #[must_use]
    pub fn busy(&self) -> bool {
        matches!(self.stage, Stage::Exporting { .. })
    }
}
