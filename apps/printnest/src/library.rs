//! Photos used by the open project, decoded in the background through the
//! sandboxed worker (never in this process).
//!
//! Decoding is on demand: a photo is only decoded when it is about to be
//! drawn ([`Library::want`]), and on-screen textures share a memory budget.
//! When the budget is full, photos not drawn recently are dropped and
//! decoded again if needed, so a project with thousands of photos can't
//! exhaust memory.

use eframe::egui;
use printnest_document::ProjectFolder;
use printnest_model::safe_path::SafePath;
use printnest_sandbox::{Preview, Worker, read_capped};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};

/// Longest edge of the on-screen preview kept for each photo.
const PREVIEW_EDGE: u32 = 1600;
/// Memory budget for decoded previews (RGBA bytes).
const TEXTURE_BUDGET: usize = 768 * 1024 * 1024;
/// Photos drawn within this many frames are never evicted.
const KEEP_FRAMES: u64 = 2;
/// Most memory one preview can take (RGBA at the preview edge).
const MAX_PREVIEW_BYTES: usize = (PREVIEW_EDGE as usize) * (PREVIEW_EDGE as usize) * 4;

/// Where a photo's bytes come from.
#[derive(Debug, Clone)]
pub enum Origin {
    /// Picked from disk; copied into the project folder on save.
    External(PathBuf),
    /// Inside a project folder (this one, or the one it was opened from).
    Project(ProjectFolder),
}

pub enum State {
    /// Known but not decoded (yet, or any more).
    Idle,
    Loading,
    Ready {
        texture: egui::TextureHandle,
        bytes: usize,
    },
    Failed(String),
}

impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            State::Idle => f.write_str("Idle"),
            State::Loading => f.write_str("Loading"),
            State::Ready { bytes, .. } => write!(f, "Ready({bytes} bytes)"),
            State::Failed(e) => write!(f, "Failed({e})"),
        }
    }
}

#[derive(Debug)]
pub struct Photo {
    pub state: State,
    pub origin: Origin,
    /// Full-resolution size, as displayed (EXIF orientation applied); kept
    /// after eviction so print-quality badges stay correct.
    pub size: Option<(u32, u32)>,
    last_used: u64,
    /// Ticket of the decode in flight; answers with another ticket are
    /// stale (e.g. for a photo that has since been renamed) and ignored.
    ticket: u64,
}

impl Photo {
    /// The texture, if decoded.
    #[must_use]
    pub fn texture(&self) -> Option<&egui::TextureHandle> {
        match &self.state {
            State::Ready { texture, .. } => Some(texture),
            _ => None,
        }
    }
}

struct Job {
    key: SafePath,
    origin: Origin,
    ticket: u64,
}

struct Done {
    key: SafePath,
    ticket: u64,
    result: Result<Preview, String>,
}

pub struct Library {
    photos: BTreeMap<String, (SafePath, Photo)>,
    jobs: Sender<Job>,
    done: Receiver<Done>,
    frame: u64,
    texture_bytes: usize,
    /// Decodes queued or running.
    pending: usize,
    next_ticket: u64,
}

impl std::fmt::Debug for Library {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Library")
            .field("photos", &self.photos.len())
            .field("texture_bytes", &self.texture_bytes)
            .finish()
    }
}

impl Origin {
    /// The file on disk holding `key`'s bytes.
    ///
    /// # Errors
    /// If the project path escapes its folder.
    pub fn path(&self, key: &SafePath) -> Result<PathBuf, String> {
        match self {
            Origin::External(p) => Ok(p.clone()),
            Origin::Project(folder) => folder.resolve(key).map_err(|e| e.to_string()),
        }
    }
}

fn load(job: &Job) -> Result<Preview, String> {
    let bytes = match &job.origin {
        Origin::External(path) => read_capped(path).map_err(|e| e.to_string())?,
        Origin::Project(folder) => folder.read_photo(&job.key).map_err(|e| e.to_string())?,
    };
    let worker = Worker::current_exe().map_err(|e| e.to_string())?;
    worker
        .preview(&bytes, PREVIEW_EDGE)
        .map_err(|e| e.to_string())
}

