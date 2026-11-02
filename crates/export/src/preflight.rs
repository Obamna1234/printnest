//! Checks a design before export and explains anything that would print
//! badly, in plain words.

use printnest_layout::effective_ppi;
use printnest_layout::geometry::corners;
use printnest_model::catalogue::Product;
use printnest_model::project::{Element, Project};
use printnest_model::units::{PrintQuality, grade_ppi};
use std::collections::HashMap;

/// How serious an issue is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Will print, but worth a look.
    Warning,
    /// Something is missing from the print.
    Problem,
}

/// One thing to check before printing.
#[derive(Debug, Clone, PartialEq)]
pub struct Issue {
    pub severity: Severity,
    /// 1-based page number.
    pub page: usize,
    pub message: String,
}

/// Runs every check. `sizes` maps each photo path to its full-resolution
/// size; a path that isn't in it is treated as missing.
#[must_use]
pub fn preflight(
    project: &Project,
    product: &Product,
    sizes: &HashMap<String, (u32, u32)>,
) -> Vec<Issue> {
    let t = product.trim();
    let (tw, th) = (t.width.0, t.height.0);
    let outer = product.bleed_mm + product.wrap_mm.unwrap_or(0.0);
    let safe = product.safe_mm;
    let mut out = Vec::new();
    for (i, page) in project.pages.iter().enumerate() {
        let n = i + 1;
        let mut push = |severity, message: String| {
            out.push(Issue {
                severity,
                page: n,
                message,
            });
        };
        for el in &page.elements {
            let r = el.rect();
            let cs = corners(r);
            let (min_x, max_x) = cs
                .iter()
                .fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.x), b.max(p.x)));
            let (min_y, max_y) = cs
                .iter()
                .fold((f64::MAX, f64::MIN), |(a, b), p| (a.min(p.y), b.max(p.y)));
            if max_x <= 0.0 || max_y <= 0.0 || min_x >= tw || min_y >= th {
                push(
                    Severity::Warning,
                    format!("{} is completely off the page and won't print.", what(el)),
                );
                continue;
            }
            match el {
                Element::Photo { src: None, .. } => push(
                    Severity::Warning,
                    "An empty photo frame will print as blank space.".into(),
                ),
                Element::Photo {
                    src: Some(src),
                    crop,
                    ..
                } => {
                    match sizes.get(src.as_str()) {
                        None => push(
                            Severity::Problem,
                            format!("Photo {src} is missing or can't be opened."),
                        ),
                        Some(&(w, h)) => {
                            if let Some(ppi) = effective_ppi(r.w, r.h, w, h, crop)
                                && grade_ppi(ppi) == PrintQuality::Poor
                            {
                                push(
                                    Severity::Warning,
                                    format!(
                                        "Photo {src} will print at {ppi:.0} ppi and may look \
                                         blurry. Use a larger photo or a smaller frame."
                                    ),
                                );
                            }
                        }
                    }
                    // Reaches the trim edge but not the bleed: a thin white
                    // line can appear after cutting.
                    let edge = 0.05;
                    let gaps = [
                        min_x < edge && min_x > -outer + edge,
                        min_y < edge && min_y > -outer + edge,
                        max_x > tw - edge && max_x < tw + outer - edge,
                        max_y > th - edge && max_y < th + outer - edge,
                    ];
                    if outer > 0.0 && gaps.iter().any(|g| *g) {
                        push(
                            Severity::Warning,
                            format!(
                                "Photo {src} touches the edge but doesn't extend into the \
                                 bleed, so a thin white line may show after cutting. \
                                 Stretch it to the red area."
                            ),
                        );
                    }
                }
                Element::Text { text, .. } => {
                    let inside = min_x >= safe - 0.01
                        && min_y >= safe - 0.01
                        && max_x <= tw - safe + 0.01
                        && max_y <= th - safe + 0.01;
                    if !inside && !text.trim().is_empty() {
                        push(
                            Severity::Warning,
                            format!(
                                "Text \"{}\" is outside the safe zone (blue dashed line) \
                                 and may be cut off.",
                                short(text)
                            ),
                        );
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| b.severity.cmp(&a.severity).then(a.page.cmp(&b.page)));
    out
}

fn what(el: &Element) -> &'static str {
    match el {
        Element::Photo { .. } => "A photo frame",
        Element::Text { .. } => "A text box",
    }
}

fn short(text: &str) -> String {
    let one_line: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let mut s: String = one_line.chars().take(30).collect();
    if one_line.chars().count() > 30 {
        s.push('…');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use printnest_model::Catalogue;
    use printnest_model::project::{Align, Crop, Rect};
    use printnest_model::safe_path::SafePath;

    fn photo(x: f64, y: f64, w: f64, h: f64, src: Option<&str>) -> Element {
        Element::Photo {
            id: format!("p{x}{y}{w}"),
            rect: Rect {
                x,
                y,
                w,
                h,
                rotation: 0.0,
            },
            src: src.map(|s| SafePath::new(s).unwrap()),
            crop: Crop::default(),
        }
    }

    #[test]
    fn finds_each_kind_of_problem() {
        let cat = Catalogue::builtin().unwrap();
        let prod = cat.product("poster-a4").unwrap().clone();
        let b = prod.bleed_mm;
        let mut p = Project::new_blank(&cat, "poster-a4", "t").unwrap();
        let els = &mut p.pages[0].elements;
        els.push(photo(
            -b,
            -b,
            210.0 + 2.0 * b,
            297.0 + 2.0 * b,
            Some("photos/good.jpg"),
        ));
        els.push(photo(0.0, 0.0, 100.0, 100.0, Some("photos/gap.jpg")));
        els.push(photo(50.0, 50.0, 100.0, 100.0, Some("photos/missing.jpg")));
        els.push(photo(60.0, 60.0, 100.0, 100.0, Some("photos/tiny.jpg")));
        els.push(photo(80.0, 80.0, 20.0, 20.0, None));
        els.push(photo(900.0, 900.0, 20.0, 20.0, None));
        els.push(Element::Text {
            id: "t".into(),
            rect: Rect {
                x: 1.0,
                y: 1.0,
                w: 50.0,
                h: 10.0,
                rotation: 0.0,
            },
            text: "Edge text".into(),
            font: "sans".into(),
            size_pt: 12.0,
            colour: "#000000".into(),
            align: Align::Left,
        });
        let sizes = HashMap::from([
            ("photos/good.jpg".to_owned(), (4000, 6000)),
            ("photos/gap.jpg".to_owned(), (4000, 4000)),
            ("photos/tiny.jpg".to_owned(), (300, 300)),
        ]);
        let issues = preflight(&p, &prod, &sizes);
        let msgs: Vec<&str> = issues.iter().map(|i| i.message.as_str()).collect();
        assert_eq!(issues.first().map(|i| i.severity), Some(Severity::Problem));
        assert!(msgs.iter().any(|m| m.contains("missing.jpg is missing")));
        assert!(msgs.iter().any(|m| m.contains("tiny.jpg will print at")));
        assert!(msgs.iter().any(|m| m.contains("gap.jpg touches the edge")));
        assert!(msgs.iter().any(|m| m.contains("empty photo frame")));
        assert!(msgs.iter().any(|m| m.contains("completely off the page")));
        assert!(msgs.iter().any(|m| m.contains("Edge text")));
        assert!(!msgs.iter().any(|m| m.contains("good.jpg")), "{msgs:?}");
    }
}
