//! The design editor: one page at a time, photo frames and text boxes.
//!
//! All geometry and editing rules live in `printnest-layout`; this module
//! draws them and turns pointer/keyboard input into edits.

use crate::library::{Library, Origin, State};
use crate::picker::display_name;
use eframe::egui::{self, Color32, Pos2, Rect as ERect, Stroke, StrokeKind, Vec2};
use printnest_document::ProjectFolder;
use printnest_layout::book::{self, AutoPhoto, CoverGeometry, Layout, PageBox, inner_edge};
use printnest_layout::crop::{crop_view, pan_by};
use printnest_layout::edit;
use printnest_layout::geometry::{self, Handle, Pt, corners};
use printnest_layout::snap::{self, Guide};
use printnest_layout::{History, effective_ppi};
use printnest_model::Catalogue;
use printnest_model::catalogue::Product;
use printnest_model::limits::{MAX_PAGE_EDGE_MM, MAX_TEXT_CHARS};
use printnest_model::project::{Align, Element, Project, Rect};
use printnest_model::safe_path::SafePath;
use printnest_model::units::{PrintQuality, grade_ppi};

const SNAP_PX: f64 = 8.0;
/// Highest photo zoom the project format allows.
const MAX_ZOOM: f64 = 20.0;
const HANDLE_PX: f32 = 7.0;
const TRIM: Color32 = Color32::from_rgb(20, 20, 20);
const SAFE: Color32 = Color32::from_rgb(40, 110, 220);
const SELECT: Color32 = Color32::from_rgb(0, 140, 255);
const GUIDE: Color32 = Color32::from_rgb(255, 0, 170);

/// What the editor asks the app to do (things that need dialogs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    New,
    Open,
    Save,
    SaveAs,
    AddPhotos,
    Export,
}

#[derive(Debug, Clone, Copy)]
enum DragKind {
    Element(Handle),
    PanPhoto,
    View,
}

#[derive(Debug, Clone)]
struct Drag {
    kind: DragKind,
    /// Element being dragged, by id (indexes shift on undo/delete).
    id: Option<String>,
    start_rect: Rect,
    start_crop: printnest_model::project::Crop,
    start_pt: Pt,
    start_view: Vec2,
    start_screen: Pos2,
}

/// Maps page millimetres to screen pixels.
#[derive(Debug, Clone, Copy)]
struct View {
    origin: Pos2, // screen position of trim (0,0)
    scale: f32,   // pixels per mm
}

impl View {
    fn to_screen(self, p: Pt) -> Pos2 {
        Pos2::new(
            self.origin.x + (p.x as f32) * self.scale,
            self.origin.y + (p.y as f32) * self.scale,
        )
    }
    fn to_page(self, p: Pos2) -> Pt {
        Pt::new(
            f64::from((p.x - self.origin.x) / self.scale),
            f64::from((p.y - self.origin.y) / self.scale),
        )
    }
    fn rect(self, x: f64, y: f64, w: f64, h: f64) -> ERect {
        ERect::from_min_max(
            self.to_screen(Pt::new(x, y)),
            self.to_screen(Pt::new(x + w, y + h)),
        )
    }
}

pub struct Editor {
    pub project: Project,
    pub folder: Option<ProjectFolder>,
    pub dirty: bool,
    pub library: Library,
    pub product: Product,
    page: usize,
    selected: Option<usize>,
    history: History,
    drag: Option<Drag>,
    guides: Vec<Guide>,
    view_zoom: f32,
    view_pan: Vec2,
    panel_edit_active: bool,
    /// Something may have changed this frame; validate before the next.
    mutated: bool,
    show_bleed: bool,
    show_safe: bool,
    pub status: String,
    /// The book cover's layout (spine position), if this is a book.
    cover_geom: Option<CoverGeometry>,
    /// The project's paper (its thickness sets the spine).
    paper: Option<printnest_model::catalogue::Paper>,
    /// Layout chosen in the toolbar picker.
    layout: Layout,
}

impl std::fmt::Debug for Editor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Editor")
            .field("title", &self.project.title)
            .finish()
    }
}

fn parse_colour(s: &str) -> Color32 {
    let hex = |i: usize| {
        s.get(i..i + 2)
            .and_then(|h| u8::from_str_radix(h, 16).ok())
            .unwrap_or(0)
    };
    Color32::from_rgb(hex(1), hex(3), hex(5))
}

fn colour_hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

fn quality_colour(q: PrintQuality) -> Color32 {
    match q {
        PrintQuality::Good => Color32::from_rgb(46, 160, 67),
        PrintQuality::Fair => Color32::from_rgb(220, 150, 20),
        PrintQuality::Poor => Color32::from_rgb(210, 60, 50),
    }
}

impl Editor {
    pub fn new(project: Project, product: Product, folder: Option<ProjectFolder>) -> Self {
        let mut library = Library::new();
        if let Some(f) = &folder {
            for p in project.photo_paths() {
                library.add(p.clone(), Origin::Project(f.clone()));
            }
        }
        Editor {
            project,
            folder,
            dirty: false,
            library,
            product,
            page: 0,
            selected: None,
            history: History::new(),
            drag: None,
            guides: Vec::new(),
            view_zoom: 1.0,
            view_pan: Vec2::ZERO,
            panel_edit_active: false,
            mutated: false,
            show_bleed: true,
            show_safe: true,
            status: String::new(),
            cover_geom: None,
            paper: None,
            layout: Layout::Single,
        }
    }

    fn trim(&self) -> (f64, f64) {
        if self.on_cover()
            && let Some(g) = self.cover_geom
        {
            return (g.width, g.height);
        }
        let t = self.product.trim();
        (t.width.0, t.height.0)
    }

    fn outer(&self) -> f64 {
        if self.on_cover()
            && let Some(g) = self.cover_geom
        {
            return g.outer;
        }
        self.product.bleed_mm + self.product.wrap_mm.unwrap_or(0.0)
    }

    /// Where new frames and text go: the whole page, or on the cover the
    /// front panel (width, height, x offset).
    fn new_element_area(&self) -> (f64, f64, f64) {
        if self.on_cover()
            && let Some(g) = self.cover_geom
        {
            let (x, _, w, h) = g.front();
            return (w, h, x);
        }
        let (w, h) = self.trim();
        (w, h, 0.0)
    }

    /// Moves just-added element `i` right by `dx` mm.
    fn shift_new(&mut self, i: usize, dx: f64) {
        if dx != 0.0
            && let Some(el) = self.element_mut(i)
        {
            rect_mut(el).x += dx;
        }
    }

    /// Measurements a layout needs for the page being edited.
    fn page_box(&self, index: usize) -> PageBox {
        let (w, h) = self.trim();
        PageBox {
            width: w,
            height: h,
            safe: self.product.safe_mm,
            bleed: self.outer(),
            gutter: self.product.gutter_mm.map(|g| (g, inner_edge(index))),
        }
    }

