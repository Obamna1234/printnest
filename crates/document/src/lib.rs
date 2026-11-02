//! Project folders on disk (SPEC section 7, rules 2 and 4).
//!
//! A saved project is a folder:
//!
//! ```text
//! Holiday 2026.printnest/
//!   project.json        the design (validated on every load)
//!   photos/             copies of the photos it uses
//! ```
//!
//! Safety rules enforced here:
//! - Every path inside a project is a [`SafePath`] joined under the folder,
//!   and no part of it may be a symlink (or resolve outside the folder), so
//!   a shared project cannot make PrintNest read or write anything else, or
//!   point thousands of photo names at one file.
//! - A new project never overwrites an existing folder; photos are written
//!   with create-new semantics and an existing file is never reused.
//! - `project.json` is replaced atomically (write a uniquely named temp
//!   file, sync, rename, sync the folder), and only if the result is small
//!   enough to open again.
//! - Photos are read size-capped and only if they are regular files.

#![forbid(unsafe_code)]

use printnest_model::Catalogue;
use printnest_model::limits::MAX_MANIFEST_BYTES;
use printnest_model::project::{Project, ProjectError};
use printnest_model::safe_path::SafePath;
use printnest_sandbox::{SandboxError, read_capped};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// File name of the design inside a project folder.
pub const PROJECT_FILE: &str = "project.json";
/// Extension of project folders.
pub const FOLDER_EXT: &str = "printnest";

/// Why a project folder operation failed.
#[derive(Debug, thiserror::Error)]
pub enum DocError {
    #[error("{0}")]
    Io(String),
    #[error("a folder or file with that name already exists")]
    Exists,
    #[error("project name may only use letters, digits, spaces, '-' and '_'")]
    BadName,
    #[error("{0} points outside the project folder")]
    Escape(String),
    #[error("the design is too large to save ({0} bytes)")]
    TooLarge(usize),
    #[error("not a PrintNest project file (expected {PROJECT_FILE})")]
    NotAProject,
    #[error(transparent)]
    Project(#[from] ProjectError),
    #[error("{0}")]
    Photo(#[from] SandboxError),
}

fn io(e: impl std::fmt::Display) -> DocError {
    DocError::Io(e.to_string())
}

/// An existing project folder, identified by its canonical path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectFolder {
    root: PathBuf,
}

impl ProjectFolder {
    /// The folder's canonical path.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Wraps an existing directory.
    ///
    /// # Errors
    /// If it doesn't exist or isn't a directory.
    pub fn existing(dir: &Path) -> Result<Self, DocError> {
        let root = dir.canonicalize().map_err(io)?;
        if !root.is_dir() {
            return Err(DocError::Io("not a folder".into()));
        }
        Ok(ProjectFolder { root })
    }

