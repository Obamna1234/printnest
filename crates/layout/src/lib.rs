//! Editor logic with no UI: everything here is plain maths on the project
//! model, so it can be tested exhaustively without a window.
//!
//! Coordinates are millimetres relative to the page's trim top-left corner
//! (the same convention as [`printnest_model::project::Rect`]).

#![forbid(unsafe_code)]

pub mod crop;
pub mod edit;
pub mod geometry;
pub mod history;
pub mod snap;

pub use crop::{CropView, effective_ppi};
pub use geometry::{Handle, Pt, hit_handle, hit_test};
pub use history::History;
pub use snap::{Guide, SnapResult, snap_move, snap_resize};
