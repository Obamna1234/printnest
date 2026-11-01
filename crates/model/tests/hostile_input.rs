//! Hostile-input tests (SPEC section 10, "hostile-file corpus").
//!
//! Two parts:
//! 1. A hand-written corpus of malicious or malformed project files that
//!    must all be rejected.
//! 2. A deterministic mutation fuzzer: thousands of randomly corrupted
//!    manifests and catalogues must each either load (and then re-validate)
//!    or return an error. A panic fails the test. Full cargo-fuzz targets
//!    arrive in milestone 2; this keeps the guarantee from day one with no
//!    extra dependencies.

// Test code: panicking on a broken fixture is the right failure mode.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use printnest_model::catalogue::{BUILTIN_TOML, Catalogue};
use printnest_model::project::{Element, Project, Rect};
use printnest_model::safe_path::SafePath;
use std::panic;

fn catalogue() -> Catalogue {
    Catalogue::builtin().expect("built-in catalogue is valid")
}

fn valid_manifest() -> String {
    let c = catalogue();
    let mut p = Project::new_blank(&c, "book-20x20", "Fuzz seed").expect("blank");
    if let Some(first) = p.pages.first_mut() {
        first.elements.push(Element::Photo {
            id: "p1".into(),
            rect: Rect {
                x: -3.0,
                y: -3.0,
                w: 206.0,
                h: 206.0,
                rotation: 0.0,
            },
            src: Some(SafePath::new("photos/a.jpg").expect("safe")),
            crop: Default::default(),
        });
        first.elements.push(Element::Text {
            id: "t1".into(),
            rect: Rect {
                x: 10.0,
                y: 10.0,
                w: 100.0,
                h: 20.0,
                rotation: 5.0,
            },
            text: "Hello".into(),
            font: "sans".into(),
            size_pt: 18.0,
            colour: "#123abc".into(),
            align: Default::default(),
        });
    }
    p.to_json().expect("serialise")
}

#[test]
fn hand_written_hostile_corpus_is_rejected() {
    let c = catalogue();
    let base = valid_manifest();
    assert!(Project::from_json(&base, &c).is_ok(), "seed must be valid");

    let cases: Vec<(&str, String)> = vec![
        ("empty", String::new()),
        ("null", "null".into()),
        ("number", "42".into()),
        (
            "wrong magic",
            base.replace("printnest-project", "evil-project"),
        ),
        (
            "future schema",
            base.replace("\"schema_version\": 1", "\"schema_version\": 4294967295"),
        ),
        (
            "negative schema",
            base.replace("\"schema_version\": 1", "\"schema_version\": -1"),
        ),
        (
            "traversal",
            base.replace("photos/a.jpg", "../../../../home/user/.ssh/id_rsa"),
        ),
        ("absolute unix", base.replace("photos/a.jpg", "/etc/shadow")),
        (
            "windows drive",
            base.replace("photos/a.jpg", "C:/Users/Public/x.jpg"),
        ),
        ("unc", base.replace("photos/a.jpg", "//server/share/x.jpg")),
        (
            "ntfs stream",
            base.replace("photos/a.jpg", "photos/a.jpg:Zone.Identifier"),
        ),
        (
            "device name",
            base.replace("photos/a.jpg", "photos/CON.jpg"),
        ),
        (
            "nul byte",
            base.replace("photos/a.jpg", "photos/a\\u0000.jpg"),
        ),
        (
            "url as src",
            base.replace("photos/a.jpg", "https://evil.example/x.jpg"),
        ),
        (
            "script field",
            base.replace("\"title\":", "\"onload\": \"x\", \"title\":"),
        ),
        (
            "html element",
            base.replace("\"type\": \"text\"", "\"type\": \"html\""),
        ),
        ("NaN-ish", base.replace("\"w\": 206.0", "\"w\": 1e400")),
        (
            "huge rect",
            base.replace("\"w\": 206.0", "\"w\": 99999999.0"),
        ),
        ("zero size", base.replace("\"w\": 206.0", "\"w\": 0.0")),
        ("bad colour", base.replace("#123abc", "javascript:alert(1)")),
        (
            "font path",
            base.replace("\"font\": \"sans\"", "\"font\": \"../../x.ttf\""),
        ),
        ("bad product", base.replace("book-20x20", "book-99x99")),
        ("bad paper", base.replace("silk-170", "canvas-380")),
        (
            "bad cover",
            base.replace("\"cover\": \"soft\"", "\"cover\": \"gold\""),
        ),
        (
            "duplicate id",
            base.replace("\"id\": \"t1\"", "\"id\": \"p1\""),
        ),
        (
            "id injection",
            base.replace("\"id\": \"t1\"", "\"id\": \"t1;rm -rf /\""),
        ),
        (
            "deep nesting",
            format!("{}{}", "{\"a\":".repeat(5_000), "}".repeat(5_000)),
        ),
        (
            "truncated",
            base.get(..base.len() / 2).unwrap_or_default().to_owned(),
        ),
        ("trailing garbage", format!("{base} trailing")),
    ];
    for (name, json) in cases {
        let result = panic::catch_unwind(|| Project::from_json(&json, &c));
        match result {
            Ok(Ok(_)) => panic!("hostile case {name:?} was ACCEPTED"),
            Ok(Err(_)) => {}
            Err(_) => panic!("hostile case {name:?} PANICKED"),
        }
    }
}

