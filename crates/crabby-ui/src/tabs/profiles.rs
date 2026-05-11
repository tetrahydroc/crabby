//! Profile state, surfaced through the App shell's profile bar.
//!
//! Per the design mockup, profiles aren't their own tab - the
//! profile is the primary context, surfaced as a name + switcher
//! dropdown in the bar below the tabs. This module owns that state:
//! loading from `mod_config.cfg`, switching the active profile, and
//! exposing display helpers.
//!
//! Switching profiles persists to disk immediately so the loader
//! reads the new active set on next launch.

use std::path::Path;

use crabby_config::ModConfig;

/// Per-tab message. Profiles tab no longer exists in the UI shell;
/// these messages drive the profile bar + the inline editor that
/// pops out from the "Edit profile" button.
#[derive(Debug, Clone)]
pub enum Message {
    /// Profile picked from the dropdown.
    SwitchProfile(String),
    /// Open the Create-profile modal (input cleared, error cleared).
    OpenCreateModal,
    /// Open the Edit-profile modal (input pre-filled with the active
    /// profile's name to allow edit-then-Save without retyping).
    OpenEditModal,
    /// Dismiss whichever modal is open.
    DismissModal,
    /// New-profile / rename-target text input edited.
    EditorInputChanged(String),
    /// Create a new profile from the current input. Becomes the
    /// active profile on success.
    CreateProfile,
    /// Rename the active profile to whatever is in the input.
    /// No-op if the input is empty or matches an existing name.
    RenameActive,
    /// Delete the named profile. Disabled in the UI when it's the
    /// active profile or the last remaining profile.
    DeleteProfile(String),
}

/// Which profile modal is currently visible (if any).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProfileModal {
    /// No modal - the profile bar is the only profile surface.
    #[default]
    None,
    /// Create-new-profile modal.
    Create,
    /// Edit (rename / delete) the active profile.
    Edit,
}

/// Per-tab state.
#[derive(Debug)]
pub struct State {
    /// Bumped via [`invalidate`] to force re-fetch on next view.
    pub generation: u64,
    /// Currently-active profile id.
    pub active: String,
    /// All profiles known to `mod_config.cfg`, sorted alphabetically.
    /// Used to populate the switcher dropdown.
    pub all: Vec<String>,
    /// Cached count of enabled mods in the active profile (for the
    /// stats line on the profile bar). Refreshed every time we
    /// reload the config.
    pub mod_count: usize,
    /// Which profile modal is open (Create / Edit / None).
    pub modal: ProfileModal,
    /// Buffer for the modal's text input - used as the new-profile
    /// name (Create) or as the rename target (Edit).
    pub editor_input: String,
    /// Last error from a profile mutation, surfaced inline in the
    /// modal. Cleared on the next successful action.
    pub editor_error: Option<String>,
    /// Generation that produced the cache. When this lags behind
    /// `generation`, the next [`refresh`] re-reads from disk.
    cache_gen: Option<u64>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            generation: 0,
            active: "default".to_string(),
            all: vec!["default".to_string()],
            mod_count: 0,
            modal: ProfileModal::None,
            editor_input: String::new(),
            editor_error: None,
            cache_gen: None,
        }
    }
}

