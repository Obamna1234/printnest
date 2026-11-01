//! Points, hit testing and resize handles, rotation-aware.

use printnest_model::project::{Element, Rect};

/// A point in page millimetres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pt {
    pub x: f64,
    pub y: f64,
}

impl Pt {
    #[must_use]
    pub fn new(x: f64, y: f64) -> Self {
        Pt { x, y }
    }
}

/// Centre of a rectangle.
#[must_use]
pub fn centre(r: &Rect) -> Pt {
    Pt::new(r.x + r.w / 2.0, r.y + r.h / 2.0)
}

/// Rotates `p` by `deg` degrees clockwise around `c` (screen coordinates,
/// y pointing down).
#[must_use]
pub fn rotate(p: Pt, c: Pt, deg: f64) -> Pt {
    let (s, co) = deg.to_radians().sin_cos();
    let (dx, dy) = (p.x - c.x, p.y - c.y);
    Pt::new(c.x + dx * co - dy * s, c.y + dx * s + dy * co)
}

/// The rectangle's four corners after rotation: TL, TR, BR, BL.
#[must_use]
pub fn corners(r: &Rect) -> [Pt; 4] {
    let c = centre(r);
    [
        rotate(Pt::new(r.x, r.y), c, r.rotation),
        rotate(Pt::new(r.x + r.w, r.y), c, r.rotation),
        rotate(Pt::new(r.x + r.w, r.y + r.h), c, r.rotation),
        rotate(Pt::new(r.x, r.y + r.h), c, r.rotation),
    ]
}

/// True if `p` lies inside the (possibly rotated) rectangle.
#[must_use]
pub fn contains(r: &Rect, p: Pt) -> bool {
    let local = rotate(p, centre(r), -r.rotation);
    local.x >= r.x && local.x <= r.x + r.w && local.y >= r.y && local.y <= r.y + r.h
}

/// Index of the topmost element under `p` (later elements are on top).
#[must_use]
pub fn hit_test(elements: &[Element], p: Pt) -> Option<usize> {
    elements
        .iter()
        .enumerate()
        .rev()
        .find(|(_, e)| contains(e.rect(), p))
        .map(|(i, _)| i)
}

/// What the pointer grabbed on a selected element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    Body,
    TopLeft,
    TopRight,
    BottomRight,
    BottomLeft,
}

/// Which handle of `r` is within `radius` mm of `p`, corners first.
#[must_use]
pub fn hit_handle(r: &Rect, p: Pt, radius: f64) -> Option<Handle> {
    let [tl, tr, br, bl] = corners(r);
    let near = |q: Pt| (q.x - p.x).hypot(q.y - p.y) <= radius;
    if near(tl) {
        Some(Handle::TopLeft)
    } else if near(tr) {
        Some(Handle::TopRight)
    } else if near(br) {
        Some(Handle::BottomRight)
    } else if near(bl) {
        Some(Handle::BottomLeft)
    } else if contains(r, p) {
        Some(Handle::Body)
    } else {
        None
    }
}

/// Smallest frame the editor will create, in mm.
pub const MIN_SIZE_MM: f64 = 3.0;

