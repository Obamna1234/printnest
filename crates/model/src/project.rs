//! The project manifest: what a user has designed.
//!
//! A project is stored as JSON (inside a `.pnest` archive from M2). The
//! manifest is plain data: no scripts, no URLs, no absolute paths. Loading
//! enforces, in order:
//!
//! 1. a byte-size cap before parsing ([`MAX_MANIFEST_BYTES`]);
//! 2. strict parsing (`deny_unknown_fields`, tagged element types);
//! 3. path validation for every photo reference ([`SafePath`]);
//! 4. semantic validation against the catalogue ([`Project::validate`]).

use crate::catalogue::{Catalogue, Family, Product};
use crate::limits::{
    MAX_ELEMENTS_PER_PAGE, MAX_ID_LEN, MAX_MANIFEST_BYTES, MAX_PAGE_EDGE_MM, MAX_PAGES,
    MAX_PHOTOS_PER_PROJECT, MAX_TEXT_CHARS, MAX_TITLE_CHARS,
};
use crate::safe_path::SafePath;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io::Read;

/// Magic string identifying a PrintNest manifest.
pub const FORMAT: &str = "printnest-project";

/// Current manifest schema version.
pub const SCHEMA_VERSION: u32 = 1;

/// Why a project failed to load or validate.
#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("project file is larger than {MAX_MANIFEST_BYTES} bytes")]
    TooLarge,
    #[error("could not read project: {0}")]
    Io(#[from] std::io::Error),
    #[error("project is not valid: {0}")]
    Parse(String),
    #[error("not a PrintNest project")]
    WrongFormat,
    #[error("project was made by a newer PrintNest (schema {0}); please update")]
    UnsupportedVersion(u32),
    #[error("{0}")]
    Invalid(String),
}

fn invalid(msg: impl Into<String>) -> ProjectError {
    ProjectError::Invalid(msg.into())
}

/// Horizontal text alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

/// Position and size of an element on its page, in mm from the page's trim
/// top-left corner. Elements may extend into the bleed.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
    /// Clockwise rotation in degrees.
    #[serde(default)]
    pub rotation: f64,
}

/// How a photo sits inside its frame.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Crop {
    /// 1.0 = photo just fills the frame; larger zooms in.
    pub zoom: f64,
    /// Pan from centre, -1.0 (left/top) to 1.0 (right/bottom).
    pub pan_x: f64,
    pub pan_y: f64,
}

impl Default for Crop {
    fn default() -> Self {
        Crop {
            zoom: 1.0,
            pan_x: 0.0,
            pan_y: 0.0,
        }
    }
}

/// Something placed on a page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum Element {
    /// A photo frame. `src` is `None` for an empty placeholder.
    Photo {
        id: String,
        rect: Rect,
        #[serde(default)]
        src: Option<SafePath>,
        #[serde(default)]
        crop: Crop,
    },
    /// A text box.
    Text {
        id: String,
        rect: Rect,
        text: String,
        #[serde(default = "default_font")]
        font: String,
        size_pt: f64,
        /// `#rrggbb`
        #[serde(default = "default_colour")]
        colour: String,
        #[serde(default)]
        align: Align,
    },
}

fn default_font() -> String {
    "sans".into()
}

fn default_colour() -> String {
    "#000000".into()
}

impl Element {
    #[must_use]
    pub fn id(&self) -> &str {
        match self {
            Element::Photo { id, .. } | Element::Text { id, .. } => id,
        }
    }

    #[must_use]
    pub fn rect(&self) -> &Rect {
        match self {
            Element::Photo { rect, .. } | Element::Text { rect, .. } => rect,
        }
    }
}

/// One page (or one side, for single-page products).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Page {
    #[serde(default, deserialize_with = "bounded::elements")]
    pub elements: Vec<Element>,
    /// Background colour, `#rrggbb`.
    #[serde(default = "default_background")]
    pub background: String,
}

impl Default for Page {
    fn default() -> Self {
        Page {
            elements: Vec::new(),
            background: default_background(),
        }
    }
}

fn default_background() -> String {
    "#ffffff".into()
}

/// A user's design.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub format: String,
    pub schema_version: u32,
    pub title: String,
    pub product: String,
    pub paper: String,
    /// Cover option id; required for books, absent otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover: Option<String>,
    #[serde(deserialize_with = "bounded::pages")]
    pub pages: Vec<Page>,
}

