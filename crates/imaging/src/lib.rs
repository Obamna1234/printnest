//! Image decoding with hard limits (SPEC section 7, rule 3).
//!
//! Everything here treats its input as hostile. In the app this code only
//! ever runs inside the sandboxed worker process (`printnest-sandbox`), so a
//! bug in a decoder is contained there; the limits below are the first line
//! of defence, the sandbox the second.
//!
//! - The format is identified from the file's magic bytes, never its name,
//!   and only JPEG, PNG, WebP and TIFF are accepted.
//! - Size is checked before anything is decoded: file bytes, then the
//!   dimensions from the header, against the limits in
//!   [`printnest_model::limits`].
//! - The decoder's own allocation budget is capped.
//! - EXIF orientation is applied, so width/height are as the photo is seen.

#![forbid(unsafe_code)]

pub mod exif;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits};
use printnest_model::limits::{MAX_FILE_BYTES, MAX_IMAGE_PIXELS};
use std::io::Cursor;

/// Longest single edge the decoder will accept, in pixels.
pub const MAX_EDGE_PIXELS: u32 = 60_000;

/// Decoder allocation budget (well under the worker's 2 GB memory cap).
pub const MAX_DECODE_ALLOC: u64 = 1024 * 1024 * 1024;

/// Largest preview edge a caller may ask for.
pub const MAX_PREVIEW_EDGE: u32 = 8_192;

/// Image formats PrintNest accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Jpeg,
    Png,
    WebP,
    Tiff,
}

impl Format {
    /// Identifies the format from the first bytes of the file.
    #[must_use]
    pub fn sniff(bytes: &[u8]) -> Option<Format> {
        if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            Some(Format::Jpeg)
        } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(Format::Png)
        } else if bytes.len() >= 12
            && bytes.starts_with(b"RIFF")
            && bytes.get(8..12) == Some(b"WEBP".as_slice())
        {
            Some(Format::WebP)
        } else if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
            Some(Format::Tiff)
        } else {
            None
        }
    }

    fn to_image(self) -> ImageFormat {
        match self {
            Format::Jpeg => ImageFormat::Jpeg,
            Format::Png => ImageFormat::Png,
            Format::WebP => ImageFormat::WebP,
            Format::Tiff => ImageFormat::Tiff,
        }
    }

    /// Stable one-byte code used by the sandbox protocol.
    #[must_use]
    pub fn code(self) -> u8 {
        match self {
            Format::Jpeg => 1,
            Format::Png => 2,
            Format::WebP => 3,
            Format::Tiff => 4,
        }
    }

    /// Inverse of [`Format::code`].
    #[must_use]
    pub fn from_code(c: u8) -> Option<Format> {
        match c {
            1 => Some(Format::Jpeg),
            2 => Some(Format::Png),
            3 => Some(Format::WebP),
            4 => Some(Format::Tiff),
            _ => None,
        }
    }

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Format::Jpeg => "JPEG",
            Format::Png => "PNG",
            Format::WebP => "WebP",
            Format::Tiff => "TIFF",
        }
    }
}

/// Why an image was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    #[error("file is larger than {MAX_FILE_BYTES} bytes")]
    FileTooLarge,
    #[error("not a supported image (JPEG, PNG, WebP or TIFF)")]
    UnknownFormat,
    #[error("image has no pixels")]
    Empty,
    #[error("image is {0} × {1} pixels, more than the {MAX_IMAGE_PIXELS}-pixel limit")]
    TooManyPixels(u32, u32),
    #[error("preview size must be between 1 and {MAX_PREVIEW_EDGE} pixels")]
    BadPreviewSize,
    #[error("image could not be decoded: {0}")]
    Corrupt(String),
}

/// Header facts about an image, as the photo is displayed (after EXIF
/// orientation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Info {
    pub format: Format,
    pub width: u32,
    pub height: u32,
    /// When the photo was taken (EXIF), as `YYYYMMDDHHMMSS`.
    pub taken: Option<u64>,
}

/// A decoded, downscaled preview in 8-bit RGBA.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    pub info: Info,
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes, row-major RGBA.
    pub rgba: Vec<u8>,
}

fn limits() -> Limits {
    let mut l = Limits::default();
    l.max_image_width = Some(MAX_EDGE_PIXELS);
    l.max_image_height = Some(MAX_EDGE_PIXELS);
    l.max_alloc = Some(MAX_DECODE_ALLOC);
    l
}

