//! The product catalogue: every printable product and paper PrintNest knows.
//!
//! Product data lives in `catalogue/products.toml` at the repo root and is
//! compiled into the binary, so the app never reads catalogue data from an
//! arbitrary path at run time. [`Catalogue::parse`] is also public so tests,
//! the CLI and future region packs can validate other catalogue files.

use crate::limits::{MAX_CATALOGUE_BYTES, MAX_ID_LEN, MAX_PAGE_EDGE_MM, MAX_PAGES};
use crate::units::{Mm, Size};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;

/// The catalogue shipped with the app.
pub const BUILTIN_TOML: &str = include_str!("../../../catalogue/products.toml");

/// Highest catalogue schema version this build understands.
pub const CATALOGUE_SCHEMA_VERSION: u32 = 1;

/// Why a catalogue failed to load.
#[derive(Debug, thiserror::Error)]
pub enum CatalogueError {
    #[error("catalogue file is larger than {MAX_CATALOGUE_BYTES} bytes")]
    TooLarge,
    #[error("catalogue is not valid TOML: {0}")]
    Parse(String),
    #[error(
        "catalogue schema version {0} is newer than this app supports ({CATALOGUE_SCHEMA_VERSION})"
    )]
    UnsupportedVersion(u32),
    #[error("{0}: {1}")]
    Invalid(String, String),
}

fn invalid(what: &str, why: impl Into<String>) -> CatalogueError {
    CatalogueError::Invalid(what.to_owned(), why.into())
}

/// Length units allowed in catalogue sizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Unit {
    Mm,
    Cm,
    In,
}

/// A size as written in the catalogue file.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawSize {
    pub width: f64,
    pub height: f64,
    pub unit: Unit,
}

impl RawSize {
    /// Converts to millimetres.
    #[must_use]
    pub fn to_mm(self) -> Size {
        let f = |v: f64| match self.unit {
            Unit::Mm => Mm(v),
            Unit::Cm => Mm::from_cm(v),
            Unit::In => Mm::from_inches(v),
        };
        Size::new(f(self.width), f(self.height))
    }
}

/// Product family; decides which editor tools and rules apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Family {
    Print,
    Poster,
    Framed,
    Canvas,
    Book,
    Calendar,
    Card,
    Gift,
}

impl Family {
    /// All families in display order.
    pub const ALL: [Family; 8] = [
        Family::Print,
        Family::Poster,
        Family::Framed,
        Family::Canvas,
        Family::Book,
        Family::Calendar,
        Family::Card,
        Family::Gift,
    ];

    /// Human-readable plural label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Family::Print => "Photo prints",
            Family::Poster => "Posters",
            Family::Framed => "Framed prints",
            Family::Canvas => "Canvas",
            Family::Book => "Photo books",
            Family::Calendar => "Calendars",
            Family::Card => "Cards",
            Family::Gift => "Gifts",
        }
    }
}

impl fmt::Display for Family {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Where a finished design can be printed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportProfile {
    KioskJpeg,
    PrintPdf,
    Home,
    Template,
}

impl ExportProfile {
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            ExportProfile::KioskJpeg => "Kiosk / photo lab (JPEG)",
            ExportProfile::PrintPdf => "Print shop (PDF)",
            ExportProfile::Home => "Home printer",
            ExportProfile::Template => "DIY / merch template",
        }
    }
}

/// Outline of the printed piece.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Shape {
    #[default]
    Rect,
    Rounded,
    Circle,
}

/// Fold direction for cards.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Fold {
    Vertical,
    Horizontal,
}

/// Allowed page counts: `min..=max` in steps of `step` from `min`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageRule {
    pub min: u32,
    pub max: u32,
    pub step: u32,
}

impl PageRule {
    /// True when `pages` is an allowed page count.
    #[must_use]
    pub fn allows(&self, pages: u32) -> bool {
        pages >= self.min
            && pages <= self.max
            && self.step > 0
            && (pages - self.min).is_multiple_of(self.step)
    }
}

/// A cover option for photo books.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cover {
    pub id: String,
    pub name: String,
    /// Board and binding allowance added to the paper stack, in mm.
    pub spine_extra_mm: f64,
    /// Cover material folded around the board edge, in mm.
    pub turn_in_mm: f64,
}

/// A paper or material.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Paper {
    pub id: String,
    pub name: String,
    pub gsm: u32,
    /// Sheet thickness, used for spine maths.
    pub thickness_mm: f64,
}