impl Project {
    /// A new, empty project for `product`, using its first paper and cover
    /// and its minimum page count.
    ///
    /// # Errors
    /// If the product id is unknown.
    pub fn new_blank(
        catalogue: &Catalogue,
        product_id: &str,
        title: &str,
    ) -> Result<Self, ProjectError> {
        let product = catalogue
            .product(product_id)
            .ok_or_else(|| invalid(format!("unknown product {product_id:?}")))?;
        let paper = product
            .papers
            .first()
            .cloned()
            .ok_or_else(|| invalid("product has no papers"))?;
        let cover = product.covers.first().map(|c| c.id.clone());
        let pages = (0..product.default_pages())
            .map(|_| Page::default())
            .collect();
        let project = Project {
            format: FORMAT.into(),
            schema_version: SCHEMA_VERSION,
            title: title.into(),
            product: product.id.clone(),
            paper,
            cover,
            pages,
        };
        project.validate(catalogue)?;
        Ok(project)
    }

    /// Parses and validates a manifest from a string.
    ///
    /// # Errors
    /// On oversize input, bad JSON, unknown fields, unsafe paths or any
    /// validation failure.
    pub fn from_json(text: &str, catalogue: &Catalogue) -> Result<Self, ProjectError> {
        if text.len() as u64 > MAX_MANIFEST_BYTES {
            return Err(ProjectError::TooLarge);
        }
        // Check the header first so a non-project file gets a clear error.
        #[derive(Deserialize)]
        struct Header {
            format: Option<String>,
            schema_version: Option<u32>,
        }
        let header: Header =
            serde_json::from_str(text).map_err(|e| ProjectError::Parse(e.to_string()))?;
        if header.format.as_deref() != Some(FORMAT) {
            return Err(ProjectError::WrongFormat);
        }
        match header.schema_version {
            Some(v) if v > SCHEMA_VERSION => return Err(ProjectError::UnsupportedVersion(v)),
            Some(0) | None => return Err(ProjectError::WrongFormat),
            Some(_) => {}
        }
        let project: Project =
            serde_json::from_str(text).map_err(|e| ProjectError::Parse(e.to_string()))?;
        project.validate(catalogue)?;
        Ok(project)
    }

    /// Reads at most [`MAX_MANIFEST_BYTES`] from `reader`, then parses.
    ///
    /// # Errors
    /// As [`Project::from_json`], plus I/O errors and invalid UTF-8.
    pub fn from_reader(reader: impl Read, catalogue: &Catalogue) -> Result<Self, ProjectError> {
        let mut buf = Vec::new();
        reader.take(MAX_MANIFEST_BYTES + 1).read_to_end(&mut buf)?;
        if buf.len() as u64 > MAX_MANIFEST_BYTES {
            return Err(ProjectError::TooLarge);
        }
        let text = String::from_utf8(buf).map_err(|_| ProjectError::Parse("not UTF-8".into()))?;
        Self::from_json(&text, catalogue)
    }

    /// Serialises to pretty JSON.
    ///
    /// # Errors
    /// Only if serialisation fails, which plain data cannot.
    pub fn to_json(&self) -> Result<String, ProjectError> {
        serde_json::to_string_pretty(self).map_err(|e| ProjectError::Parse(e.to_string()))
    }

    /// Compact JSON (what project folders store, so a project that loads
    /// always fits back under the manifest size limit when saved).
    ///
    /// # Errors
    /// If serialisation fails (it cannot for a validated project).
    pub fn to_json_compact(&self) -> Result<String, ProjectError> {
        serde_json::to_string(self).map_err(|e| ProjectError::Parse(e.to_string()))
    }

