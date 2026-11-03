//! Photo-book maths: how pages pair into spreads, where the spine sits on
//! the flat cover, a set of original page layouts, and auto-fill.
//!
//! The layouts are plain geometric arrangements (full page, halves, a
//! large photo with two small ones, a grid); they are PrintNest's own and
//! not copied from any other product.

use crate::edit::new_id;
use printnest_model::catalogue::{Paper, Product};
use printnest_model::limits::MAX_ELEMENTS_PER_PAGE;
use printnest_model::project::{Align, Crop, Element, Project, Rect};
use printnest_model::safe_path::SafePath;

/// One open-book view: left and right page indexes (0-based). The first
/// page sits alone on the right, and with an even page count the last
/// sits alone on the left, as in a printed book.
#[must_use]
pub fn spreads(page_count: usize) -> Vec<(Option<usize>, Option<usize>)> {
    if page_count == 0 {
        return Vec::new();
    }
    let mut out = vec![(None, Some(0))];
    let mut left = 1;
    while left < page_count {
        let right = (left + 1 < page_count).then_some(left + 1);
        out.push((Some(left), right));
        left += 2;
    }
    out
}

/// Which edge of a page is next to the binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inner {
    /// Right-hand page: binding on its left.
    Left,
    /// Left-hand page: binding on its right.
    Right,
}

/// The binding side of page `index` (0-based; page 1 is a right page).
#[must_use]
pub fn inner_edge(index: usize) -> Inner {
    if index % 2 == 0 {
        Inner::Left
    } else {
        Inner::Right
    }
}

/// Where things are on the flat cover (mm, from the trimmed cover's
/// top-left corner; back cover on the left, front on the right).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CoverGeometry {
    /// Trimmed flat size: back + spine + front.
    pub width: f64,
    pub height: f64,
    pub spine_x0: f64,
    pub spine_x1: f64,
    /// Extra artwork around the edge (bleed, or material folded round the
    /// boards on hard covers).
    pub outer: f64,
}

impl CoverGeometry {
    /// The cover for `product` with `pages` pages on `paper`.
    #[must_use]
    pub fn new(product: &Product, pages: u32, paper: &Paper, cover_id: &str) -> Option<Self> {
        let cover = product.covers.iter().find(|c| c.id == cover_id)?;
        let spine = product.spine_width(pages, paper, cover_id)?.0;
        let t = product.trim();
        let (w, h) = (t.width.0, t.height.0);
        Some(CoverGeometry {
            width: 2.0 * w + spine,
            height: h,
            spine_x0: w,
            spine_x1: w + spine,
            outer: cover.turn_in_mm.max(product.bleed_mm),
        })
    }

    /// The front cover panel.
    #[must_use]
    pub fn front(&self) -> (f64, f64, f64, f64) {
        (self.spine_x1, 0.0, self.width - self.spine_x1, self.height)
    }
}

/// An original page layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// One photo filling the page edge to edge.
    FullBleed,
    /// One photo inside the margins.
    Single,
    /// One photo with a caption below.
    PhotoCaption,
    /// Two photos side by side.
    TwoAcross,
    /// Two photos one above the other.
    TwoDown,
    /// One large photo and two small ones.
    ThreeFeature,
    /// Four photos in a grid.
    FourGrid,
}

impl Layout {
    pub const ALL: [Layout; 7] = [
        Layout::FullBleed,
        Layout::Single,
        Layout::PhotoCaption,
        Layout::TwoAcross,
        Layout::TwoDown,
        Layout::ThreeFeature,
        Layout::FourGrid,
    ];

    /// Short name for menus.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Layout::FullBleed => "1 photo, edge to edge",
            Layout::Single => "1 photo",
            Layout::PhotoCaption => "1 photo + caption",
            Layout::TwoAcross => "2 photos across",
            Layout::TwoDown => "2 photos stacked",
            Layout::ThreeFeature => "3 photos",
            Layout::FourGrid => "4 photos",
        }
    }

    /// How many photos it holds.
    #[must_use]
    pub fn photos(self) -> usize {
        match self {
            Layout::FullBleed | Layout::Single | Layout::PhotoCaption => 1,
            Layout::TwoAcross | Layout::TwoDown => 2,
            Layout::ThreeFeature => 3,
            Layout::FourGrid => 4,
        }
    }
}

/// Page measurements a layout needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageBox {
    pub width: f64,
    pub height: f64,
    pub safe: f64,
    pub bleed: f64,
    /// Extra margin on the binding side (books), and which side that is.
    pub gutter: Option<(f64, Inner)>,
}

