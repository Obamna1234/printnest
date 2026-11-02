//! Export: turns a design into files a printer accepts (SPEC section 6).
//!
//! - **Kiosk / photo lab**: one JPEG per page at exactly the print's
//!   proportions, ≤ 300 ppi and ≤ 10,000 px a side, plus `PRINT-ME.txt`.
//! - **Print shop**: one PDF with bleed, trim/bleed boxes and crop marks.
//! - **Home**: A4 or Letter PDF at 100% scale; anything larger is split
//!   over several sheets with overlap and labelled for taping together.
//!
//! Every file is new: exports go into a freshly created folder inside the
//! folder the user picked, and nothing existing is overwritten. JPEGs are
//! encoded from rendered pixels, so no camera metadata (GPS, serial
//! numbers) can survive into them. Same design → same bytes.

#![forbid(unsafe_code)]

pub mod pdf;
pub mod preflight;

use image::codecs::jpeg::{JpegEncoder, PixelDensity};
use printnest_model::catalogue::Product;
use printnest_model::project::Project;
use printnest_model::safe_path::SafePath;
use printnest_render::{Area, Photo, Raster, RenderError, needed_edge, page_pixels, render_page};
use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use preflight::{Issue, Severity, preflight};

/// Highest resolution exported.
pub const TARGET_PPI: f64 = 300.0;
/// Kiosks and photo labs reject very large files.
pub const KIOSK_MAX_EDGE: u32 = 10_000;
/// JPEG quality (sharp, still reasonably small).
pub const JPEG_QUALITY: u8 = 92;
/// Largest photo edge requested from the decoder.
pub const MAX_PHOTO_EDGE: u32 = 8_192;

/// Home printer paper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Paper {
    A4,
    Letter,
}

impl Paper {
    /// Width and height in mm (portrait).
    #[must_use]
    pub fn size_mm(self) -> (f64, f64) {
        match self {
            Paper::A4 => (210.0, 297.0),
            Paper::Letter => (215.9, 279.4),
        }
    }
    fn name(self) -> &'static str {
        match self {
            Paper::A4 => "A4",
            Paper::Letter => "Letter",
        }
    }
}

/// What to export for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    Kiosk,
    PrintShop,
    Home(Paper),
}

impl Profile {
    fn folder_suffix(self) -> &'static str {
        match self {
            Profile::Kiosk => "kiosk",
            Profile::PrintShop => "print shop",
            Profile::Home(Paper::A4) => "home A4",
            Profile::Home(Paper::Letter) => "home Letter",
        }
    }
}

/// Why an export failed.
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error(transparent)]
    Render(#[from] RenderError),
    #[error("couldn't write the export: {0}")]
    Io(String),
    #[error("couldn't encode the image: {0}")]
    Encode(String),
    #[error("the export was cancelled")]
    Cancelled,
}

fn io(e: impl std::fmt::Display) -> ExportError {
    ExportError::Io(e.to_string())
}

/// What an export produced.
#[derive(Debug, Clone)]
pub struct ExportReport {
    /// The new folder holding everything.
    pub folder: PathBuf,
    pub files: Vec<PathBuf>,
    /// Resolution the pages were rendered at.
    pub ppi: f64,
}

/// Supplies decoded photos (through the sandbox) at no more than the given
/// edge length. Returning `None` leaves the photo out.
pub type PhotoLoader<'a> = dyn FnMut(&str, u32) -> Option<Arc<Photo>> + 'a;

/// Called after each page with (done, total); return `false` to cancel.
pub type Progress<'a> = dyn FnMut(usize, usize) -> bool + 'a;

/// A file-name-safe version of `name` (letters, digits, space, - and _).
#[must_use]
pub fn safe_base_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(60)
        .collect();
    let trimmed = cleaned.trim().trim_matches('_').trim();
    // Checked as a file name too: `CON.jpg` is reserved on Windows.
    let usable = SafePath::new(&format!("{trimmed} - kiosk")).is_ok()
        && SafePath::new(&format!("{trimmed}.jpg")).is_ok();
    if trimmed.is_empty() || !usable {
        "PrintNest".into()
    } else {
        trimmed.to_owned()
    }
}

/// Creates `<parent>/<base> - <suffix>` (or ` (2)`, ` (3)`… if taken).
fn new_folder(parent: &Path, base: &str, suffix: &str) -> Result<PathBuf, ExportError> {
    for n in 1..100u32 {
        let name = if n == 1 {
            format!("{base} - {suffix}")
        } else {
            format!("{base} - {suffix} ({n})")
        };
        let dir = parent.join(name);
        match fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(io(e)),
        }
    }
    Err(ExportError::Io(
        "too many earlier exports with this name".into(),
    ))
}

