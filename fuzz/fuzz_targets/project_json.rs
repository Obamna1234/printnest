//! Any bytes as a project manifest: must return Ok or Err, never panic,
//! and anything accepted must round-trip and contain only safe paths.
#![no_main]
use libfuzzer_sys::fuzz_target;
use printnest_model::{Catalogue, Project};
use std::sync::OnceLock;

static CAT: OnceLock<Catalogue> = OnceLock::new();

fuzz_target!(|data: &[u8]| {
    let cat = CAT.get_or_init(|| Catalogue::builtin().expect("built-in catalogue"));
    if let Ok(p) = Project::from_reader(data, cat) {
        let json = p.to_json().expect("serialise");
        let again = Project::from_json(&json, cat).expect("accepted project must round-trip");
        assert_eq!(p, again);
        for path in p.photo_paths() {
            assert!(!path.as_str().split('/').any(|c| c == ".." || c.is_empty()));
        }
    }
});
