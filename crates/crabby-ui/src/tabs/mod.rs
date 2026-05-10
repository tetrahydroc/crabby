//! Per-tab views and update handlers.
//!
//! Each tab is its own module owning its [`State`] type plus a
//! `Message` enum and `update` / `view` functions. The top-level
//! [`App`](crate::App) routes messages by variant; tabs never reach
//! into other tabs' state.
//!
//! Adding a tab:
//!
//! 1. New module under `tabs/`.
//! 2. Pub-use its `Message` and `State`.
//! 3. New variant on [`Tab`] + matching match arm in `App::view`.
//! 4. New variant on [`crate::Message`] forwarding to the tab's
//!    `Message`.

pub mod diagnostics;
pub mod logs;
pub mod mods;
pub mod profiles;
pub mod saves;
pub mod settings;

/// Discriminator for the active tab. Drives both the tab strip and
/// the body render. Mirrors the four tabs in the design mockup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    /// Mods list - unified installed + (later) available; toggle, install.
    #[default]
    Mods,
    /// Per-profile save slots + backups.
    Saves,
    /// Filterable session log.
    Logs,
    /// Application + bake settings (will host diagnostics until that
    /// gets its own dedicated tab).
    Settings,
}
