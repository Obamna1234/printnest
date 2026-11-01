//! Any text as a catalogue file: must not panic; accepted products must
//! have sane geometry.
#![no_main]
use libfuzzer_sys::fuzz_target;
use printnest_model::Catalogue;

fuzz_target!(|s: &str| {
    if let Ok(cat) = Catalogue::parse(s) {
        for p in cat.products() {
            let t = p.trim();
            assert!(t.width.0.is_finite() && t.width.0 > 0.0);
            assert!(t.height.0.is_finite() && t.height.0 > 0.0);
            assert!(p.page_rule().allows(p.default_pages()));
        }
    }
});