/// A printable product.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Product {
    pub id: String,
    pub family: Family,
    pub name: String,
    pub size: RawSize,
    pub bleed_mm: f64,
    pub safe_mm: f64,
    pub papers: Vec<String>,
    pub exports: Vec<ExportProfile>,
    #[serde(default)]
    pub pages: Option<PageRule>,
    #[serde(default)]
    pub covers: Vec<Cover>,
    #[serde(default)]
    pub gutter_mm: Option<f64>,
    #[serde(default)]
    pub wrap_mm: Option<f64>,
    #[serde(default)]
    pub binding_mm: Option<f64>,
    #[serde(default)]
    pub mat_opening: Option<RawSize>,
    #[serde(default)]
    pub fold: Option<Fold>,
    #[serde(default)]
    pub shape: Shape,
    #[serde(default)]
    pub verified: bool,
}

impl Product {
    /// Finished (trim) size of one page.
    #[must_use]
    pub fn trim(&self) -> Size {
        self.size.to_mm()
    }

    /// Bleed margin.
    #[must_use]
    pub fn bleed(&self) -> Mm {
        Mm(self.bleed_mm)
    }

    /// Safe margin inside the trim.
    #[must_use]
    pub fn safe(&self) -> Mm {
        Mm(self.safe_mm)
    }

    /// Size of the artwork including bleed (and canvas wrap, if any).
    #[must_use]
    pub fn artwork(&self) -> Size {
        let extra = Mm(self.bleed_mm + self.wrap_mm.unwrap_or(0.0));
        self.trim().grown_by(extra)
    }

    /// Page-count rule; single-sided products allow exactly 1 page.
    #[must_use]
    pub fn page_rule(&self) -> PageRule {
        self.pages.unwrap_or(PageRule {
            min: 1,
            max: 1,
            step: 1,
        })
    }

    /// Default page count for a new project.
    #[must_use]
    pub fn default_pages(&self) -> u32 {
        self.page_rule().min
    }

    /// Looks up a cover option.
    #[must_use]
    pub fn cover(&self, id: &str) -> Option<&Cover> {
        self.covers.iter().find(|c| c.id == id)
    }

    /// True when the product allows this paper.
    #[must_use]
    pub fn allows_paper(&self, paper_id: &str) -> bool {
        self.papers.iter().any(|p| p == paper_id)
    }

    /// Spine width for a book with `pages` pages on `paper` with `cover`.
    ///
    /// Each sheet (leaf) carries two pages, so the paper stack is
    /// `ceil(pages / 2) × thickness`, plus the cover's board allowance.
    /// Returns `None` for products without covers or unknown covers.
    #[must_use]
    pub fn spine_width(&self, pages: u32, paper: &Paper, cover_id: &str) -> Option<Mm> {
        let cover = self.cover(cover_id)?;
        let leaves = f64::from(pages.div_ceil(2));
        Some(Mm(leaves * paper.thickness_mm + cover.spine_extra_mm))
    }

