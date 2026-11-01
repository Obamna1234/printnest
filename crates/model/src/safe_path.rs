//! Validation of file paths stored inside project files.
//!
//! A project refers to its photos by paths relative to the project folder.
//! A hostile project could try to point outside that folder to read or
//! overwrite other files (path traversal / zip-slip). [`SafePath`] can only
//! be constructed from a path that stays inside the project folder on every
//! operating system PrintNest supports.
//!
//! Rules (SPEC section 7, rule 2):
//! - forward slashes only, no backslashes
//! - not absolute, no drive letters, no UNC or device paths
//! - no `.` or `..` components, no empty components
//! - no control characters and none of `<>:"|?*`
//! - no invisible Unicode formatting characters (bidi overrides, zero-width
//!   characters, BOM, tag characters) that make a name display misleadingly
//! - no look-alikes of `/`, `\`, `.` or `:` that Windows "best-fit" code-page
//!   conversion can turn into the real character (the WorstFit attack)
//! - no Windows reserved names (CON, NUL, COM1, COM¹, …) in any component
//! - no component ending in a dot or space (Windows silently strips these)
//! - at most [`MAX_PATH_BYTES`] bytes and [`MAX_PATH_DEPTH`] components

use crate::limits::{MAX_PATH_BYTES, MAX_PATH_DEPTH};
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;

/// Why a path was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SafePathError {
    #[error("path is empty")]
    Empty,
    #[error("path is longer than {MAX_PATH_BYTES} bytes")]
    TooLong,
    #[error("path is nested deeper than {MAX_PATH_DEPTH} folders")]
    TooDeep,
    #[error("path must be relative to the project folder")]
    Absolute,
    #[error("path uses backslashes; use forward slashes")]
    Backslash,
    #[error("path contains an empty, '.' or '..' component")]
    BadComponent,
    #[error("path contains a forbidden character")]
    ForbiddenChar,
    #[error("path contains an invisible or look-alike Unicode character")]
    DeceptiveChar,
    #[error("path uses a reserved device name")]
    ReservedName,
    #[error("path component ends in a dot or space")]
    TrailingDotOrSpace,
}

/// A relative path that is guaranteed to stay inside the project folder.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct SafePath(String);

impl SafePath {
    /// Validates `raw` and wraps it.
    ///
    /// # Errors
    /// Returns the first rule the path breaks.
    pub fn new(raw: &str) -> Result<Self, SafePathError> {
        validate(raw)?;
        Ok(SafePath(raw.to_owned()))
    }

    /// The validated path, always using `/` separators.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The path's components.
    pub fn components(&self) -> impl Iterator<Item = &str> {
        self.0.split('/')
    }
}

impl fmt::Display for SafePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for SafePath {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        SafePath::new(&raw).map_err(serde::de::Error::custom)
    }
}

const RESERVED: &[&str] = &[
    "CON",
    "PRN",
    "AUX",
    "NUL",
    "CONIN$",
    "CONOUT$",
    "CLOCK$",
    "COM0",
    "COM1",
    "COM2",
    "COM3",
    "COM4",
    "COM5",
    "COM6",
    "COM7",
    "COM8",
    "COM9",
    "LPT0",
    "LPT1",
    "LPT2",
    "LPT3",
    "LPT4",
    "LPT5",
    "LPT6",
    "LPT7",
    "LPT8",
    "LPT9",
    // Windows also treats superscript digits as device numbers.
    "COM\u{b9}",
    "COM\u{b2}",
    "COM\u{b3}",
    "LPT\u{b9}",
    "LPT\u{b2}",
    "LPT\u{b3}",
];