    /// Every distinct photo this project references.
    #[must_use]
    pub fn photo_paths(&self) -> Vec<&SafePath> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for page in &self.pages {
            for el in &page.elements {
                if let Element::Photo { src: Some(p), .. } = el
                    && seen.insert(p)
                {
                    out.push(p);
                }
            }
        }
        out
    }

    /// Checks the project against the catalogue and all limits.
    ///
    /// # Errors
    /// Describes the first problem found.
    pub fn validate(&self, catalogue: &Catalogue) -> Result<(), ProjectError> {
        if self.format != FORMAT {
            return Err(ProjectError::WrongFormat);
        }
        if self.schema_version == 0 || self.schema_version > SCHEMA_VERSION {
            return Err(ProjectError::UnsupportedVersion(self.schema_version));
        }
        check_text_field("title", &self.title, MAX_TITLE_CHARS, false)?;

        let product = catalogue
            .product(&self.product)
            .ok_or_else(|| invalid(format!("unknown product {:?}", self.product)))?;
        if !product.allows_paper(&self.paper) {
            return Err(invalid(format!(
                "paper {:?} is not available for {}",
                self.paper, product.name
            )));
        }
        validate_cover(product, self.cover.as_deref())?;

        let count = u32::try_from(self.pages.len()).unwrap_or(u32::MAX);
        if count > MAX_PAGES {
            return Err(invalid(format!("more than {MAX_PAGES} pages")));
        }
        let rule = product.page_rule();
        if !rule.allows(count) {
            return Err(invalid(format!(
                "{} must have {}–{} pages in steps of {} (has {count})",
                product.name, rule.min, rule.max, rule.step
            )));
        }

        let mut ids = HashSet::new();
        for (n, page) in self.pages.iter().enumerate() {
            let page_no = n + 1;
            check_colour(&page.background)
                .map_err(|e| invalid(format!("page {page_no} background: {e}")))?;
            if page.elements.len() > MAX_ELEMENTS_PER_PAGE {
                return Err(invalid(format!(
                    "page {page_no} has more than {MAX_ELEMENTS_PER_PAGE} elements"
                )));
            }
            for el in &page.elements {
                let id = el.id();
                check_element_id(id).map_err(|e| invalid(format!("page {page_no}: {e}")))?;
                if !ids.insert(id) {
                    return Err(invalid(format!("duplicate element id {id:?}")));
                }
                check_rect(el.rect()).map_err(|e| invalid(format!("page {page_no}, {id}: {e}")))?;
                validate_element(el).map_err(|e| invalid(format!("page {page_no}, {id}: {e}")))?;
            }
        }

        if self.photo_paths().len() > MAX_PHOTOS_PER_PROJECT {
            return Err(invalid(format!(
                "more than {MAX_PHOTOS_PER_PROJECT} photos"
            )));
        }
        Ok(())
    }
}

/// List deserialisers that stop as soon as a limit is passed, so a hostile
/// manifest with millions of tiny entries is rejected without first being
/// built in memory (the byte cap alone allows ~5 million `{}` entries).
mod bounded {
    use super::{Element, Page};
    use crate::limits::{MAX_ELEMENTS_PER_PAGE, MAX_PAGES};
    use serde::de::{Deserialize, Deserializer, Error, SeqAccess, Visitor};
    use std::fmt;
    use std::marker::PhantomData;

    struct Bounded<T> {
        max: usize,
        what: &'static str,
        _t: PhantomData<T>,
    }

    impl<'de, T: Deserialize<'de>> Visitor<'de> for Bounded<T> {
        type Value = Vec<T>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "a list of at most {} {}", self.max, self.what)
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<T>, A::Error> {
            let mut out = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(self.max));
            while let Some(item) = seq.next_element()? {
                if out.len() >= self.max {
                    return Err(A::Error::custom(format!(
                        "more than {} {}",
                        self.max, self.what
                    )));
                }
                out.push(item);
            }
            Ok(out)
        }
    }

    pub fn pages<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Page>, D::Error> {
        d.deserialize_seq(Bounded {
            max: usize::try_from(MAX_PAGES).unwrap_or(usize::MAX),
            what: "pages",
            _t: PhantomData,
        })
    }

    pub fn elements<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Element>, D::Error> {
        d.deserialize_seq(Bounded {
            max: MAX_ELEMENTS_PER_PAGE,
            what: "elements on a page",
            _t: PhantomData,
        })
    }
}

fn validate_cover(product: &Product, cover: Option<&str>) -> Result<(), ProjectError> {
    match (product.family, cover) {
        (Family::Book, Some(c)) if product.cover(c).is_some() => Ok(()),
        (Family::Book, Some(c)) => Err(invalid(format!(
            "cover {c:?} is not available for {}",
            product.name
        ))),
        (Family::Book, None) => Err(invalid("photo books need a cover type")),
        (_, Some(_)) => Err(invalid("only photo books have a cover type")),
        (_, None) => Ok(()),
    }
}