/// Writes a new file (never replacing one).
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), ExportError> {
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(io)?;
    f.write_all(bytes).and_then(|()| f.sync_all()).map_err(io)
}

/// Encodes a rendered page as a baseline JPEG with its print resolution.
///
/// # Errors
/// If encoding fails.
pub fn encode_jpeg(r: &Raster, ppi: f64) -> Result<Vec<u8>, ExportError> {
    let mut out = Vec::new();
    let mut enc = JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY);
    let dpi = ppi.round().clamp(1.0, f64::from(u16::MAX)) as u16;
    enc.set_pixel_density(PixelDensity::dpi(dpi));
    enc.encode(&r.rgb, r.width, r.height, image::ExtendedColorType::Rgb8)
        .map_err(|e| ExportError::Encode(e.to_string()))?;
    Ok(out)
}

/// The resolution to render `area` at: [`TARGET_PPI`], lowered if needed
/// to stay within `max_edge` pixels and the renderer's limits.
#[must_use]
pub fn choose_ppi(product: &Product, area: Area, max_edge: u32) -> f64 {
    let (w, h) = printnest_render::area_mm(product, area);
    let longest_in = w.max(h) / 25.4;
    let mut ppi = TARGET_PPI.min(f64::from(max_edge) / longest_in).floor();
    while ppi > 30.0 && page_pixels(product, area, ppi).is_err() {
        ppi = (ppi * 0.9).floor();
    }
    ppi.max(30.0)
}

/// Renders page `i`, decoding only the photos it shows.
fn render(
    project: &Project,
    product: &Product,
    i: usize,
    ppi: f64,
    area: Area,
    loader: &mut PhotoLoader<'_>,
) -> Result<Raster, ExportError> {
    let mut photos = HashMap::new();
    let srcs: HashSet<String> = project
        .pages
        .get(i)
        .map(|p| {
            p.elements
                .iter()
                .filter_map(|e| match e {
                    printnest_model::project::Element::Photo { src: Some(s), .. } => {
                        Some(s.as_str().to_owned())
                    }
                    printnest_model::project::Element::Photo { .. }
                    | printnest_model::project::Element::Text { .. } => None,
                })
                .collect()
        })
        .unwrap_or_default();
    for src in srcs {
        let edge = needed_edge(project, i, &src, ppi, MAX_PHOTO_EDGE);
        if let Some(p) = loader(&src, edge) {
            photos.insert(src, p);
        }
    }
    Ok(render_page(project, product, i, &photos, ppi, area)?)
}

/// Exports `project` for `profile` into a new folder inside `parent`.
///
/// # Errors
/// On render, encode or write failure, or if `progress` cancels. Files
/// already written stay in the new folder.
pub fn export(
    project: &Project,
    product: &Product,
    profile: Profile,
    parent: &Path,
    loader: &mut PhotoLoader<'_>,
    progress: &mut Progress<'_>,
) -> Result<ExportReport, ExportError> {
    let base = safe_base_name(&project.title);
    let folder = new_folder(parent, &base, profile.folder_suffix())?;
    let pages = project.pages.len();
    let mut files = Vec::new();
    let area = match profile {
        Profile::PrintShop => Area::Artwork,
        Profile::Kiosk | Profile::Home(_) => Area::Trim,
    };
    let ppi = choose_ppi(product, area, KIOSK_MAX_EDGE);
    let width = pages.to_string().len().max(2);
    let mut images = Vec::new();
    for i in 0..pages {
        let raster = render(project, product, i, ppi, area, loader)?;
        let jpeg = encode_jpeg(&raster, ppi)?;
        match profile {
            Profile::Kiosk => {
                let name = if pages == 1 {
                    format!("{base}.jpg")
                } else {
                    format!("{base}-{:0width$}.jpg", i + 1)
                };
                let path = folder.join(name);
                write_new(&path, &jpeg)?;
                files.push(path);
            }
            Profile::PrintShop | Profile::Home(_) => images.push(pdf::Jpeg {
                width: raster.width,
                height: raster.height,
                data: jpeg,
            }),
        }
        if !progress(i + 1, pages) {
            return Err(ExportError::Cancelled);
        }
    }
    match profile {
        Profile::Kiosk => {
            let path = folder.join("PRINT-ME.txt");
            write_new(&path, kiosk_note(project, product, pages).as_bytes())?;
            files.push(path);
        }
        Profile::PrintShop => {
            let path = folder.join(format!("{base} - print shop.pdf"));
            let pdf_pages = shop_pages(product, images.len());
            write_new(&path, &pdf::write(&project.title, &pdf_pages, &images))?;
            files.push(path);
            let note = folder.join("PRINT-ME.txt");
            write_new(&note, shop_note(product).as_bytes())?;
            files.push(note);
        }
        Profile::Home(paper) => {
            let path = folder.join(format!("{base} - {}.pdf", paper.name()));
            let pdf_pages = home_pages(product, paper, images.len());
            write_new(&path, &pdf::write(&project.title, &pdf_pages, &images))?;
            files.push(path);
        }
    }
    Ok(ExportReport { folder, files, ppi })
}

