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
//! it. Each running copy of the app writes its own `session-…` subfolder,
//! so two copies never overwrite or clear each other's recovery.

use printnest_document::ProjectFolder;
use printnest_model::Catalogue;
use printnest_model::limits::MAX_PHOTOS_PER_PROJECT;
use printnest_model::project::Project;
use printnest_model::safe_path::SafePath;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;

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
        // A relative XDG_CONFIG_HOME is invalid by spec: use ~/.config.
        var("XDG_CONFIG_HOME")
            .filter(|p| p.is_absolute())
            .or_else(|| var("HOME").map(|h| h.join(".config")))
    }?;
    base.is_absolute()
        .then(|| base.join("PrintNest").join("recovery"))
}

/// This run's own subfolder name.
fn session() -> &'static str {
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(|| {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        format!("session-{nanos}-{}", std::process::id())
    })
}

fn own_folder(create: bool) -> Option<ProjectFolder> {
    let d = dir()?.join(session());
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
    let f = own_folder(true).ok_or("no settings folder on this computer")?;
    // Paths that aren't valid Unicode can't be stored as JSON text; those
    // photos would come back missing, so they are left out explicitly.
    let map: BTreeMap<&str, &str> = sources
        .iter()
        .filter_map(|(k, p)| Some((k.as_str(), p.to_str()?)))
        .collect();
    let json = serde_json::to_vec(&map).map_err(|e| e.to_string())?;
    let tmp = f.root().join(format!("{SOURCES}.tmp"));
    let written = std::fs::File::create(&tmp)
        .and_then(|mut file| file.write_all(&json).and_then(|()| file.sync_all()))
        .and_then(|()| std::fs::rename(&tmp, f.root().join(SOURCES)));
    written.map_err(|e| e.to_string())?;
    f.save_recovery(project, catalogue)
        .map_err(|e| e.to_string())
}

/// A recovered design: its folder, the design and where its photos are.
pub type Recovered = (PathBuf, Project, Vec<(SafePath, PathBuf)>);

/// The most recent recovered design from an earlier run, if any.
#[must_use]
pub fn load(catalogue: &Catalogue) -> Option<Recovered> {
    let base = dir()?;
    let mut sessions: Vec<PathBuf> = std::fs::read_dir(&base)
        .ok()?
        .flatten()
        .take(100)
        .filter(|e| e.file_name().to_string_lossy().starts_with("session-"))
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .filter(|p| p.file_name().is_some_and(|n| n != session()))
        .collect();
    // Names sort by start time (fixed-width enough for this century).
    sessions.sort();
    sessions.into_iter().rev().find_map(|d| {
        let f = ProjectFolder::existing(&d).ok()?;
        let project = f.recovery_any(catalogue)?;
        Some((d, project, read_sources(&f)))
    })
}

/// The photo list; a damaged or missing list just means no photos (the
/// design itself is still recovered).
fn read_sources(f: &ProjectFolder) -> Vec<(SafePath, PathBuf)> {
    let path = f.root().join(SOURCES);
    let Ok(bytes) = printnest_sandbox::read_capped(&path) else {
        return Vec::new();
    };
    if bytes.len() as u64 > MAX_SOURCES_BYTES {
        return Vec::new();
    }
    let Ok(map) = serde_json::from_slice::<BTreeMap<String, String>>(&bytes) else {
        return Vec::new();
    };
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

/// Forgets a recovered design from an earlier run (the user discarded or
/// opened it; opening moves it into this run's own slot on autosave).
pub fn forget(session_dir: &std::path::Path) {
    if let Some(base) = dir()
        && session_dir.parent() == Some(base.as_path())
    {
        let _ = std::fs::remove_dir_all(session_dir);
    }
}

/// Forgets this run's own recovery copy (the design was saved or
/// discarded).
pub fn clear() {
    if let Some(d) = dir() {
        let _ = std::fs::remove_dir_all(d.join(session()));
    }
}