    /// Creates `<parent>/<name>.printnest/`. Never reuses an existing folder.
    ///
    /// # Errors
    /// On an invalid name, an existing folder, or I/O failure.
    pub fn create(parent: &Path, name: &str) -> Result<Self, DocError> {
        let name = clean_name(name)?;
        let dir = parent.join(format!("{name}.{FOLDER_EXT}"));
        fs::create_dir(&dir).map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                DocError::Exists
            } else {
                io(e)
            }
        })?;
        fs::create_dir(dir.join("photos")).map_err(io)?;
        Self::existing(&dir)
    }

    /// Resolves a project-relative path to a real path inside the folder.
    /// No existing part of the path may be a symlink, and existing parts must
    /// canonicalise to somewhere inside the folder. Returns the canonical
    /// path when the file exists.
    ///
    /// # Errors
    /// If any part is a symlink or the path escapes the folder.
    pub fn resolve(&self, path: &SafePath) -> Result<PathBuf, DocError> {
        let escape = || DocError::Escape(path.to_string());
        let mut p = self.root.clone();
        let mut exists = true;
        for c in path.components() {
            p.push(c);
            if !exists {
                continue;
            }
            match fs::symlink_metadata(&p) {
                Ok(m) if m.file_type().is_symlink() => return Err(escape()),
                Ok(_) => {}
                Err(_) => exists = false,
            }
        }
        if exists {
            let real = p.canonicalize().map_err(|_| escape())?;
            if !real.starts_with(&self.root) {
                return Err(escape());
            }
            return Ok(real);
        }
        // Check the deepest existing ancestor too (defence in depth).
        let mut probe = p.as_path();
        while let Some(parent) = probe.parent() {
            probe = parent;
            if fs::symlink_metadata(probe).is_ok() {
                let real = probe.canonicalize().map_err(|_| escape())?;
                if !real.starts_with(&self.root) {
                    return Err(escape());
                }
                break;
            }
        }
        Ok(p)
    }

    /// Names already used under `photos/` on disk, as lower-case
    /// project paths (`photos/name.jpg`), so new photos never reuse one.
    /// Reads at most 10,000 entries.
    #[must_use]
    pub fn photo_names(&self) -> std::collections::HashSet<String> {
        fs::read_dir(self.root.join("photos"))
            .into_iter()
            .flatten()
            .flatten()
            .take(10_000)
            .map(|e| format!("photos/{}", e.file_name().to_string_lossy()).to_lowercase())
            .collect()
    }

    /// Reads a photo referenced by the project, size-capped.
    ///
    /// # Errors
    /// If it escapes the folder, isn't a regular file, or is too large.
    pub fn read_photo(&self, path: &SafePath) -> Result<Vec<u8>, DocError> {
        let real = self.resolve(path)?;
        Ok(read_capped(&real)?)
    }

    /// Copies a photo from anywhere the user picked (or another project)
    /// into this project at `dest`. Never overwrites or reuses an existing
    /// file: if `dest` is taken (including by a name differing only in case
    /// on Windows/macOS) this returns [`DocError::Exists`] and the caller
    /// picks another name.
    ///
    /// # Errors
    /// On escape, an existing destination, I/O failure or an unreadable
    /// source.
    pub fn import_photo(&self, dest: &SafePath, source: &Path) -> Result<(), DocError> {
        let target = self.resolve(dest)?;
        if fs::symlink_metadata(&target).is_ok() {
            return Err(DocError::Exists);
        }
        let bytes = read_capped(source)?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(io)?;
            // Re-check after creating directories.
            self.resolve(dest)?;
        }
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::AlreadyExists {
                    DocError::Exists
                } else {
                    io(e)
                }
            })?;
        let written = f.write_all(&bytes).and_then(|()| f.sync_all());
        if let Err(e) = written {
            drop(f);
            let _ = fs::remove_file(&target);
            return Err(io(e));
        }
        Ok(())
    }

    /// Writes the design atomically.
    ///
    /// # Errors
    /// If the project is invalid, too large to reopen, or the write fails.
    pub fn save(&self, project: &Project, catalogue: &Catalogue) -> Result<(), DocError> {
        project.validate(catalogue)?;
        let mut json = project.to_json_compact()?;
        json.push('\n');
        if u64::try_from(json.len()).map_or(true, |n| n > MAX_MANIFEST_BYTES) {
            return Err(DocError::TooLarge(json.len()));
        }
        // A unique temp name: a stray file or folder can't block saving.
        let (tmp, mut f) = (0..100u32)
            .find_map(|n| {
                let tmp = self
                    .root
                    .join(format!(".{PROJECT_FILE}.{}.{n}.tmp", std::process::id()));
                OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&tmp)
                    .ok()
                    .map(|f| (tmp, f))
            })
            .ok_or_else(|| DocError::Io("couldn't create a temporary file".into()))?;
        let written = f
            .write_all(json.as_bytes())
            .and_then(|()| f.sync_all())
            .and_then(|()| {
                drop(f);
                fs::rename(&tmp, self.root.join(PROJECT_FILE))
            });
        if let Err(e) = written {
            let _ = fs::remove_file(&tmp);
            return Err(io(e));
        }
        // Make the rename itself durable (not possible on Windows; NTFS
        // journals metadata).
        #[cfg(unix)]
        if let Ok(dir) = fs::File::open(&self.root) {
            let _ = dir.sync_all();
        }
        Ok(())
    }

    /// Loads the design from this folder.
    ///
    /// # Errors
    /// If the file is missing, too large, invalid or escapes the folder.
    pub fn load(&self, catalogue: &Catalogue) -> Result<Project, DocError> {
        let path = self.root.join(PROJECT_FILE);
        let meta = fs::symlink_metadata(&path).map_err(io)?;
        if !meta.is_file() {
            return Err(DocError::NotAProject);
        }
        let file = fs::File::open(&path).map_err(io)?;
        // The file opened must be the one checked (no swap in between).
        let opened = file.metadata().map_err(io)?;
        if !opened.is_file() {
            return Err(DocError::NotAProject);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if (opened.dev(), opened.ino()) != (meta.dev(), meta.ino()) {
                return Err(DocError::NotAProject);
            }
        }
        let project = Project::from_reader(file, catalogue)?;
        // Every referenced photo must resolve inside the folder.
        for p in project.photo_paths() {
            self.resolve(p)?;
        }
        Ok(project)
    }
}

/// Opens a project given its `project.json` (or the folder itself).
///
/// # Errors
/// As [`ProjectFolder::load`].
pub fn open(path: &Path, catalogue: &Catalogue) -> Result<(ProjectFolder, Project), DocError> {
    let dir = if path.is_dir() {
        path.to_path_buf()
    } else if path.file_name().is_some_and(|n| n == PROJECT_FILE) {
        path.parent()
            .map(Path::to_path_buf)
            .ok_or(DocError::NotAProject)?
    } else {
        return Err(DocError::NotAProject);
    };
    let folder = ProjectFolder::existing(&dir)?;
    let project = folder.load(catalogue)?;
    Ok((folder, project))
}