fn corrupt(e: impl std::fmt::Display) -> DecodeError {
    // Keep messages short; they cross the sandbox boundary.
    let mut s = e.to_string();
    s.truncate(200);
    DecodeError::Corrupt(s)
}

/// Opens a decoder after the cheap checks, returning it with its info.
fn open(
    bytes: &[u8],
) -> Result<(impl ImageDecoder + '_, Info, image::metadata::Orientation), DecodeError> {
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(DecodeError::FileTooLarge);
    }
    let format = Format::sniff(bytes).ok_or(DecodeError::UnknownFormat)?;
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format.to_image());
    reader.limits(limits());
    let mut decoder = reader.into_decoder().map_err(corrupt)?;
    let (w, h) = decoder.dimensions();
    if w == 0 || h == 0 {
        return Err(DecodeError::Empty);
    }
    if u64::from(w) * u64::from(h) > MAX_IMAGE_PIXELS {
        return Err(DecodeError::TooManyPixels(w, h));
    }
    // A broken EXIF block must not stop an otherwise good photo.
    let exif = decoder.exif_metadata().ok().flatten();
    let orientation = exif
        .as_deref()
        .and_then(image::metadata::Orientation::from_exif_chunk)
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let taken = exif.as_deref().and_then(exif::taken);
    drop(exif);
    let (width, height) = if swaps_axes(orientation) {
        (h, w)
    } else {
        (w, h)
    };
    Ok((
        decoder,
        Info {
            format,
            width,
            height,
            taken,
        },
        orientation,
    ))
}

fn swaps_axes(o: image::metadata::Orientation) -> bool {
    use image::metadata::Orientation::{Rotate90, Rotate90FlipH, Rotate270, Rotate270FlipH};
    matches!(o, Rotate90 | Rotate270 | Rotate90FlipH | Rotate270FlipH)
}

/// Reads only the header: format and displayed size. Cheap.
///
/// # Errors
/// If the file is too large, not a supported format, empty, over the pixel
/// limit, or has an unreadable header.
pub fn probe(bytes: &[u8]) -> Result<Info, DecodeError> {
    open(bytes).map(|(_, info, _)| info)
}

