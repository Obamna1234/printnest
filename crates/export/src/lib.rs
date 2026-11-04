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
use printnest_layout::book::CoverGeometry;
use printnest_model::catalogue::Product;
use printnest_model::project::Project;
use printnest_model::safe_path::SafePath;
use printnest_render::{
    Area, Canvas, Photo, Raster, RenderError, canvas_pixels, fit_budget, needed_edge_on,
    page_pixels, render_canvas,
};
use std::collections::HashMap;
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
    /// Photos that couldn't be decoded and are missing from the print.
    pub missing: Vec<String>,
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
        .collect();
    // At most 100 bytes (file names are limited in bytes, not characters).
    let mut end = 0;
    for (i, c) in cleaned.char_indices() {
        if i + c.len_utf8() > 100 {
            break;
        }
        end = i + c.len_utf8();
    }
    let trimmed = cleaned
        .get(..end)
        .unwrap_or("")
        .trim()
        .trim_matches('_')
        .trim();
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

/// Writes `bytes` to a new file at `path`, never replacing one: the data
/// goes to `<path>.partial` first and is moved into place only when
/// complete, so an interrupted export never looks finished.
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), ExportError> {
    write_new_with(path, |f| f.write_all(bytes))
}

fn partial_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(std::ffi::OsStr::to_os_string)
        .unwrap_or_default();
    name.push(".partial");
    path.with_file_name(name)
}

fn write_new_with(
    path: &Path,
    fill: impl FnOnce(&mut fs::File) -> std::io::Result<()>,
) -> Result<(), ExportError> {
    let tmp = partial_path(path);
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .map_err(io)?;
    let written = fill(&mut f).and_then(|()| f.sync_all());
    drop(f);
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(io(e));
    }
    // A hard link fails if `path` exists, so nothing is ever replaced.
    // File systems without hard links (FAT USB sticks) fall back to a
    // checked rename inside our own freshly created folder.
    let placed = match fs::hard_link(&tmp, path) {
        Ok(()) => fs::remove_file(&tmp),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(e),
        Err(_) if fs::symlink_metadata(path).is_err() => fs::rename(&tmp, path),
        Err(e) => Err(e),
    };
    placed.map_err(|e| {
        let _ = fs::remove_file(&tmp);
        io(e)
    })
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

/// Renders page `i`, decoding only the photos it shows, at sizes that fit
/// the per-page memory budget. Photos that fail to load go in `missing`.
#[allow(clippy::too_many_arguments)]
fn render(
    pg: &printnest_model::project::Page,
    canvas: Canvas,
    ppi: f64,
    area: Area,
    sizes: &HashMap<String, (u32, u32)>,
    loader: &mut PhotoLoader<'_>,
    cancelled: &dyn Fn() -> bool,
    missing: &mut Vec<String>,
) -> Result<Raster, ExportError> {
    let srcs: Vec<String> = {
        let set: std::collections::BTreeSet<String> = pg
            .elements
            .iter()
            .filter_map(|e| match e {
                printnest_model::project::Element::Photo { src: Some(s), .. } => {
                    Some(s.as_str().to_owned())
                }
                printnest_model::project::Element::Photo { .. }
                | printnest_model::project::Element::Text { .. } => None,
            })
            .collect();
        set.into_iter().collect()
    };
    let mut wanted = Vec::new();
    for src in &srcs {
        match sizes.get(src) {
            Some(&(w, h)) if w > 0 && h > 0 => {
                let edge = needed_edge_on(pg, src, (w, h), ppi, MAX_PHOTO_EDGE);
                let aspect = f64::from(w.min(h)) / f64::from(w.max(h));
                wanted.push((src.clone(), edge, aspect));
            }
            // Unknown size: it couldn't be opened when checked.
            _ => {
                if !missing.contains(src) {
                    missing.push(src.clone());
                }
            }
        }
    }
    let edges = fit_budget(&wanted.iter().map(|(_, e, a)| (*e, *a)).collect::<Vec<_>>());
    let mut photos = HashMap::new();
    for ((src, _, _), edge) in wanted.into_iter().zip(edges) {
        if cancelled() {
            return Err(ExportError::Cancelled);
        }
        match loader(&src, edge) {
            Some(p) => {
                photos.insert(src, p);
            }
            None => {
                if !missing.contains(&src) {
                    missing.push(src);
                }
            }
        }
    }
    match render_canvas(pg, canvas, photos, ppi, area, cancelled) {
        Ok(r) => Ok(r),
        Err(RenderError::Cancelled) => Err(ExportError::Cancelled),
        Err(e) => Err(e.into()),
    }
}

