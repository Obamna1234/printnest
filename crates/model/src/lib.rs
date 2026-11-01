//! PrintNest data model.
//!
//! This crate holds the parts of PrintNest that every other crate depends on:
//!
//! - [`units`]: millimetre-based measurement and print-resolution maths
//! - [`limits`]: hard limits applied to all untrusted input (SPEC section 7)
//! - [`safe_path`]: validation of file paths stored inside project files
//! - [`catalogue`]: the product catalogue, loaded from embedded TOML data
//! - [`project`]: the project manifest format and its validation
//!
//! Nothing in this crate performs I/O on its own apart from the size-capped
//! readers in [`project`], and nothing here can panic on malformed input.

#![forbid(unsafe_code)]

pub mod catalogue;
pub mod limits;
pub mod project;
pub mod safe_path;
pub mod units;

pub use catalogue::{Catalogue, CatalogueError, Family, Paper, Product};
pub use project::{Project, ProjectError};
pub use safe_path::{SafePath, SafePathError};
pub use units::Mm;
