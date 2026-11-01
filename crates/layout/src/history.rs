//! Undo / redo as snapshots of the project, limited by step count and by an
//! estimate of the memory the snapshots use (a large project gets fewer
//! undo steps rather than using gigabytes).

use printnest_model::project::{Element, Project};
use printnest_model::safe_path::SafePath;
use std::collections::VecDeque;

/// Most undo steps kept.
pub const MAX_UNDO: usize = 200;
/// Rough memory budget for all undo + redo snapshots together.
pub const MAX_HISTORY_BYTES: usize = 256 * 1024 * 1024;

/// Approximate heap + inline size of a project snapshot. Cheap: no
/// serialisation, just lengths.
#[must_use]
pub fn approx_size(p: &Project) -> usize {
    let el = |e: &Element| -> usize {
        let fixed = std::mem::size_of::<Element>() + e.id().len();
        match e {
            Element::Photo { src, .. } => fixed + src.as_ref().map_or(0, |s| s.as_str().len()),
            Element::Text {
                text, font, colour, ..
            } => fixed + text.len() + font.len() + colour.len(),
        }
    };
    let pages: usize = p
        .pages
        .iter()
        .map(|pg| 64 + pg.background.len() + pg.elements.iter().map(el).sum::<usize>())
        .sum();
    256 + p.title.len() + p.product.len() + p.paper.len() + pages
}

/// Undo/redo stacks.
#[derive(Debug, Default, Clone)]
pub struct History {
    past: VecDeque<(Project, usize)>,
    future: Vec<(Project, usize)>,
    bytes: usize,
}

impl History {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Records `before` as an undo point. Clears redo.
    pub fn record(&mut self, before: &Project) {
        if self.past.back().map(|(p, _)| p) == Some(before) {
            return;
        }
        for (_, size) in self.future.drain(..) {
            self.bytes = self.bytes.saturating_sub(size);
        }
        let size = approx_size(before);
        self.past.push_back((before.clone(), size));
        self.bytes = self.bytes.saturating_add(size);
        // Drop the oldest steps, but always keep the newest one.
        while self.past.len() > 1 && (self.past.len() > MAX_UNDO || self.bytes > MAX_HISTORY_BYTES)
        {
            if let Some((_, s)) = self.past.pop_front() {
                self.bytes = self.bytes.saturating_sub(s);
            }
        }
    }

    /// Steps back, returning the project to show. `current` goes to redo.
    pub fn undo(&mut self, current: &Project) -> Option<Project> {
        let (prev, size) = self.past.pop_back()?;
        self.bytes = self.bytes.saturating_sub(size);
        let cs = approx_size(current);
        self.future.push((current.clone(), cs));
        self.bytes = self.bytes.saturating_add(cs);
        Some(prev)
    }

