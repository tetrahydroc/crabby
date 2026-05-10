//! iced-based launcher UI for crabby-loader.
//!
//! # Crate layout
//!
//! - [`app`] - top-level `App` state + message dispatch. Owns the
//!   currently-selected tab and shared state (game dir, manifest, etc.).
//! - [`tabs`] - per-tab views and update handlers. Each tab is a
//!   self-contained module that the top-level dispatcher routes
//!   messages into.
//!
//! # Error convention
//!
//! UI-surface failures convert into [`CrabbyError::Platform`] (window
//! creation, renderer init) or propagate a [`CrabbyError::Bake`]
//! unchanged when the underlying bake pipeline fails. The UI's only
//! job is to surface the error chain.
//!
//! [`CrabbyError::Platform`]: crabby_error::CrabbyError::Platform
//! [`CrabbyError::Bake`]: crabby_error::CrabbyError::Bake

#![deny(missing_docs)]

pub mod app;
pub mod launcher_config;
pub mod modpack_ui;
pub mod open;
pub mod profile_modal;
pub mod quick_theme;
pub mod style;
pub mod tabs;
pub mod theme;

pub use app::{App, Message};
pub use launcher_config::LauncherConfig;
pub use theme::{CrabbyTheme, Mode, Palette};