    /// Flat cover artwork size (back + spine + front, with turn-in all round).
    #[must_use]
    pub fn cover_spread(&self, pages: u32, paper: &Paper, cover_id: &str) -> Option<Size> {
        let cover = self.cover(cover_id)?;
        let spine = self.spine_width(pages, paper, cover_id)?;
        let trim = self.trim();
        let flat = Size::new(trim.width * 2.0 + spine, trim.height);
        Some(flat.grown_by(Mm(cover.turn_in_mm)))
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogueFile {
    schema_version: u32,
    #[serde(default)]
    paper: Vec<Paper>,
    #[serde(default)]
    product: Vec<Product>,
}

/// A validated product catalogue.
#[derive(Debug, Clone)]
pub struct Catalogue {
    papers: Vec<Paper>,
    products: Vec<Product>,
}

impl Catalogue {
    /// Loads the catalogue compiled into the app.
    ///
    /// # Errors
    /// Only if the built-in data is broken, which the test suite prevents.
    pub fn builtin() -> Result<Self, CatalogueError> {
        Self::parse(BUILTIN_TOML)
    }

    /// Parses and validates catalogue TOML.
    ///
    /// # Errors
    /// Returns the first problem found.
    pub fn parse(text: &str) -> Result<Self, CatalogueError> {
        if text.len() > MAX_CATALOGUE_BYTES {
            return Err(CatalogueError::TooLarge);
        }
        let file: CatalogueFile =
            toml::from_str(text).map_err(|e| CatalogueError::Parse(e.to_string()))?;
        if file.schema_version == 0 || file.schema_version > CATALOGUE_SCHEMA_VERSION {
            return Err(CatalogueError::UnsupportedVersion(file.schema_version));
        }
        let cat = Catalogue {
            papers: file.paper,
            products: file.product,
        };
        cat.validate()?;
        Ok(cat)
    }

    /// All products, in file order.
    #[must_use]
    pub fn products(&self) -> &[Product] {
        &self.products
    }

    /// All papers, in file order.
    #[must_use]
    pub fn papers(&self) -> &[Paper] {
        &self.papers
    }

    /// Products in one family.
    pub fn in_family(&self, family: Family) -> impl Iterator<Item = &Product> {
        self.products.iter().filter(move |p| p.family == family)
    }

    /// Looks up a product by id.
    #[must_use]
    pub fn product(&self, id: &str) -> Option<&Product> {
        self.products.iter().find(|p| p.id == id)
    }

    /// Looks up a paper by id.
    #[must_use]
    pub fn paper(&self, id: &str) -> Option<&Paper> {
        self.papers.iter().find(|p| p.id == id)
    }

    fn validate(&self) -> Result<(), CatalogueError> {
        if self.products.is_empty() {
            return Err(invalid("catalogue", "has no products"));
        }

        let mut paper_ids = HashSet::new();
        for paper in &self.papers {
            check_id(&paper.id).map_err(|e| invalid(&format!("paper {:?}", paper.id), e))?;
            if !paper_ids.insert(paper.id.as_str()) {
                return Err(invalid(&format!("paper {}", paper.id), "duplicate id"));
            }
            check_name(&paper.name).map_err(|e| invalid(&format!("paper {}", paper.id), e))?;
            if !(paper.thickness_mm.is_finite()
                && paper.thickness_mm > 0.0
                && paper.thickness_mm <= 5.0)
            {
                return Err(invalid(
                    &format!("paper {}", paper.id),
                    "thickness_mm must be between 0 and 5",
                ));
            }
            if paper.gsm == 0 || paper.gsm > 2_000 {
                return Err(invalid(
                    &format!("paper {}", paper.id),
                    "gsm must be 1..=2000",
                ));
            }
        }

        let mut product_ids = HashSet::new();
        for p in &self.products {
            let what = format!("product {:?}", p.id);
            check_id(&p.id).map_err(|e| invalid(&what, e))?;
            if !product_ids.insert(p.id.as_str()) {
                return Err(invalid(&what, "duplicate id"));
            }
            check_name(&p.name).map_err(|e| invalid(&what, e))?;
            validate_product(p, &paper_ids).map_err(|e| invalid(&what, e))?;
        }
        Ok(())
    }
}

fn check_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > MAX_ID_LEN {
        return Err(format!("id must be 1..={MAX_ID_LEN} bytes"));
    }
    if !id
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err("id may only use a-z, 0-9 and '-'".into());
    }
    Ok(())
}

fn check_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() || name.chars().count() > 120 {
        return Err("name must be 1..=120 characters".into());
    }
    if name.chars().any(char::is_control) {
        return Err("name contains control characters".into());
    }
    Ok(())
}

fn check_margin(label: &str, v: f64, max: f64) -> Result<(), String> {
    if v.is_finite() && (0.0..=max).contains(&v) {
        Ok(())
    } else {
        Err(format!("{label} must be between 0 and {max} mm"))
    }
}

