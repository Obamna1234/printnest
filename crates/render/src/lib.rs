//! Print rasteriser: draws one page of a project at print resolution.
//!
//! Pure computation on already-decoded pixels. Photos are decoded by the
//! caller through the sandboxed worker (`printnest-sandbox`) and handed in
//! as RGBA; nothing here reads files, decodes untrusted data or touches the
//! network. Rendering is single-threaded with SIMD disabled, so the same
//! project renders to the same bytes on every machine (SPEC section 6).
//!
//! Text uses the same bundled fonts as the editor (Ubuntu Light and Hack),
//! so what prints matches what is shown on screen.

#![forbid(unsafe_code)]

mod text;

use printnest_layout::crop::crop_view;
use printnest_model::catalogue::Product;
use printnest_model::project::{Element, Page, Project, Rect};
use std::collections::HashMap;
use std::sync::Arc;
use vello_cpu::color::{AlphaColor, PremulRgba8, Srgb};
use vello_cpu::kurbo::{Affine, Point, Rect as KRect};
use vello_cpu::peniko::{Extend, ImageQuality, ImageSampler};
use vello_cpu::{Image, ImageSource, Level, Pixmap, RenderContext, RenderSettings, Resources};

/// Largest page this renders, in pixels (≈ 600 MB while rendering).
/// A0 at 300 ppi is 140 MP.
pub const MAX_RENDER_PIXELS: u64 = 150_000_000;
/// Most photo area painted on one page, as a multiple of the page's own
/// area (a hostile project could otherwise stack hundreds of full-page
/// photos and take many minutes per page).
pub const MAX_PAINT_FACTOR: f64 = 12.0;
/// Most photo pixels decoded for one page (≈ 400 MB as RGBA). Larger
/// requests are scaled down proportionally.
pub const MAX_PAGE_PHOTO_PIXELS: u64 = 100_000_000;
/// Longest side the renderer supports.
pub const MAX_RENDER_EDGE: u32 = u16::MAX as u32;

/// A decoded photo: straight (non-premultiplied) RGBA, row-major.
#[derive(Clone)]
pub struct Photo {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for Photo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Photo({}x{})", self.width, self.height)
    }
}

/// Which part of the page to render.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Area {
    /// Trim plus bleed (and canvas wrap): what a print shop needs.
    Artwork,
    /// Exactly the finished size: what a kiosk or home printer needs.
    Trim,
}

/// A rendered page: opaque 8-bit sRGB, row-major, 3 bytes per pixel.
#[derive(Clone, PartialEq, Eq)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

impl std::fmt::Debug for Raster {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Raster({}x{})", self.width, self.height)
    }
}

impl Raster {
    /// The pixel at (x, y), if inside.
    #[must_use]
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 3]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = (usize::try_from(y).ok()? * usize::try_from(self.width).ok()?
            + usize::try_from(x).ok()?)
            * 3;
        let px = self.rgb.get(i..i + 3)?;
        <[u8; 3]>::try_from(px).ok()
    }
}

/// Why a page couldn't be rendered.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum RenderError {
    #[error("page {0} doesn't exist")]
    NoPage(usize),
    #[error("{0} × {1} px is too large to render; choose a lower resolution")]
    TooLarge(u64, u64),
    #[error("resolution must be between 30 and 1200 ppi")]
    BadPpi,
    #[error("cancelled")]
    Cancelled,
    #[error("this page has too many overlapping photos to render")]
    TooComplex,
}

/// The surface a page is drawn on: its trimmed size and the extra artwork
/// around it (bleed, canvas wrap or a book cover's turn-in), in mm.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Canvas {
    pub width: f64,
    pub height: f64,
    pub outer: f64,
}

impl Canvas {
    /// An ordinary page of `product`.
    #[must_use]
    pub fn page(product: &Product) -> Self {
        let t = product.trim();
        Canvas {
            width: t.width.0,
            height: t.height.0,
            outer: outer_mm(product),
        }
    }