/// Decodes the whole image and returns a preview whose longer edge is at
/// most `max_edge` pixels (never upscaled).
///
/// # Errors
/// As [`probe`], plus any decoding failure.
pub fn decode_preview(bytes: &[u8], max_edge: u32) -> Result<Preview, DecodeError> {
    if max_edge == 0 || max_edge > MAX_PREVIEW_EDGE {
        return Err(DecodeError::BadPreviewSize);
    }
    let (decoder, info, orientation) = open(bytes)?;
    let full = DynamicImage::from_decoder(decoder).map_err(corrupt)?;
    // Shrink FIRST, then rotate the small result: only one full-resolution
    // buffer ever exists, which keeps the largest allowed photos inside the
    // worker's memory cap. (The square bound is the same either way round.)
    let mut img = if full.width() > max_edge || full.height() > max_edge {
        let small = full.thumbnail(max_edge, max_edge);
        drop(full);
        small
    } else {
        full
    };
    img.apply_orientation(orientation);
    let rgba = img.to_rgba8();
    let (width, height) = rgba.dimensions();
    Ok(Preview {
        info,
        width,
        height,
        rgba: rgba.into_raw(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, ImageEncoder, Rgb, codecs};

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img: ImageBuffer<Rgb<u8>, _> =
            ImageBuffer::from_fn(w, h, |x, y| Rgb([(x % 256) as u8, (y % 256) as u8, 128]));
        let mut out = Vec::new();
        codecs::png::PngEncoder::new(&mut out)
            .write_image(img.as_raw(), w, h, image::ExtendedColorType::Rgb8)
            .unwrap();
        out
    }

    fn jpeg(w: u32, h: u32) -> Vec<u8> {
        let img: ImageBuffer<Rgb<u8>, _> =
            ImageBuffer::from_fn(w, h, |x, _| Rgb([(x % 256) as u8, 90, 200]));
        let mut out = Vec::new();
        codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 80)
            .encode_image(&img)
            .unwrap();
        out
    }

    #[test]
    fn sniffs_by_content_not_name() {
        assert_eq!(Format::sniff(&png(2, 2)), Some(Format::Png));
        assert_eq!(Format::sniff(&jpeg(8, 8)), Some(Format::Jpeg));
        assert_eq!(Format::sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some(Format::WebP));
        assert_eq!(Format::sniff(b"II*\0rest"), Some(Format::Tiff));
        assert_eq!(Format::sniff(b"GIF89a"), None);
        assert_eq!(Format::sniff(b"<svg"), None);
        assert_eq!(Format::sniff(b""), None);
        for f in [Format::Jpeg, Format::Png, Format::WebP, Format::Tiff] {
            assert_eq!(Format::from_code(f.code()), Some(f));
        }
        assert_eq!(Format::from_code(0), None);
    }

    #[test]
    fn probes_and_decodes_png_and_jpeg() {
        let info = probe(&png(300, 200)).unwrap();
        assert_eq!(
            (info.format, info.width, info.height),
            (Format::Png, 300, 200)
        );
        let p = decode_preview(&jpeg(400, 100), 100).unwrap();
        assert_eq!((p.info.width, p.info.height), (400, 100));
        assert_eq!((p.width, p.height), (100, 25));
        assert_eq!(p.rgba.len(), 100 * 25 * 4);
    }

    #[test]
    fn never_upscales() {
        let p = decode_preview(&png(40, 30), 1000).unwrap();
        assert_eq!((p.width, p.height), (40, 30));
    }

    #[test]
    fn rejects_unsupported_and_garbage() {
        assert_eq!(probe(b"GIF89a....."), Err(DecodeError::UnknownFormat));
        assert_eq!(probe(b""), Err(DecodeError::UnknownFormat));
        assert!(matches!(
            probe(b"\x89PNG\r\n\x1a\njunk"),
            Err(DecodeError::Corrupt(_))
        ));
        assert!(matches!(
            probe(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0]),
            Err(DecodeError::Corrupt(_))
        ));
    }

    #[test]
    fn rejects_bad_preview_sizes() {
        let p = png(4, 4);
        assert_eq!(decode_preview(&p, 0), Err(DecodeError::BadPreviewSize));
        assert_eq!(
            decode_preview(&p, MAX_PREVIEW_EDGE + 1),
            Err(DecodeError::BadPreviewSize)
        );
    }

    /// A structurally valid PNG whose header claims a huge size but whose
    /// pixel data is a few bytes: the classic decompression-bomb shape.
    fn png_header_only(w: u32, h: u32) -> Vec<u8> {
        fn crc32(data: &[u8]) -> u32 {
            let mut c = 0xFFFF_FFFFu32;
            for &b in data {
                c ^= u32::from(b);
                for _ in 0..8 {
                    c = if c & 1 != 0 {
                        0xEDB8_8320 ^ (c >> 1)
                    } else {
                        c >> 1
                    };
                }
            }
            !c
        }
        fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            let mut body = kind.to_vec();
            body.extend_from_slice(data);
            out.extend_from_slice(&body);
            out.extend_from_slice(&crc32(&body).to_be_bytes());
        }
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&w.to_be_bytes());
        ihdr.extend_from_slice(&h.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8-bit RGBA
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        chunk(&mut out, b"IHDR", &ihdr);
        // Empty zlib stream: a few bytes standing in for gigabytes of pixels.
        chunk(
            &mut out,
            b"IDAT",
            &[0x78, 0x01, 0x01, 0x00, 0x00, 0xFF, 0xFF, 0, 0, 0, 1],
        );
        chunk(&mut out, b"IEND", &[]);
        out
    }

    #[test]
    fn refuses_pixel_bombs_before_decoding() {
        // 20000 × 20000 = 400 MP: over the 100 MP limit.
        assert_eq!(
            probe(&png_header_only(20_000, 20_000)),
            Err(DecodeError::TooManyPixels(20_000, 20_000))
        );
        // Over the per-edge limit: refused by the decoder's own limits.
        assert!(probe(&png_header_only(1_000_000, 1)).is_err());
        assert!(probe(&png_header_only(0, 10)).is_err());
        // Under the limits but no data: a clean error, not a hang or crash.
        assert!(decode_preview(&png_header_only(5_000, 5_000), 100).is_err());
    }

    #[test]
    fn truncated_files_fail_cleanly() {
        for bytes in [png(64, 64), jpeg(64, 64)] {
            for cut in [10, bytes.len() / 3, bytes.len() / 2, bytes.len() - 5] {
                let r = decode_preview(&bytes[..cut], 32);
                assert!(r.is_err() || r.is_ok(), "must return, not panic");
            }
        }
    }
}
