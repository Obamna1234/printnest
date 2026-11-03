//! Crash recovery for designs that have never been saved.
//!
//! Once a minute, while an unsaved design has changes, it is written to
//! PrintNest's own settings folder (never anywhere else):
//!
//! - Linux: `$XDG_CONFIG_HOME/PrintNest/recovery` (or `~/.config/…`)
//! - macOS: `~/Library/Application Support/PrintNest/recovery`
//! - Windows: `%APPDATA%\PrintNest\recovery`
//!
//! It holds the design (validated on load like any project) and a list of
//! where its photos are on this computer. On the next start the user is
//! offered the design back; saving it properly, or discarding it, clears
//! the folder. Only one unsaved design is kept.

use printnest_document::ProjectFolder;
use printnest_model::Catalogue;
use printnest_model::limits::MAX_PHOTOS_PER_PROJECT;
use printnest_model::project::Project;
use printnest_model::safe_path::SafePath;
use std::collections::BTreeMap;
use std::path::PathBuf;

const SOURCES: &str = "photo-sources.json";
/// Larger lists are not ours (the file is still treated as untrusted).
const MAX_SOURCES_BYTES: u64 = 4 * 1024 * 1024;

/// The app's recovery folder (not created).
#[must_use]
pub fn dir() -> Option<PathBuf> {
    let var = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let base = if cfg!(windows) {
        var("APPDATA")
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|h| h.join("Library").join("Application Support"))
    } else {
        var("XDG_CONFIG_HOME").or_else(|| var("HOME").map(|h| h.join(".config")))
    }?;
    base.is_absolute()
        .then(|| base.join("PrintNest").join("recovery"))
}

fn folder(create: bool) -> Option<ProjectFolder> {
    let d = dir()?;
    if create {
        std::fs::create_dir_all(&d).ok()?;
    }
    ProjectFolder::existing(&d).ok()
}

/// Writes an unsaved design and where its photos are.
///
/// # Errors
/// As a message for the status line.
pub fn save(
    project: &Project,
    catalogue: &Catalogue,
    sources: &[(SafePath, PathBuf)],
) -> Result<(), String> {
    let f = folder(true).ok_or("no settings folder on this computer")?;
    let map: BTreeMap<&str, String> = sources
        .iter()
        .map(|(k, p)| (k.as_str(), p.to_string_lossy().into_owned()))
        .collect();
    let json = serde_json::to_vec(&map).map_err(|e| e.to_string())?;
    let tmp = f.root().join(format!("{SOURCES}.tmp"));
    std::fs::write(&tmp, &json)
        .and_then(|()| std::fs::rename(&tmp, f.root().join(SOURCES)))
        .map_err(|e| e.to_string())?;
    f.save_recovery(project, catalogue)
        .map_err(|e| e.to_string())
}

/// The recovered design and its photo locations, if there is one.
#[must_use]
pub fn load(catalogue: &Catalogue) -> Option<(Project, Vec<(SafePath, PathBuf)>)> {
    let f = folder(false)?;
    let project = f.recovery_any(catalogue)?;
    let path = f.root().join(SOURCES);
    let meta = std::fs::symlink_metadata(&path).ok();
    let sources = match meta {
        Some(m) if m.is_file() && m.len() <= MAX_SOURCES_BYTES => {
            let text = std::fs::read(&path).ok()?;
            let map: BTreeMap<String, String> = serde_json::from_slice(&text).ok()?;
            map.into_iter()
                .take(MAX_PHOTOS_PER_PROJECT)
                .filter_map(|(k, p)| {
                    let key = SafePath::new(&k).ok()?;
                    let p = PathBuf::from(p);
                    // Only absolute paths to the user's own photos.
                    p.is_absolute().then_some((key, p))
                })
                .collect()
        }
        _ => Vec::new(),
    };
    Some((project, sources))
}

/// Forgets the recovered design.
pub fn clear() {
    if let Some(d) = dir() {
        for name in [printnest_document::RECOVERY_FILE, SOURCES] {
            let _ = std::fs::remove_file(d.join(name));
        }
    }
}