    /// Steps forward again.
    pub fn redo(&mut self, current: &Project) -> Option<Project> {
        let (next, size) = self.future.pop()?;
        self.bytes = self.bytes.saturating_sub(size);
        let cs = approx_size(current);
        self.past.push_back((current.clone(), cs));
        self.bytes = self.bytes.saturating_add(cs);
        Some(next)
    }

    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }

    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }

    /// Renames a photo in every snapshot, so undo/redo after a save that
    /// stored a photo under a new name keeps pointing at the right file.
    pub fn retarget_photo(&mut self, old: &SafePath, new: &SafePath) {
        for (p, _) in self.past.iter_mut().chain(self.future.iter_mut()) {
            crate::edit::retarget_photo(p, old, new);
        }
    }

    /// Estimated memory held by snapshots.
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn clear(&mut self) {
        self.past.clear();
        self.future.clear();
        self.bytes = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use printnest_model::Catalogue;
    use printnest_model::limits::MAX_TEXT_CHARS;
    use printnest_model::project::{Align, Rect};

    #[test]
    fn undo_redo_round_trip() {
        let cat = Catalogue::builtin().unwrap();
        let a = Project::new_blank(&cat, "print-6x4", "a").unwrap();
        let mut b = a.clone();
        b.title = "b".into();
        let mut c = a.clone();
        c.title = "c".into();

        let mut h = History::new();
        h.record(&a);
        h.record(&b);
        // current is c
        let back = h.undo(&c).unwrap();
        assert_eq!(back.title, "b");
        let back2 = h.undo(&back).unwrap();
        assert_eq!(back2.title, "a");
        assert!(!h.can_undo());
        let fwd = h.redo(&back2).unwrap();
        assert_eq!(fwd.title, "b");
        // A new edit clears redo.
        h.record(&fwd);
        assert!(!h.can_redo());
    }

    #[test]
    fn duplicate_snapshots_are_skipped_and_history_is_capped() {
        let cat = Catalogue::builtin().unwrap();
        let mut p = Project::new_blank(&cat, "print-6x4", "x").unwrap();
        let mut h = History::new();
        h.record(&p);
        h.record(&p);
        let mut steps = 0;
        let mut cur = p.clone();
        while let Some(prev) = h.undo(&cur) {
            cur = prev;
            steps += 1;
        }
        assert_eq!(steps, 1);
        for i in 0..(MAX_UNDO + 50) {
            p.title = format!("t{i}");
            h.record(&p);
        }
        assert_eq!(h.past.len(), MAX_UNDO);
    }

    #[test]
    fn retarget_reaches_undo_and_redo_snapshots() {
        use printnest_model::project::{Crop, Rect};
        let cat = Catalogue::builtin().unwrap();
        let old = SafePath::new("photos/a.jpg").unwrap();
        let new = SafePath::new("photos/a-2.jpg").unwrap();
        let mut p = Project::new_blank(&cat, "poster-a4", "x").unwrap();
        p.pages[0].elements.push(Element::Photo {
            id: "p".into(),
            rect: Rect {
                x: 0.0,
                y: 0.0,
                w: 10.0,
                h: 10.0,
                rotation: 0.0,
            },
            src: Some(old.clone()),
            crop: Crop::default(),
        });
        let mut h = History::new();
        // Undo point holds the old name; the current design is elsewhere.
        h.record(&p);
        let mut cur = p.clone();
        cur.title = "moved".into();
        // Save renamed the photo: caller fixes `cur`, history fixes snapshots.
        crate::edit::retarget_photo(&mut cur, &old, &new);
        h.retarget_photo(&old, &new);
        let back = h.undo(&cur).unwrap();
        assert_eq!(back.photo_paths(), vec![&new]);
        // Redo snapshots are covered too.
        let mut h2 = History::new();
        h2.record(&p);
        let mut cur2 = p.clone();
        cur2.title = "y".into();
        let undone = h2.undo(&cur2).unwrap(); // `cur2` (old name) is now in redo
        let mut undone = undone;
        crate::edit::retarget_photo(&mut undone, &old, &new);
        h2.retarget_photo(&old, &new);
        let fwd = h2.redo(&undone).unwrap();
        assert_eq!(fwd.photo_paths(), vec![&new]);
    }

    #[test]
    fn big_projects_get_fewer_steps_not_gigabytes() {
        let cat = Catalogue::builtin().unwrap();
        let mut p = Project::new_blank(&cat, "poster-a4", "big").unwrap();
        // ~10 MB of text in one project.
        for i in 0..1000 {
            p.pages[0].elements.push(Element::Text {
                id: format!("t{i}"),
                rect: Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 10.0,
                    h: 10.0,
                    rotation: 0.0,
                },
                text: "x".repeat(MAX_TEXT_CHARS),
                font: "sans".into(),
                size_pt: 12.0,
                colour: "#000000".into(),
                align: Align::Left,
            });
        }
        let size = approx_size(&p);
        assert!(size > 10_000_000);
        let mut h = History::new();
        for i in 0..MAX_UNDO {
            p.title = format!("t{i}");
            h.record(&p);
        }
        assert!(h.bytes() <= MAX_HISTORY_BYTES, "{}", h.bytes());
        assert!(h.past.len() < MAX_UNDO);
        assert!(h.can_undo());
    }
}