/// Validates a user-typed project name.
///
/// # Errors
/// If it is empty, too long or uses other characters.
pub fn clean_name(name: &str) -> Result<String, DocError> {
    let name = name.trim();
    let ok = !name.is_empty()
        && name.chars().count() <= 80
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == ' ' || c == '-' || c == '_')
        && SafePath::new(name).is_ok();
    if ok {
        Ok(name.to_owned())
    } else {
        Err(DocError::BadName)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use printnest_model::project::{Crop, Element, Rect};

    fn tmp(tag: &str) -> PathBuf {
        // nosemgrep: rust.lang.security.temp-dir.temp-dir -- test-only scratch folder
        let d = std::env::temp_dir().join(format!("printnest-doc-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn sample(cat: &Catalogue, src: &str) -> Project {
        let mut p = Project::new_blank(cat, "print-6x4", "Beach").unwrap();
        p.pages[0].elements.push(Element::Photo {
            id: "p1".into(),
            rect: Rect {
                x: 0.0,
                y: 0.0,
                w: 50.0,
                h: 50.0,
                rotation: 0.0,
            },
            src: Some(SafePath::new(src).unwrap()),
            crop: Crop::default(),
        });
        p
    }

    #[test]
    fn save_and_open_round_trip() {
        let cat = Catalogue::builtin().unwrap();
        let base = tmp("roundtrip");
        let photo_src = base.join("IMG_1.jpg");
        fs::write(&photo_src, b"\xff\xd8\xffjpegish").unwrap();

        let folder = ProjectFolder::create(&base, "Holiday 2026").unwrap();
        let dest = SafePath::new("photos/IMG_1.jpg").unwrap();
        folder.import_photo(&dest, &photo_src).unwrap();
        let p = sample(&cat, "photos/IMG_1.jpg");
        folder.save(&p, &cat).unwrap();
        // Saving again replaces atomically.
        folder.save(&p, &cat).unwrap();
        let leftovers = fs::read_dir(folder.root())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp")
            })
            .count();
        assert_eq!(leftovers, 0);

        let (f2, loaded) = open(&folder.root().join(PROJECT_FILE), &cat).unwrap();
        assert_eq!(loaded, p);
        assert_eq!(f2.read_photo(&dest).unwrap(), b"\xff\xd8\xffjpegish");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn never_overwrites_an_existing_project_folder() {
        let base = tmp("exists");
        ProjectFolder::create(&base, "Book").unwrap();
        assert!(matches!(
            ProjectFolder::create(&base, "Book"),
            Err(DocError::Exists)
        ));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn rejects_bad_names() {
        for n in [
            "",
            "   ",
            "../x",
            "a/b",
            "CON",
            "x:y",
            "a\u{202e}b",
            &"x".repeat(81),
        ] {
            assert!(clean_name(n).is_err(), "{n:?}");
        }
        assert_eq!(clean_name("  My Book 2026 ").unwrap(), "My Book 2026");
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escapes_are_refused() {
        let cat = Catalogue::builtin().unwrap();
        let base = tmp("symlink");
        let secret = base.join("secret.txt");
        fs::write(&secret, b"top secret").unwrap();
        let folder = ProjectFolder::create(&base, "Evil").unwrap();

        // A photo that is really a symlink to a file outside the project.
        std::os::unix::fs::symlink(&secret, folder.root().join("photos/cat.jpg")).unwrap();
        // A photos-folder symlink to outside.
        std::os::unix::fs::symlink(&base, folder.root().join("photos/out")).unwrap();

        let leak = SafePath::new("photos/cat.jpg").unwrap();
        assert!(matches!(folder.read_photo(&leak), Err(DocError::Escape(_))));
        let via_dir = SafePath::new("photos/out/secret.txt").unwrap();
        assert!(matches!(
            folder.read_photo(&via_dir),
            Err(DocError::Escape(_))
        ));
        // Importing through the escaping directory is refused too.
        let src = base.join("src.jpg");
        fs::write(&src, b"x").unwrap();
        let sneaky = SafePath::new("photos/out/new.jpg").unwrap();
        assert!(matches!(
            folder.import_photo(&sneaky, &src),
            Err(DocError::Escape(_))
        ));
        assert!(!base.join("new.jpg").exists());

        // And a project referencing it won't even open.
        let p = sample(&cat, "photos/cat.jpg");
        let json = p.to_json().unwrap();
        fs::write(folder.root().join(PROJECT_FILE), json).unwrap();
        assert!(matches!(
            open(folder.root(), &cat),
            Err(DocError::Escape(_))
        ));
        let _ = fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn project_file_symlink_is_refused() {
        let cat = Catalogue::builtin().unwrap();
        let base = tmp("projlink");
        let folder = ProjectFolder::create(&base, "Linked").unwrap();
        let elsewhere = base.join("other.json");
        fs::write(
            &elsewhere,
            Project::new_blank(&cat, "print-6x4", "x")
                .unwrap()
                .to_json()
                .unwrap(),
        )
        .unwrap();
        std::os::unix::fs::symlink(&elsewhere, folder.root().join(PROJECT_FILE)).unwrap();
        assert!(matches!(folder.load(&cat), Err(DocError::NotAProject)));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn open_requires_a_project_file() {
        let cat = Catalogue::builtin().unwrap();
        let base = tmp("notproj");
        let f = base.join("random.json");
        fs::write(&f, b"{}").unwrap();
        assert!(matches!(open(&f, &cat), Err(DocError::NotAProject)));
        let empty = ProjectFolder::create(&base, "Empty").unwrap();
        assert!(open(empty.root(), &cat).is_err());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn import_never_overwrites() {
        let base = tmp("noover");
        let folder = ProjectFolder::create(&base, "P").unwrap();
        let a = base.join("a.jpg");
        let b = base.join("b.jpg");
        fs::write(&a, b"AAA").unwrap();
        fs::write(&b, b"BBB").unwrap();
        let dest = SafePath::new("photos/x.jpg").unwrap();
        folder.import_photo(&dest, &a).unwrap();
        // A second import to the same name is refused, never silently reused.
        assert!(matches!(
            folder.import_photo(&dest, &b),
            Err(DocError::Exists)
        ));
        assert_eq!(folder.read_photo(&dest).unwrap(), b"AAA");
        let _ = fs::remove_dir_all(&base);
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_inside_the_folder_are_refused_too() {
        // Many names pointing at one file inside the project would multiply
        // memory use; any symlink is refused.
        let base = tmp("innerlink");
        let folder = ProjectFolder::create(&base, "P").unwrap();
        fs::write(folder.root().join("photos/real.jpg"), b"x").unwrap();
        std::os::unix::fs::symlink(
            folder.root().join("photos/real.jpg"),
            folder.root().join("photos/alias.jpg"),
        )
        .unwrap();
        let alias = SafePath::new("photos/alias.jpg").unwrap();
        assert!(matches!(
            folder.read_photo(&alias),
            Err(DocError::Escape(_))
        ));
        let real = SafePath::new("photos/real.jpg").unwrap();
        assert_eq!(folder.read_photo(&real).unwrap(), b"x");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn stray_temp_folder_does_not_block_saving() {
        let cat = Catalogue::builtin().unwrap();
        let base = tmp("straytmp");
        let folder = ProjectFolder::create(&base, "P").unwrap();
        fs::create_dir(folder.root().join("project.json.tmp")).unwrap();
        let p = Project::new_blank(&cat, "print-6x4", "x").unwrap();
        folder.save(&p, &cat).unwrap();
        assert_eq!(folder.load(&cat).unwrap(), p);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn a_saved_project_always_reopens() {
        use printnest_model::limits::MAX_TEXT_CHARS;
        use printnest_model::project::Align;
        let cat = Catalogue::builtin().unwrap();
        let base = tmp("toolarge");
        let folder = ProjectFolder::create(&base, "P").unwrap();
        let mut p = Project::new_blank(&cat, "poster-a4", "x").unwrap();
        // Close to the limit in compact form: must save and reopen.
        for i in 0..160 {
            p.pages[0].elements.push(Element::Text {
                id: format!("t{i}"),
                rect: Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 10.0,
                    h: 10.0,
                    rotation: 0.0,
                },
                text: "\u{00e9}".repeat(MAX_TEXT_CHARS),
                font: "sans".into(),
                size_pt: 12.0,
                colour: "#000000".into(),
                align: Align::Left,
            });
        }
        p.validate(&cat).unwrap();
        folder.save(&p, &cat).unwrap();
        assert_eq!(folder.load(&cat).unwrap(), p);
        // Over the limit: refused, and the good file on disk is untouched.
        let mut big = Project::new_blank(&cat, "book-20x20", "big").unwrap();
        let mut n = 0;
        for page in big.pages.iter_mut().take(6) {
            for el in &p.pages[0].elements {
                let mut el = el.clone();
                if let Element::Text { id, .. } = &mut el {
                    *id = format!("e{n}");
                }
                n += 1;
                page.elements.push(el);
            }
        }
        big.validate(&cat).unwrap();
        assert!(matches!(
            folder.save(&big, &cat),
            Err(DocError::TooLarge(_))
        ));
        assert_eq!(folder.load(&cat).unwrap(), p);
        let _ = fs::remove_dir_all(&base);
    }
}
