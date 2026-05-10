//! Persistent launcher preferences (user-level, not per-install).
//!
//! Stores the manually-picked game-directory path plus any future UI
//! preferences (theme, density, last-active-tab) that should survive
//! restarts.
//!
//! Location: the platform's user-config dir per the `directories`
//! crate, plus the `crabby` subdir:
//!
//! - Linux: `~/.config/crabby/launcher.toml`
//! - macOS: `~/Library/Application Support/crabby/launcher.toml`
//! - Windows: `%APPDATA%\crabby\launcher.toml`
//!
//! All fields are optional and the file-missing case is "all
//! defaults," so a fresh install needs no setup.

use std::fs;
use std::path::PathBuf;

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

/// Launcher-level user prefs. Fields are flat and additive - adding a
/// new one is a no-op for existing config files thanks to serde's
/// `default` on missing keys.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LauncherConfig {
    /// Manually-picked game directory. `None` = use auto-detect (see
    /// `crabby_install::detect_game_dir`). Persisted when a directory
    /// is picked via the first-run picker.
    #[serde(default)]
    pub game_dir: Option<PathBuf>,
    /// Persisted theme preferences - mode, density, accent + bg-tint
    /// hues, and saved palette swatches. Defaults match the design's
    /// dark/teal/cool starting state.
    #[serde(default)]
    pub theme: ThemePrefs,
    /// Per-profile last-played timestamp (Unix seconds). Set on the
    /// `LaunchFinished(Spawned)` arm - failed launches don't tick.
    /// `BTreeMap` for deterministic toml on-disk ordering.
    #[serde(default)]
    pub last_played: std::collections::BTreeMap<String, u64>,
    /// Per-mod override of the conflicts panel's collapsed state on
    /// the mod detail page. Only stores explicit user toggles -
    /// missing entries fall back to the default (open when the mod
    /// has any Hard conflict, closed otherwise). Lets future severity
    /// shifts re-default sensibly without losing user preferences for
    /// explicitly-collapsed mods.
    #[serde(default)]
    pub mod_conflict_panel_collapsed: std::collections::BTreeMap<String, bool>,
    /// Gate destructive UI actions (slot move, future delete flows)
    /// behind an explicit confirmation step. Default true - matches
    /// "safe by default." Power users can flip this off in
    /// Settings → General to remove the extra click.
    #[serde(default = "default_confirm_destructive_actions")]
    pub confirm_destructive_actions: bool,
}

fn default_confirm_destructive_actions() -> bool {
    true
}

/// Persisted theme state. Mirrors the dynamic axes in
/// [`crate::theme::CrabbyTheme`] plus a saved-colors store and per-tone
/// pill overrides.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemePrefs {
    /// `"dark"` or `"light"`. Stored as String so future modes can land
    /// without breaking older configs.
    #[serde(default = "default_mode")]
    pub mode: String,
    /// Accent OKLCH lightness ([0.40, 0.85] practical range).
    #[serde(default = "default_accent_l")]
    pub accent_l: f32,
    /// Accent OKLCH chroma ([0.0, 0.30] practical range).
    #[serde(default = "default_accent_c")]
    pub accent_c: f32,
    /// Accent OKLCH hue in degrees [0, 360).
    #[serde(default = "default_accent_h")]
    pub accent_h: f32,
    /// Background-tint hue in degrees [0, 360).
    #[serde(default = "default_bg_tint_h")]
    pub bg_tint_h: f32,
    /// User-saved accent presets, `(L, C, H)`. Capped at
    /// [`MAX_SAVED_COLORS`] by the UI on add.
    #[serde(default = "default_saved_colors")]
    pub saved_colors: Vec<[f32; 3]>,
    /// Per-tone pill overrides. Key is the tone name (`"ok"`, `"accent"`,
    /// `"err"`, `"warn"`, `"neutral"`); value is `(L, C, H)`. Missing
    /// keys fall back to the built-in tone color. Stored as a BTreeMap
    /// so toml round-trip stays deterministic across runs.
    #[serde(default)]
    pub pill_overrides: std::collections::BTreeMap<String, [f32; 3]>,
}

/// Maximum number of swatches the saved-colors row holds. Older entries
/// are dropped when adding past this cap.
pub const MAX_SAVED_COLORS: usize = 12;

fn default_mode() -> String { "dark".into() }
fn default_accent_l() -> f32 { 0.72 }
fn default_accent_c() -> f32 { 0.12 }
fn default_accent_h() -> f32 { 220.0 }
fn default_bg_tint_h() -> f32 { 240.0 }

/// Curated starter palette - the design's 9 hue presets at canonical
/// L/C. Pre-seeded so the saved-colors row isn't empty on first launch.
fn default_saved_colors() -> Vec<[f32; 3]> {
    [220.0, 200.0, 165.0, 130.0, 80.0, 30.0, 350.0, 290.0, 260.0]
        .into_iter()
        .map(|h| [0.72_f32, 0.12_f32, h])
        .collect()
}

