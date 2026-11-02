//! Text layout and glyph outlines, matching the editor's conventions:
//! the font's em size is the point size, lines are `ascent - descent +
//! leading` apart, the first baseline is `ascent` below the box top, and
//! lines wrap greedily at spaces (breaking long words) within the box width.

use printnest_model::project::{Align, Rect};
use skrifa::instance::{LocationRef, Size};
use skrifa::metrics::GlyphMetrics;
use skrifa::outline::{DrawSettings, OutlinePen};
use skrifa::{FontRef, GlyphId, MetadataProvider};
use vello_cpu::kurbo::BezPath;

/// A bundled font.
pub(crate) struct Face {
    font: Option<FontRef<'static>>,
}

impl Face {
    /// The editor's fonts: "mono" is Hack, everything else Ubuntu Light.
    pub(crate) fn for_name(name: &str) -> Self {
        let bytes = if name == "mono" {
            epaint_default_fonts::HACK_REGULAR
        } else {
            epaint_default_fonts::UBUNTU_LIGHT
        };
        Face {
            font: FontRef::new(bytes).ok(),
        }
    }
}

/// Collects a glyph outline into a path, offset and flipped (fonts are y-up).
struct Pen<'p> {
    path: &'p mut BezPath,
    x: f64,
    y: f64,
}

impl Pen<'_> {
    fn p(&self, x: f32, y: f32) -> (f64, f64) {
        (self.x + f64::from(x), self.y - f64::from(y))
    }
}

impl OutlinePen for Pen<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        let p = self.p(x, y);
        self.path.move_to(p);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.p(x, y);
        self.path.line_to(p);
    }
    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        let (c, p) = (self.p(cx0, cy0), self.p(x, y));
        self.path.quad_to(c, p);
    }
    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        let (c0, c1, p) = (self.p(cx0, cy0), self.p(cx1, cy1), self.p(x, y));
        self.path.curve_to(c0, c1, p);
    }
    fn close(&mut self) {
        self.path.close_path();
    }
}

fn glyph(font: &FontRef<'_>, c: char) -> Option<GlyphId> {
    let c = if c == '\t' { ' ' } else { c };
    font.charmap().map(c).filter(|g| g.to_u32() != 0)
}

fn advance(font: &FontRef<'_>, gm: &GlyphMetrics<'_>, c: char) -> f64 {
    glyph(font, c)
        .and_then(|g| gm.advance_width(g))
        .map_or(0.0, f64::from)
}

fn width(font: &FontRef<'_>, gm: &GlyphMetrics<'_>, s: &str) -> f64 {
    s.chars().map(|c| advance(font, gm, c)).sum()
}

/// Greedy word wrap of one paragraph into lines no wider than `max`.
fn wrap(font: &FontRef<'_>, gm: &GlyphMetrics<'_>, para: &str, max: f64) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut line_w = 0.0;
    for word in para.split_inclusive(' ') {
        let w = width(font, gm, word);
        let trimmed_w = width(font, gm, word.trim_end_matches(' '));
        if line_w + trimmed_w <= max || line.is_empty() {
            if line.is_empty() && trimmed_w > max {
                // A word longer than the box: break it by character.
                for c in word.chars() {
                    let cw = advance(font, gm, c);
                    if line_w + cw > max && !line.is_empty() {
                        lines.push(std::mem::take(&mut line));
                        line_w = 0.0;
                    }
                    line.push(c);
                    line_w += cw;
                }
                continue;
            }
            line.push_str(word);
            line_w += w;
        } else {
            lines.push(std::mem::take(&mut line).trim_end_matches(' ').to_owned());
            line.push_str(word);
            line_w = w;
        }
    }
    lines.push(line.trim_end_matches(' ').to_owned());
    lines
}

/// Glyph outlines for `text` in a box `rect`, in page millimetres relative
/// to the page origin, before the element's rotation.
pub(crate) fn layout(
    face: &Face,
    text: &str,
    size_pt: f64,
    rect: &Rect,
    align: Align,
) -> Vec<BezPath> {
    let Some(font) = &face.font else {
        return Vec::new();
    };
    let em_mm = size_pt * 25.4 / 72.0;
    if !(em_mm.is_finite() && em_mm > 0.0) {
        return Vec::new();
    }
    let size = Size::new(em_mm as f32);
    let loc = LocationRef::default();
    let m = font.metrics(size, loc);
    let gm = font.glyph_metrics(size, loc);
    let outlines = font.outline_glyphs();
    let line_h = f64::from(m.ascent - m.descent + m.leading);
    let mut out = Vec::new();
    let mut baseline = rect.y + f64::from(m.ascent);
    for para in text.split('\n') {
        for line in wrap(font, &gm, para, rect.w) {
            let lw = width(font, &gm, &line);
            let mut x = match align {
                Align::Left => rect.x,
                Align::Center => rect.x + (rect.w - lw) / 2.0,
                Align::Right => rect.x + rect.w - lw,
            };
            let mut path = BezPath::new();
            for c in line.chars() {
                if let Some(g) = glyph(font, c) {
                    if let Some(o) = outlines.get(g) {
                        let mut pen = Pen {
                            path: &mut path,
                            x,
                            y: baseline,
                        };
                        let _ = o.draw(DrawSettings::unhinted(size, loc), &mut pen);
                    }
                    x += gm.advance_width(g).map_or(0.0, f64::from);
                }
            }
            if !path.elements().is_empty() {
                out.push(path);
            }
            baseline += line_h;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_at_spaces_and_breaks_long_words() {
        let face = Face::for_name("sans");
        let font = face.font.as_ref().unwrap();
        let size = Size::new(10.0);
        let gm = font.glyph_metrics(size, LocationRef::default());
        let one = width(font, &gm, "hello");
        let lines = wrap(font, &gm, "hello world again", one * 1.6);
        assert_eq!(lines, vec!["hello", "world", "again"]);
        let long = wrap(font, &gm, "abcdefghijklmnop", one);
        assert!(long.len() > 1);
        assert_eq!(long.concat(), "abcdefghijklmnop");
        assert_eq!(wrap(font, &gm, "", 10.0), vec![String::new()]);
    }

    #[test]
    fn both_fonts_load_and_draw() {
        let r = Rect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 20.0,
            rotation: 0.0,
        };
        for f in ["sans", "mono"] {
            let paths = layout(
                &Face::for_name(f),
                "Hi there\nline 2",
                12.0,
                &r,
                Align::Left,
            );
            assert_eq!(paths.len(), 2, "{f}");
        }
        // Unknown characters are skipped, not drawn as boxes or panics.
        assert!(layout(&Face::for_name("sans"), "\u{10FFFF}", 12.0, &r, Align::Left).is_empty());
    }
}
