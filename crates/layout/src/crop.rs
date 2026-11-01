//! How a photo fills its frame, and how sharp it will print.
//!
//! Photos "cover" their frame: scaled so the frame is completely filled,
//! then multiplied by the crop zoom. Pan moves the visible window across
//! the overflow: -1 shows the left/top edge, 0 the centre, 1 the right/
//! bottom edge.

use printnest_model::project::Crop;
use printnest_model::units::MM_PER_INCH;

/// The visible part of a photo inside a frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CropView {
    /// Visible region in normalised image coordinates (0..1).
    pub u0: f64,
    pub v0: f64,
    pub u1: f64,
    pub v1: f64,
    /// Millimetres per image pixel at the printed size.
    pub mm_per_px: f64,
}

/// Computes the visible region of an `img_w × img_h` photo in a
/// `frame_w × frame_h` mm frame. Returns `None` for degenerate input.
#[must_use]
pub fn crop_view(
    frame_w: f64,
    frame_h: f64,
    img_w: u32,
    img_h: u32,
    crop: &Crop,
) -> Option<CropView> {
    let (iw, ih) = (f64::from(img_w), f64::from(img_h));
    if !(frame_w > 0.0 && frame_h > 0.0 && iw > 0.0 && ih > 0.0) {
        return None;
    }
    let zoom = if crop.zoom.is_finite() {
        crop.zoom.clamp(1.0, 20.0)
    } else {
        1.0
    };
    let pan = |p: f64| {
        if p.is_finite() {
            p.clamp(-1.0, 1.0)
        } else {
            0.0
        }
    };
    let base = (frame_w / iw).max(frame_h / ih);
    let k = base * zoom; // mm per pixel
    let (shown_w, shown_h) = (iw * k, ih * k);
    let (over_x, over_y) = (shown_w - frame_w, shown_h - frame_h);
    let u0 = (over_x / 2.0) * (1.0 + pan(crop.pan_x)) / shown_w;
    let v0 = (over_y / 2.0) * (1.0 + pan(crop.pan_y)) / shown_h;
    Some(CropView {
        u0,
        v0,
        u1: u0 + frame_w / shown_w,
        v1: v0 + frame_h / shown_h,
        mm_per_px: k,
    })
}

/// Pixels per inch the photo will print at in this frame.
#[must_use]
pub fn effective_ppi(
    frame_w: f64,
    frame_h: f64,
    img_w: u32,
    img_h: u32,
    crop: &Crop,
) -> Option<f64> {
    crop_view(frame_w, frame_h, img_w, img_h, crop).map(|v| MM_PER_INCH / v.mm_per_px)
}

/// Converts a drag of `dx_mm, dy_mm` (in page space) into a new pan, so the
/// photo follows the pointer.
#[must_use]
pub fn pan_by(
    frame_w: f64,
    frame_h: f64,
    img_w: u32,
    img_h: u32,
    crop: &Crop,
    dx_mm: f64,
    dy_mm: f64,
) -> Crop {
    let mut out = *crop;
    if let Some(v) = crop_view(frame_w, frame_h, img_w, img_h, crop) {
        let over_x = f64::from(img_w) * v.mm_per_px - frame_w;
        let over_y = f64::from(img_h) * v.mm_per_px - frame_h;
        // Moving the image right (dx > 0) shows more of its left side.
        if over_x > 1e-9 {
            out.pan_x = (crop.pan_x - 2.0 * dx_mm / over_x).clamp(-1.0, 1.0);
        }
        if over_y > 1e-9 {
            out.pan_y = (crop.pan_y - 2.0 * dy_mm / over_y).clamp(-1.0, 1.0);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    fn crop(zoom: f64, pan_x: f64, pan_y: f64) -> Crop {
        Crop { zoom, pan_x, pan_y }
    }

    #[test]
    fn same_aspect_shows_whole_photo() {
        // 6×4 in frame (152.4 × 101.6 mm), 3:2 photo.
        let v = crop_view(152.4, 101.6, 3000, 2000, &crop(1.0, 0.0, 0.0)).unwrap();
        assert!(close(v.u0, 0.0) && close(v.u1, 1.0) && close(v.v0, 0.0) && close(v.v1, 1.0));
        // 3000 px over 6 in = 500 ppi.
        assert!(close(
            effective_ppi(152.4, 101.6, 3000, 2000, &crop(1.0, 0.0, 0.0)).unwrap(),
            500.0
        ));
    }

    #[test]
    fn square_frame_crops_sides_of_landscape_photo() {
        // 100 mm square frame, 4000×2000 photo: height fills, half the width shows.
        let v = crop_view(100.0, 100.0, 4000, 2000, &crop(1.0, 0.0, 0.0)).unwrap();
        assert!(close(v.v0, 0.0) && close(v.v1, 1.0));
        assert!(close(v.u0, 0.25) && close(v.u1, 0.75));
        let left = crop_view(100.0, 100.0, 4000, 2000, &crop(1.0, -1.0, 0.0)).unwrap();
        assert!(close(left.u0, 0.0) && close(left.u1, 0.5));
        let right = crop_view(100.0, 100.0, 4000, 2000, &crop(1.0, 1.0, 0.0)).unwrap();
        assert!(close(right.u0, 0.5) && close(right.u1, 1.0));
    }

    #[test]
    fn zoom_halves_ppi() {
        let a = effective_ppi(100.0, 100.0, 2000, 2000, &crop(1.0, 0.0, 0.0)).unwrap();
        let b = effective_ppi(100.0, 100.0, 2000, 2000, &crop(2.0, 0.0, 0.0)).unwrap();
        assert!(close(a, 2.0 * b));
        let v = crop_view(100.0, 100.0, 2000, 2000, &crop(2.0, 0.0, 0.0)).unwrap();
        assert!(close(v.u0, 0.25) && close(v.u1, 0.75));
    }

    #[test]
    fn visible_region_always_inside_image() {
        for zoom in [1.0, 1.5, 3.0, 20.0] {
            for px in [-1.0, -0.3, 0.0, 0.7, 1.0] {
                for (w, h) in [(1000, 3000), (5000, 100), (64, 64)] {
                    let v = crop_view(80.0, 120.0, w, h, &crop(zoom, px, -px)).unwrap();
                    assert!(v.u0 >= -1e-9 && v.u1 <= 1.0 + 1e-9 && v.u0 < v.u1);
                    assert!(v.v0 >= -1e-9 && v.v1 <= 1.0 + 1e-9 && v.v0 < v.v1);
                }
            }
        }
    }

    #[test]
    fn degenerate_and_hostile_values() {
        assert!(crop_view(0.0, 10.0, 10, 10, &crop(1.0, 0.0, 0.0)).is_none());
        assert!(crop_view(10.0, 10.0, 0, 10, &crop(1.0, 0.0, 0.0)).is_none());
        let v = crop_view(10.0, 10.0, 10, 10, &crop(f64::NAN, f64::INFINITY, -9.0)).unwrap();
        assert!(v.u0.is_finite() && v.v1.is_finite());
    }

    #[test]
    fn panning_follows_the_pointer_and_clamps() {
        let c = crop(1.0, 0.0, 0.0);
        // Landscape photo in square frame: 100 mm overflow horizontally.
        let moved = pan_by(100.0, 100.0, 4000, 2000, &c, 25.0, 0.0);
        assert!(close(moved.pan_x, -0.5));
        assert!(
            close(moved.pan_y, 0.0),
            "no vertical overflow, no vertical pan"
        );
        let far = pan_by(100.0, 100.0, 4000, 2000, &c, -1000.0, 0.0);
        assert!(close(far.pan_x, 1.0));
    }
}