fn validate_product(p: &Product, papers: &HashSet<&str>) -> Result<(), String> {
    let trim = p.trim();
    for (label, v) in [("width", trim.width.0), ("height", trim.height.0)] {
        if !(v.is_finite() && (10.0..=MAX_PAGE_EDGE_MM).contains(&v)) {
            return Err(format!(
                "{label} must be between 10 and {MAX_PAGE_EDGE_MM} mm"
            ));
        }
    }
    check_margin("bleed_mm", p.bleed_mm, 20.0)?;
    let short_edge = trim.width.0.min(trim.height.0);
    check_margin("safe_mm", p.safe_mm, short_edge / 3.0)?;

    if p.papers.is_empty() {
        return Err("must list at least one paper".into());
    }
    for paper in &p.papers {
        if !papers.contains(paper.as_str()) {
            return Err(format!("unknown paper {paper:?}"));
        }
    }
    if p.exports.is_empty() {
        return Err("must list at least one export profile".into());
    }

    if let Some(rule) = p.pages {
        if rule.step == 0 || rule.min == 0 || rule.min > rule.max || rule.max > MAX_PAGES {
            return Err(format!(
                "pages must satisfy 1 <= min <= max <= {MAX_PAGES} and step >= 1"
            ));
        }
        if !(rule.max - rule.min).is_multiple_of(rule.step) {
            return Err("pages: max must be reachable from min in whole steps".into());
        }
    }

    if let Some(g) = p.gutter_mm {
        check_margin("gutter_mm", g, 40.0)?;
    }
    if let Some(b) = p.binding_mm {
        check_margin("binding_mm", b, 40.0)?;
    }

    match p.family {
        Family::Book => {
            if p.pages.is_none() {
                return Err("books need a pages rule".into());
            }
            if p.covers.is_empty() {
                return Err("books need at least one cover option".into());
            }
            if p.gutter_mm.is_none() {
                return Err("books need gutter_mm".into());
            }
        }
        Family::Calendar | Family::Card => {
            if p.pages.is_none() {
                return Err("calendars and cards need a pages rule".into());
            }
        }
        _ => {
            if !p.covers.is_empty() {
                return Err("only books have cover options".into());
            }
        }
    }
    if p.family == Family::Card && p.fold.is_none() {
        return Err("cards need a fold direction".into());
    }
    if p.family != Family::Card && p.fold.is_some() {
        return Err("only cards have a fold".into());
    }

    match (p.family, p.wrap_mm) {
        (Family::Canvas, Some(w)) => {
            if !(w.is_finite() && (10.0..=60.0).contains(&w)) {
                return Err("wrap_mm must be between 10 and 60".into());
            }
        }
        (Family::Canvas, None) => return Err("canvas needs wrap_mm".into()),
        (_, Some(_)) => return Err("only canvas has wrap_mm".into()),
        (_, None) => {}
    }

    if let Some(mat) = p.mat_opening {
        if p.family != Family::Framed {
            return Err("only framed products have a mat_opening".into());
        }
        let m = mat.to_mm();
        if !(m.width.0 > 0.0
            && m.height.0 > 0.0
            && m.width.0 < trim.width.0
            && m.height.0 < trim.height.0)
        {
            return Err("mat_opening must be smaller than the print".into());
        }
    }

    let mut cover_ids = HashSet::new();
    for c in &p.covers {
        check_id(&c.id)?;
        check_name(&c.name)?;
        if !cover_ids.insert(c.id.as_str()) {
            return Err(format!("duplicate cover id {:?}", c.id));
        }
        check_margin("spine_extra_mm", c.spine_extra_mm, 20.0)?;
        check_margin("turn_in_mm", c.turn_in_mm, 30.0)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn builtin() -> Catalogue {
        Catalogue::builtin().expect("built-in catalogue must be valid")
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn builtin_catalogue_loads() {
        let cat = builtin();
        assert!(cat.products().len() >= 40);
        for family in Family::ALL {
            assert!(
                cat.in_family(family).count() > 0,
                "no products in {family:?}"
            );
        }
    }

    #[test]
    fn every_product_has_sane_geometry() {
        for p in builtin().products() {
            let trim = p.trim();
            let art = p.artwork();
            assert!(art.width.0 >= trim.width.0, "{}", p.id);
            assert!(art.height.0 >= trim.height.0, "{}", p.id);
            assert!(
                p.safe_mm * 2.0 < trim.width.0.min(trim.height.0),
                "{}",
                p.id
            );
            assert!(p.page_rule().allows(p.default_pages()), "{}", p.id);
        }
    }

    #[test]
    fn six_by_four_is_152_by_102_mm() {
        let p = builtin().product("print-6x4").cloned().expect("exists");
        let t = p.trim();
        assert!(close(t.width.0, 152.4));
        assert!(close(t.height.0, 101.6));
    }

    #[test]
    fn canvas_artwork_includes_wrap_and_bleed() {
        // 30×30 cm + (38 mm wrap + 5 mm bleed) on each side = 386 mm square.
        let p = builtin().product("canvas-30x30").cloned().expect("exists");
        let a = p.artwork();
        assert!(close(a.width.0, 386.0));
        assert!(close(a.height.0, 386.0));
    }

    #[test]
    fn spine_width_hand_calculated() {
        let cat = builtin();
        let book = cat.product("book-20x20").expect("exists");
        let silk = cat.paper("silk-170").expect("exists");
        // 40 pages = 20 leaves × 0.16 mm = 3.2 mm, + 4 mm hard cover = 7.2 mm.
        let s = book.spine_width(40, silk, "hard").expect("hard cover");
        assert!(close(s.0, 7.2));
        // 21 pages rounds up to 11 leaves: 1.76 + 1.0 soft = 2.76 mm.
        let s = book.spine_width(21, silk, "soft").expect("soft cover");
        assert!(close(s.0, 2.76));
        assert!(book.spine_width(40, silk, "spiral").is_none());
    }

    #[test]
    fn cover_spread_hand_calculated() {
        let cat = builtin();
        let book = cat.product("book-20x20").expect("exists");
        let silk = cat.paper("silk-170").expect("exists");
        // Flat: 200 + 7.2 + 200 = 407.2 wide, 200 tall; +15 mm turn-in all round.
        let c = book.cover_spread(40, silk, "hard").expect("cover");
        assert!(close(c.width.0, 437.2));
        assert!(close(c.height.0, 230.0));
    }

    #[test]
    fn page_rules() {
        let cat = builtin();
        let book = cat.product("book-20x20").expect("exists").page_rule();
        assert!(book.allows(20));
        assert!(book.allows(22));
        assert!(!book.allows(21));
        assert!(!book.allows(18));
        assert!(!book.allows(102));
        let cal = cat
            .product("calendar-a4-portrait")
            .expect("exists")
            .page_rule();
        assert!(cal.allows(13));
        assert!(!cal.allows(12));
        let print = cat.product("print-6x4").expect("exists").page_rule();
        assert!(print.allows(1));
        assert!(!print.allows(2));
    }

    #[test]
    fn rejects_unknown_fields() {
        let t = BUILTIN_TOML.replacen("bleed_mm = 2", "bleed_mm = 2\nscript = \"x\"", 1);
        assert!(matches!(
            Catalogue::parse(&t),
            Err(CatalogueError::Parse(_))
        ));
    }

    #[test]
    fn rejects_future_schema() {
        let t = BUILTIN_TOML.replacen("schema_version = 1", "schema_version = 99", 1);
        assert!(matches!(
            Catalogue::parse(&t),
            Err(CatalogueError::UnsupportedVersion(99))
        ));
    }

    #[test]
    fn rejects_duplicate_ids() {
        let t = BUILTIN_TOML.replacen("id = \"print-7x5\"", "id = \"print-6x4\"", 1);
        assert!(matches!(
            Catalogue::parse(&t),
            Err(CatalogueError::Invalid(..))
        ));
    }

    #[test]
    fn rejects_bad_ids_and_values() {
        let cases = [
            ("id = \"print-6x4\"", "id = \"Print 6x4\""),
            ("id = \"print-6x4\"", "id = \"../evil\""),
            ("bleed_mm = 2", "bleed_mm = -1"),
            ("bleed_mm = 2", "bleed_mm = nan"),
            ("thickness_mm = 0.16", "thickness_mm = 0"),
            (
                "size = { width = 6, height = 4, unit = \"in\" }",
                "size = { width = 6, height = 0, unit = \"in\" }",
            ),
            (
                "size = { width = 6, height = 4, unit = \"in\" }",
                "size = { width = 1e9, height = 4, unit = \"in\" }",
            ),
            (
                "papers = [\"gloss-250\", \"lustre-260\", \"matte-200\"]",
                "papers = [\"nope\"]",
            ),
            (
                "pages = { min = 20, max = 100, step = 2 }",
                "pages = { min = 20, max = 100, step = 0 }",
            ),
            (
                "pages = { min = 20, max = 100, step = 2 }",
                "pages = { min = 20, max = 101, step = 2 }",
            ),
            ("wrap_mm = 18", "wrap_mm = 500"),
        ];
        for (from, to) in cases {
            let t = BUILTIN_TOML.replacen(from, to, 1);
            assert_ne!(t, BUILTIN_TOML, "test pattern {from:?} not found");
            assert!(Catalogue::parse(&t).is_err(), "accepted {to:?}");
        }
    }

    #[test]
    fn rejects_oversized_input() {
        let big = "#".repeat(MAX_CATALOGUE_BYTES + 1);
        assert!(matches!(
            Catalogue::parse(&big),
            Err(CatalogueError::TooLarge)
        ));
    }

    #[test]
    fn rejects_garbage() {
        for junk in ["", "not toml {{{", "schema_version = \"one\"", "\u{0}\u{1}"] {
            assert!(Catalogue::parse(junk).is_err(), "{junk:?}");
        }
    }
}
