//! Hard limits for untrusted input (SPEC section 7, rule 3).
//!
//! Changing any value here weakens or strengthens the app's defences, so
//! edits need CODEOWNERS review. Every limit has a test that proves it is
//! enforced.

/// Largest image the app will decode, in pixels (100 megapixels).
pub const MAX_IMAGE_PIXELS: u64 = 100_000_000;

/// Largest single input file, in bytes (500 MB).
pub const MAX_FILE_BYTES: u64 = 500 * 1024 * 1024;

/// Largest project manifest (JSON), in bytes (16 MB).
pub const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;

/// Most photos referenced by one project.
pub const MAX_PHOTOS_PER_PROJECT: usize = 2_000;

/// Most pages in one project (covers the thickest photo book plus slack).
pub const MAX_PAGES: u32 = 400;

/// Most elements (frames, text boxes) on one page.
pub const MAX_ELEMENTS_PER_PAGE: usize = 200;

/// Most characters in one text box.
pub const MAX_TEXT_CHARS: usize = 10_000;

/// Longest identifier (element ids, product ids), in bytes.
pub const MAX_ID_LEN: usize = 64;

/// Longest project title, in characters.
pub const MAX_TITLE_CHARS: usize = 200;

/// Most entries in a project archive (zip).
pub const MAX_ARCHIVE_ENTRIES: usize = 10_000;

/// Largest allowed ratio of uncompressed to compressed archive size
/// (zip-bomb guard).
pub const MAX_COMPRESSION_RATIO: u64 = 20;

/// Longest relative path stored in a project, in bytes.
pub const MAX_PATH_BYTES: usize = 255;

/// Deepest folder nesting for a relative path.
pub const MAX_PATH_DEPTH: usize = 8;

/// Seconds the sandboxed decoder may spend on one file.
pub const DECODE_TIMEOUT_SECS: u64 = 10;

/// Memory cap for the sandboxed decoder process, in bytes (2 GB).
pub const DECODE_MEMORY_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Largest physical page edge the app will accept, in millimetres (5 m).
pub const MAX_PAGE_EDGE_MM: f64 = 5_000.0;

/// Largest catalogue data file, in bytes.
pub const MAX_CATALOGUE_BYTES: usize = 1024 * 1024;
