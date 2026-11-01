//! Any string as a project-relative path: accepted paths must never be able
//! to leave the project folder on any OS.
#![no_main]
use libfuzzer_sys::fuzz_target;
use printnest_model::SafePath;

fuzz_target!(|s: &str| {
    if let Ok(p) = SafePath::new(s) {
        let p = p.as_str();
        assert!(!p.starts_with('/') && !p.contains('\\') && !p.contains(':'));
        assert!(!p.split('/').any(|c| c.is_empty() || c == "." || c == ".."));
        assert!(!p.chars().any(char::is_control));
    }
});