/// Invisible formatting characters (Unicode category Cf and friends) and
/// look-alikes that Windows best-fit mapping turns into path syntax.
/// Also used to neutralise untrusted text before showing it in the UI.
#[must_use]
pub fn is_deceptive(c: char) -> bool {
    matches!(c,
        '\u{00AD}'                  // soft hyphen
        | '\u{034F}'                // combining grapheme joiner
        | '\u{061C}'                // Arabic letter mark
        | '\u{115F}' | '\u{1160}'    // Hangul fillers
        | '\u{17B4}' | '\u{17B5}'
        | '\u{180B}'..='\u{180F}'   // Mongolian selectors / vowel separator
        | '\u{200B}'..='\u{200F}'   // zero-width, LRM, RLM
        | '\u{202A}'..='\u{202E}'   // bidi embeddings and overrides
        | '\u{2060}'..='\u{206F}'   // word joiner, invisible operators, isolates
        | '\u{3164}'                // Hangul filler
        | '\u{FE00}'..='\u{FE0F}'   // variation selectors
        | '\u{FEFF}'                // byte order mark
        | '\u{FFA0}'                // halfwidth Hangul filler
        | '\u{FFF9}'..='\u{FFFB}'   // interlinear annotation
        | '\u{1D173}'..='\u{1D17A}' // musical formatting
        | '\u{E0000}'..='\u{E007F}' // tag characters
        | '\u{E0100}'..='\u{E01EF}' // variation selectors supplement
        // Look-alikes of / \\ . : that best-fit code pages map to ASCII.
        | '\u{FF0F}' | '\u{2215}' | '\u{2044}' | '\u{29F8}' // slashes
        | '\u{FF3C}' | '\u{FE68}' | '\u{29F5}' | '\u{29F9}' // backslashes
        | '\u{00A5}' | '\u{20A9}'  // yen / won: backslash in CJK code pages
        | '\u{FF0E}' | '\u{2024}' | '\u{FE52}' // dots
        | '\u{FF1A}' | '\u{2236}' | '\u{FE55}' | '\u{A789}' // colons
    )
}