    /// Size of `area` in millimetres.
    #[must_use]
    pub fn area_mm(self, area: Area) -> (f64, f64) {
        match area {
            Area::Trim => (self.width, self.height),
            Area::Artwork => (
                self.width + 2.0 * self.outer,
                self.height + 2.0 * self.outer,
            ),
        }
    }
}

/// Millimetres from the artwork's top-left corner to the trim's.
#[must_use]
pub fn outer_mm(product: &Product) -> f64 {
    product.bleed_mm + product.wrap_mm.unwrap_or(0.0)
}

/// Size of `area` in millimetres.
#[must_use]
pub fn area_mm(product: &Product, area: Area) -> (f64, f64) {
    Canvas::page(product).area_mm(area)
}

/// Pixel size of `area` at `ppi`.
///
/// # Errors
/// If `ppi` is out of range or the result is too large.
pub fn page_pixels(product: &Product, area: Area, ppi: f64) -> Result<(u32, u32), RenderError> {
    canvas_pixels(Canvas::page(product), area, ppi)
}

/// Pixel size of `area` of `canvas` at `ppi`.
///
/// # Errors
/// If `ppi` is out of range or the result is too large.
pub fn canvas_pixels(canvas: Canvas, area: Area, ppi: f64) -> Result<(u32, u32), RenderError> {
    if !(ppi.is_finite() && (30.0..=1200.0).contains(&ppi)) {
        return Err(RenderError::BadPpi);
    }
    let (w, h) = canvas.area_mm(area);
    let px = |mm: f64| (mm / 25.4 * ppi).round().max(1.0);
    let (pw, ph) = (px(w), px(h));
    let too_large = || RenderError::TooLarge(pw as u64, ph as u64);
    if pw > f64::from(MAX_RENDER_EDGE) || ph > f64::from(MAX_RENDER_EDGE) {
        return Err(too_large());
    }
    // In range: both are positive integers below 65,536.
    let (w, h) = (pw as u32, ph as u32);
    if u64::from(w) * u64::from(h) > MAX_RENDER_PIXELS {
        return Err(too_large());
    }
    Ok((w, h))
}

/// Parses `#rrggbb` (validated by the project model); black on error.
#[must_use]
pub fn parse_colour(s: &str) -> [u8; 3] {
    let hex = |i: usize| {
        s.get(i..i + 2)
            .and_then(|h| u8::from_str_radix(h, 16).ok())
            .unwrap_or(0)
    };
    [hex(1), hex(3), hex(5)]
}

fn colour(rgb: [u8; 3]) -> AlphaColor<Srgb> {
    let [r, g, b] = rgb;
    AlphaColor::from_rgba8(r, g, b, 255)
}

fn to_pixmap(p: &Photo) -> Option<Pixmap> {
    let w = u16::try_from(p.width).ok()?;
    let h = u16::try_from(p.height).ok()?;
    let n = usize::from(w) * usize::from(h);
    if p.rgba.len() != n * 4 || n == 0 {
        return None;
    }
    let data = p
        .rgba
        .chunks_exact(4)
        .map(|c| {
            let (r, g, b, a) = match *c {
                [r, g, b, a] => (r, g, b, a),
                _ => (0, 0, 0, 0),
            };
            let m = |v: u8| ((u16::from(v) * u16::from(a) + 127) / 255) as u8;
            PremulRgba8 {
                r: m(r),
                g: m(g),
                b: m(b),
                a,
            }
        })
        .collect();
    Some(Pixmap::from_parts(data, w, h))
}

/// Maps millimetres on the page (trim origin) to output pixels.
fn page_transform(canvas: Canvas, area: Area, ppi: f64) -> Affine {
    let off = match area {
        Area::Artwork => canvas.outer,
        Area::Trim => 0.0,
    };
    Affine::scale(ppi / 25.4) * Affine::translate((off, off))
}

/// Rotation of an element about its centre, in page millimetres.
fn element_transform(r: &Rect) -> Affine {
    let c = Point::new(r.x + r.w / 2.0, r.y + r.h / 2.0);
    Affine::rotate_about(r.rotation.to_radians(), c)
}