/// Exports `project` for `profile` into a new folder inside `parent`.
/// `cover` is the book cover's layout ([`cover_geometry`]); without it no
/// cover file is made. `sizes` holds each photo's full-resolution size (from the sandbox's
/// probe); photos without one are treated as missing. On any failure or
/// cancellation the new folder is removed again.
///
/// # Errors
/// On render, encode or write failure, or if `progress` cancels.
#[allow(clippy::too_many_arguments)]
pub fn export(
    project: &Project,
    product: &Product,
    profile: Profile,
    parent: &Path,
    cover: Option<CoverGeometry>,
    sizes: &HashMap<String, (u32, u32)>,
    loader: &mut PhotoLoader<'_>,
    progress: &mut Progress<'_>,
    cancelled: &dyn Fn() -> bool,
) -> Result<ExportReport, ExportError> {
    let base = safe_base_name(&project.title);
    let folder = new_folder(parent, &base, profile.folder_suffix())?;
    let result = export_into(
        project, product, profile, &folder, &base, cover, sizes, loader, progress, cancelled,
    );
    if result.is_err() {
        // Only ever our own, freshly created folder.
        let _ = fs::remove_dir_all(&folder);
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn export_into(
    project: &Project,
    product: &Product,
    profile: Profile,
    folder: &Path,
    base: &str,
    cover: Option<CoverGeometry>,
    sizes: &HashMap<String, (u32, u32)>,
    loader: &mut PhotoLoader<'_>,
    progress: &mut Progress<'_>,
    cancelled: &dyn Fn() -> bool,
) -> Result<ExportReport, ExportError> {
    let pages = project.pages.len();
    let mut files = Vec::new();
    let mut missing = Vec::new();
    let (area, max_edge) = match profile {
        Profile::PrintShop => (Area::Artwork, printnest_render::MAX_RENDER_EDGE),
        Profile::Kiosk => (Area::Trim, KIOSK_MAX_EDGE),
        Profile::Home(_) => (Area::Trim, printnest_render::MAX_RENDER_EDGE),
    };
    let ppi = choose_ppi(product, area, max_edge);
    let width = pages.to_string().len().max(2);
    let pdf_name = match profile {
        Profile::Kiosk => None,
        Profile::PrintShop => Some(format!("{base} - print shop.pdf")),
        Profile::Home(paper) => Some(format!("{base} - {}.pdf", paper.name())),
    };
    // PDFs stream to a .partial file page by page.
    let mut pdf = match &pdf_name {
        Some(name) => {
            let tmp = partial_path(&folder.join(name));
            let f = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp)
                .map_err(io)?;
            Some(pdf::Writer::new(std::io::BufWriter::new(f)).map_err(io)?)
        }
        None => None,
    };
    for i in 0..pages {
        let pg = project
            .pages
            .get(i)
            .ok_or(ExportError::Render(RenderError::NoPage(i)))?;
        let raster = render(
            pg,
            Canvas::page(product),
            ppi,
            area,
            sizes,
            loader,
            cancelled,
            &mut missing,
        )?;
        let jpeg = encode_jpeg(&raster, ppi)?;
        let (w, h) = (raster.width, raster.height);
        drop(raster);
        match pdf.as_mut() {
            None => {
                let name = if pages == 1 {
                    format!("{base}.jpg")
                } else {
                    format!("{base}-{:0width$}.jpg", i + 1)
                };
                let path = folder.join(name);
                write_new(&path, &jpeg)?;
                files.push(path);
            }
            Some(w_pdf) => {
                w_pdf
                    .add_image(&pdf::Jpeg {
                        width: w,
                        height: h,
                        data: jpeg,
                    })
                    .map_err(io)?;
            }
        }
        if !progress(i + 1, pages) || cancelled() {
            return Err(ExportError::Cancelled);
        }
    }
    if let (Some(w_pdf), Some(name)) = (pdf, pdf_name) {
        let t = product.trim();
        let pdf_pages = match profile {
            Profile::Home(paper) => home_pages(t.width.0, t.height.0, paper, pages),
            Profile::PrintShop | Profile::Kiosk => shop_pages(Canvas::page(product), pages, None),
        };
        files.push(finish_pdf(
            w_pdf,
            &project.title,
            &pdf_pages,
            &folder.join(name),
        )?);
    }
    // Books: the cover is a separate file (printers handle it apart).
    if let (Some(cover), Some(geom)) = (&project.cover_design, cover)
        && !matches!(profile, Profile::Kiosk)
    {
        let canvas = Canvas {
            width: geom.width,
            height: geom.height,
            outer: geom.outer,
        };
        let (area, cover_ppi) = match profile {
            Profile::PrintShop => (Area::Artwork, choose_canvas_ppi(canvas, Area::Artwork)),
            Profile::Home(_) | Profile::Kiosk => {
                (Area::Trim, choose_canvas_ppi(canvas, Area::Trim))
            }
        };
        let raster = render(
            cover,
            canvas,
            cover_ppi,
            area,
            sizes,
            loader,
            cancelled,
            &mut missing,
        )?;
        let jpeg = encode_jpeg(&raster, cover_ppi)?;
        let (name, sheets) = match profile {
            Profile::Home(paper) => (
                format!("{base} - cover - {}.pdf", paper.name()),
                home_pages(geom.width, geom.height, paper, 1),
            ),
            Profile::PrintShop | Profile::Kiosk => (
                format!("{base} - cover.pdf"),
                shop_pages(canvas, 1, Some((geom.spine_x0, geom.spine_x1))),
            ),
        };
        let path = folder.join(&name);
        let f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(partial_path(&path))
            .map_err(io)?;
        let mut w = pdf::Writer::new(std::io::BufWriter::new(f)).map_err(io)?;
        w.add_image(&pdf::Jpeg {
            width: raster.width,
            height: raster.height,
            data: jpeg,
        })
        .map_err(io)?;
        files.push(finish_pdf(w, &project.title, &sheets, &path)?);
        if !progress(pages, pages) || cancelled() {
            return Err(ExportError::Cancelled);
        }
    }
    let note = match profile {
        Profile::Kiosk => Some(kiosk_note(project, product, pages)),
        Profile::PrintShop => Some(shop_note(product)),
        Profile::Home(_) => None,
    };
    if let Some(text) = note {
        let path = folder.join("PRINT-ME.txt");
        write_new(&path, text.as_bytes())?;
        files.push(path);
    }
    missing.sort();
    Ok(ExportReport {
        folder: folder.to_path_buf(),
        files,
        ppi,
        missing,
    })
}

/// The book cover's layout for `project`, if it has one (pass the result
/// to [`export`]).
#[must_use]
pub fn cover_geometry(
    project: &Project,
    catalogue: &printnest_model::Catalogue,
) -> Option<CoverGeometry> {
    let product = catalogue.product(&project.product)?;
    let paper = catalogue.paper(&project.paper)?;
    let pages = u32::try_from(project.pages.len()).ok()?;
    project.cover_design.as_ref()?;
    CoverGeometry::new(product, pages, paper, project.cover.as_deref()?)
}

/// Like [`choose_ppi`] for any canvas (e.g. a book cover).
fn choose_canvas_ppi(canvas: Canvas, area: Area) -> f64 {
    let mut ppi = TARGET_PPI;
    while ppi > 30.0 && canvas_pixels(canvas, area, ppi).is_err() {
        ppi = (ppi * 0.9).floor();
    }
    ppi.max(30.0)
}

/// Finishes a streamed PDF and moves it into place without replacing
/// anything.
fn finish_pdf(
    w: pdf::Writer<std::io::BufWriter<fs::File>>,
    title: &str,
    pages: &[pdf::Page],
    path: &Path,
) -> Result<PathBuf, ExportError> {
    let buf = w.finish(title, pages).map_err(io)?;
    let f = buf.into_inner().map_err(|e| io(e.error()))?;
    f.sync_all().map_err(io)?;
    drop(f);
    let tmp = partial_path(path);
    let placed = match fs::hard_link(&tmp, path) {
        Ok(()) => fs::remove_file(&tmp),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(e),
        Err(_) if fs::symlink_metadata(path).is_err() => fs::rename(&tmp, path),
        Err(e) => Err(e),
    };
    placed.map_err(io)?;
    Ok(path.to_path_buf())
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
/// Print-shop sheets for `n` images of `canvas`, with crop marks, and
/// spine fold marks for a book cover (`spine`: its x range on the trim).
fn shop_pages(canvas: Canvas, n: usize, spine: Option<(f64, f64)>) -> Vec<pdf::Page> {
    use pdf::{Op, pt};
    let outer = canvas.outer;
    let margin = 12.0; // room for crop marks
    let (aw, ah) = (canvas.width + 2.0 * outer, canvas.height + 2.0 * outer);
    let media = (pt(aw + 2.0 * margin), pt(ah + 2.0 * margin));
    let (bx0, by0) = (pt(margin), pt(margin));
    let (bx1, by1) = (pt(margin + aw), pt(margin + ah));
    let (tx0, ty0) = (pt(margin + outer), pt(margin + outer));
    let (tx1, ty1) = (
        pt(margin + outer + canvas.width),
        pt(margin + outer + canvas.height),
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
            // Book covers: where the spine folds, outside the artwork.
            if let Some((s0, s1)) = spine {
                for x in [s0, s1] {
                    let x = pt(margin + outer + x);
                    ops.push(mark((x, by1 + pt(2.0)), (x, by1 + pt(8.0))));
                    ops.push(mark((x, by0 - pt(2.0)), (x, by0 - pt(8.0))));
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
fn home_pages(dw: f64, dh: f64, paper: Paper, n: usize) -> Vec<pdf::Page> {
    use pdf::{Op, pt};
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