/// Space between photos.
const GAP: f64 = 4.0;

/// The photo slots (and caption slot, if any) of `layout` on `page`.
#[must_use]
pub fn slots(layout: Layout, page: PageBox) -> (Vec<Rect>, Option<Rect>) {
    let r = |x: f64, y: f64, w: f64, h: f64| Rect {
        x,
        y,
        w: w.max(1.0),
        h: h.max(1.0),
        rotation: 0.0,
    };
    if layout == Layout::FullBleed {
        let b = page.bleed;
        return (
            vec![r(-b, -b, page.width + 2.0 * b, page.height + 2.0 * b)],
            None,
        );
    }
    // Inner area: safe margins, wider on the binding side.
    let (mut x0, mut x1) = (page.safe, page.width - page.safe);
    match page.gutter {
        Some((g, Inner::Left)) => x0 = x0.max(g),
        Some((g, Inner::Right)) => x1 = x1.min(page.width - g),
        None => {}
    }
    let (y0, y1) = (page.safe, page.height - page.safe);
    let (w, h) = ((x1 - x0).max(2.0), (y1 - y0).max(2.0));
    let half_w = (w - GAP) / 2.0;
    let half_h = (h - GAP) / 2.0;
    match layout {
        Layout::FullBleed | Layout::Single => (vec![r(x0, y0, w, h)], None),
        Layout::PhotoCaption => {
            let cap = (h * 0.12).clamp(8.0, 30.0);
            (
                vec![r(x0, y0, w, h - cap - GAP)],
                Some(r(x0, y1 - cap, w, cap)),
            )
        }
        Layout::TwoAcross => (
            vec![r(x0, y0, half_w, h), r(x0 + half_w + GAP, y0, half_w, h)],
            None,
        ),
        Layout::TwoDown => (
            vec![r(x0, y0, w, half_h), r(x0, y0 + half_h + GAP, w, half_h)],
            None,
        ),
        Layout::ThreeFeature => {
            let big_h = (h - GAP) * 0.62;
            let small_h = h - GAP - big_h;
            (
                vec![
                    r(x0, y0, w, big_h),
                    r(x0, y0 + big_h + GAP, half_w, small_h),
                    r(x0 + half_w + GAP, y0 + big_h + GAP, half_w, small_h),
                ],
                None,
            )
        }
        Layout::FourGrid => (
            vec![
                r(x0, y0, half_w, half_h),
                r(x0 + half_w + GAP, y0, half_w, half_h),
                r(x0, y0 + half_h + GAP, half_w, half_h),
                r(x0 + half_w + GAP, y0 + half_h + GAP, half_w, half_h),
            ],
            None,
        ),
    }
}

/// Replaces page `index` with `layout`, filling its frames with `photos` in
/// order (missing ones become empty frames). Text boxes already on the
/// page are kept on top. Returns `false` if the page doesn't exist.
pub fn apply_layout(
    project: &mut Project,
    index: usize,
    layout: Layout,
    photos: &[SafePath],
    page: PageBox,
) -> bool {
    if index >= project.pages.len() {
        return false;
    }
    let (frames, caption) = slots(layout, page);
    let mut new_elements = Vec::new();
    for (i, rect) in frames.into_iter().enumerate() {
        let id = new_id_avoiding(project, "photo", &new_elements);
        new_elements.push(Element::Photo {
            id,
            rect,
            src: photos.get(i).cloned(),
            crop: Crop::default(),
        });
    }
    let Some(target) = project.pages.get(index) else {
        return false;
    };
    let kept_text: Vec<Element> = target
        .elements
        .iter()
        .filter(|e| matches!(e, Element::Text { .. }))
        .cloned()
        .collect();
    let had_text = !kept_text.is_empty();
    new_elements.extend(kept_text);
    if let Some(rect) = caption.filter(|_| !had_text) {
        let id = new_id_avoiding(project, "text", &new_elements);
        new_elements.push(Element::Text {
            id,
            rect,
            text: "Caption".into(),
            font: "sans".into(),
            size_pt: (rect.h / 25.4 * 72.0 * 0.45).clamp(8.0, 36.0).round(),
            colour: "#222222".into(),
            align: Align::Center,
        });
    }
    new_elements.truncate(MAX_ELEMENTS_PER_PAGE);
    if let Some(target) = project.pages.get_mut(index) {
        target.elements = new_elements;
    }
    true
}