fn check_element_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > MAX_ID_LEN {
        return Err(format!("element id must be 1..={MAX_ID_LEN} bytes"));
    }
    if !id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("element id may only use letters, digits, '-' and '_'".into());
    }
    Ok(())
}

fn check_rect(r: &Rect) -> Result<(), String> {
    let lim = MAX_PAGE_EDGE_MM;
    let all_finite = [r.x, r.y, r.w, r.h, r.rotation]
        .iter()
        .all(|v| v.is_finite());
    if !all_finite {
        return Err("position contains a non-number".into());
    }
    if !(r.w > 0.0 && r.h > 0.0 && r.w <= lim && r.h <= lim) {
        return Err(format!("size must be between 0 and {lim} mm"));
    }
    if r.x.abs() > lim || r.y.abs() > lim {
        return Err(format!("position must be within ±{lim} mm"));
    }
    if r.rotation.abs() > 360.0 {
        return Err("rotation must be within ±360°".into());
    }
    Ok(())
}

fn validate_element(el: &Element) -> Result<(), String> {
    match el {
        Element::Photo { crop, .. } => {
            if !(crop.zoom.is_finite() && (1.0..=20.0).contains(&crop.zoom)) {
                return Err("zoom must be between 1 and 20".into());
            }
            for v in [crop.pan_x, crop.pan_y] {
                if !(v.is_finite() && (-1.0..=1.0).contains(&v)) {
                    return Err("pan must be between -1 and 1".into());
                }
            }
            Ok(())
        }
        Element::Text {
            text,
            font,
            size_pt,
            colour,
            ..
        } => {
            check_text_field("text", text, MAX_TEXT_CHARS, true).map_err(|e| e.to_string())?;
            if font.is_empty()
                || font.len() > MAX_ID_LEN
                || !font
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            {
                return Err("font must be a font id (a-z, 0-9, '-')".into());
            }
            if !(size_pt.is_finite() && (4.0..=400.0).contains(size_pt)) {
                return Err("text size must be between 4 and 400 pt".into());
            }
            check_colour(colour)
        }
    }
}

fn check_text_field(
    label: &str,
    value: &str,
    max_chars: usize,
    allow_newlines: bool,
) -> Result<(), ProjectError> {
    if value.chars().count() > max_chars {
        return Err(invalid(format!(
            "{label} is longer than {max_chars} characters"
        )));
    }
    let bad = value
        .chars()
        .any(|c| c.is_control() && !(allow_newlines && (c == '\n' || c == '\t')));
    if bad {
        return Err(invalid(format!("{label} contains control characters")));
    }
    Ok(())
}