impl State {
    /// Drop cached state.
    pub fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.cache_gen = None;
    }

    /// Read `mod_config.cfg` and refresh the cached profile list +
    /// active-profile stats. Idempotent; cheap when nothing changed.
    pub fn refresh(&mut self, game_dir: Option<&Path>) {
        if self.cache_gen == Some(self.generation) {
            return;
        }
        let Some(dir) = game_dir else {
            self.cache_gen = Some(self.generation);
            return;
        };
        let cfg = match ModConfig::load_or_default(dir) {
            Ok(cfg) => cfg,
            Err(e) => {
                tracing::warn!(error = %e, "ui: profile state load failed");
                self.cache_gen = Some(self.generation);
                return;
            }
        };
        self.active = cfg.active_profile.clone();
        self.all = cfg.profiles.keys().cloned().collect();
        if self.all.is_empty() {
            self.all.push("default".to_string());
        }
        self.mod_count = cfg
            .profiles
            .get(&self.active)
            .map(|p| p.mods.values().filter(|e| e.enabled).count())
            .unwrap_or(0);
        self.cache_gen = Some(self.generation);
    }

    /// Active profile's display name.
    #[must_use]
    pub fn active_profile_name(&self) -> &str {
        &self.active
    }

    /// Active profile's enabled-mod count.
    #[must_use]
    pub fn active_mod_count(&self) -> usize {
        self.mod_count
    }

    /// All profile names, sorted alphabetically. Drives the
    /// switcher dropdown.
    #[must_use]
    pub fn all_profiles(&self) -> &[String] {
        &self.all
    }

    /// Apply a message.
    pub fn update(&mut self, message: Message, game_dir: Option<&Path>) {
        match message {
            Message::SwitchProfile(name) => {
                if let Some(dir) = game_dir {
                    if let Err(e) = switch_profile(dir, &name) {
                        tracing::warn!(profile = %name, error = %e, "ui: profile switch failed");
                        self.editor_error = Some(format!("switch failed: {e}"));
                        return;
                    }
                }
                self.active = name;
                self.invalidate();
                self.refresh(game_dir);
            }
            Message::OpenCreateModal => {
                self.modal = ProfileModal::Create;
                self.editor_input.clear();
                self.editor_error = None;
            }
            Message::OpenEditModal => {
                self.modal = ProfileModal::Edit;
                // Pre-fill with the active profile's name so a rename
                // is "edit text → Save" rather than "type the existing
                // name from scratch".
                self.editor_input = self.active.clone();
                self.editor_error = None;
            }
            Message::DismissModal => {
                self.modal = ProfileModal::None;
                self.editor_input.clear();
                self.editor_error = None;
            }
            Message::EditorInputChanged(s) => {
                self.editor_input = s;
                self.editor_error = None;
            }
            Message::CreateProfile => {
                let name = sanitize_profile_name(&self.editor_input);
                let Some(name) = name else {
                    self.editor_error = Some("name cannot be empty".into());
                    return;
                };
                let Some(dir) = game_dir else {
                    self.editor_error = Some("set game directory first".into());
                    return;
                };
                if self.all.iter().any(|p| p == &name) {
                    self.editor_error = Some(format!("\"{name}\" already exists"));
                    return;
                }
                match create_profile(dir, &name) {
                    Ok(()) => {
                        self.editor_input.clear();
                        self.editor_error = None;
                        self.modal = ProfileModal::None;
                        self.active = name;
                        self.invalidate();
                        self.refresh(game_dir);
                    }
                    Err(e) => {
                        tracing::warn!(profile = %name, error = %e, "ui: profile create failed");
                        self.editor_error = Some(format!("create failed: {e}"));
                    }
                }
            }
            Message::RenameActive => {
                let new_name = sanitize_profile_name(&self.editor_input);
                let Some(new_name) = new_name else {
                    self.editor_error = Some("name cannot be empty".into());
                    return;
                };
                if new_name == self.active {
                    self.editor_error = Some("same name".into());
                    return;
                }
                let Some(dir) = game_dir else {
                    self.editor_error = Some("set game directory first".into());
                    return;
                };
                if self.all.iter().any(|p| p == &new_name) {
                    self.editor_error = Some(format!("\"{new_name}\" already exists"));
                    return;
                }
                let old_name = self.active.clone();
                match rename_profile(dir, &old_name, &new_name) {
                    Ok(()) => {
                        self.editor_input.clear();
                        self.editor_error = None;
                        self.modal = ProfileModal::None;
                        self.active = new_name;
                        self.invalidate();
                        self.refresh(game_dir);
                    }
                    Err(e) => {
                        tracing::warn!(profile = %old_name, error = %e, "ui: profile rename failed");
                        self.editor_error = Some(format!("rename failed: {e}"));
                    }
                }
            }
            Message::DeleteProfile(name) => {
                if self.all.len() <= 1 {
                    self.editor_error = Some("can't delete the last profile".into());
                    return;
                }
                if name == self.active {
                    self.editor_error = Some("can't delete the active profile".into());
                    return;
                }
                let Some(dir) = game_dir else {
                    self.editor_error = Some("set game directory first".into());
                    return;
                };
                match delete_profile(dir, &name) {
                    Ok(()) => {
                        self.editor_error = None;
                        self.modal = ProfileModal::None;
                        self.invalidate();
                        self.refresh(game_dir);
                    }
                    Err(e) => {
                        tracing::warn!(profile = %name, error = %e, "ui: profile delete failed");
                        self.editor_error = Some(format!("delete failed: {e}"));
                    }
                }
            }
        }
    }
}