/// A fresh id not used in the project or in `pending`.
fn new_id_avoiding(project: &Project, prefix: &str, pending: &[Element]) -> String {
    let base = new_id(project, prefix);
    if !pending.iter().any(|e| e.id() == base) {
        return base;
    }
    (2u64..)
        .map(|n| format!("{base}-{n}"))
        .find(|id| !pending.iter().any(|e| e.id() == id))
        .unwrap_or(base)
}

/// A photo for auto-fill: its path and pixel size.
#[derive(Debug, Clone)]
pub struct AutoPhoto {
    pub path: SafePath,
    pub width: u32,
    pub height: u32,
}

/// A layout for `n` photos (1..=4) that suits their shapes.
#[must_use]
pub fn pick_layout(photos: &[AutoPhoto]) -> Layout {
    let landscape = |p: &AutoPhoto| p.width >= p.height;
    match photos {
        [] | [_] => Layout::FullBleed,
        [a, b] if landscape(a) && landscape(b) => Layout::TwoDown,
        [_, _] => Layout::TwoAcross,
        [_, _, _] => Layout::ThreeFeature,
        _ => Layout::FourGrid,
    }
}

/// Fills the book's empty pages with `photos`, in order, spreading them
/// evenly (1 to 4 per page). Pages that already have something on them
/// are left alone. Returns how many photos were placed.
pub fn auto_fill(
    project: &mut Project,
    photos: &[AutoPhoto],
    page_box: impl Fn(usize) -> PageBox,
) -> usize {
    let empty: Vec<usize> = project
        .pages
        .iter()
        .enumerate()
        .filter(|(_, p)| p.elements.is_empty())
        .map(|(i, _)| i)
        .collect();
    let mut placed = 0;
    let mut rest = photos;
    for (n, &index) in empty.iter().enumerate() {
        if rest.is_empty() {
            break;
        }
        let pages_left = empty.len() - n;
        let per_page = rest.len().div_ceil(pages_left).clamp(1, 4);
        let (now, later) = rest.split_at(per_page.min(rest.len()));
        let layout = pick_layout(now);
        let paths: Vec<SafePath> = now.iter().map(|p| p.path.clone()).collect();
        if apply_layout(project, index, layout, &paths, page_box(index)) {
            placed += paths.len();
        }
        rest = later;
    }
    placed
}

#[cfg(test)]
mod tests {
    use super::*;
    use printnest_model::Catalogue;

    fn book() -> (Catalogue, Project) {
        let cat = Catalogue::builtin().unwrap();
        let p = Project::new_blank(&cat, "book-20x20", "Book").unwrap();
        (cat, p)
    }

    fn page_box(i: usize) -> PageBox {
        PageBox {
            width: 200.0,
            height: 200.0,
            safe: 8.0,
            bleed: 3.0,
            gutter: Some((10.0, inner_edge(i))),
        }
    }

    #[test]
    fn spreads_pair_like_a_printed_book() {
        assert!(spreads(0).is_empty());
        assert_eq!(spreads(1), vec![(None, Some(0))]);
        let s = spreads(20);
        assert_eq!(s.first(), Some(&(None, Some(0))));
        assert_eq!(s.get(1), Some(&(Some(1), Some(2))));
        assert_eq!(s.last(), Some(&(Some(19), None)));
        assert_eq!(s.len(), 11);
        // Every page appears exactly once.
        let mut seen: Vec<usize> = s.iter().flat_map(|(l, r)| [*l, *r]).flatten().collect();
        seen.sort_unstable();
        assert_eq!(seen, (0..20).collect::<Vec<_>>());
        assert_eq!(inner_edge(0), Inner::Left);
        assert_eq!(inner_edge(1), Inner::Right);
    }

    #[test]
    fn cover_has_back_spine_front() {
        let (cat, p) = book();
        let prod = cat.product("book-20x20").unwrap();
        let paper = cat.paper(&p.paper).unwrap();
        let g = CoverGeometry::new(prod, 20, paper, "hard").unwrap();
        let spine = prod.spine_width(20, paper, "hard").unwrap().0;
        assert!((g.width - (400.0 + spine)).abs() < 1e-9);
        assert!((g.spine_x1 - g.spine_x0 - spine).abs() < 1e-9);
        assert_eq!(g.front().0, g.spine_x1);
        assert!(g.outer >= 15.0, "hard covers fold round the boards");
        // More pages, thicker spine.
        let g2 = CoverGeometry::new(prod, 100, paper, "hard").unwrap();
        assert!(g2.spine_x1 - g2.spine_x0 > g.spine_x1 - g.spine_x0);
        assert!(CoverGeometry::new(prod, 20, paper, "nope").is_none());
    }