/// Renders page `page` of `project`. `photos` maps a project photo path to
/// its decoded pixels (taken by value: each is freed as soon as it has been
/// converted, so only one extra copy exists at a time); photos missing from
/// the map are left out. `cancelled` is polled between elements.
///
/// # Errors
/// If the page doesn't exist, is too large at `ppi`, paints far more photo
/// area than a real design would, or is cancelled.
pub fn render_page(
    project: &Project,
    product: &Product,
    page: usize,
    photos: HashMap<String, Arc<Photo>>,
    ppi: f64,
    area: Area,
    cancelled: &dyn Fn() -> bool,
) -> Result<Raster, RenderError> {
    let pg = project.pages.get(page).ok_or(RenderError::NoPage(page))?;
    render_canvas(pg, Canvas::page(product), photos, ppi, area, cancelled)
}

/// Renders `pg` on `canvas` (used for book covers, whose size depends on
/// the spine). Same limits and behaviour as [`render_page`].
///
/// # Errors
/// As [`render_page`].
pub fn render_canvas(
    pg: &Page,
    canvas: Canvas,
    mut photos: HashMap<String, Arc<Photo>>,
    ppi: f64,
    area: Area,
    cancelled: &dyn Fn() -> bool,
) -> Result<Raster, RenderError> {
    let (w, h) = canvas_pixels(canvas, area, ppi)?;
    let (w16, h16) = (
        u16::try_from(w).map_err(|_| RenderError::TooLarge(u64::from(w), u64::from(h)))?,
        u16::try_from(h).map_err(|_| RenderError::TooLarge(u64::from(w), u64::from(h)))?,
    );
    // Deterministic: one thread, only the SIMD every CPU of this architecture has.
    let settings = RenderSettings {
        level: Level::baseline(),
        num_threads: 0,
    };
    let mut ctx = RenderContext::new_with(w16, h16, settings);
    let page_tf = page_transform(canvas, area, ppi);

    ctx.set_paint(colour(parse_colour(&pg.background)));
    ctx.fill_rect(&KRect::new(0.0, 0.0, f64::from(w), f64::from(h)));

    // Bound the work: total photo area painted, relative to the page.
    let page_area = {
        let (aw, ah) = canvas.area_mm(area);
        aw * ah
    };
    let painted: f64 = pg
        .elements
        .iter()
        .filter_map(|e| match e {
            Element::Photo {
                rect, src: Some(_), ..
            } => Some(rect.w * rect.h),
            Element::Photo { .. } | Element::Text { .. } => None,
        })
        .sum();
    if !(painted.is_finite() && painted <= page_area * MAX_PAINT_FACTOR) {
        return Err(RenderError::TooComplex);
    }
    // Convert each photo once, dropping the decoded copy straight away.
    let keys: Vec<String> = photos.keys().cloned().collect();
    let mut pixmaps: HashMap<String, Arc<Pixmap>> = HashMap::new();
    for key in keys {
        if let Some(photo) = photos.remove(&key)
            && let Some(pm) = to_pixmap(&photo)
        {
            pixmaps.insert(key, Arc::new(pm));
        }
    }
    for el in &pg.elements {
        if cancelled() {
            return Err(RenderError::Cancelled);
        }
        match el {
            Element::Photo {
                rect,
                src: Some(src),
                crop,
                ..
            } => {
                let Some(pm) = pixmaps.get(src.as_str()).map(Arc::clone) else {
                    continue;
                };
                let (iw, ih) = (f64::from(pm.width()), f64::from(pm.height()));
                let Some(v) = crop_view(
                    rect.w,
                    rect.h,
                    u32::from(pm.width()),
                    u32::from(pm.height()),
                    crop,
                ) else {
                    continue;
                };
                let (du, dv) = (v.u1 - v.u0, v.v1 - v.v0);
                if du <= 0.0 || dv <= 0.0 {
                    continue;
                }
                // Image pixel space -> frame (mm) -> rotated page -> output.
                let tf = page_tf
                    * element_transform(rect)
                    * Affine::translate((rect.x, rect.y))
                    * Affine::scale_non_uniform(rect.w / (du * iw), rect.h / (dv * ih))
                    * Affine::translate((-v.u0 * iw, -v.v0 * ih));
                ctx.set_transform(tf);
                ctx.set_paint(Image {
                    image: ImageSource::Pixmap(pm),
                    sampler: ImageSampler {
                        x_extend: Extend::Pad,
                        y_extend: Extend::Pad,
                        quality: ImageQuality::High,
                        alpha: 1.0,
                    },
                });
                ctx.fill_rect(&KRect::new(v.u0 * iw, v.v0 * ih, v.u1 * iw, v.v1 * ih));
                ctx.reset_transform();
            }
            Element::Photo { src: None, .. } => {}
            Element::Text {
                rect,
                text,
                font,
                size_pt,
                colour: c,
                align,
                ..
            } => {
                let face = text::Face::for_name(font);
                let paths = text::layout(&face, text, *size_pt, rect, *align);
                ctx.set_transform(page_tf * element_transform(rect));
                ctx.set_paint(colour(parse_colour(c)));
                for p in &paths {
                    ctx.fill_path(p);
                }
                ctx.reset_transform();
            }
        }
    }

    let mut target = Pixmap::new(w16, h16);
    ctx.flush();
    let mut resources = Resources::new();
    ctx.render(&mut target, &mut resources);
    let rgb = target.data().iter().flat_map(|p| [p.r, p.g, p.b]).collect();
    Ok(Raster {
        width: w,
        height: h,
        rgb,
    })
}

