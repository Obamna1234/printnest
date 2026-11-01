//! Draws a product's print geometry to scale.
//!
//! Colour key (matches SPEC section 5):
//! - red tint: bleed (trimmed off)
//! - black line: trim (finished edge)
//! - blue dashed line: safe zone (keep text and faces inside)
//! - orange: canvas wrap, mat window, book gutter, card fold

use eframe::egui::{self, Color32, Pos2, Rect, Stroke, StrokeKind, Vec2};
use printnest_model::catalogue::{Family, Fold, Product, Shape};

const TRIM: Color32 = Color32::from_rgb(20, 20, 20);
const SAFE: Color32 = Color32::from_rgb(40, 110, 220);
const EXTRA: Color32 = Color32::from_rgb(230, 130, 20);
const PAPER: Color32 = Color32::from_rgb(252, 252, 250);

pub fn draw(ui: &mut egui::Ui, p: &Product, show_bleed: bool, show_safe: bool, show_extras: bool) {
    let trim = p.trim();
    let wrap = p.wrap_mm.unwrap_or(0.0) as f32;
    let bleed = p.bleed_mm as f32;
    let outer = wrap + bleed;
    // Books are shown as an open two-page spread.
    let spread = p.family == Family::Book;
    let page_w = trim.width.0 as f32;
    let page_h = trim.height.0 as f32;
    let trim_w = if spread { page_w * 2.0 } else { page_w };
    let full = Vec2::new(trim_w + outer * 2.0, page_h + outer * 2.0);

    let avail = ui.available_rect_before_wrap().shrink(24.0);
    let (response, painter) = ui.allocate_painter(ui.available_size(), egui::Sense::hover());
    let _ = response;
    if full.x <= 0.0 || full.y <= 0.0 || avail.width() <= 0.0 || avail.height() <= 0.0 {
        return;
    }
    let scale = (avail.width() / full.x).min(avail.height() / full.y);
    let size = full * scale;
    let origin = avail.center() - size / 2.0;
    // mm (relative to artwork top-left) → screen.
    let to_screen = |x: f32, y: f32| Pos2::new(origin.x + x * scale, origin.y + y * scale);
    let rect_mm = |x: f32, y: f32, w: f32, h: f32| {
        Rect::from_min_max(to_screen(x, y), to_screen(x + w, y + h))
    };

    let artwork = rect_mm(0.0, 0.0, full.x, full.y);
    let trim_rect = rect_mm(outer, outer, trim_w, page_h);

    painter.rect_filled(artwork, 0.0, PAPER);
    if show_bleed && bleed > 0.0 {
        // Paint the bleed band only (four strips around the wrap/trim box).
        let inner = rect_mm(bleed, bleed, full.x - bleed * 2.0, full.y - bleed * 2.0);
        for strip in ring(artwork, inner) {
            painter.rect_filled(strip, 0.0, Color32::from_rgba_unmultiplied(210, 60, 60, 90));
        }
    }
    if show_extras && wrap > 0.0 {
        let face = trim_rect;
        let wrap_box = rect_mm(bleed, bleed, full.x - bleed * 2.0, full.y - bleed * 2.0);
        for strip in ring(wrap_box, face) {
            painter.rect_filled(strip, 0.0, EXTRA.gamma_multiply(0.18));
        }
        label(
            &painter,
            wrap_box.center_top() + Vec2::new(0.0, 4.0),
            "wrap edge",
        );
    }

    match p.shape {
        Shape::Rect => {
            painter.rect_stroke(trim_rect, 0.0, Stroke::new(1.5, TRIM), StrokeKind::Middle);
        }
        Shape::Rounded => {
            painter.rect_stroke(
                trim_rect,
                8.0 * scale,
                Stroke::new(1.5, TRIM),
                StrokeKind::Middle,
            );
        }
        Shape::Circle => {
            painter.rect_stroke(
                trim_rect,
                0.0,
                Stroke::new(0.5, TRIM.gamma_multiply(0.3)),
                StrokeKind::Middle,
            );
            painter.circle_stroke(
                trim_rect.center(),
                trim_rect.width().min(trim_rect.height()) / 2.0,
                Stroke::new(1.5, TRIM),
            );
        }
    }

    if spread {
        let mid = to_screen(outer + page_w, outer);
        let bottom = to_screen(outer + page_w, outer + page_h);
        painter.line_segment([mid, bottom], Stroke::new(1.0, TRIM.gamma_multiply(0.5)));
    }

    if show_safe {
        let s = p.safe_mm as f32;
        let pages: Vec<Rect> = if spread {
            vec![
                rect_mm(outer + s, outer + s, page_w - s * 2.0, page_h - s * 2.0),
                rect_mm(
                    outer + page_w + s,
                    outer + s,
                    page_w - s * 2.0,
                    page_h - s * 2.0,
                ),
            ]
        } else {
            vec![rect_mm(
                outer + s,
                outer + s,
                page_w - s * 2.0,
                page_h - s * 2.0,
            )]
        };
        for r in pages {
            if r.width() > 0.0 && r.height() > 0.0 {
                dashed_rect(&painter, r, Stroke::new(1.0, SAFE));
            }
        }
    }

    if show_extras {
        if let Some(g) = p.gutter_mm
            && spread
        {
            let g = g as f32;
            let r = rect_mm(outer + page_w - g, outer, g * 2.0, page_h);
            painter.rect_filled(r, 0.0, EXTRA.gamma_multiply(0.15));
            label(&painter, r.center_top() + Vec2::new(0.0, 4.0), "gutter");
        }
        if let Some(m) = p.mat_opening {
            let m = m.to_mm();
            let (mw, mh) = (m.width.0 as f32, m.height.0 as f32);
            let r = rect_mm(
                outer + (page_w - mw) / 2.0,
                outer + (page_h - mh) / 2.0,
                mw,
                mh,
            );
            painter.rect_stroke(r, 0.0, Stroke::new(2.0, EXTRA), StrokeKind::Middle);
            label(&painter, r.center_top() + Vec2::new(0.0, 4.0), "mat window");
        }
        if let Some(fold) = p.fold {
            let (a, b) = match fold {
                Fold::Vertical => (to_screen(outer, outer), to_screen(outer, outer + page_h)),
                Fold::Horizontal => (to_screen(outer, outer), to_screen(outer + page_w, outer)),
            };
            painter.extend(egui::Shape::dashed_line(
                &[a, b],
                Stroke::new(2.0, EXTRA),
                6.0,
                4.0,
            ));
            label(&painter, a + Vec2::new(6.0, 4.0), "fold");
        }
        if let Some(b) = p.binding_mm {
            let r = rect_mm(outer, outer, trim_w, b as f32);
            painter.rect_filled(r, 0.0, EXTRA.gamma_multiply(0.15));
            label(&painter, r.center_top() + Vec2::new(0.0, 2.0), "binding");
        }
    }

    let caption = format!(
        "{:.0} × {:.0} mm{}",
        trim.width.0,
        trim.height.0,
        if spread {
            " per page (open spread shown)"
        } else {
            ""
        }
    );
    painter.text(
        artwork.center_bottom() + Vec2::new(0.0, 6.0),
        egui::Align2::CENTER_TOP,
        caption,
        egui::FontId::proportional(13.0),
        ui.visuals().text_color(),
    );
}

/// The four strips between an outer and an inner rectangle.
fn ring(outer: Rect, inner: Rect) -> [Rect; 4] {
    [
        Rect::from_min_max(outer.min, Pos2::new(outer.max.x, inner.min.y)),
        Rect::from_min_max(Pos2::new(outer.min.x, inner.max.y), outer.max),
        Rect::from_min_max(
            Pos2::new(outer.min.x, inner.min.y),
            Pos2::new(inner.min.x, inner.max.y),
        ),
        Rect::from_min_max(
            Pos2::new(inner.max.x, inner.min.y),
            Pos2::new(outer.max.x, inner.max.y),
        ),
    ]
}

fn dashed_rect(painter: &egui::Painter, r: Rect, stroke: Stroke) {
    let pts = [
        r.left_top(),
        r.right_top(),
        r.right_bottom(),
        r.left_bottom(),
        r.left_top(),
    ];
    painter.extend(egui::Shape::dashed_line(&pts, stroke, 6.0, 4.0));
}

fn label(painter: &egui::Painter, at: Pos2, text: &str) {
    painter.text(
        at,
        egui::Align2::CENTER_TOP,
        text,
        egui::FontId::proportional(11.0),
        EXTRA,
    );
}
