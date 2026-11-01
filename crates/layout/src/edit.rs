//! Small editing helpers: ids, photo names, new frames and text.

use printnest_model::limits::{MAX_ELEMENTS_PER_PAGE, MAX_ID_LEN, MAX_PAGE_EDGE_MM};
use printnest_model::project::{Align, Crop, Element, Project, Rect};
use printnest_model::safe_path::SafePath;
use std::collections::HashSet;

/// A fresh element id not used anywhere in the project, like `photo-7`.
#[must_use]
pub fn new_id(project: &Project, prefix: &str) -> String {
    let used: HashSet<&str> = project
        .pages
        .iter()
        .flat_map(|p| p.elements.iter().map(Element::id))
        .collect();
    let prefix: String = prefix
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(MAX_ID_LEN - 12)
        .collect();
    let prefix = if prefix.is_empty() {
        "el".to_owned()
    } else {
        prefix
    };
    (1u64..)
        .map(|n| format!("{prefix}-{n}"))
        .find(|id| !used.contains(id.as_str()))
        .unwrap_or_else(|| prefix.clone())
}

/// A project-relative, validated name for a photo picked from disk:
/// `photos/<safe name>.<ext>`, unique among `taken`. Uniqueness ignores
/// case, because Windows and macOS treat `a.jpg` and `A.JPG` as one file.
#[must_use]
pub fn photo_path(original_name: &str, taken: &HashSet<String>) -> Option<SafePath> {
    let taken: HashSet<String> = taken.iter().map(|t| t.to_lowercase()).collect();
    let (stem, ext) = match original_name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s, e.to_ascii_lowercase()),
        _ => (original_name, String::from("img")),
    };
    let mut ext: String = ext
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(5)
        .collect();
    if ext.is_empty() {
        ext = "img".into();
    }
    let mut clean: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(60)
        .collect();
    if clean.trim_matches('_').is_empty() {
        clean = "photo".into();
    }
    // A name the path rules still refuse (e.g. Windows device names like
    // CON) gets a neutral prefix rather than being dropped.
    let candidate = |base: &str, n: u32| {
        if n == 1 {
            format!("photos/{base}.{ext}")
        } else {
            format!("photos/{base}-{n}.{ext}")
        }
    };
    let base = if SafePath::new(&candidate(&clean, 1)).is_ok() {
        clean
    } else {
        format!("photo_{clean}")
    };
    (1u32..10_000)
        .map(|n| candidate(&base, n))
        .find(|p| !taken.contains(&p.to_lowercase()))
        .and_then(|p| SafePath::new(&p).ok())
}

/// Points every frame showing `old` at `new` instead (used when a photo
/// had to be stored under a different name).
pub fn retarget_photo(project: &mut Project, old: &SafePath, new: &SafePath) {
    for el in project.pages.iter_mut().flat_map(|p| p.elements.iter_mut()) {
        if let Element::Photo { src: Some(s), .. } = el
            && s == old
        {
            *s = new.clone();
        }
    }
}

/// Error when a page is full.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageFull;

fn page_mut(project: &mut Project, page: usize) -> Option<&mut printnest_model::project::Page> {
    project.pages.get_mut(page)
}

/// Adds a photo frame covering the whole page including bleed.
///
/// # Errors
/// If the page already holds the maximum number of elements or doesn't exist.
pub fn add_full_bleed_photo(
    project: &mut Project,
    page: usize,
    w: f64,
    h: f64,
    bleed: f64,
    src: Option<SafePath>,
) -> Result<usize, PageFull> {
    let id = new_id(project, "photo");
    let el = Element::Photo {
        id,
        rect: Rect {
            x: -bleed,
            y: -bleed,
            w: w + 2.0 * bleed,
            h: h + 2.0 * bleed,
            rotation: 0.0,
        },
        src,
        crop: Crop::default(),
    };
    push(project, page, el)
}

