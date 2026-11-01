//! Deterministic mutation fuzzing of the image decoders on stable Rust.
//! The cargo-fuzz targets in /fuzz go much deeper in CI (nightly); this
//! keeps a no-panic guarantee in every normal `cargo test` run.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use image::{ImageBuffer, ImageEncoder, Rgb, Rgba, codecs};
use printnest_imaging::{decode_preview, probe};
use printnest_model::limits::MAX_IMAGE_PIXELS;
use std::panic;
use std::time::{Duration, Instant};

fn seeds() -> Vec<(&'static str, Vec<u8>)> {
    let rgb: ImageBuffer<Rgb<u8>, _> =
        ImageBuffer::from_fn(37, 23, |x, y| Rgb([(x * 7) as u8, (y * 11) as u8, 99]));
    let rgba: ImageBuffer<Rgba<u8>, _> =
        ImageBuffer::from_fn(19, 31, |x, y| Rgba([(x * 3) as u8, (y * 5) as u8, 7, 200]));
    let mut png = Vec::new();
    codecs::png::PngEncoder::new(&mut png)
        .write_image(rgba.as_raw(), 19, 31, image::ExtendedColorType::Rgba8)
        .unwrap();
    let mut jpeg = Vec::new();
    codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 75)
        .encode_image(&rgb)
        .unwrap();
    let mut tiff = std::io::Cursor::new(Vec::new());
    codecs::tiff::TiffEncoder::new(&mut tiff)
        .write_image(rgb.as_raw(), 37, 23, image::ExtendedColorType::Rgb8)
        .unwrap();
    let mut webp = Vec::new();
    codecs::webp::WebPEncoder::new_lossless(&mut webp)
        .write_image(rgba.as_raw(), 19, 31, image::ExtendedColorType::Rgba8)
        .unwrap();
    vec![
        ("png", png),
        ("jpeg", jpeg),
        ("tiff", tiff.into_inner()),
        ("webp", webp),
    ]
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
}

fn mutate(rng: &mut Rng, seed: &[u8]) -> Vec<u8> {
    let mut v = seed.to_vec();
    for _ in 0..1 + rng.below(6) {
        if v.is_empty() {
            break;
        }
        let i = rng.below(v.len());
        match rng.below(5) {
            0 => v[i] = rng.next() as u8,
            1 => v[i] ^= 1 << rng.below(8),
            // Overwrite 4 bytes with an extreme value (sizes, lengths, offsets).
            2 => {
                let val = [0u32, 1, 0x7FFF_FFFF, 0xFFFF_FFFF, 65_535, 60_001][rng.below(6)];
                for (k, b) in val.to_be_bytes().iter().enumerate() {
                    if let Some(x) = v.get_mut(i + k) {
                        *x = *b;
                    }
                }
            }
            3 => v.truncate(i),
            _ => {
                let end = (i + 1 + rng.below(32)).min(v.len());
                let chunk = v[i..end].to_vec();
                let at = rng.below(v.len());
                v.splice(at..at, chunk);
            }
        }
    }
    v
}

#[test]
fn mutated_images_never_panic_or_exceed_limits() {
    let mut rng = Rng(0x1A6E_5EED_0000_0042);
    for (name, seed) in seeds() {
        // Seeds themselves decode.
        decode_preview(&seed, 64).unwrap_or_else(|e| panic!("{name} seed: {e}"));
        for i in 0..1_500 {
            let bytes = mutate(&mut rng, &seed);
            let start = Instant::now();
            let r = panic::catch_unwind(|| (probe(&bytes), decode_preview(&bytes, 64)));
            let elapsed = start.elapsed();
            match r {
                Err(_) => panic!("{name} iteration {i} panicked; input {bytes:02x?}"),
                Ok((p, d)) => {
                    if let Ok(info) = p {
                        assert!(u64::from(info.width) * u64::from(info.height) <= MAX_IMAGE_PIXELS);
                    }
                    if let Ok(prev) = d {
                        assert!(prev.width <= 64 && prev.height <= 64);
                        assert_eq!(
                            prev.rgba.len() as u64,
                            u64::from(prev.width) * u64::from(prev.height) * 4
                        );
                    }
                }
            }
            assert!(
                elapsed < Duration::from_secs(5),
                "{name} iteration {i} took {elapsed:?}"
            );
        }
    }
}