/// Longest edge (in photo pixels) worth decoding for `src` on this page so
/// it prints at `ppi`: the photo's on-paper size comes from its real
/// dimensions `size` and each frame's crop. Capped at `max`.
#[must_use]
pub fn needed_edge(
    project: &Project,
    page: usize,
    src: &str,
    size: (u32, u32),
    ppi: f64,
    max: u32,
) -> u32 {
    let Some(pg) = project.pages.get(page) else {
        return 0;
    };
    needed_edge_on(pg, src, size, ppi, max)
}

/// As [`needed_edge`] for any page (e.g. a book cover).
#[must_use]
pub fn needed_edge_on(pg: &Page, src: &str, size: (u32, u32), ppi: f64, max: u32) -> u32 {
    let mut need = 0.0f64;
    for el in &pg.elements {
        if let Element::Photo {
            rect,
            src: Some(s),
            crop,
            ..
        } = el
            && s.as_str() == src
            && let Some(v) = crop_view(rect.w, rect.h, size.0, size.1, crop)
        {
            // Whole photo's long side on paper, in output pixels.
            let long_mm = f64::from(size.0.max(size.1)) * v.mm_per_px;
            need = need.max(long_mm / 25.4 * ppi);
        }
    }
    if need.is_finite() {
        (need.ceil().clamp(1.0, f64::from(max))) as u32
    } else {
        max
    }
}

/// Scales the requested edges down so their total pixels fit
/// [`MAX_PAGE_PHOTO_PIXELS`]. `edges` pairs each request with the photo's
/// aspect (short side / long side, 0..=1).
#[must_use]
pub fn fit_budget(edges: &[(u32, f64)]) -> Vec<u32> {
    let px = |e: u32, a: f64| f64::from(e) * f64::from(e) * a.clamp(0.0, 1.0);
    let total: f64 = edges.iter().map(|&(e, a)| px(e, a)).sum();
    let budget = MAX_PAGE_PHOTO_PIXELS as f64;
    let k = if total > budget {
        (budget / total).sqrt()
    } else {
        1.0
    };
    edges
        .iter()
        .map(|&(e, _)| ((f64::from(e) * k).floor() as u32).max(1))
        .collect()
}

#[cfg(test)]
mod tests;