fn check_colour(c: &str) -> Result<(), String> {
    let ok = c.len() == 7 && c.starts_with('#') && c.bytes().skip(1).all(|b| b.is_ascii_hexdigit());
    if ok {
        Ok(())
    } else {
        Err(format!("colour {c:?} must look like #rrggbb"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cat() -> Catalogue {
        Catalogue::builtin().expect("valid")
    }

    fn sample() -> Project {
        let c = cat();
        let mut p = Project::new_blank(&c, "print-6x4", "Beach day").expect("blank");
        p.pages[0].elements.push(Element::Photo {
            id: "photo-1".into(),
            rect: Rect {
                x: -2.0,
                y: -2.0,
                w: 156.4,
                h: 105.6,
                rotation: 0.0,
            },
            src: Some(SafePath::new("photos/beach.jpg").expect("safe")),
            crop: Crop::default(),
        });
        p.pages[0].elements.push(Element::Text {
            id: "caption".into(),
            rect: Rect {
                x: 10.0,
                y: 80.0,
                w: 130.0,
                h: 12.0,
                rotation: 0.0,
            },
            text: "Cottesloe, summer".into(),
            font: "sans".into(),
            size_pt: 14.0,
            colour: "#ffffff".into(),
            align: Align::Center,
        });
        p
    }

    #[test]
    fn blank_projects_for_every_product_are_valid() {
        let c = cat();
        for product in c.products() {
            let p = Project::new_blank(&c, &product.id, "Test").expect(&product.id);
            assert_eq!(p.pages.len() as u32, product.default_pages());
            assert!(p.validate(&c).is_ok(), "{}", product.id);
        }
    }

    #[test]
    fn round_trips_through_json() {
        let c = cat();
        let p = sample();
        let json = p.to_json().expect("serialise");
        let back = Project::from_json(&json, &c).expect("parse");
        assert_eq!(p, back);
        assert_eq!(back.photo_paths().len(), 1);
    }

    #[test]
    fn rejects_non_projects() {
        let c = cat();
        assert!(matches!(
            Project::from_json("{\"hello\": 1}", &c),
            Err(ProjectError::WrongFormat)
        ));
        assert!(matches!(
            Project::from_json("[]", &c),
            Err(ProjectError::Parse(_))
        ));
        assert!(Project::from_json("", &c).is_err());
    }

    #[test]
    fn rejects_newer_schema() {
        let c = cat();
        let json = sample()
            .to_json()
            .expect("ser")
            .replace("\"schema_version\": 1", "\"schema_version\": 7");
        assert!(matches!(
            Project::from_json(&json, &c),
            Err(ProjectError::UnsupportedVersion(7))
        ));
    }

    #[test]
    fn rejects_unknown_fields_anywhere() {
        let c = cat();
        let base = sample().to_json().expect("ser");
        for (from, to) in [
            ("\"title\":", "\"script\": \"alert(1)\",\n  \"title\":"),
            ("\"zoom\":", "\"url\": \"https://evil.example\", \"zoom\":"),
            ("\"type\": \"text\"", "\"type\": \"html\""),
        ] {
            let bad = base.replacen(from, to, 1);
            assert_ne!(bad, base, "pattern {from:?} missing");
            assert!(
                matches!(Project::from_json(&bad, &c), Err(ProjectError::Parse(_))),
                "accepted {to}"
            );
        }
    }

    #[test]
    fn rejects_unsafe_photo_paths() {
        let c = cat();
        let base = sample().to_json().expect("ser");
        for evil in [
            "../../../etc/passwd",
            "/etc/passwd",
            "C:/Windows/System32/config/SAM",
            "\\\\\\\\attacker\\\\share\\\\x.jpg",
            "photos/../../x.jpg",
        ] {
            let bad = base.replace("photos/beach.jpg", evil);
            assert!(
                matches!(Project::from_json(&bad, &c), Err(ProjectError::Parse(_))),
                "accepted path {evil}"
            );
        }
    }

    #[test]
    fn rejects_bad_numbers() {
        let c = cat();
        let mut p = sample();
        if let Element::Photo { rect, .. } = &mut p.pages[0].elements[0] {
            rect.w = -1.0;
        }
        assert!(p.validate(&c).is_err());

        let mut p = sample();
        if let Element::Photo { rect, .. } = &mut p.pages[0].elements[0] {
            rect.x = 1e12;
        }
        assert!(p.validate(&c).is_err());

        let mut p = sample();
        if let Element::Photo { crop, .. } = &mut p.pages[0].elements[0] {
            crop.zoom = 0.5;
        }
        assert!(p.validate(&c).is_err());

        let mut p = sample();
        if let Element::Text { size_pt, .. } = &mut p.pages[0].elements[1] {
            *size_pt = 10_000.0;
        }
        assert!(p.validate(&c).is_err());
    }

    #[test]
    fn rejects_bad_text_and_colours() {
        let c = cat();
        let mut p = sample();
        if let Element::Text { colour, .. } = &mut p.pages[0].elements[1] {
            *colour = "red".into();
        }
        assert!(p.validate(&c).is_err());

        let mut p = sample();
        if let Element::Text { text, .. } = &mut p.pages[0].elements[1] {
            *text = "x".repeat(MAX_TEXT_CHARS + 1);
        }
        assert!(p.validate(&c).is_err());

        let mut p = sample();
        if let Element::Text { text, .. } = &mut p.pages[0].elements[1] {
            *text = "bell\u{7}".into();
        }
        assert!(p.validate(&c).is_err());

        let mut p = sample();
        if let Element::Text { font, .. } = &mut p.pages[0].elements[1] {
            *font = "../../fonts/x".into();
        }
        assert!(p.validate(&c).is_err());

        let mut p = sample();
        p.title = "x".repeat(MAX_TITLE_CHARS + 1);
        assert!(p.validate(&c).is_err());
    }

    #[test]
    fn rejects_duplicate_and_bad_ids() {
        let c = cat();
        let mut p = sample();
        let dup = p.pages[0].elements[0].clone();
        p.pages[0].elements.push(dup);
        assert!(p.validate(&c).is_err());

        let mut p = sample();
        if let Element::Text { id, .. } = &mut p.pages[0].elements[1] {
            *id = "has space".into();
        }
        assert!(p.validate(&c).is_err());
    }

    #[test]
    fn enforces_product_rules() {
        let c = cat();
        let mut p = sample();
        p.product = "no-such-product".into();
        assert!(p.validate(&c).is_err());

        let mut p = sample();
        p.paper = "canvas-380".into();
        assert!(p.validate(&c).is_err());

        let mut p = sample();
        p.pages.push(Page::default());
        assert!(p.validate(&c).is_err(), "6x4 print allows 1 page");

        let mut p = sample();
        p.cover = Some("hard".into());
        assert!(p.validate(&c).is_err(), "prints have no cover");

        let mut book = Project::new_blank(&c, "book-20x20", "Book").expect("blank");
        assert_eq!(book.cover.as_deref(), Some("soft"));
        book.cover = None;
        assert!(book.validate(&c).is_err());
        book.cover = Some("spiral".into());
        assert!(book.validate(&c).is_err());
        book.cover = Some("hard".into());
        book.pages.push(Page::default());
        assert!(book.validate(&c).is_err(), "21 pages breaks step of 2");
        book.pages.push(Page::default());
        assert!(book.validate(&c).is_ok());
    }

    #[test]
    fn enforces_element_limit() {
        let c = cat();
        let mut p = Project::new_blank(&c, "poster-a3", "Lots").expect("blank");
        for i in 0..=MAX_ELEMENTS_PER_PAGE {
            p.pages[0].elements.push(Element::Photo {
                id: format!("e{i}"),
                rect: Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 10.0,
                    h: 10.0,
                    rotation: 0.0,
                },
                src: None,
                crop: Crop::default(),
            });
        }
        assert!(p.validate(&c).is_err());
    }

    #[test]
    fn size_cap_is_enforced_before_parsing() {
        let c = cat();
        let huge = " ".repeat(MAX_MANIFEST_BYTES as usize + 1);
        assert!(matches!(
            Project::from_json(&huge, &c),
            Err(ProjectError::TooLarge)
        ));
        let reader = std::io::repeat(b' ').take(MAX_MANIFEST_BYTES + 10);
        assert!(matches!(
            Project::from_reader(reader, &c),
            Err(ProjectError::TooLarge)
        ));
    }

    #[test]
    fn huge_lists_are_cut_off_while_parsing() {
        let c = cat();
        let base = sample().to_json().expect("ser");
        // ~5 million empty pages fits under the byte cap; must be refused
        // during parsing, not after allocating them all.
        let many_pages = format!("[{}{{}}]", "{},".repeat(5_000_000));
        let bad = base.replacen(
            &base[base.find("\"pages\": [").expect("pages")..base.rfind(']').expect("end") + 1],
            &format!("\"pages\": {many_pages}"),
            1,
        );
        assert!(bad.len() as u64 <= MAX_MANIFEST_BYTES);
        match Project::from_json(&bad, &c) {
            Err(ProjectError::Parse(msg)) => assert!(msg.contains("more than 400 pages"), "{msg}"),
            other => panic!("expected a page-limit error, got {other:?}"),
        }

        let many_elements = format!(
            "[{}{{\"type\":\"photo\",\"id\":\"last\",\"rect\":{{\"x\":0,\"y\":0,\"w\":1,\"h\":1}}}}]",
            "{\"type\":\"photo\",\"id\":\"e\",\"rect\":{\"x\":0,\"y\":0,\"w\":1,\"h\":1}},"
                .repeat(MAX_ELEMENTS_PER_PAGE + 5)
        );
        let blank = Project::new_blank(&c, "print-6x4", "x")
            .expect("blank")
            .to_json()
            .expect("ser");
        let bad = blank.replacen(
            "\"elements\": []",
            &format!("\"elements\": {many_elements}"),
            1,
        );
        assert_ne!(bad, blank);
        match Project::from_json(&bad, &c) {
            Err(ProjectError::Parse(msg)) => {
                assert!(msg.contains("more than 200 elements"), "{msg}")
            }
            other => panic!("expected an element-limit error, got {other:?}"),
        }
    }

    #[test]
    fn deep_nesting_is_rejected_not_crashed() {
        let c = cat();
        let deep = format!("{}{}", "[".repeat(100_000), "]".repeat(100_000));
        assert!(Project::from_json(&deep, &c).is_err());
    }
}