/// Resizes `start` by dragging `handle` by `(dx, dy)` page mm. Works in the
/// element's own (unrotated) axes and keeps the opposite corner fixed.
/// `keep_aspect` preserves the starting proportions.
#[must_use]
pub fn resize(start: &Rect, handle: Handle, dx: f64, dy: f64, keep_aspect: bool) -> Rect {
    if handle == Handle::Body {
        let mut r = *start;
        r.x += dx;
        r.y += dy;
        return r;
    }
    // Express the drag in the element's local axes.
    let local = rotate(Pt::new(dx, dy), Pt::new(0.0, 0.0), -start.rotation);
    let (sx, sy) = match handle {
        Handle::TopLeft => (-1.0, -1.0),
        Handle::TopRight => (1.0, -1.0),
        Handle::BottomRight => (1.0, 1.0),
        Handle::BottomLeft => (-1.0, 1.0),
        Handle::Body => (0.0, 0.0),
    };
    let mut w = (start.w + sx * local.x).max(MIN_SIZE_MM);
    let mut h = (start.h + sy * local.y).max(MIN_SIZE_MM);
    if keep_aspect && start.h > 0.0 {
        let aspect = start.w / start.h;
        if (w / start.w) > (h / start.h) {
            h = (w / aspect).max(MIN_SIZE_MM);
        } else {
            w = (h * aspect).max(MIN_SIZE_MM);
        }
    }
    // Keep the opposite corner where it was (in page space).
    let opposite_local = Pt::new(
        if sx < 0.0 { start.x + start.w } else { start.x },
        if sy < 0.0 { start.y + start.h } else { start.y },
    );
    let anchor = rotate(opposite_local, centre(start), start.rotation);
    // New centre is half the new diagonal away from the anchor.
    let half = rotate(
        Pt::new(sx * w / 2.0, sy * h / 2.0),
        Pt::new(0.0, 0.0),
        start.rotation,
    );
    let c = Pt::new(anchor.x + half.x, anchor.y + half.y);
    Rect {
        x: c.x - w / 2.0,
        y: c.y - h / 2.0,
        w,
        h,
        rotation: start.rotation,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use printnest_model::project::Crop;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    fn rect(x: f64, y: f64, w: f64, h: f64, rotation: f64) -> Rect {
        Rect {
            x,
            y,
            w,
            h,
            rotation,
        }
    }

    fn photo(id: &str, r: Rect) -> Element {
        Element::Photo {
            id: id.into(),
            rect: r,
            src: None,
            crop: Crop::default(),
        }
    }

    #[test]
    fn hit_test_prefers_topmost() {
        let els = vec![
            photo("a", rect(0.0, 0.0, 100.0, 100.0, 0.0)),
            photo("b", rect(50.0, 50.0, 100.0, 100.0, 0.0)),
        ];
        assert_eq!(hit_test(&els, Pt::new(75.0, 75.0)), Some(1));
        assert_eq!(hit_test(&els, Pt::new(10.0, 10.0)), Some(0));
        assert_eq!(hit_test(&els, Pt::new(200.0, 10.0)), None);
    }

    #[test]
    fn hit_test_respects_rotation() {
        // A long thin bar rotated 90° becomes tall and thin.
        let r = rect(0.0, 45.0, 100.0, 10.0, 90.0);
        assert!(contains(&r, Pt::new(50.0, 10.0)));
        assert!(!contains(&r, Pt::new(10.0, 50.0)));
    }

    #[test]
    fn handles_found_at_corners() {
        let r = rect(10.0, 10.0, 50.0, 30.0, 0.0);
        assert_eq!(
            hit_handle(&r, Pt::new(10.5, 10.5), 2.0),
            Some(Handle::TopLeft)
        );
        assert_eq!(
            hit_handle(&r, Pt::new(60.0, 40.0), 2.0),
            Some(Handle::BottomRight)
        );
        assert_eq!(hit_handle(&r, Pt::new(30.0, 20.0), 2.0), Some(Handle::Body));
        assert_eq!(hit_handle(&r, Pt::new(90.0, 90.0), 2.0), None);
    }

    #[test]
    fn resize_keeps_opposite_corner() {
        let s = rect(10.0, 10.0, 50.0, 30.0, 0.0);
        let r = resize(&s, Handle::BottomRight, 10.0, 5.0, false);
        assert!(close(r.x, 10.0) && close(r.y, 10.0) && close(r.w, 60.0) && close(r.h, 35.0));
        let r = resize(&s, Handle::TopLeft, 10.0, 5.0, false);
        assert!(close(r.x + r.w, 60.0) && close(r.y + r.h, 40.0) && close(r.w, 40.0));
    }

    #[test]
    fn resize_rotated_keeps_opposite_corner_in_page_space() {
        let s = rect(0.0, 0.0, 40.0, 20.0, 30.0);
        let anchor_before = corners(&s)[0];
        let r = resize(&s, Handle::BottomRight, 7.0, -3.0, false);
        let anchor_after = corners(&r)[0];
        assert!(close(anchor_before.x, anchor_after.x) && close(anchor_before.y, anchor_after.y));
    }

    #[test]
    fn resize_enforces_minimum_and_aspect() {
        let s = rect(0.0, 0.0, 40.0, 20.0, 0.0);
        let tiny = resize(&s, Handle::BottomRight, -500.0, -500.0, false);
        assert!(tiny.w >= MIN_SIZE_MM && tiny.h >= MIN_SIZE_MM);
        let a = resize(&s, Handle::BottomRight, 40.0, 0.0, true);
        assert!(close(a.w / a.h, 2.0));
    }

    #[test]
    fn body_drag_moves() {
        let s = rect(1.0, 2.0, 3.0, 4.0, 15.0);
        let r = resize(&s, Handle::Body, 5.0, -1.0, false);
        assert!(close(r.x, 6.0) && close(r.y, 1.0) && close(r.w, 3.0) && close(r.rotation, 15.0));
    }
}