/// Adds a photo frame of `fw × fh` mm centred on the page.
///
/// # Errors
/// If the page is full or doesn't exist.
pub fn add_photo_frame(
    project: &mut Project,
    page: usize,
    page_w: f64,
    page_h: f64,
    src: Option<SafePath>,
) -> Result<usize, PageFull> {
    let fw = (page_w * 0.5).max(10.0);
    let fh = (page_h * 0.5).max(10.0);
    let id = new_id(project, "photo");
    let el = Element::Photo {
        id,
        rect: Rect {
            x: (page_w - fw) / 2.0,
            y: (page_h - fh) / 2.0,
            w: fw,
            h: fh,
            rotation: 0.0,
        },
        src,
        crop: Crop::default(),
    };
    push(project, page, el)
}

/// Adds a text box near the bottom of the safe area.
///
/// # Errors
/// If the page is full or doesn't exist.
pub fn add_text(
    project: &mut Project,
    page: usize,
    page_w: f64,
    page_h: f64,
    safe: f64,
) -> Result<usize, PageFull> {
    let id = new_id(project, "text");
    let w = (page_w - 2.0 * safe).max(10.0);
    let h = (page_h * 0.12).clamp(8.0, 40.0);
    let size_pt = (h / 25.4 * 72.0 * 0.6).clamp(8.0, 72.0);
    let el = Element::Text {
        id,
        rect: Rect {
            x: safe,
            y: page_h - safe - h,
            w,
            h,
            rotation: 0.0,
        },
        text: "Your text".into(),
        font: "sans".into(),
        size_pt: size_pt.round(),
        colour: "#222222".into(),
        align: Align::Center,
    };
    push(project, page, el)
}

/// Moves a duplicate 5 mm along, staying within the page limits.
fn offset(v: f64) -> f64 {
    (v + 5.0).clamp(-MAX_PAGE_EDGE_MM, MAX_PAGE_EDGE_MM)
}

fn push(project: &mut Project, page: usize, el: Element) -> Result<usize, PageFull> {
    let p = page_mut(project, page).ok_or(PageFull)?;
    if p.elements.len() >= MAX_ELEMENTS_PER_PAGE {
        return Err(PageFull);
    }
    p.elements.push(el);
    Ok(p.elements.len() - 1)
}

/// Duplicates element `idx` on `page`, offset by 5 mm, with a new id.
///
/// # Errors
/// If the page is full or the element doesn't exist.
pub fn duplicate(project: &mut Project, page: usize, idx: usize) -> Result<usize, PageFull> {
    let el = project
        .pages
        .get(page)
        .and_then(|p| p.elements.get(idx))
        .cloned()
        .ok_or(PageFull)?;
    let prefix = match &el {
        Element::Photo { .. } => "photo",
        Element::Text { .. } => "text",
    };
    let new = new_id(project, prefix);
    let el = match el {
        Element::Photo {
            rect, src, crop, ..
        } => Element::Photo {
            id: new,
            rect: Rect {
                x: offset(rect.x),
                y: offset(rect.y),
                ..rect
            },
            src,
            crop,
        },
        Element::Text {
            rect,
            text,
            font,
            size_pt,
            colour,
            align,
            ..
        } => Element::Text {
            id: new,
            rect: Rect {
                x: offset(rect.x),
                y: offset(rect.y),
                ..rect
            },
            text,
            font,
            size_pt,
            colour,
            align,
        },
    };
    push(project, page, el)
}

/// Moves element `idx` one step toward the front (`up`) or back.
pub fn reorder(project: &mut Project, page: usize, idx: usize, up: bool) -> Option<usize> {
    let p = page_mut(project, page)?;
    let to = if up {
        idx.checked_add(1)?
    } else {
        idx.checked_sub(1)?
    };
    if to >= p.elements.len() || idx >= p.elements.len() {
        return None;
    }
    p.elements.swap(idx, to);
    Some(to)
}

#[cfg(test)]
mod tests {
    use super::*;
    use printnest_model::Catalogue;

    fn proj() -> (Catalogue, Project) {
        let cat = Catalogue::builtin().unwrap();
        let p = Project::new_blank(&cat, "poster-a4", "t").unwrap();
        (cat, p)
    }

    #[test]
    fn ids_are_unique_and_valid() {
        let (cat, mut p) = proj();
        for _ in 0..20 {
            add_photo_frame(&mut p, 0, 210.0, 297.0, None).unwrap();
            add_text(&mut p, 0, 210.0, 297.0, 6.0).unwrap();
        }
        assert!(p.validate(&cat).is_ok(), "{:?}", p.validate(&cat));
        assert_eq!(new_id(&p, "photo"), "photo-21");
        assert_eq!(new_id(&p, "../evil id"), "evil-1".replace("evil", "evilid"));
    }

