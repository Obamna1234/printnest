# Bundled assets

Every non-code file shipped in PrintNest is listed here with its source and
licence. Allowed licences: original work (MIT OR Apache-2.0), CC0-1.0,
OFL-1.1, MIT, Apache-2.0. Anything else needs a maintainer decision first.

| File | Source | Licence | Notes |
| --- | --- | --- | --- |
| `catalogue/*.toml` | Original (sizes are standard physical dimensions) | MIT OR Apache-2.0 | `verified = false` until checked against a retailer |
| UI font (via `epaint_default_fonts`) | Ubuntu-Light, Hack, Noto Emoji, emoji-icon-font, bundled by egui | Ubuntu Font Licence 1.0, MIT, OFL-1.1, Apache-2.0 | Interface text, and text in exports (Ubuntu Light, Hack), drawn as pixels: font files are never embedded in exported files. Allowed by a single-crate exception in `deny.toml` |
| `tests/fixtures/rotated-64x48.jpg` | Original (generated gradient + EXIF orientation tag) | MIT OR Apache-2.0 | Test fixture only, not shipped |
| `tests/fixtures/gradient*-40x30.{png,webp,tiff}` | Original (generated gradients) | MIT OR Apache-2.0 | Fuzz seeds / test fixtures, not shipped |

No images or templates are bundled in the app yet.
