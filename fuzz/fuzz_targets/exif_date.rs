//! Any bytes as an EXIF block: reading the capture date must never panic,
//! and any date it returns must be one the app accepts.
#![no_main]
use libfuzzer_sys::fuzz_target;
use printnest_imaging::exif;

fuzz_target!(|data: &[u8]| {
    if let Some(t) = exif::taken(data) {
        assert!(exif::is_valid_taken(t), "{t}");
    }
    if let Some(t) = exif::parse_date(data) {
        assert!(exif::is_valid_taken(t), "{t}");
    }
});
