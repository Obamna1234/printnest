//! PrintNest desktop app.
//!
//! Milestone 0/1 window: browse the product catalogue, see each product's
//! print geometry (trim, bleed, safe zone, wrap, mat, gutter, fold), work out
//! book spines and check whether a photo is sharp enough to print.
//!
//! The full editor arrives in milestone 3. This window performs no file or
//! network I/O at all.

#![forbid(unsafe_code)]
// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod preview;

use eframe::egui;
use printnest_model::catalogue::{Catalogue, Family, Product};
use printnest_model::units::{Mm, PrintQuality, Size, grade_photo};

fn main() -> eframe::Result {
    let catalogue = match Catalogue::builtin() {
        Ok(c) => c,
        Err(e) => {
            // Only reachable if the shipped catalogue is broken; tests prevent it.
            return Err(eframe::Error::AppCreation(Box::new(e)));
        }
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("PrintNest")
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([820.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native(
        "PrintNest",
        options,
        Box::new(|_cc| Ok(Box::new(PrintNestApp::new(catalogue)))),
    )
}

/// Options shown on the canvas.
#[derive(Debug, Clone, Copy)]
struct Overlays {
    bleed: bool,
    safe: bool,
    extras: bool,
}

#[derive(Debug)]
struct PrintNestApp {
    catalogue: Catalogue,
    family: Family,
    product_id: String,
    paper_id: String,
    cover_id: String,
    pages: u32,
    photo_w: u32,
    photo_h: u32,
    overlays: Overlays,
}

impl PrintNestApp {
    fn new(catalogue: Catalogue) -> Self {
        let mut app = PrintNestApp {
            catalogue,
            family: Family::Book,
            product_id: String::new(),
            paper_id: String::new(),
            cover_id: String::new(),
            pages: 0,
            photo_w: 4000,
            photo_h: 3000,
            overlays: Overlays {
                bleed: true,
                safe: true,
                extras: true,
            },
        };
        let first = app
            .catalogue
            .in_family(Family::Book)
            .next()
            .map(|p| p.id.clone());
        if let Some(id) = first {
            app.select(&id);
        }
        app
    }

    fn product(&self) -> Option<&Product> {
        self.catalogue.product(&self.product_id)
    }

    /// Switches product and resets dependent choices to valid defaults.
    fn select(&mut self, id: &str) {
        let Some(p) = self.catalogue.product(id) else {
            return;
        };
        self.family = p.family;
        self.product_id = p.id.clone();
        self.paper_id = p.papers.first().cloned().unwrap_or_default();
        self.cover_id = p.covers.first().map(|c| c.id.clone()).unwrap_or_default();
        self.pages = p.default_pages();
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.heading("Products");
        ui.add_space(4.0);
        let mut picked: Option<String> = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for family in Family::ALL {
                let open = family == self.family;
                egui::CollapsingHeader::new(family.label())
                    .id_salt(family.label())
                    .open(if open { Some(true) } else { None })
                    .show(ui, |ui| {
                        for p in self.catalogue.in_family(family) {
                            let selected = p.id == self.product_id;
                            if ui.selectable_label(selected, &p.name).clicked() {
                                picked = Some(p.id.clone());
                            }
                        }
                    });
            }
        });
        if let Some(id) = picked {
            self.select(&id);
        }
    }

    fn details(&mut self, ui: &mut egui::Ui) {
        let Some(p) = self.product().cloned() else {
            ui.label("Pick a product on the left.");
            return;
        };
        ui.add_space(6.0);
        ui.heading(&p.name);
        ui.label(egui::RichText::new(p.family.label()).weak());
        ui.add_space(8.0);

        let trim = p.trim();
        let art = p.artwork();
        egui::Grid::new("geometry")
            .num_columns(2)
            .spacing([12.0, 4.0])
            .show(ui, |ui| {
                row(ui, "Finished size", &fmt_size(trim));
                row(
                    ui,
                    "",
                    &format!(
                        "{:.2} × {:.2} in",
                        trim.width.inches(),
                        trim.height.inches()
                    ),
                );
                row(ui, "Bleed", &p.bleed().to_string());
                if let Some(w) = p.wrap_mm {
                    row(ui, "Canvas wrap", &Mm(w).to_string());
                }
                row(ui, "Artwork size", &fmt_size(art));
                row(
                    ui,
                    "Pixels at 300 ppi",
                    &format!(
                        "{} × {}",
                        art.width.pixels_at(300.0),
                        art.height.pixels_at(300.0)
                    ),
                );
                row(ui, "Safe margin", &p.safe().to_string());
                if let Some(m) = p.mat_opening {
                    row(ui, "Mat opening", &fmt_size(m.to_mm()));
                }
                if let Some(g) = p.gutter_mm {
                    row(ui, "Gutter", &Mm(g).to_string());
                }
                if let Some(b) = p.binding_mm {
                    row(ui, "Binding margin", &Mm(b).to_string());
                }
            });

        ui.add_space(10.0);
        ui.separator();
        ui.label(egui::RichText::new("Paper").strong());
        egui::ComboBox::from_id_salt("paper")
            .selected_text(
                self.catalogue
                    .paper(&self.paper_id)
                    .map_or("—", |x| x.name.as_str()),
            )
            .width(220.0)
            .show_ui(ui, |ui| {
                for id in &p.papers {
                    if let Some(paper) = self.catalogue.paper(id) {
                        ui.selectable_value(&mut self.paper_id, id.clone(), &paper.name);
                    }
                }
            });

        let rule = p.page_rule();
        if rule.max > rule.min {
            ui.add_space(6.0);
            ui.label(egui::RichText::new("Pages").strong());
            ui.add(
                egui::Slider::new(&mut self.pages, rule.min..=rule.max)
                    .step_by(f64::from(rule.step)),
            );
        }

        if !p.covers.is_empty() {
            ui.add_space(6.0);
            ui.label(egui::RichText::new("Cover").strong());
            ui.horizontal_wrapped(|ui| {
                for c in &p.covers {
                    ui.radio_value(&mut self.cover_id, c.id.clone(), &c.name);
                }
            });
            if let Some(paper) = self.catalogue.paper(&self.paper_id)
                && let (Some(spine), Some(spread)) = (
                    p.spine_width(self.pages, paper, &self.cover_id),
                    p.cover_spread(self.pages, paper, &self.cover_id),
                )
            {
                ui.add_space(4.0);
                egui::Grid::new("cover").num_columns(2).show(ui, |ui| {
                    row(ui, "Spine width", &spine.to_string());
                    row(ui, "Flat cover", &fmt_size(spread));
                });
            }
        }

        ui.add_space(10.0);
        ui.separator();
        ui.label(egui::RichText::new("Will my photo print sharply?").strong());
        ui.horizontal(|ui| {
            ui.add(
                egui::DragValue::new(&mut self.photo_w)
                    .range(1..=50_000)
                    .suffix(" px"),
            );
            ui.label("×");
            ui.add(
                egui::DragValue::new(&mut self.photo_h)
                    .range(1..=50_000)
                    .suffix(" px"),
            );
        });
        let art_oriented = orient(art, self.photo_w, self.photo_h);
        if let Some((ppi, grade)) = grade_photo(
            u64::from(self.photo_w),
            u64::from(self.photo_h),
            art_oriented,
        ) {
            let (colour, words) = match grade {
                PrintQuality::Good => (egui::Color32::from_rgb(46, 160, 67), "Sharp"),
                PrintQuality::Fair => (egui::Color32::from_rgb(210, 150, 20), "Acceptable"),
                PrintQuality::Poor => (egui::Color32::from_rgb(210, 60, 50), "Likely blurry"),
            };
            ui.horizontal(|ui| {
                let (dot, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                ui.painter().circle_filled(dot.center(), 5.0, colour);
                ui.label(
                    egui::RichText::new(format!("{words}: {ppi:.0} ppi full-bleed"))
                        .color(colour)
                        .strong(),
                );
            });
        }

        ui.add_space(10.0);
        ui.separator();
        ui.label(egui::RichText::new("How you can print it").strong());
        for e in &p.exports {
            ui.label(format!("• {}", e.label()));
        }
        if !p.verified {
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(
                    "Size not yet checked against a retailer. Confirm with your print shop.",
                )
                .weak()
                .small(),
            );
        }
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.overlays.bleed, "Bleed");
            ui.checkbox(&mut self.overlays.safe, "Safe zone");
            ui.checkbox(&mut self.overlays.extras, "Wrap / mat / gutter / fold");
        });
        ui.add_space(4.0);
        if let Some(p) = self.product().cloned() {
            preview::draw(
                ui,
                &p,
                self.overlays.bleed,
                self.overlays.safe,
                self.overlays.extras,
            );
        }
    }
}

impl eframe::App for PrintNestApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(
                        "Offline · no network access · your photos never leave this computer",
                    )
                    .small(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!(
                            "PrintNest {} · editor arrives in milestone 3",
                            env!("CARGO_PKG_VERSION")
                        ))
                        .small()
                        .weak(),
                    );
                });
            });
        });
        egui::Panel::left("products")
            .default_size(250.0)
            .show(ui, |ui| self.sidebar(ui));
        egui::Panel::right("details")
            .default_size(300.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.details(ui));
            });
        egui::CentralPanel::default_margins().show(ui, |ui| self.canvas(ui));
    }
}

fn row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.label(egui::RichText::new(label).weak());
    ui.label(value);
    ui.end_row();
}

fn fmt_size(s: Size) -> String {
    format!("{:.1} × {:.1} mm", s.width.0, s.height.0)
}

/// Rotates the artwork to match the photo's orientation.
fn orient(art: Size, w: u32, h: u32) -> Size {
    if (w >= h) == (art.width.0 >= art.height.0) {
        art
    } else {
        Size::new(art.height, art.width)
    }
}
