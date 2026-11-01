//! Any bytes as a photo, in-process (the app runs this inside the sandbox):
//! must not panic, must respect the pixel limits, and previews must have
//! exactly width*height*4 bytes.
#![no_main]
use libfuzzer_sys::fuzz_target;
use printnest_imaging::{decode_preview, probe};
use printnest_model::limits::MAX_IMAGE_PIXELS;

fuzz_target!(|data: &[u8]| {
    if let Ok(info) = probe(data) {
        assert!(u64::from(info.width) * u64::from(info.height) <= MAX_IMAGE_PIXELS);
    }
    if let Ok(p) = decode_preview(data, 64) {
        assert!(p.width <= 64 && p.height <= 64);
        assert_eq!(p.rgba.len() as u64, u64::from(p.width) * u64::from(p.height) * 4);
    }
});