fn size_text(product: &Product) -> String {
    let t = product.trim();
    format!(
        "{:.0} × {:.0} mm ({:.1} × {:.1} in)",
        t.width.0,
        t.height.0,
        t.width.0 / 25.4,
        t.height.0 / 25.4
    )
}

fn kiosk_note(project: &Project, product: &Product, pages: usize) -> String {
    format!(
        "PrintNest export: {title}\r\n\
         Product: {name}\r\n\
         Print size: {size}\r\n\
         Files: {pages} JPEG image(s)\r\n\r\n\
         How to print at a photo kiosk or online photo lab\r\n\
         1. Copy the .jpg files to a USB stick, SD card or your phone.\r\n\
         2. At the kiosk, choose the print size above.\r\n\
         3. If the kiosk offers \"fill\", \"crop to fit\" or \"auto-enhance\", turn it\r\n\
            off: the files are already exactly the right shape and colour.\r\n\
         4. Print one first to check it before printing many.\r\n\r\n\
         These files contain no location or camera information.\r\n",
        title = project.title,
        name = product.name,
        size = size_text(product),
    )
}

fn shop_note(product: &Product) -> String {
    format!(
        "PrintNest print-shop PDF\r\n\
         Finished (trim) size: {size}\r\n\
         Bleed: {bleed} mm on every side (included), crop marks outside it.\r\n\
         Colour: sRGB images. Ask the shop to print at 100% (no scaling).\r\n",
        size = size_text(product),
        bleed = product.bleed_mm + product.wrap_mm.unwrap_or(0.0),
    )
}

/// Print-shop pages: artwork (trim + bleed) with crop marks around it.
fn shop_pages(product: &Product, n: usize) -> Vec<pdf::Page> {
    use pdf::{Op, pt};
    let t = product.trim();
    let outer = product.bleed_mm + product.wrap_mm.unwrap_or(0.0);
    let margin = 12.0; // room for crop marks
    let (aw, ah) = (t.width.0 + 2.0 * outer, t.height.0 + 2.0 * outer);
    let media = (pt(aw + 2.0 * margin), pt(ah + 2.0 * margin));
    let (bx0, by0) = (pt(margin), pt(margin));
    let (bx1, by1) = (pt(margin + aw), pt(margin + ah));
    let (tx0, ty0) = (pt(margin + outer), pt(margin + outer));
    let (tx1, ty1) = (
        pt(margin + outer + t.width.0),
        pt(margin + outer + t.height.0),
    );
    let (gap, len) = (pt(outer + 2.0), pt(outer + 10.0));
    let mark = |from: (f64, f64), to: (f64, f64)| Op::Line {
        from,
        to,
        width: 0.25,
        grey: 0.0,
    };
    (0..n)
        .map(|i| {
            let mut ops = vec![Op::Image {
                index: i,
                x: bx0,
                y: by0,
                w: bx1 - bx0,
                h: by1 - by0,
                clip: None,
            }];
            for (x, dir) in [(tx0, -1.0), (tx1, 1.0)] {
                for (y, vdir) in [(ty0, -1.0), (ty1, 1.0)] {
                    // Horizontal and vertical marks at each trim corner,
                    // outside the bleed.
                    ops.push(mark((x + dir * gap, y), (x + dir * len, y)));
                    ops.push(mark((x, y + vdir * gap), (x, y + vdir * len)));
                }
            }
            pdf::Page {
                media,
                trim: Some([tx0, ty0, tx1, ty1]),
                bleed: Some([bx0, by0, bx1, by1]),
                ops,
            }
        })
        .collect()
}