    #[test]
    fn photo_names_are_safe_and_unique() {
        let mut taken = HashSet::new();
        let a = photo_path("IMG_1234.JPG", &taken).unwrap();
        assert_eq!(a.as_str(), "photos/IMG_1234.jpg");
        taken.insert(a.as_str().to_owned());
        let b = photo_path("IMG_1234.JPG", &taken).unwrap();
        assert_eq!(b.as_str(), "photos/IMG_1234-2.jpg");
        // Case-only differences are the same file on Windows and macOS.
        let c = photo_path("img_1234.jpg", &taken).unwrap();
        assert_eq!(c.as_str(), "photos/img_1234-2.jpg");
        for hostile in [
            "../../etc/passwd",
            "CON.jpg",
            "a\u{202e}gpj.exe",
            "...",
            "",
            "photo:1.png",
            "日本.jpg",
        ] {
            let p = photo_path(hostile, &HashSet::new()).unwrap();
            assert!(SafePath::new(p.as_str()).is_ok(), "{hostile:?} -> {p}");
            assert!(p.as_str().starts_with("photos/"));
        }
    }

    #[test]
    fn full_bleed_frame_covers_bleed() {
        let (cat, mut p) = proj();
        let i = add_full_bleed_photo(&mut p, 0, 210.0, 297.0, 3.0, None).unwrap();
        let r = *p.pages[0].elements[i].rect();
        assert_eq!((r.x, r.y, r.w, r.h), (-3.0, -3.0, 216.0, 303.0));
        assert!(p.validate(&cat).is_ok());
    }

    #[test]
    fn page_limit_is_respected() {
        let (_, mut p) = proj();
        for _ in 0..MAX_ELEMENTS_PER_PAGE {
            add_photo_frame(&mut p, 0, 210.0, 297.0, None).unwrap();
        }
        assert_eq!(add_text(&mut p, 0, 210.0, 297.0, 6.0), Err(PageFull));
        assert_eq!(add_text(&mut p, 9, 210.0, 297.0, 6.0), Err(PageFull));
    }

    #[test]
    fn duplicate_and_reorder() {
        let (cat, mut p) = proj();
        add_photo_frame(&mut p, 0, 210.0, 297.0, None).unwrap();
        add_text(&mut p, 0, 210.0, 297.0, 6.0).unwrap();
        let d = duplicate(&mut p, 0, 1).unwrap();
        assert_eq!(d, 2);
        assert!(p.validate(&cat).is_ok());
        assert_eq!(reorder(&mut p, 0, 2, false), Some(1));
        assert_eq!(reorder(&mut p, 0, 0, false), None);
        assert_eq!(reorder(&mut p, 0, 2, true), None);
    }

    #[test]
    fn duplicate_at_the_edge_stays_valid() {
        let (cat, mut p) = proj();
        add_photo_frame(&mut p, 0, 210.0, 297.0, None).unwrap();
        if let Element::Photo { rect, .. } = &mut p.pages[0].elements[0] {
            rect.x = MAX_PAGE_EDGE_MM;
            rect.y = MAX_PAGE_EDGE_MM;
        }
        assert!(p.validate(&cat).is_ok());
        duplicate(&mut p, 0, 0).unwrap();
        assert!(p.validate(&cat).is_ok(), "{:?}", p.validate(&cat));
    }

    #[test]
    fn retarget_changes_only_matching_frames() {
        let (_, mut p) = proj();
        let a = SafePath::new("photos/a.jpg").unwrap();
        let b = SafePath::new("photos/b.jpg").unwrap();
        let c = SafePath::new("photos/c.jpg").unwrap();
        add_photo_frame(&mut p, 0, 210.0, 297.0, Some(a.clone())).unwrap();
        add_photo_frame(&mut p, 0, 210.0, 297.0, Some(b.clone())).unwrap();
        retarget_photo(&mut p, &a, &c);
        let srcs: Vec<_> = p.photo_paths().into_iter().cloned().collect();
        assert_eq!(srcs, vec![c, b]);
    }
}