impl Library {
    #[must_use]
    pub fn new() -> Self {
        let (jobs, job_rx) = channel::<Job>();
        let (done_tx, done) = channel::<Done>();
        // One background thread decodes photos one at a time (each decode
        // is its own sandboxed process).
        std::thread::spawn(move || {
            while let Ok(job) = job_rx.recv() {
                let result = load(&job);
                if done_tx
                    .send(Done {
                        key: job.key,
                        ticket: job.ticket,
                        result,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Library {
            photos: BTreeMap::new(),
            jobs,
            done,
            frame: 0,
            texture_bytes: 0,
            pending: 0,
            next_ticket: 1,
        }
    }

    /// Registers `key` (decoded later, when first drawn).
    pub fn add(&mut self, key: SafePath, origin: Origin) {
        if self.photos.contains_key(key.as_str()) {
            return;
        }
        self.photos.insert(
            key.as_str().to_owned(),
            (
                key,
                Photo {
                    state: State::Idle,
                    origin,
                    size: None,
                    last_used: 0,
                    ticket: 0,
                },
            ),
        );
    }

    /// Marks `key` as drawn this frame and starts decoding it if needed
    /// and there is room in the memory budget (counting decodes in flight
    /// at their largest possible size).
    pub fn want(&mut self, key: &str) {
        let frame = self.frame;
        let committed = self
            .texture_bytes
            .saturating_add(self.pending.saturating_mul(MAX_PREVIEW_BYTES));
        let room = committed.saturating_add(MAX_PREVIEW_BYTES) <= TEXTURE_BUDGET;
        let Some((k, photo)) = self.photos.get_mut(key) else {
            return;
        };
        photo.last_used = frame;
        if room && matches!(photo.state, State::Idle) {
            self.pending = self.pending.saturating_add(1);
            let ticket = self.next_ticket;
            self.next_ticket = self.next_ticket.wrapping_add(1);
            photo.ticket = ticket;
            photo.state = State::Loading;
            let _ = self.jobs.send(Job {
                key: k.clone(),
                origin: photo.origin.clone(),
                ticket,
            });
        }
    }

    /// Picks up finished decodes and keeps within the memory budget. Call
    /// once per frame, before drawing.
    pub fn poll(&mut self, ctx: &egui::Context) {
        self.frame = self.frame.wrapping_add(1);
        while let Ok(done) = self.done.try_recv() {
            // Every job sent produces exactly one answer.
            self.pending = self.pending.saturating_sub(1);
            let Some((_, photo)) = self.photos.get_mut(done.key.as_str()) else {
                continue;
            };
            if !matches!(photo.state, State::Loading) || photo.ticket != done.ticket {
                continue;
            }
            photo.state = match done.result {
                Ok(p) => {
                    let size = [p.width as usize, p.height as usize];
                    let bytes = p.rgba.len();
                    let image = egui::ColorImage::from_rgba_unmultiplied(size, &p.rgba);
                    photo.size = Some((p.info.width, p.info.height));
                    self.texture_bytes = self.texture_bytes.saturating_add(bytes);
                    State::Ready {
                        texture: ctx.load_texture(
                            done.key.as_str(),
                            image,
                            egui::TextureOptions::LINEAR,
                        ),
                        bytes,
                    }
                }
                Err(e) => State::Failed(e),
            };
        }
        self.evict();
        if self.loading() > 0 {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    /// Drops least-recently-drawn textures to keep room for new decodes.
    fn evict(&mut self) {
        let limit = TEXTURE_BUDGET.saturating_sub(MAX_PREVIEW_BYTES);
        if self.texture_bytes <= limit {
            return;
        }
        let mut ready: Vec<(u64, String)> = self
            .photos
            .iter()
            .filter(|(_, (_, p))| matches!(p.state, State::Ready { .. }))
            .filter(|(_, (_, p))| self.frame.saturating_sub(p.last_used) > KEEP_FRAMES)
            .map(|(k, (_, p))| (p.last_used, k.clone()))
            .collect();
        ready.sort_unstable();
        for (_, key) in ready {
            if self.texture_bytes <= limit {
                break;
            }
            if let Some((_, p)) = self.photos.get_mut(&key)
                && let State::Ready { bytes, .. } = p.state
            {
                self.texture_bytes = self.texture_bytes.saturating_sub(bytes);
                p.state = State::Idle;
            }
        }
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Photo> {
        self.photos.get(key).map(|(_, p)| p)
    }

    #[must_use]
    pub fn keys(&self) -> std::collections::HashSet<String> {
        self.photos.keys().cloned().collect()
    }

    /// All keys in display order.
    #[must_use]
    pub fn list(&self) -> Vec<SafePath> {
        self.photos.values().map(|(k, _)| k.clone()).collect()
    }

    #[must_use]
    pub fn loading(&self) -> usize {
        self.photos
            .values()
            .filter(|(_, p)| matches!(p.state, State::Loading))
            .count()
    }

    /// Where `key`'s bytes currently live.
    #[must_use]
    pub fn origin(&self, key: &str) -> Option<&Origin> {
        self.photos.get(key).map(|(_, p)| &p.origin)
    }

    /// Records that `key` is now stored in `folder` as `stored`.
    pub fn saved_as(&mut self, key: &SafePath, stored: &SafePath, folder: &ProjectFolder) {
        let Some((_, mut photo)) = self.photos.remove(key.as_str()) else {
            return;
        };
        photo.origin = Origin::Project(folder.clone());
        if stored != key && matches!(photo.state, State::Loading) {
            // The answer will arrive under the old name; decode again later.
            photo.state = State::Idle;
        }
        self.photos
            .insert(stored.as_str().to_owned(), (stored.clone(), photo));
    }
}