/// Small deterministic PRNG (xorshift64*) so failures are reproducible
/// without adding a dependency.
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

const INTERESTING: &[&[u8]] = &[
    b"\"",
    b"\\",
    b"{",
    b"}",
    b"[",
    b"]",
    b":",
    b",",
    b"-",
    b"..",
    b"/",
    b"\0",
    b"\xff",
    b"1e309",
    b"-0",
    b"NaN",
    b"null",
    b"true",
    b"999999999999999999999",
    b"\\u0000",
    b"\\ud800",
    b"../",
    b"C:",
    b"CON",
];

fn mutate(rng: &mut Rng, input: &[u8]) -> Vec<u8> {
    let mut out = input.to_vec();
    let rounds = 1 + rng.below(8);
    for _ in 0..rounds {
        let pos = rng.below(out.len() + 1);
        match rng.below(5) {
            0 if !out.is_empty() => {
                let p = pos.min(out.len() - 1);
                if let Some(b) = out.get_mut(p) {
                    *b = rng.next() as u8;
                }
            }
            1 if !out.is_empty() => {
                let end = (pos + 1 + rng.below(16)).min(out.len());
                let start = pos.min(end);
                out.drain(start..end);
            }
            2 => {
                let token = INTERESTING
                    .get(rng.below(INTERESTING.len()))
                    .copied()
                    .unwrap_or(b"");
                let p = pos.min(out.len());
                out.splice(p..p, token.iter().copied());
            }
            3 if !out.is_empty() => {
                // Duplicate a chunk (grows structures, repeats keys).
                let start = pos.min(out.len() - 1);
                let end = (start + 1 + rng.below(64)).min(out.len());
                let chunk: Vec<u8> = out.get(start..end).map(<[u8]>::to_vec).unwrap_or_default();
                let p = rng.below(out.len() + 1);
                out.splice(p..p, chunk);
            }
            _ => {
                let p = pos.min(out.len());
                out.truncate(p);
            }
        }
    }
    out
}

#[test]
fn mutated_projects_never_panic() {
    let c = catalogue();
    let seed = valid_manifest().into_bytes();
    let mut rng = Rng(0x5EED_1234_ABCD_0001);
    let mut accepted = 0usize;
    for i in 0..15_000 {
        let bytes = mutate(&mut rng, &seed);
        let result = panic::catch_unwind(|| Project::from_reader(bytes.as_slice(), &c));
        match result {
            Err(_) => panic!(
                "iteration {i} panicked on input: {:?}",
                String::from_utf8_lossy(&bytes)
            ),
            Ok(Ok(project)) => {
                accepted += 1;
                // Anything accepted must survive a round trip and re-validate.
                let json = project.to_json().expect("serialise accepted project");
                let again = Project::from_json(&json, &c).expect("accepted project round-trips");
                assert_eq!(
                    project, again,
                    "iteration {i}: round trip changed the project"
                );
                for path in project.photo_paths() {
                    assert!(SafePath::new(path.as_str()).is_ok());
                    assert!(!path.as_str().contains(".."));
                }
            }
            Ok(Err(_)) => {}
        }
    }
    // Sanity: the fuzzer must be producing some valid variants too.
    assert!(
        accepted > 0,
        "no mutated input was accepted; fuzzer is too destructive"
    );
}

#[test]
fn mutated_catalogues_never_panic() {
    let seed = BUILTIN_TOML.as_bytes();
    let mut rng = Rng(0xCA7A_1060_0000_0002);
    for i in 0..5_000 {
        let bytes = mutate(&mut rng, seed);
        let text = String::from_utf8_lossy(&bytes).into_owned();
        if panic::catch_unwind(|| Catalogue::parse(&text)).is_err() {
            panic!("catalogue iteration {i} panicked");
        }
    }
}

#[test]
fn random_paths_never_escape() {
    let mut rng = Rng(0xFA7B_0000_0000_0003);
    let alphabet: &[u8] = b"ab./\\:C~ \0\n<>|?*";
    for _ in 0..50_000 {
        let len = rng.below(24);
        let s: String = (0..len)
            .map(|_| *alphabet.get(rng.below(alphabet.len())).unwrap_or(&b'a') as char)
            .collect();
        if let Ok(p) = SafePath::new(&s) {
            let p = p.as_str();
            assert!(!p.starts_with('/'), "{p:?}");
            assert!(!p.contains('\\'), "{p:?}");
            assert!(!p.contains(':'), "{p:?}");
            assert!(
                !p.split('/').any(|c| c == ".." || c == "." || c.is_empty()),
                "{p:?}"
            );
            assert!(!p.chars().any(char::is_control), "{p:?}");
        }
    }
}