    #[test]
    fn layouts_stay_in_the_safe_area_and_off_the_gutter() {
        for layout in Layout::ALL {
            for i in 0..2 {
                let pb = page_box(i);
                let (frames, caption) = slots(layout, pb);
                assert_eq!(frames.len(), layout.photos(), "{layout:?}");
                for r in frames.iter().chain(caption.iter()) {
                    if layout == Layout::FullBleed {
                        assert!(r.x <= -3.0 && r.x + r.w >= 203.0);
                        continue;
                    }
                    let (gl, gr) = match inner_edge(i) {
                        Inner::Left => (10.0, 8.0),
                        Inner::Right => (8.0, 10.0),
                    };
                    assert!(
                        r.x >= gl - 1e-9 && r.x + r.w <= 200.0 - gr + 1e-9,
                        "{layout:?} {r:?}"
                    );
                    assert!(
                        r.y >= 8.0 - 1e-9 && r.y + r.h <= 192.0 + 1e-9,
                        "{layout:?} {r:?}"
                    );
                }
                // No overlaps between frames.
                for (a, ra) in frames.iter().enumerate() {
                    for rb in frames.iter().skip(a + 1) {
                        let apart = ra.x + ra.w <= rb.x + 1e-9
                            || rb.x + rb.w <= ra.x + 1e-9
                            || ra.y + ra.h <= rb.y + 1e-9
                            || rb.y + rb.h <= ra.y + 1e-9;
                        assert!(apart, "{layout:?} overlap");
                    }
                }
            }
        }
    }

    #[test]
    fn apply_layout_keeps_text_and_validates() {
        let (cat, mut p) = book();
        crate::edit::add_text(&mut p, 0, 200.0, 200.0, 8.0).unwrap();
        let a = SafePath::new("photos/a.jpg").unwrap();
        assert!(apply_layout(
            &mut p,
            0,
            Layout::ThreeFeature,
            &[a.clone()],
            page_box(0)
        ));
        let els = &p.pages[0].elements;
        assert_eq!(els.len(), 4);
        assert!(matches!(&els[0], Element::Photo { src: Some(s), .. } if *s == a));
        assert!(matches!(&els[1], Element::Photo { src: None, .. }));
        assert!(matches!(&els[3], Element::Text { .. }));
        assert!(p.validate(&cat).is_ok(), "{:?}", p.validate(&cat));
        assert!(!apply_layout(&mut p, 99, Layout::Single, &[], page_box(0)));
        // Caption layouts add a caption only when there's no text yet.
        assert!(apply_layout(
            &mut p,
            1,
            Layout::PhotoCaption,
            &[],
            page_box(1)
        ));
        assert_eq!(p.pages[1].elements.len(), 2);
        assert!(p.validate(&cat).is_ok());
    }

    #[test]
    fn auto_fill_spreads_photos_over_empty_pages() {
        let (cat, mut p) = book();
        // Page 2 already designed: left alone.
        apply_layout(&mut p, 1, Layout::Single, &[], page_box(1));
        let photos: Vec<AutoPhoto> = (0..45)
            .map(|i| AutoPhoto {
                path: SafePath::new(&format!("photos/p{i}.jpg")).unwrap(),
                width: if i % 3 == 0 { 3000 } else { 4000 },
                height: 3000,
            })
            .collect();
        let placed = auto_fill(&mut p, &photos, page_box);
        assert_eq!(placed, 45);
        assert_eq!(p.photo_paths().len(), 45);
        assert_eq!(p.pages[1].elements.len(), 1, "designed page untouched");
        // Order is kept: first photo on page 1.
        assert!(
            matches!(&p.pages[0].elements[0], Element::Photo { src: Some(s), .. } if s.as_str() == "photos/p0.jpg")
        );
        // No page gets more than 4.
        assert!(p.pages.iter().all(|pg| pg.elements.len() <= 4));
        assert!(p.validate(&cat).is_ok(), "{:?}", p.validate(&cat));
        // More photos than 4 per page can hold: the rest are left over.
        let (_, mut q) = book();
        let many: Vec<AutoPhoto> = (0..200)
            .map(|i| AutoPhoto {
                path: SafePath::new(&format!("photos/m{i}.jpg")).unwrap(),
                width: 4000,
                height: 3000,
            })
            .collect();
        assert_eq!(auto_fill(&mut q, &many, page_box), 80);
    }
}