/// Persist a new active profile. Creates the profile section if it
/// doesn't already exist (matching the CLI's behavior).
fn switch_profile(game_dir: &Path, name: &str) -> Result<(), crabby_error::CrabbyError> {
    let mut cfg = ModConfig::load_or_default(game_dir)?;
    if !cfg.profiles.contains_key(name) {
        cfg.profiles.insert(name.to_string(), Default::default());
    }
    cfg.active_profile = name.to_string();
    cfg.save(game_dir)?;
    // Don't refresh mod_index here. The next bake will refresh it
    // with full overlay-source info. Refreshing here without that
    // info would overwrite the persisted overlay metadata and break
    // the runtime cache strip for overlay-bearing mods. The runtime
    // shim falls back to per-id targeted scanning when the index is
    // missing or stale, so launching pre-bake against the new
    // profile still works.
    Ok(())
}

/// Create a new profile and make it active. Caller must guard against
/// the duplicate-name case before calling - the duplicate is still
/// an error here if the config-file race produces one, but the UI
/// checks first so the error path is rare.
fn create_profile(game_dir: &Path, name: &str) -> Result<(), crabby_error::CrabbyError> {
    let mut cfg = ModConfig::load_or_default(game_dir)?;
    if cfg.profiles.contains_key(name) {
        return Err(crabby_error::CrabbyError::Platform {
            context: format!("profile {name:?} already exists"),
            source: "duplicate profile".into(),
        });
    }
    cfg.profiles.insert(name.to_string(), Default::default());
    cfg.active_profile = name.to_string();
    cfg.save(game_dir)?;
    // Don't refresh mod_index here, the bake does it. See use_profile.
    Ok(())
}

/// Rename a profile in-place. Preserves the mod set verbatim. Updates
/// `active_profile` if the renamed one was active.
fn rename_profile(
    game_dir: &Path,
    old_name: &str,
    new_name: &str,
) -> Result<(), crabby_error::CrabbyError> {
    let mut cfg = ModConfig::load_or_default(game_dir)?;
    if cfg.profiles.contains_key(new_name) {
        return Err(crabby_error::CrabbyError::Platform {
            context: format!("profile {new_name:?} already exists"),
            source: "duplicate profile".into(),
        });
    }
    let Some(profile) = cfg.profiles.remove(old_name) else {
        return Err(crabby_error::CrabbyError::Platform {
            context: format!("profile {old_name:?} not found"),
            source: "unknown profile".into(),
        });
    };
    cfg.profiles.insert(new_name.to_string(), profile);
    if cfg.active_profile == old_name {
        cfg.active_profile = new_name.to_string();
    }
    cfg.save(game_dir)?;
    Ok(())
}

/// Delete a profile. Caller must ensure it's not the active or
/// last-remaining profile - double-checked here so the persisted
/// state can't end up dangling, but the UI presents the error first.
fn delete_profile(game_dir: &Path, name: &str) -> Result<(), crabby_error::CrabbyError> {
    let mut cfg = ModConfig::load_or_default(game_dir)?;
    if cfg.active_profile == name {
        return Err(crabby_error::CrabbyError::Platform {
            context: format!("can't delete active profile {name:?}"),
            source: "active profile".into(),
        });
    }
    if cfg.profiles.len() <= 1 {
        return Err(crabby_error::CrabbyError::Platform {
            context: "can't delete the last remaining profile".into(),
            source: "single profile".into(),
        });
    }
    if cfg.profiles.remove(name).is_none() {
        return Err(crabby_error::CrabbyError::Platform {
            context: format!("profile {name:?} not found"),
            source: "unknown profile".into(),
        });
    }
    cfg.save(game_dir)?;
    Ok(())
}

/// Trim the editor input and reject empty/whitespace-only / control-char
/// names. The TOML config can technically hold any quoted string, but
/// keeping names to printable, non-whitespace-bookended values avoids
/// surprises in the CLI and the dropdown.
fn sanitize_profile_name(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.chars().any(|c| c.is_control()) {
        return None;
    }
    Some(trimmed.to_string())
}
