//! Snapping to page guides and other elements' edges and centres.

use printnest_model::project::Rect;

/// A guide line the editor should draw while snapping.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Guide {
    /// Vertical line at x (mm).
    V(f64),
    /// Horizontal line at y (mm).
    H(f64),
}

/// Result of a snap: the adjusted rectangle and the guides that caught it.
#[derive(Debug, Clone, PartialEq)]
pub struct SnapResult {
    pub rect: Rect,
    pub guides: Vec<Guide>,
}

/// Page lines everything snaps to: bleed edges, trim edges, safe edges and
/// the centre, for one axis of length `len`.
#[must_use]
pub fn page_lines(len: f64, bleed: f64, safe: f64) -> Vec<f64> {
    vec![-bleed, 0.0, safe, len / 2.0, len - safe, len, len + bleed]
}

fn best(candidates: &[f64], targets: &[f64], threshold: f64) -> Option<(f64, f64)> {
    // (offset to apply, target line)
    let mut found: Option<(f64, f64)> = None;
    for &c in candidates {
        for &t in targets {
            let d = t - c;
            if d.abs() <= threshold && found.is_none_or(|(o, _)| d.abs() < o.abs()) {
                found = Some((d, t));
            }
        }
    }
    found
}

/// Lines from other elements (edges and centres, ignoring rotation).
fn element_lines(others: &[Rect]) -> (Vec<f64>, Vec<f64>) {
    let mut xs = Vec::new();
    let mut ys = Vec::new();
    for r in others {
        xs.extend([r.x, r.x + r.w / 2.0, r.x + r.w]);
        ys.extend([r.y, r.y + r.h / 2.0, r.y + r.h]);
    }
    (xs, ys)
}

/// Snaps a moved rectangle so its left/centre/right and top/middle/bottom
/// line up with page lines or other elements within `threshold` mm.
#[must_use]
pub fn snap_move(
    r: Rect,
    page_w: f64,
    page_h: f64,
    bleed: f64,
    safe: f64,
    others: &[Rect],
    threshold: f64,
) -> SnapResult {
    let (ex, ey) = element_lines(others);
    let mut tx = page_lines(page_w, bleed, safe);
    tx.extend(ex);
    let mut ty = page_lines(page_h, bleed, safe);
    ty.extend(ey);
    let mut out = r;
    let mut guides = Vec::new();
    if let Some((d, t)) = best(&[r.x, r.x + r.w / 2.0, r.x + r.w], &tx, threshold) {
        out.x += d;
        guides.push(Guide::V(t));
    }
    if let Some((d, t)) = best(&[r.y, r.y + r.h / 2.0, r.y + r.h], &ty, threshold) {
        out.y += d;
        guides.push(Guide::H(t));
    }
    SnapResult { rect: out, guides }
}

/// Snaps the moving edges of a resized (unrotated) rectangle. `moving_left`
/// / `moving_top` say which edges the handle controls.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn snap_resize(
    r: Rect,
    moving_left: bool,
    moving_top: bool,
    page_w: f64,
    page_h: f64,
    bleed: f64,
    safe: f64,
    others: &[Rect],
    threshold: f64,
) -> SnapResult {
    if r.rotation.abs() > 1e-9 {
        return SnapResult {
            rect: r,
            guides: Vec::new(),
        };
    }
    let (ex, ey) = element_lines(others);
    let mut tx = page_lines(page_w, bleed, safe);
    tx.extend(ex);
    let mut ty = page_lines(page_h, bleed, safe);
    ty.extend(ey);
    let mut out = r;
    let mut guides = Vec::new();
    let edge_x = if moving_left { r.x } else { r.x + r.w };
    if let Some((d, t)) = best(&[edge_x], &tx, threshold) {
        if moving_left {
            out.x += d;
            out.w -= d;
        } else {
            out.w += d;
        }
        guides.push(Guide::V(t));
    }
    let edge_y = if moving_top { r.y } else { r.y + r.h };
    if let Some((d, t)) = best(&[edge_y], &ty, threshold) {
        if moving_top {
            out.y += d;
            out.h -= d;
        } else {
            out.h += d;
        }
        guides.push(Guide::H(t));
    }
    if out.w < crate::geometry::MIN_SIZE_MM || out.h < crate::geometry::MIN_SIZE_MM {
        return SnapResult {
            rect: r,
            guides: Vec::new(),
        };
    }
    SnapResult { rect: out, guides }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect {
            x,
            y,
            w,
            h,
            rotation: 0.0,
        }
    }

    #[test]
    fn snaps_to_bleed_edge() {
        // 6×4 print, 2 mm bleed: dragging near the bleed corner snaps exactly.
        let s = snap_move(
            rect(-1.4, -2.6, 50.0, 30.0),
            152.4,
            101.6,
            2.0,
            4.0,
            &[],
            1.5,
        );
        assert!((s.rect.x + 2.0).abs() < 1e-9 && (s.rect.y + 2.0).abs() < 1e-9);
        assert_eq!(s.guides.len(), 2);
    }

    #[test]
    fn snaps_centre_to_page_centre() {
        let s = snap_move(
            rect(50.0, 10.0, 52.0, 20.0),
            152.4,
            101.6,
            2.0,
            4.0,
            &[],
            1.0,
        );
        assert!((s.rect.x + 26.0 - 76.2).abs() < 1e-9);
        assert!(s.guides.contains(&Guide::V(76.2)));
    }

    #[test]
    fn snaps_to_other_elements() {
        let other = rect(20.0, 20.0, 30.0, 30.0);
        let s = snap_move(
            rect(50.6, 33.0, 10.0, 10.0),
            300.0,
            300.0,
            3.0,
            5.0,
            &[other],
            1.0,
        );
        assert!(
            (s.rect.x - 50.0).abs() < 1e-9,
            "left edge meets other's right edge"
        );
    }

    #[test]
    fn no_snap_beyond_threshold() {
        let r = rect(40.0, 40.0, 10.0, 10.0);
        let s = snap_move(r, 300.0, 300.0, 3.0, 5.0, &[], 1.0);
        assert_eq!(s.rect, r);
        assert!(s.guides.is_empty());
    }

    #[test]
    fn resize_snaps_only_moving_edges() {
        let s = snap_resize(
            rect(10.0, 10.0, 141.8, 50.0),
            false,
            false,
            152.4,
            101.6,
            2.0,
            4.0,
            &[],
            1.0,
        );
        assert!((s.rect.x + s.rect.w - 152.4).abs() < 1e-9);
        assert!((s.rect.x - 10.0).abs() < 1e-9, "fixed edge must not move");
    }
}