    /// Replaces the current page with the chosen layout, keeping its photos
    /// (in order) and text.
    fn apply_layout(&mut self) {
        if self.on_cover() {
            return;
        }
        let photos: Vec<SafePath> = self
            .elements()
            .iter()
            .filter_map(|e| match e {
                Element::Photo { src: Some(s), .. } => Some(s.clone()),
                Element::Photo { .. } | Element::Text { .. } => None,
            })
            .collect();
        let dropped = photos.len().saturating_sub(self.layout.photos());
        let before = self.project.clone();
        let page_box = self.page_box(self.page);
        if book::apply_layout(&mut self.project, self.page, self.layout, &photos, page_box) {
            self.recorded(&before);
            self.selected = None;
            self.status = if dropped > 0 {
                format!("Layout applied; {dropped} photo(s) didn't fit and stay in the tray.")
            } else {
                "Layout applied.".into()
            };
        }
    }

    /// Fills the book's empty pages with tray photos not used yet.
    fn auto_fill(&mut self) {
        let used: std::collections::HashSet<String> = self
            .project
            .photo_paths()
            .iter()
            .map(|p| p.as_str().to_owned())
            .collect();
        let photos: Vec<AutoPhoto> = self
            .library
            .list()
            .into_iter()
            .filter(|k| !used.contains(k.as_str()))
            .map(|k| {
                // Unknown sizes (not decoded yet) count as landscape.
                let (width, height) = self
                    .library
                    .get(k.as_str())
                    .and_then(|p| p.size)
                    .unwrap_or((4, 3));
                AutoPhoto {
                    path: k,
                    width,
                    height,
                }
            })
            .collect();
        if photos.is_empty() {
            self.status = "Add photos first: auto-fill uses the photos in the tray that aren't \
                           on a page yet."
                .into();
            return;
        }
        let before = self.project.clone();
        let boxes: Vec<PageBox> = (0..self.project.pages.len())
            .map(|i| {
                let t = self.product.trim();
                PageBox {
                    width: t.width.0,
                    height: t.height.0,
                    safe: self.product.safe_mm,
                    bleed: self.product.bleed_mm + self.product.wrap_mm.unwrap_or(0.0),
                    gutter: self.product.gutter_mm.map(|g| (g, inner_edge(i))),
                }
            })
            .collect();
        let fallback = boxes.first().copied();
        let placed = book::auto_fill(&mut self.project, &photos, |i| {
            boxes.get(i).copied().or(fallback).unwrap_or(PageBox {
                width: 100.0,
                height: 100.0,
                safe: 5.0,
                bleed: 3.0,
                gutter: None,
            })
        });
        if placed > 0 {
            self.recorded(&before);
            self.selected = None;
        }
        let left = photos.len() - placed;
        self.status = match (placed, left) {
            (0, _) => "No empty pages to fill.".into(),
            (n, 0) => format!("Placed {n} photo(s) on empty pages."),
            (n, l) => {
                format!("Placed {n} photo(s); {l} didn't fit (add pages, or fill pages by hand).")
            }
        };
    }

    fn checkpoint(&mut self) {
        self.history.record(&self.project);
        self.dirty = true;
        self.mutated = true;
    }

    /// Records `before` as the undo point for a change already made.
    fn recorded(&mut self, before: &Project) {
        self.history.record(before);
        self.dirty = true;
        self.mutated = true;
    }

    /// A photo was stored under a different name: update the design and
    /// every undo/redo snapshot.
    pub fn retarget_photo(&mut self, old: &SafePath, new: &SafePath) {
        edit::retarget_photo(&mut self.project, old, new);
        self.history.retarget_photo(old, new);
    }

    /// Where the bytes of every photo the design uses live. Project
    /// photos are re-checked on every read (symlinks, escapes).
    pub fn photo_sources(&self) -> std::collections::HashMap<String, Origin> {
        self.project
            .photo_paths()
            .into_iter()
            .filter_map(|k| {
                Some((
                    k.as_str().to_owned(),
                    self.library.origin(k.as_str())?.clone(),
                ))
            })
            .collect()
    }

    /// True while the user is mid-gesture (don't autosave under them).
    pub fn busy(&self) -> bool {
        self.drag.is_some() || self.panel_edit_active
    }

    fn id_at(&self, i: usize) -> Option<String> {
        self.elements().get(i).map(|e| e.id().to_owned())
    }

    fn index_of(&self, id: &str) -> Option<usize> {
        self.elements().iter().position(|e| e.id() == id)
    }

    fn elements(&self) -> &[Element] {
        edit::page(&self.project, self.page).map_or(&[], |p| p.elements.as_slice())
    }

    fn element_mut(&mut self, i: usize) -> Option<&mut Element> {
        edit::page_mut(&mut self.project, self.page).and_then(|p| p.elements.get_mut(i))
    }

    /// True while the book cover is being edited.
    fn on_cover(&self) -> bool {
        self.page == edit::COVER
    }