fn validate(raw: &str) -> Result<(), SafePathError> {
    if raw.is_empty() {
        return Err(SafePathError::Empty);
    }
    if raw.len() > MAX_PATH_BYTES {
        return Err(SafePathError::TooLong);
    }
    if raw.contains('\\') {
        return Err(SafePathError::Backslash);
    }
    if raw.starts_with('/') || raw.starts_with('~') {
        return Err(SafePathError::Absolute);
    }
    // Drive letters (C:) and any other colon (also blocks NTFS streams).
    if raw.contains(':') {
        return Err(SafePathError::Absolute);
    }

    let mut depth = 0usize;
    for part in raw.split('/') {
        depth += 1;
        if depth > MAX_PATH_DEPTH {
            return Err(SafePathError::TooDeep);
        }
        if part.is_empty() || part == "." || part == ".." {
            return Err(SafePathError::BadComponent);
        }
        if part
            .chars()
            .any(|c| c.is_control() || matches!(c, '<' | '>' | '"' | '|' | '?' | '*'))
        {
            return Err(SafePathError::ForbiddenChar);
        }
        if part.chars().any(is_deceptive) {
            return Err(SafePathError::DeceptiveChar);
        }
        if part.ends_with('.') || part.ends_with(' ') {
            return Err(SafePathError::TrailingDotOrSpace);
        }
        let stem = part.split('.').next().unwrap_or(part).trim_end();
        if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(stem)) {
            return Err(SafePathError::ReservedName);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_normal_paths() {
        for p in [
            "photos/beach.jpg",
            "photos/2026/Holiday 01.JPG",
            "cover.png",
            "photos/émilie-été.heic",
            "a/b/c/d/e/f/g/h.jpg",
        ] {
            assert!(SafePath::new(p).is_ok(), "should accept {p}");
        }
    }

    #[test]
    fn rejects_traversal() {
        for p in [
            "../secret.jpg",
            "photos/../../etc/passwd",
            "photos/./x.jpg",
            "..",
            ".",
        ] {
            assert_eq!(SafePath::new(p), Err(SafePathError::BadComponent), "{p}");
        }
    }

    #[test]
    fn rejects_absolute_and_drive_paths() {
        for p in [
            "/etc/passwd",
            "~/Pictures/a.jpg",
            "C:/Windows/win.ini",
            "c:x.jpg",
            "photos/a.jpg:hidden",
        ] {
            assert_eq!(SafePath::new(p), Err(SafePathError::Absolute), "{p}");
        }
    }

    #[test]
    fn rejects_backslash_and_unc() {
        for p in ["photos\\a.jpg", "\\\\server\\share\\a.jpg", "..\\..\\a.jpg"] {
            assert_eq!(SafePath::new(p), Err(SafePathError::Backslash), "{p}");
        }
    }

    #[test]
    fn rejects_empty_components() {
        assert_eq!(SafePath::new(""), Err(SafePathError::Empty));
        assert_eq!(
            SafePath::new("photos//a.jpg"),
            Err(SafePathError::BadComponent)
        );
        assert_eq!(SafePath::new("photos/"), Err(SafePathError::BadComponent));
    }

    #[test]
    fn rejects_windows_reserved_names() {
        for p in [
            "CON",
            "photos/nul.jpg",
            "aux",
            "COM1.png",
            "lpt9.txt",
            "Con .jpg",
        ] {
            assert!(SafePath::new(p).is_err(), "{p}");
        }
        // Names that merely start with a reserved word are fine.
        assert!(SafePath::new("console.jpg").is_ok());
        assert!(SafePath::new("nullabor.jpg").is_ok());
    }

    #[test]
    fn rejects_superscript_device_names() {
        for p in [
            "COM\u{b9}.jpg",
            "LPT\u{b2}",
            "com\u{b3}.txt",
            "photos/Lpt\u{b9}.png",
            "CLOCK$",
        ] {
            assert_eq!(SafePath::new(p), Err(SafePathError::ReservedName), "{p:?}");
        }
    }

    #[test]
    fn rejects_invisible_and_lookalike_chars() {
        for p in [
            "a\u{202e}gpj.exe",          // right-to-left override: shows as "aexe.jpg"
            "photo\u{200b}.jpg",         // zero-width space
            "\u{feff}a.jpg",             // byte order mark
            "a\u{2066}b.jpg",            // bidi isolate
            "\u{ff0e}\u{ff0e}\u{ff0f}x", // fullwidth ../x
            "a\u{ff3c}b.jpg",            // fullwidth backslash
            "a\u{00a5}b.jpg",            // yen sign (backslash in Japanese code page)
            "c\u{ff1a}x.jpg",            // fullwidth colon
            "a\u{2215}b.jpg",            // division slash
            "a\u{e0041}.jpg",            // tag character
        ] {
            assert_eq!(SafePath::new(p), Err(SafePathError::DeceptiveChar), "{p:?}");
        }
        // Ordinary non-ASCII names stay fine.
        for p in [
            "photos/日本の夏.jpg",
            "café/naïve.png",
            "Ølberg-2026.jpg",
            "😀.jpg",
        ] {
            assert!(SafePath::new(p).is_ok(), "{p:?}");
        }
    }

    #[test]
    fn rejects_trailing_dot_or_space() {
        assert_eq!(
            SafePath::new("photos./a.jpg"),
            Err(SafePathError::TrailingDotOrSpace)
        );
        assert_eq!(
            SafePath::new("a.jpg "),
            Err(SafePathError::TrailingDotOrSpace)
        );
    }

    #[test]
    fn rejects_control_and_forbidden_chars() {
        for p in [
            "a\0.jpg", "a\n.jpg", "a<b.jpg", "a|b.jpg", "a?.jpg", "a*.jpg", "a\".jpg",
        ] {
            assert_eq!(SafePath::new(p), Err(SafePathError::ForbiddenChar), "{p:?}");
        }
    }

    #[test]
    fn enforces_length_and_depth() {
        let long = "a".repeat(MAX_PATH_BYTES + 1);
        assert_eq!(SafePath::new(&long), Err(SafePathError::TooLong));
        let deep = ["d"; MAX_PATH_DEPTH + 1].join("/");
        assert_eq!(SafePath::new(&deep), Err(SafePathError::TooDeep));
    }

    #[test]
    fn deserialize_validates() {
        let ok: Result<SafePath, _> = serde_json::from_str("\"photos/a.jpg\"");
        assert!(ok.is_ok());
        let bad: Result<SafePath, _> = serde_json::from_str("\"../a.jpg\"");
        assert!(bad.is_err());
    }
}