impl Default for ThemePrefs {
    fn default() -> Self {
        Self {
            mode: default_mode(),
            accent_l: default_accent_l(),
            accent_c: default_accent_c(),
            accent_h: default_accent_h(),
            bg_tint_h: default_bg_tint_h(),
            saved_colors: default_saved_colors(),
            pill_overrides: std::collections::BTreeMap::new(),
        }
    }
}

impl LauncherConfig {
    /// Load the persisted config. Returns the default config when no
    /// file exists or the file fails to parse - parse errors never
    /// bubble to the UI because a malformed config shouldn't block
    /// startup.
    #[must_use]
    pub fn load() -> Self {
        let Some(path) = config_path() else {
            return Self::default();
        };
        if !path.is_file() {
            return Self::default();
        }
        match fs::read_to_string(&path) {
            Ok(s) => match toml::from_str::<Self>(&s) {
                Ok(cfg) => {
                    debug!(path = %path.display(), "ui: launcher config loaded");
                    cfg
                }
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "ui: launcher config parse failed; using defaults");
                    Self::default()
                }
            },
            Err(e) => {
                warn!(path = %path.display(), error = %e, "ui: launcher config read failed; using defaults");
                Self::default()
            }
        }
    }

    /// Persist to disk. Best-effort - write failures log a warning
    /// rather than bubbling, since the launcher is functional without
    /// persistence (next restart just re-runs auto-detect).
    pub fn save(&self) {
        let Some(path) = config_path() else {
            warn!("ui: launcher config has no resolvable path; skipping save");
            return;
        };
        if let Some(parent) = path.parent()
            && let Err(e) = fs::create_dir_all(parent)
        {
            warn!(path = %parent.display(), error = %e, "ui: launcher config dir create failed");
            return;
        }
        match toml::to_string_pretty(self) {
            Ok(s) => {
                if let Err(e) = fs::write(&path, s) {
                    warn!(path = %path.display(), error = %e, "ui: launcher config write failed");
                } else {
                    debug!(path = %path.display(), "ui: launcher config saved");
                }
            }
            Err(e) => warn!(error = %e, "ui: launcher config serialize failed"),
        }
    }

}

/// Resolve the launcher config path. `None` if the platform doesn't
/// expose a user-config dir (rare; defensive).
#[must_use]
pub fn config_path() -> Option<PathBuf> {
    ProjectDirs::from("", "", "crabby")
        .map(|p| p.config_dir().join("launcher.toml"))
}

/// Resolve the launcher log directory. Same parent as the config file,
/// in a `logs/` subdir, e.g. `~/.config/crabby/logs/`. `None` if the
/// platform doesn't expose a user-config dir.
///
/// `tracing_appender` writes a daily-rotated file in here named like
/// `launcher.log.YYYY-MM-DD`; the [`current_log_path`] helper resolves
/// today's path.
#[must_use]
pub fn log_dir() -> Option<PathBuf> {
    ProjectDirs::from("", "", "crabby").map(|p| p.config_dir().join("logs"))
}

/// Filename prefix used by the rotating file appender. `tracing_appender`
/// joins this with a date suffix to produce `launcher.log.YYYY-MM-DD`.
pub const LOG_FILE_PREFIX: &str = "launcher.log";

/// Today's log file path. `None` when the user-config dir can't be
/// resolved. Used by the Logs tab to know where to read from; the
/// file may not exist yet (no log lines written), so callers should
/// treat missing as "empty log".
#[must_use]
pub fn current_log_path() -> Option<PathBuf> {
    let dir = log_dir()?;
    // Match tracing_appender's daily-rotation naming. UTC dates are
    // used for the lookup; tracing_appender defaults to UTC too.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    let date = days_to_ymd(now / 86_400);
    Some(dir.join(format!("{LOG_FILE_PREFIX}.{date}")))
}

/// Convert "days since 1970-01-01" to `YYYY-MM-DD`. Mirrors what
/// `tracing_appender`'s rolling-file writer uses internally - pulling
/// chrono just for one date format would be heavy.
fn days_to_ymd(days: u64) -> String {
    // 1970-01-01 was a Thursday; weekday isn't needed so just count
    // days through years and months. Standard proleptic Gregorian.
    let mut year: i64 = 1970;
    let mut remaining = days as i64;
    loop {
        let days_in_year = if is_leap(year) { 366 } else { 365 };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        year += 1;
    }
    let months_normal = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let months_leap = [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let months = if is_leap(year) { months_leap } else { months_normal };
    let mut month = 0_usize;
    while month < 12 && remaining >= months[month] {
        remaining -= months[month];
        month += 1;
    }
    let day = remaining + 1;
    format!("{:04}-{:02}-{:02}", year, month + 1, day)
}

const fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_to_ymd_known_dates() {
        assert_eq!(days_to_ymd(0), "1970-01-01");
        assert_eq!(days_to_ymd(31), "1970-02-01");
        // 2000-01-01 is exactly 30 years × 365 + 7 leap days = 10957.
        assert_eq!(days_to_ymd(10957), "2000-01-01");
        // 2024-02-29 (leap year).
        assert_eq!(days_to_ymd(19782), "2024-02-29");
    }
}
