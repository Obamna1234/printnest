//! Physical units and print-resolution maths.
//!
//! PrintNest stores every length in millimetres. Inches only exist at the
//! edges (catalogue data and UI display).

use serde::{Deserialize, Serialize};
use std::fmt;

/// Millimetres per inch, exactly.
pub const MM_PER_INCH: f64 = 25.4;

/// A length in millimetres.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize, Default)]
#[serde(transparent)]
pub struct Mm(pub f64);

impl Mm {
    /// Zero length.
    pub const ZERO: Mm = Mm(0.0);

    /// Builds a length from inches.
    #[must_use]
    pub fn from_inches(inches: f64) -> Self {
        Mm(inches * MM_PER_INCH)
    }

    /// Builds a length from centimetres.
    #[must_use]
    pub fn from_cm(cm: f64) -> Self {
        Mm(cm * 10.0)
    }

    /// The length in inches.
    #[must_use]
    pub fn inches(self) -> f64 {
        self.0 / MM_PER_INCH
    }

    /// True when the value is a real, non-negative number.
    #[must_use]
    pub fn is_valid_length(self) -> bool {
        self.0.is_finite() && self.0 >= 0.0
    }

    /// Pixels needed to print this length at `ppi` pixels per inch,
    /// rounded up so the print never falls short.
    #[must_use]
    pub fn pixels_at(self, ppi: f64) -> u64 {
        let px = (self.inches() * ppi).ceil();
        if px.is_finite() && px > 0.0 {
            // Safe: clamped to a positive finite value before the cast.
            px.min(u64::MAX as f64) as u64
        } else {
            0
        }
    }
}

impl std::ops::Add for Mm {
    type Output = Mm;
    fn add(self, rhs: Mm) -> Mm {
        Mm(self.0 + rhs.0)
    }
}

impl std::ops::Sub for Mm {
    type Output = Mm;
    fn sub(self, rhs: Mm) -> Mm {
        Mm(self.0 - rhs.0)
    }
}

impl std::ops::Mul<f64> for Mm {
    type Output = Mm;
    fn mul(self, rhs: f64) -> Mm {
        Mm(self.0 * rhs)
    }
}

impl fmt::Display for Mm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.1} mm", self.0)
    }
}

/// A width × height size.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Size {
    pub width: Mm,
    pub height: Mm,
}

impl Size {
    #[must_use]
    pub fn new(width: Mm, height: Mm) -> Self {
        Size { width, height }
    }

    /// Grows the size by `margin` on every side.
    #[must_use]
    pub fn grown_by(self, margin: Mm) -> Self {
        Size {
            width: self.width + margin * 2.0,
            height: self.height + margin * 2.0,
        }
    }

    /// Width divided by height, or `None` for a zero height.
    #[must_use]
    pub fn aspect(self) -> Option<f64> {
        if self.height.0 > 0.0 {
            Some(self.width.0 / self.height.0)
        } else {
            None
        }
    }
}

/// How well a photo will print at its placed size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PrintQuality {
    /// 250 ppi or more: sharp.
    Good,
    /// 150 to 249 ppi: acceptable for most prints.
    Fair,
    /// Under 150 ppi: likely to look soft or pixelated.
    Poor,
}

/// Green threshold for the DPI badge (SPEC section 5).
pub const GOOD_PPI: f64 = 250.0;
/// Amber threshold for the DPI badge (SPEC section 5).
pub const FAIR_PPI: f64 = 150.0;

/// Effective resolution of a photo placed in a frame.
///
/// `pixels` is the photo's pixel count along one edge (after cropping) and
/// `printed` is the printed length of that same edge. Returns `None` when
/// the printed length is zero or invalid.
#[must_use]
pub fn effective_ppi(pixels: u64, printed: Mm) -> Option<f64> {
    let inches = printed.inches();
    if inches.is_finite() && inches > 0.0 {
        Some(pixels as f64 / inches)
    } else {
        None
    }
}

/// Grades an effective resolution for the DPI badge.
#[must_use]
pub fn grade_ppi(ppi: f64) -> PrintQuality {
    if ppi >= GOOD_PPI {
        PrintQuality::Good
    } else if ppi >= FAIR_PPI {
        PrintQuality::Fair
    } else {
        PrintQuality::Poor
    }
}

/// Grades a whole photo against a frame, using the weaker of the two edges.
#[must_use]
pub fn grade_photo(px_w: u64, px_h: u64, frame: Size) -> Option<(f64, PrintQuality)> {
    let w = effective_ppi(px_w, frame.width)?;
    let h = effective_ppi(px_h, frame.height)?;
    let ppi = w.min(h);
    Some((ppi, grade_ppi(ppi)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn inch_conversion_is_exact() {
        assert!(close(Mm::from_inches(1.0).0, 25.4));
        assert!(close(Mm::from_inches(6.0).0, 152.4));
        assert!(close(Mm(101.6).inches(), 4.0));
        assert!(close(Mm::from_cm(30.0).0, 300.0));
    }

    #[test]
    fn pixels_for_6x4_at_300ppi() {
        // A 6×4 inch print at 300 ppi is the classic 1800 × 1200 pixels.
        assert_eq!(Mm::from_inches(6.0).pixels_at(300.0), 1800);
        assert_eq!(Mm::from_inches(4.0).pixels_at(300.0), 1200);
    }

    #[test]
    fn pixels_for_a4_at_300ppi() {
        // 210 × 297 mm at 300 ppi = 2480.3 × 3507.9, rounded up.
        assert_eq!(Mm(210.0).pixels_at(300.0), 2481);
        assert_eq!(Mm(297.0).pixels_at(300.0), 3508);
    }

    #[test]
    fn pixels_never_negative_or_nan() {
        assert_eq!(Mm(-5.0).pixels_at(300.0), 0);
        assert_eq!(Mm(f64::NAN).pixels_at(300.0), 0);
        assert_eq!(Mm(10.0).pixels_at(f64::NAN), 0);
    }

    #[test]
    fn ppi_grading_thresholds() {
        assert_eq!(grade_ppi(300.0), PrintQuality::Good);
        assert_eq!(grade_ppi(250.0), PrintQuality::Good);
        assert_eq!(grade_ppi(249.9), PrintQuality::Fair);
        assert_eq!(grade_ppi(150.0), PrintQuality::Fair);
        assert_eq!(grade_ppi(149.9), PrintQuality::Poor);
    }

    #[test]
    fn photo_grade_uses_weaker_edge() {
        // 3000 × 2000 px photo in an 8×10 inch frame:
        // 3000/8 = 375 ppi wide, 2000/10 = 200 ppi tall → Fair at 200.
        let frame = Size::new(Mm::from_inches(8.0), Mm::from_inches(10.0));
        let (ppi, q) = grade_photo(3000, 2000, frame).unwrap_or((0.0, PrintQuality::Poor));
        assert!(close(ppi, 200.0));
        assert_eq!(q, PrintQuality::Fair);
    }

    #[test]
    fn zero_frame_has_no_grade() {
        assert!(grade_photo(100, 100, Size::new(Mm::ZERO, Mm(10.0))).is_none());
    }

    #[test]
    fn size_with_bleed() {
        let s = Size::new(Mm(210.0), Mm(297.0)).grown_by(Mm(3.0));
        assert!(close(s.width.0, 216.0));
        assert!(close(s.height.0, 303.0));
    }
}