    /// Adds picked photo files to the project (as frames) and the library.
    pub fn add_photo_files(&mut self, paths: Vec<std::path::PathBuf>) {
        let multi_page = self.project.pages.len() > 1;
        let mut added = 0;
        for path in paths {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "photo.jpg".into());
            let mut taken = self.library.keys();
            // Never pick a name a file in the project folder already has
            // (a shared project may contain photos it doesn't use).
            if let Some(f) = &self.folder {
                taken.extend(f.photo_names());
            }
            taken.extend(
                self.project
                    .photo_paths()
                    .iter()
                    .map(|p| p.as_str().to_owned()),
            );
            let Some(key) = edit::photo_path(&name, &taken) else {
                continue;
            };
            self.library.add(key.clone(), Origin::External(path));
            // Books (and other multi-page products) collect photos in the
            // tray, ready for Auto-fill or clicking onto a page; single
            // prints place them straight away.
            if multi_page || self.place_photo(key) {
                added += 1;
            }
        }
        if added > 0 {
            self.status = if multi_page {
                format!(
                    "Added {added} photo(s) to the tray. Use Auto-fill, or click a photo to \
                     put it on this page."
                )
            } else {
                format!("Added {added} photo(s). They load in the background.")
            };
        }
    }

    /// Puts a photo into the selected empty frame, or a new frame.
    fn place_photo(&mut self, key: SafePath) -> bool {
        let (w, h) = self.trim();
        let bleed = self.outer();
        if let Some(i) = self.selected
            && let Some(Element::Photo { src: None, .. }) = self.elements().get(i)
        {
            self.checkpoint();
            if let Some(Element::Photo { src, .. }) = self.element_mut(i) {
                *src = Some(key);
            }
            return true;
        }
        let empty = self.elements().is_empty();
        let before = self.project.clone();
        let res = if empty {
            edit::add_full_bleed_photo(&mut self.project, self.page, w, h, bleed, Some(key))
        } else {
            edit::add_photo_frame(&mut self.project, self.page, w, h, Some(key))
        };
        match res {
            Ok(i) => {
                self.recorded(&before);
                self.selected = Some(i);
                true
            }
            Err(_) => {
                self.status = "This page is full.".into();
                false
            }
        }
    }

    fn undo(&mut self) {
        if let Some(p) = self.history.undo(&self.project) {
            self.project = p;
            self.dirty = true;
            self.mutated = true;
            self.selected = None;
        }
    }

    fn redo(&mut self) {
        if let Some(p) = self.history.redo(&self.project) {
            self.project = p;
            self.dirty = true;
            self.mutated = true;
            self.selected = None;
        }
    }

    fn delete_selected(&mut self) {
        let Some(i) = self.selected else { return };
        if i < self.elements().len() {
            self.checkpoint();
            if let Some(page) = edit::page_mut(&mut self.project, self.page) {
                page.elements.remove(i);
            }
            self.selected = None;
        }
    }

    fn nudge(&mut self, dx: f64, dy: f64) {
        let Some(i) = self.selected else { return };
        self.checkpoint();
        if let Some(el) = self.element_mut(i) {
            let r = rect_mut(el);
            r.x = (r.x + dx).clamp(-MAX_PAGE_EDGE_MM, MAX_PAGE_EDGE_MM);
            r.y = (r.y + dy).clamp(-MAX_PAGE_EDGE_MM, MAX_PAGE_EDGE_MM);
        }
    }

    // ------------------------------------------------------------------ UI

    /// `modal`: a dialog is open, so keyboard shortcuts must not edit the
    /// design behind it.
    pub fn ui(&mut self, ui: &mut egui::Ui, catalogue: &Catalogue, modal: bool) -> Vec<Request> {
        let mut requests = Vec::new();
        self.library.poll(ui.ctx());
        self.paper = catalogue.paper(&self.project.paper).cloned();
        self.cover_geom = self.project.cover.as_deref().and_then(|c| {
            let paper = catalogue.paper(&self.project.paper)?;
            let pages = u32::try_from(self.project.pages.len()).ok()?;
            CoverGeometry::new(&self.product, pages, paper, c)
        });
        if self.on_cover() && (self.cover_geom.is_none() || self.project.cover_design.is_none()) {
            self.page = 0;
        }
        // Undo/redo can change the page count under us.
        if !self.on_cover() && self.page >= self.project.pages.len() {
            self.page = self.project.pages.len().saturating_sub(1);
            self.selected = None;
        }
        if !modal {
            self.shortcuts(ui.ctx(), &mut requests);
        }

        egui::Panel::top("toolbar").show(ui, |ui| self.toolbar(ui, &mut requests));
        egui::Panel::bottom("tray")
            .resizable(false)
            .exact_size(118.0)
            .show(ui, |ui| self.tray(ui, &mut requests));
        egui::Panel::right("props")
            .default_size(280.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.properties(ui));
            });
        // Decode (or keep) the photos shown on this page.
        let shown: Vec<String> = self
            .elements()
            .iter()
            .filter_map(|e| match e {
                Element::Photo { src: Some(s), .. } => Some(s.as_str().to_owned()),
                Element::Photo { .. } | Element::Text { .. } => None,
            })
            .collect();
        for key in shown {
            self.library.want(&key);
        }
        egui::CentralPanel::default_margins().show(ui, |ui| self.canvas(ui));

        // Never let an invalid project into memory (and thus into a save).
        if std::mem::take(&mut self.mutated)
            && let Err(e) = self.project.validate(catalogue)
        {
            self.status = format!("Undoing an invalid change: {e}");
            self.undo();
            // Start a fresh undo step for whatever comes next.
            self.panel_edit_active = false;
        }
        requests
    }

    fn shortcuts(&mut self, ctx: &egui::Context, requests: &mut Vec<Request>) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        // Undo/delete/duplicate/nudge mid-drag would fight the drag.
        if self.drag.is_some() {
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.drag = None;
                self.guides.clear();
            }
            return;
        }
        use egui::{Key, Modifiers};
        let cmd = Modifiers::COMMAND;
        let cmd_shift = Modifiers::COMMAND | Modifiers::SHIFT;
        let (undo, redo, redo2, save, dup, del, bksp) = ctx.input_mut(|i| {
            (
                i.consume_key(cmd, Key::Z),
                i.consume_key(cmd_shift, Key::Z),
                i.consume_key(cmd, Key::Y),
                i.consume_key(cmd, Key::S),
                i.consume_key(cmd, Key::D),
                i.consume_key(Modifiers::NONE, Key::Delete),
                i.consume_key(Modifiers::NONE, Key::Backspace),
            )
        });
        if redo || redo2 {
            self.redo();
        } else if undo {
            self.undo();
        }
        if save {
            requests.push(Request::Save);
        }
        if dup && let Some(i) = self.selected {
            let before = self.project.clone();
            if let Ok(n) = edit::duplicate(&mut self.project, self.page, i) {
                self.recorded(&before);
                self.selected = Some(n);
            }
        }
        if del || bksp {
            self.delete_selected();
        }
        let step = if ctx.input(|i| i.modifiers.shift) {
            10.0
        } else {
            1.0
        };
        let arrows = ctx.input(|i| {
            [
                (i.key_pressed(Key::ArrowLeft), -step, 0.0),
                (i.key_pressed(Key::ArrowRight), step, 0.0),
                (i.key_pressed(Key::ArrowUp), 0.0, -step),
                (i.key_pressed(Key::ArrowDown), 0.0, step),
            ]
        });
        for (pressed, dx, dy) in arrows {
            if pressed {
                self.nudge(dx, dy);
            }
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            self.selected = None;
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, requests: &mut Vec<Request>) {
        ui.horizontal_wrapped(|ui| {
            if ui.button("New").clicked() {
                requests.push(Request::New);
            }
            if ui.button("Open").clicked() {
                requests.push(Request::Open);
            }
            if ui.button("Save").on_hover_text("Ctrl+S").clicked() {
                requests.push(Request::Save);
            }
            if ui.button("Save as…").clicked() {
                requests.push(Request::SaveAs);
            }
            ui.separator();
            if ui
                .add_enabled(self.history.can_undo(), egui::Button::new("Undo"))
                .on_hover_text("Ctrl+Z")
                .clicked()
            {
                self.undo();
            }
            if ui
                .add_enabled(self.history.can_redo(), egui::Button::new("Redo"))
                .on_hover_text("Ctrl+Shift+Z")
                .clicked()
            {
                self.redo();
            }
            ui.separator();
            if ui.button("Add photos…").clicked() {
                requests.push(Request::AddPhotos);
            }
            if ui
                .button(egui::RichText::new("Export…").strong())
                .on_hover_text("Make files for a kiosk, print shop or home printer")
                .clicked()
            {
                requests.push(Request::Export);
            }
            if ui.button("Empty frame").clicked() {
                let (w, h, dx) = self.new_element_area();
                let before = self.project.clone();
                if let Ok(i) = edit::add_photo_frame(&mut self.project, self.page, w, h, None) {
                    self.shift_new(i, dx);
                    self.recorded(&before);
                    self.selected = Some(i);
                }
            }
            if ui.button("Add text").clicked() {
                let (w, h, dx) = self.new_element_area();
                let before = self.project.clone();
                if let Ok(i) =
                    edit::add_text(&mut self.project, self.page, w, h, self.product.safe_mm)
                {
                    self.shift_new(i, dx);
                    self.recorded(&before);
                    self.selected = Some(i);
                }
            }
            ui.separator();
            let pages = self.project.pages.len();
            let has_cover = self.cover_geom.is_some() && self.project.cover_design.is_some();
            if pages > 1 || has_cover {
                let first = !has_cover && self.page == 0;
                if ui
                    .add_enabled(!self.on_cover() && !first, egui::Button::new("< Prev"))
                    .clicked()
                {
                    self.page = if self.page == 0 {
                        edit::COVER
                    } else {
                        self.page - 1
                    };
                    self.selected = None;
                }
                if self.on_cover() {
                    ui.label(egui::RichText::new("Cover").strong());
                } else {
                    ui.label(format!("Page {} of {pages}", self.page + 1));
                }
                let last = !self.on_cover() && self.page + 1 >= pages;
                if ui.add_enabled(!last, egui::Button::new("Next >")).clicked() {
                    self.page = if self.on_cover() { 0 } else { self.page + 1 };
                    self.selected = None;
                }
                if has_cover && !self.on_cover() && ui.button("Cover").clicked() {
                    self.page = edit::COVER;
                    self.selected = None;
                }
                let rule = self.product.page_rule();
                if rule.max > rule.min && !self.on_cover() {
                    let step = rule.step;
                    if ui
                        .button(format!("+{step} pages"))
                        .on_hover_text("Add blank pages after this one")
                        .clicked()
                    {
                        let before = self.project.clone();
                        let (product, paper) = (self.product.clone(), self.paper.clone());
                        match book::add_pages(
                            &mut self.project,
                            &product,
                            paper.as_ref(),
                            self.page,
                        ) {
                            Some(first) => {
                                self.recorded(&before);
                                self.page = first;
                                self.selected = None;
                            }
                            None => {
                                self.status =
                                    format!("This book can have at most {} pages.", rule.max);
                            }
                        }
                    }
                    if ui
                        .button(format!("-{step} pages"))
                        .on_hover_text("Remove this page and the next (undo brings them back)")
                        .clicked()
                    {
                        let before = self.project.clone();
                        let (product, paper) = (self.product.clone(), self.paper.clone());
                        if book::remove_pages(
                            &mut self.project,
                            &product,
                            paper.as_ref(),
                            self.page,
                        ) {
                            self.recorded(&before);
                            self.page = self.page.min(self.project.pages.len().saturating_sub(1));
                            self.selected = None;
                        } else {
                            self.status = format!(
                                "This book needs at least {} pages (and pages to remove).",
                                rule.min
                            );
                        }
                    }
                }
                ui.separator();
            }
            if !self.on_cover() {
                egui::ComboBox::from_id_salt("layout")
                    .selected_text(self.layout.name())
                    .show_ui(ui, |ui| {
                        for l in Layout::ALL {
                            ui.selectable_value(&mut self.layout, l, l.name());
                        }
                    });
                if ui
                    .button("Apply layout")
                    .on_hover_text("Rearrange this page; its photos and text are kept")
                    .clicked()
                {
                    self.apply_layout();
                }
            }
            if pages > 1
                && ui
                    .button("Auto-fill")
                    .on_hover_text("Put the tray's unused photos on the empty pages, in order")
                    .clicked()
            {
                self.auto_fill();
            }
            ui.separator();
            ui.checkbox(&mut self.show_bleed, "Bleed");
            ui.checkbox(&mut self.show_safe, "Safe zone");
            ui.add(
                egui::Slider::new(&mut self.view_zoom, 0.25..=4.0)
                    .logarithmic(true)
                    .text("zoom"),
            );
            if ui.button("Fit").clicked() {
                self.view_zoom = 1.0;
                self.view_pan = Vec2::ZERO;
            }
        });
    }

    fn tray(&mut self, ui: &mut egui::Ui, requests: &mut Vec<Request>) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Photos").strong());
            let loading = self.library.loading();
            if loading > 0 {
                ui.spinner();
                ui.label(format!("decoding {loading} safely…"));
            }
            ui.label(
                egui::RichText::new(
                    "Click a photo to put it in the selected empty frame, or on the page.",
                )
                .weak()
                .small(),
            );
        });
        let mut place: Option<SafePath> = None;
        egui::ScrollArea::horizontal().show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .add_sized([80.0, 72.0], egui::Button::new("+\nAdd photos"))
                    .clicked()
                {
                    requests.push(Request::AddPhotos);
                }
                for key in self.library.list() {
                    let size = egui::vec2(96.0, 72.0);
                    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
                    // Only photos scrolled into view are decoded.
                    if !ui.is_rect_visible(rect) {
                        continue;
                    }
                    self.library.want(key.as_str());
                    let Some(photo) = self.library.get(key.as_str()) else {
                        continue;
                    };
                    let name = display_name(key.as_str());
                    let dims = photo
                        .size
                        .map(|(w, h)| format!("\n{w} × {h} px"))
                        .unwrap_or_default();
                    match (&photo.state, photo.texture()) {
                        (_, Some(texture)) => {
                            let tex =
                                egui::load::SizedTexture::new(texture.id(), texture.size_vec2());
                            egui::Image::new(tex)
                                .fit_to_exact_size(size)
                                .maintain_aspect_ratio(true)
                                .paint_at(ui, rect);
                            if response.hovered() {
                                ui.painter().rect_stroke(
                                    rect,
                                    2.0,
                                    Stroke::new(2.0, SELECT),
                                    StrokeKind::Inside,
                                );
                            }
                        }
                        (State::Failed(_), _) => {
                            ui.painter().text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                "Can't open",
                                egui::FontId::proportional(12.0),
                                Color32::from_rgb(210, 60, 50),
                            );
                        }
                        _ => {
                            ui.painter().rect_filled(rect, 2.0, Color32::from_gray(60));
                            ui.painter().text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                "…",
                                egui::FontId::proportional(16.0),
                                Color32::from_gray(180),
                            );
                        }
                    }
                    let tip = match &photo.state {
                        State::Failed(e) => format!("{name}\n{}", display_name(e)),
                        _ => format!("{name}{dims}"),
                    };
                    let clicked = response.clicked();
                    response.on_hover_text(tip);
                    if clicked && photo.texture().is_some() {
                        place = Some(key.clone());
                    }
                }
            });
        });
        if let Some(k) = place {
            self.place_photo(k);
        }
    }

    fn properties(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.heading(&self.product.name);
        ui.label(egui::RichText::new(display_name(&self.project.title)).weak());
        ui.separator();
        let Some(i) = self.selected.filter(|&i| i < self.elements().len()) else {
            ui.label("Select something on the page to edit it.");
            ui.add_space(8.0);
            ui.label(egui::RichText::new("Tips").strong());
            for tip in [
                "Drag to move, drag a corner to resize (Shift keeps proportions).",
                "Right-drag inside a photo to move it within its frame.",
                "Scroll over a selected photo to zoom it.",
                "Middle-drag or Space-drag the background to pan the view.",
                "Arrow keys nudge 1 mm (Shift: 10 mm). Ctrl+D duplicates.",
            ] {
                ui.label(egui::RichText::new(format!("• {tip}")).small());
            }
            return;
        };
        // Snapshot just this element (cloning the whole project every frame
        // is slow for big projects); rebuild the undo point only on change.
        let Some(el_before) = self.elements().get(i).cloned() else {
            return;
        };
        let pointer_down = ui.input(|i| i.pointer.any_down());
        let mut typing = false;
        let mut remove = false;
        let mut order: Option<bool> = None;
        let ready_size = self.photo_size(i);
        let Some(el) = self.element_mut(i) else {
            return;
        };
        match el {
            Element::Photo {
                rect, src, crop, ..
            } => {
                ui.label(egui::RichText::new("Photo frame").strong());
                if let Some(s) = src {
                    ui.label(egui::RichText::new(s.as_str()).small().weak());
                }
                rect_controls(ui, rect);
                ui.separator();
                ui.label("Photo inside the frame");
                // Ranges match the project format, and values are never
                // changed just by showing them (only by user edits).
                let edits = egui::SliderClamping::Edits;
                ui.add(
                    egui::Slider::new(&mut crop.zoom, 1.0..=MAX_ZOOM)
                        .logarithmic(true)
                        .clamping(edits)
                        .text("zoom"),
                );
                ui.add(
                    egui::Slider::new(&mut crop.pan_x, -1.0..=1.0)
                        .clamping(edits)
                        .text("left - right"),
                );
                ui.add(
                    egui::Slider::new(&mut crop.pan_y, -1.0..=1.0)
                        .clamping(edits)
                        .text("top - bottom"),
                );
                if ui.button("Reset crop").clicked() {
                    *crop = printnest_model::project::Crop::default();
                }
                if src.is_some() && ui.button("Remove photo (keep frame)").clicked() {
                    *src = None;
                }
                if let Some((w, h)) = ready_size
                    && let Some(ppi) = effective_ppi(rect.w, rect.h, w, h, crop)
                {
                    let q = grade_ppi(ppi);
                    let words = match q {
                        PrintQuality::Good => "Sharp",
                        PrintQuality::Fair => "Acceptable",
                        PrintQuality::Poor => {
                            "Likely blurry — use a larger photo or a smaller frame"
                        }
                    };
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(format!("{ppi:.0} ppi · {words}"))
                            .color(quality_colour(q))
                            .strong(),
                    );
                    ui.label(
                        egui::RichText::new(format!("Original {w} × {h} px"))
                            .small()
                            .weak(),
                    );
                }
            }
            Element::Text {
                rect,
                text,
                font,
                size_pt,
                colour,
                align,
                ..
            } => {
                ui.label(egui::RichText::new("Text").strong());
                typing = ui
                    .add(
                        egui::TextEdit::multiline(text)
                            .char_limit(MAX_TEXT_CHARS)
                            .desired_rows(3)
                            .desired_width(f32::INFINITY),
                    )
                    .has_focus();
                // Pasted control characters (lone CR, ESC…) would fail
                // validation and undo the whole typing session: drop them.
                if text
                    .chars()
                    .any(|c| c.is_control() && c != '\n' && c != '\t')
                {
                    text.retain(|c| !c.is_control() || c == '\n' || c == '\t');
                }
                ui.horizontal(|ui| {
                    ui.label("Size");
                    ui.add(
                        egui::DragValue::new(size_pt)
                            .range(4.0..=400.0)
                            .clamp_existing_to_range(false)
                            .suffix(" pt"),
                    );
                    let mut rgb = parse_colour(colour).to_array();
                    let mut c3 = [rgb[0], rgb[1], rgb[2]];
                    if ui.color_edit_button_srgb(&mut c3).changed() {
                        rgb[..3].copy_from_slice(&c3);
                        *colour = colour_hex(c3);
                    }
                });
                ui.horizontal(|ui| {
                    ui.selectable_value(align, Align::Left, "Left");
                    ui.selectable_value(align, Align::Center, "Centre");
                    ui.selectable_value(align, Align::Right, "Right");
                });
                ui.horizontal(|ui| {
                    ui.label("Font");
                    ui.selectable_value(font, "sans".to_owned(), "Sans");
                    ui.selectable_value(font, "mono".to_owned(), "Mono");
                });
                rect_controls(ui, rect);
            }
        }
        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("Bring forward").clicked() {
                order = Some(true);
            }
            if ui.button("Send back").clicked() {
                order = Some(false);
            }
        });
        if ui.button("Delete").clicked() {
            remove = true;
        }

        let changed = self.elements().get(i) != Some(&el_before);
        if changed {
            // One undo step per gesture: a drag on a control, or a typing
            // session in the text box.
            if !self.panel_edit_active {
                let mut before = self.project.clone();
                if let Some(slot) =
                    edit::page_mut(&mut before, self.page).and_then(|p| p.elements.get_mut(i))
                {
                    *slot = el_before;
                }
                self.recorded(&before);
            }
            self.mutated = true;
            self.panel_edit_active = pointer_down || typing;
        } else if !pointer_down && !typing {
            self.panel_edit_active = false;
        }
        if let Some(up) = order {
            let snapshot = self.project.clone();
            if let Some(n) = edit::reorder(&mut self.project, self.page, i, up) {
                self.recorded(&snapshot);
                self.selected = Some(n);
            }
        }
        if remove {
            self.delete_selected();
        }
    }

    fn photo_size(&self, i: usize) -> Option<(u32, u32)> {
        let Some(Element::Photo { src: Some(s), .. }) = self.elements().get(i) else {
            return None;
        };
        self.library.get(s.as_str()).and_then(|p| p.size)
    }

    // -------------------------------------------------------------- canvas

    fn canvas(&mut self, ui: &mut egui::Ui) {
        let (response, painter) =
            ui.allocate_painter(ui.available_size(), egui::Sense::click_and_drag());
        let area = response.rect.shrink(24.0);
        let (tw, th) = self.trim();
        let outer = self.outer();
        let full_w = (tw + 2.0 * outer) as f32;
        let full_h = (th + 2.0 * outer) as f32;
        if area.width() <= 0.0 || area.height() <= 0.0 || full_w <= 0.0 {
            return;
        }
        let fit = (area.width() / full_w).min(area.height() / full_h);
        let scale = fit * self.view_zoom;
        let art_size = Vec2::new(full_w, full_h) * scale;
        let art_min = area.center() - art_size / 2.0 + self.view_pan;
        let view = View {
            origin: art_min + Vec2::splat(outer as f32 * scale),
            scale,
        };

        // Scroll: zoom the selected photo when hovering it, else the view.
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll.abs() > 0.0 {
                let hover_pt = response.hover_pos().map(|p| view.to_page(p));
                let over_selected_photo = matches!(
                    (self.selected, hover_pt),
                    (Some(i), Some(pt)) if matches!(self.elements().get(i), Some(Element::Photo { rect, src: Some(_), .. }) if geometry::contains(rect, pt))
                );
                if over_selected_photo && let Some(i) = self.selected {
                    if !self.panel_edit_active {
                        self.checkpoint();
                        self.panel_edit_active = true;
                    }
                    if let Some(Element::Photo { crop, .. }) = self.element_mut(i) {
                        crop.zoom =
                            (crop.zoom * (1.0 + f64::from(scroll) / 400.0)).clamp(1.0, MAX_ZOOM);
                        self.mutated = true;
                    }
                } else {
                    self.view_zoom = (self.view_zoom * (1.0 + scroll / 400.0)).clamp(0.25, 4.0);
                }
            } else if !ui.input(|i| i.pointer.any_down()) {
                self.panel_edit_active = false;
            }
        }

        self.handle_pointer(ui, &response, view);
        self.paint(&painter, view, art_min, art_size);
    }

    fn handle_pointer(&mut self, ui: &egui::Ui, response: &egui::Response, view: View) {
        let handle_mm = f64::from(HANDLE_PX / view.scale) * 1.5;
        let space = ui.input(|i| i.key_down(egui::Key::Space));
        let shift = ui.input(|i| i.modifiers.shift);

        if response.drag_started()
            && let Some(pos) = response.interact_pointer_pos()
        {
            let pt = view.to_page(pos);
            let middle = response.dragged_by(egui::PointerButton::Middle);
            let secondary = response.dragged_by(egui::PointerButton::Secondary);
            let selected_hit = self
                .selected
                .and_then(|i| self.elements().get(i).map(|e| (i, *e.rect())))
                .and_then(|(i, r)| geometry::hit_handle(&r, pt, handle_mm).map(|h| (i, r, h)));
            let base = |kind, id: Option<String>, rect: Rect, crop| Drag {
                kind,
                id,
                start_rect: rect,
                start_crop: crop,
                start_pt: pt,
                start_view: self.view_pan,
                start_screen: pos,
            };
            let crop_of = |e: Option<&Element>| match e {
                Some(Element::Photo { crop, .. }) => *crop,
                _ => printnest_model::project::Crop::default(),
            };
            self.drag = if middle || space {
                Some(base(
                    DragKind::View,
                    None,
                    Rect {
                        x: 0.0,
                        y: 0.0,
                        w: 1.0,
                        h: 1.0,
                        rotation: 0.0,
                    },
                    crop_of(None),
                ))
            } else if secondary {
                geometry::hit_test(self.elements(), pt).and_then(|i| {
                    let e = self.elements().get(i);
                    matches!(e, Some(Element::Photo { src: Some(_), .. })).then(|| {
                        base(
                            DragKind::PanPhoto,
                            self.id_at(i),
                            *e.map_or(&DEFAULT_RECT, Element::rect),
                            crop_of(e),
                        )
                    })
                })
            } else if let Some((i, r, h)) = selected_hit {
                Some(base(
                    DragKind::Element(h),
                    self.id_at(i),
                    r,
                    crop_of(self.elements().get(i)),
                ))
            } else if let Some(i) = geometry::hit_test(self.elements(), pt) {
                self.selected = Some(i);
                let e = self.elements().get(i);
                Some(base(
                    DragKind::Element(Handle::Body),
                    self.id_at(i),
                    *e.map_or(&DEFAULT_RECT, Element::rect),
                    crop_of(e),
                ))
            } else {
                self.selected = None;
                None
            };
            if matches!(
                self.drag,
                Some(Drag {
                    kind: DragKind::Element(_) | DragKind::PanPhoto,
                    ..
                })
            ) {
                self.checkpoint();
            }
        }

        if let (Some(d), Some(pos)) = (self.drag.clone(), response.interact_pointer_pos())
            && response.dragged()
        {
            let pt = view.to_page(pos);
            let index = d.id.as_deref().and_then(|id| self.index_of(id));
            if index.is_none() && !matches!(d.kind, DragKind::View) {
                // The element went away mid-drag (undo/delete): stop.
                self.drag = None;
                self.guides.clear();
                return;
            }
            let index = index.unwrap_or(usize::MAX);
            let (dx, dy) = (pt.x - d.start_pt.x, pt.y - d.start_pt.y);
            self.mutated |= !matches!(d.kind, DragKind::View);
            match d.kind {
                DragKind::View => {
                    self.view_pan = d.start_view + (pos - d.start_screen);
                }
                DragKind::PanPhoto => {
                    let size = self.photo_size(index);
                    if let (Some((w, h)), Some(Element::Photo { rect, crop, .. })) =
                        (size, self.element_mut(index))
                    {
                        *crop = pan_by(rect.w, rect.h, w, h, &d.start_crop, dx, dy);
                    }
                }
                DragKind::Element(h) => {
                    let (tw, th) = self.trim();
                    let (bleed, safe) = (self.outer(), self.product.safe_mm);
                    let others: Vec<Rect> = self
                        .elements()
                        .iter()
                        .enumerate()
                        .filter(|(j, _)| *j != index)
                        .map(|(_, e)| *e.rect())
                        .collect();
                    let threshold = SNAP_PX / f64::from(view.scale);
                    let moved = geometry::resize(&d.start_rect, h, dx, dy, shift);
                    let snapped = match h {
                        Handle::Body => {
                            snap::snap_move(moved, tw, th, bleed, safe, &others, threshold)
                        }
                        Handle::TopLeft => snap::snap_resize(
                            moved, true, true, tw, th, bleed, safe, &others, threshold,
                        ),
                        Handle::TopRight => snap::snap_resize(
                            moved, false, true, tw, th, bleed, safe, &others, threshold,
                        ),
                        Handle::BottomRight => snap::snap_resize(
                            moved, false, false, tw, th, bleed, safe, &others, threshold,
                        ),
                        Handle::BottomLeft => snap::snap_resize(
                            moved, true, false, tw, th, bleed, safe, &others, threshold,
                        ),
                    };
                    self.guides = snapped.guides;
                    let lim = MAX_PAGE_EDGE_MM;
                    let mut r = snapped.rect;
                    r.x = r.x.clamp(-lim, lim);
                    r.y = r.y.clamp(-lim, lim);
                    r.w = r.w.clamp(geometry::MIN_SIZE_MM, lim);
                    r.h = r.h.clamp(geometry::MIN_SIZE_MM, lim);
                    // Keep stored values tidy: 0.01 mm is far below print precision.
                    let round = |v: f64| (v * 100.0).round() / 100.0;
                    r = Rect {
                        x: round(r.x),
                        y: round(r.y),
                        w: round(r.w),
                        h: round(r.h),
                        ..r
                    };
                    if let Some(el) = self.element_mut(index) {
                        *rect_mut(el) = r;
                    }
                }
            }
        }

        if response.drag_stopped() {
            self.drag = None;
            self.guides.clear();
        }
        if response.clicked()
            && let Some(pos) = response.interact_pointer_pos()
        {
            self.selected = geometry::hit_test(self.elements(), view.to_page(pos));
        }
    }

    fn paint(&self, painter: &egui::Painter, view: View, art_min: Pos2, art_size: Vec2) {
        let (tw, th) = self.trim();
        let outer = self.outer();
        let artwork = ERect::from_min_size(art_min, art_size);
        let trim = view.rect(0.0, 0.0, tw, th);
        let bg = edit::page(&self.project, self.page)
            .map_or(Color32::WHITE, |p| parse_colour(&p.background));
        painter.rect_filled(artwork.expand(1.0), 0.0, Color32::from_gray(70));
        painter.rect_filled(artwork, 0.0, bg);

        for (i, el) in self.elements().iter().enumerate() {
            self.paint_element(painter, view, el, self.selected == Some(i));
        }

        // Everything outside the trim gets cut off: tint it.
        if self.show_bleed && outer > 0.0 {
            for strip in ring(artwork, trim) {
                painter.rect_filled(strip, 0.0, Color32::from_rgba_unmultiplied(220, 50, 50, 70));
            }
        }
        painter.rect_stroke(trim, 0.0, Stroke::new(1.0, TRIM), StrokeKind::Middle);
        if self.show_safe {
            let s = self.product.safe_mm;
            let r = view.rect(s, s, tw - 2.0 * s, th - 2.0 * s);
            let pts = [
                r.left_top(),
                r.right_top(),
                r.right_bottom(),
                r.left_bottom(),
                r.left_top(),
            ];
            painter.extend(egui::Shape::dashed_line(
                &pts,
                Stroke::new(1.0, SAFE),
                6.0,
                4.0,
            ));
        }
        for g in &self.guides {
            let (a, b) = match *g {
                Guide::V(x) => (
                    view.to_screen(Pt::new(x, -outer)),
                    view.to_screen(Pt::new(x, th + outer)),
                ),
                Guide::H(y) => (
                    view.to_screen(Pt::new(-outer, y)),
                    view.to_screen(Pt::new(tw + outer, y)),
                ),
            };
            painter.line_segment([a, b], Stroke::new(1.0, GUIDE));
        }
        // Selection on top of the tint.
        if let Some(el) = self.selected.and_then(|i| self.elements().get(i)) {
            let c = corners(el.rect()).map(|p| view.to_screen(p));
            let mut ring_pts = c.to_vec();
            ring_pts.extend(c.first().copied());
            painter.add(egui::Shape::line(ring_pts, Stroke::new(2.0, SELECT)));
            for p in c {
                painter.rect_filled(
                    ERect::from_center_size(p, Vec2::splat(HANDLE_PX * 2.0)),
                    2.0,
                    Color32::WHITE,
                );
                painter.rect_stroke(
                    ERect::from_center_size(p, Vec2::splat(HANDLE_PX * 2.0)),
                    2.0,
                    Stroke::new(1.5, SELECT),
                    StrokeKind::Middle,
                );
            }
        }
        if self.on_cover()
            && let Some(g) = self.cover_geom
        {
            for x in [g.spine_x0, g.spine_x1] {
                let pts = [
                    view.to_screen(Pt::new(x, -outer)),
                    view.to_screen(Pt::new(x, th + outer)),
                ];
                painter.extend(egui::Shape::dashed_line(
                    &pts,
                    Stroke::new(1.0, Color32::from_rgb(200, 120, 0)),
                    5.0,
                    4.0,
                ));
            }
            for (x, label) in [
                (g.spine_x0 / 2.0, "Back"),
                ((g.spine_x0 + g.spine_x1) / 2.0, "Spine"),
                ((g.spine_x1 + g.width) / 2.0, "Front"),
            ] {
                painter.text(
                    view.to_screen(Pt::new(x, -outer)) - Vec2::new(0.0, 4.0),
                    egui::Align2::CENTER_BOTTOM,
                    label,
                    egui::FontId::proportional(12.0),
                    Color32::from_rgb(200, 120, 0),
                );
            }
        }
        let where_ = if self.on_cover() {
            "cover (back · spine · front)".to_owned()
        } else {
            format!("page {} of {}", self.page + 1, self.project.pages.len())
        };
        painter.text(
            artwork.center_bottom() + Vec2::new(0.0, 6.0),
            egui::Align2::CENTER_TOP,
            format!("{tw:.0} × {th:.0} mm · {where_}"),
            egui::FontId::proportional(12.0),
            Color32::from_gray(160),
        );
    }

    fn paint_element(&self, painter: &egui::Painter, view: View, el: &Element, selected: bool) {
        let quad = corners(el.rect()).map(|p| view.to_screen(p));
        match el {
            Element::Photo {
                rect, src, crop, ..
            } => {
                let photo = src.as_ref().and_then(|s| self.library.get(s.as_str()));
                let ready = photo.and_then(|p| p.texture().zip(p.size));
                match ready {
                    Some((texture, (width, height))) => {
                        if let Some(v) = crop_view(rect.w, rect.h, width, height, crop) {
                            let uv = [
                                Pos2::new(v.u0 as f32, v.v0 as f32),
                                Pos2::new(v.u1 as f32, v.v0 as f32),
                                Pos2::new(v.u1 as f32, v.v1 as f32),
                                Pos2::new(v.u0 as f32, v.v1 as f32),
                            ];
                            let mut mesh = egui::Mesh::with_texture(texture.id());
                            for (p, t) in quad.iter().zip(uv) {
                                mesh.vertices.push(egui::epaint::Vertex {
                                    pos: *p,
                                    uv: t,
                                    color: Color32::WHITE,
                                });
                            }
                            mesh.add_triangle(0, 1, 2);
                            mesh.add_triangle(0, 2, 3);
                            painter.add(egui::Shape::mesh(mesh));
                            if let Some(ppi) = effective_ppi(rect.w, rect.h, width, height, crop) {
                                let q = grade_ppi(ppi);
                                let anchor = quad[0] + Vec2::new(8.0, 8.0);
                                let label = format!("{ppi:.0} ppi");
                                let badge = ERect::from_min_size(anchor, Vec2::new(64.0, 18.0));
                                painter.rect_filled(badge, 9.0, Color32::from_black_alpha(160));
                                painter.circle_filled(
                                    badge.left_center() + Vec2::new(9.0, 0.0),
                                    4.5,
                                    quality_colour(q),
                                );
                                painter.text(
                                    badge.left_center() + Vec2::new(17.0, 0.0),
                                    egui::Align2::LEFT_CENTER,
                                    label,
                                    egui::FontId::proportional(11.0),
                                    Color32::WHITE,
                                );
                            }
                        }
                    }
                    None => {
                        painter.add(egui::Shape::convex_polygon(
                            quad.to_vec(),
                            Color32::from_gray(225),
                            Stroke::new(1.0, Color32::from_gray(160)),
                        ));
                        let msg = match (src, photo.map(|p| &p.state)) {
                            (None, _) => "Empty frame\nclick a photo below",
                            (_, Some(State::Loading | State::Idle | State::Ready { .. })) => {
                                "Decoding photo…"
                            }
                            (_, Some(State::Failed(_))) => "This photo can't be opened",
                            (Some(_), None) => "Photo missing",
                        };
                        let c = view.to_screen(geometry::centre(rect));
                        painter.text(
                            c,
                            egui::Align2::CENTER_CENTER,
                            msg,
                            egui::FontId::proportional(12.0),
                            Color32::from_gray(90),
                        );
                    }
                }
                if !selected {
                    let mut pts = quad.to_vec();
                    pts.extend(quad.first().copied());
                    painter.add(egui::Shape::dashed_line(
                        &pts,
                        Stroke::new(0.5, Color32::from_gray(150)),
                        4.0,
                        4.0,
                    ));
                }
            }
            Element::Text {
                rect,
                text,
                font,
                size_pt,
                colour,
                align,
                ..
            } => {
                let px = (*size_pt as f32) * 25.4 / 72.0 * view.scale;
                if !(px.is_finite() && px > 1.0) {
                    return;
                }
                let family = if font == "mono" {
                    egui::FontFamily::Monospace
                } else {
                    egui::FontFamily::Proportional
                };
                let mut job = egui::text::LayoutJob::simple(
                    text.clone(),
                    egui::FontId::new(px.min(2000.0), family),
                    parse_colour(colour),
                    (rect.w as f32) * view.scale,
                );
                let (halign, ax) = match align {
                    Align::Left => (egui::Align::LEFT, 0.0),
                    Align::Center => (egui::Align::Center, rect.w / 2.0),
                    Align::Right => (egui::Align::RIGHT, rect.w),
                };
                job.halign = halign;
                let galley = painter.layout_job(job);
                let anchor = geometry::rotate(
                    Pt::new(rect.x + ax, rect.y),
                    geometry::centre(rect),
                    rect.rotation,
                );
                let shape = egui::epaint::TextShape::new(
                    view.to_screen(anchor),
                    galley,
                    parse_colour(colour),
                )
                .with_angle(rect.rotation.to_radians() as f32);
                painter.add(shape);
                if !selected {
                    let mut pts = quad.to_vec();
                    pts.extend(quad.first().copied());
                    painter.add(egui::Shape::dashed_line(
                        &pts,
                        Stroke::new(0.5, Color32::from_gray(170)),
                        3.0,
                        4.0,
                    ));
                }
            }
        }
    }
}