/// Home pages: the design at 100% on A4/Letter, tiled if it doesn't fit.
fn home_pages(product: &Product, paper: Paper, n: usize) -> Vec<pdf::Page> {
    use pdf::{Op, pt};
    let t = product.trim();
    let (dw, dh) = (t.width.0, t.height.0);
    let margin = 8.0; // most home printers can't print to the edge
    let overlap = 10.0;
    let (pw0, ph0) = paper.size_mm();
    // Use the paper orientation that matches the design.
    let (pw, ph) = if (dw > dh) == (pw0 > ph0) {
        (pw0, ph0)
    } else {
        (ph0, pw0)
    };
    let (avail_w, avail_h) = (pw - 2.0 * margin, ph - 2.0 * margin - 8.0); // 8 mm for labels
    let cut = |from: (f64, f64), to: (f64, f64)| Op::Line {
        from,
        to,
        width: 0.3,
        grey: 0.6,
    };
    let mut pages = Vec::new();
    for i in 0..n {
        // Fits on one sheet: at 100% if it fits inside the printer's
        // margins, otherwise (a design the size of the paper itself)
        // shrunk just enough to fit, rather than split over four sheets.
        let fits_paper = dw <= pw + 0.01 && dh <= ph + 0.01;
        if fits_paper {
            let scale = (avail_w / dw).min(avail_h / dh).min(1.0);
            let (sw, sh) = (dw * scale, dh * scale);
            let (x0, y0) = ((pw - sw) / 2.0, (ph - sh) / 2.0 + 4.0);
            let (x1, y1) = (x0 + sw, y0 + sh);
            let mut ops = vec![Op::Image {
                index: i,
                x: pt(x0),
                y: pt(y0),
                w: pt(sw),
                h: pt(sh),
                clip: None,
            }];
            for (x, d) in [(x0, -1.0), (x1, 1.0)] {
                ops.push(cut((pt(x), pt(y0 - 2.0)), (pt(x), pt(y0 - 5.0))));
                ops.push(cut((pt(x), pt(y1 + 2.0)), (pt(x), pt(y1 + 5.0))));
                ops.push(cut((pt(x + d * 2.0), pt(y0)), (pt(x + d * 5.0), pt(y0))));
                ops.push(cut((pt(x + d * 2.0), pt(y1)), (pt(x + d * 5.0), pt(y1))));
            }
            let how = if scale < 0.999 {
                format!(
                    "Shrunk to {:.0}% to fit inside your printer's margins (for full size, use the kiosk JPEG).",
                    (scale * 100.0).floor()
                )
            } else {
                "Print at 100% / Actual size (not \"fit to page\").".to_owned()
            };
            ops.push(Op::Label {
                x: pt(margin),
                y: pt(margin) - pt(2.0),
                size: 7.0,
                text: format!("{how} Cut on the grey marks. Page {} of {n}.", i + 1),
            });
            pages.push(pdf::Page {
                media: (pt(pw), pt(ph)),
                trim: None,
                bleed: None,
                ops,
            });
            continue;
        }
        // Tiles: each sheet shows a window of the design; neighbours overlap.
        let step_w = avail_w - overlap;
        let step_h = avail_h - overlap;
        let cols = ((dw - overlap) / step_w).ceil().max(1.0) as usize;
        let rows = ((dh - overlap) / step_h).ceil().max(1.0) as usize;
        for row in 0..rows {
            for col in 0..cols {
                let (wx, wy) = (col as f64 * step_w, row as f64 * step_h); // window, design mm (top-left)
                let (ww, wh) = (avail_w.min(dw - wx), avail_h.min(dh - wy));
                // Design placed so the window sits at the page margin.
                // PDF y is up: design top at page top margin.
                let page_top = ph - margin;
                let img_x = margin - wx;
                let img_y = page_top + wy - dh;
                let clip = [pt(margin), pt(page_top - wh), pt(ww), pt(wh)];
                let mut ops = vec![Op::Image {
                    index: i,
                    x: pt(img_x),
                    y: pt(img_y),
                    w: pt(dw),
                    h: pt(dh),
                    clip: Some(clip),
                }];
                // Overlap guides: tape the next sheet over these lines.
                if col + 1 < cols {
                    let x = pt(margin + ww - overlap);
                    ops.push(cut((x, pt(page_top - wh)), (x, pt(page_top))));
                }
                if row + 1 < rows {
                    let y = pt(page_top - wh + overlap);
                    ops.push(cut((pt(margin), y), (pt(margin + ww), y)));
                }
                let label = format!(
                    "Page {} of {n}, sheet {}{} ({} across x {} down). Print at 100% / Actual size. \
                     Overlap {overlap:.0} mm at the grey lines.",
                    i + 1,
                    char::from(b'A'.saturating_add(u8::try_from(row.min(25)).unwrap_or(25))),
                    col + 1,
                    cols,
                    rows
                );
                ops.push(Op::Label {
                    x: pt(margin),
                    y: pt(margin) - pt(2.0),
                    size: 7.0,
                    text: label,
                });
                pages.push(pdf::Page {
                    media: (pt(pw), pt(ph)),
                    trim: None,
                    bleed: None,
                    ops,
                });
            }
        }
    }
    pages
}

#[cfg(test)]
mod tests;