const DEFAULT_RECT: Rect = Rect {
    x: 0.0,
    y: 0.0,
    w: 1.0,
    h: 1.0,
    rotation: 0.0,
};

fn rect_mut(el: &mut Element) -> &mut Rect {
    match el {
        Element::Photo { rect, .. } | Element::Text { rect, .. } => rect,
    }
}

fn rect_controls(ui: &mut egui::Ui, r: &mut Rect) {
    let lim = MAX_PAGE_EDGE_MM;
    egui::Grid::new("rect")
        .num_columns(4)
        .spacing([6.0, 4.0])
        .show(ui, |ui| {
            ui.label("X");
            ui.add(
                egui::DragValue::new(&mut r.x)
                    .range(-lim..=lim)
                    .speed(0.5)
                    .clamp_existing_to_range(false)
                    .fixed_decimals(1)
                    .suffix(" mm"),
            );
            ui.label("Y");
            ui.add(
                egui::DragValue::new(&mut r.y)
                    .range(-lim..=lim)
                    .speed(0.5)
                    .clamp_existing_to_range(false)
                    .fixed_decimals(1)
                    .suffix(" mm"),
            );
            ui.end_row();
            ui.label("W");
            ui.add(
                egui::DragValue::new(&mut r.w)
                    .range(geometry::MIN_SIZE_MM..=lim)
                    .speed(0.5)
                    .clamp_existing_to_range(false)
                    .fixed_decimals(1)
                    .suffix(" mm"),
            );
            ui.label("H");
            ui.add(
                egui::DragValue::new(&mut r.h)
                    .range(geometry::MIN_SIZE_MM..=lim)
                    .speed(0.5)
                    .clamp_existing_to_range(false)
                    .fixed_decimals(1)
                    .suffix(" mm"),
            );
            ui.end_row();
        });
    ui.add(
        egui::Slider::new(&mut r.rotation, -180.0..=180.0)
            .clamping(egui::SliderClamping::Never)
            .text("rotation °"),
    );
}

fn ring(outer: ERect, inner: ERect) -> [ERect; 4] {
    [
        ERect::from_min_max(outer.min, Pos2::new(outer.max.x, inner.min.y)),
        ERect::from_min_max(Pos2::new(outer.min.x, inner.max.y), outer.max),
        ERect::from_min_max(
            Pos2::new(outer.min.x, inner.min.y),
            Pos2::new(inner.min.x, inner.max.y),
        ),
        ERect::from_min_max(
            Pos2::new(inner.max.x, inner.min.y),
            Pos2::new(outer.max.x, inner.max.y),
        ),
    ]
}
