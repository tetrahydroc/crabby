//! Top-level application state and view composition.
//!
//! The shell is three horizontal bands stacked vertically:
//!
//! 1. **Tab bar** - logo, 4 tab buttons (Mods/Saves/Logs/Settings),
//!    theme toggle.
//! 2. **Profile bar** - avatar, profile-name dropdown, stats,
//!    Edit + Launch buttons.
//! 3. **Body** - active tab's content. Tabs render their own
//!    contents; the shell just routes.
//! 4. **Status bar** - at the bottom: game version, path, last sync.
//!
//! The floating quick-theme panel is deferred to a later pass.

use std::path::PathBuf;

use iced::widget::{button, column, container, pick_list, row, text};
use iced::{Element, Length, Task, Theme};

use crate::launcher_config::LauncherConfig;
use crate::modpack_ui::{ImportTarget, ModImportStatus, ModOutcome, ModResolution, ModpackState};
use crate::style;
use crate::style::{
    ButtonKind, SurfaceKind, TextTone, band_style, button_style, surface_style, text_color,
    text_style,
};
use crate::tabs::{Tab, diagnostics, logs, mods, profiles, saves, settings};
use crate::theme::CrabbyTheme;

/// Crabby version embedded at compile time. Shown on the status bar.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// MW game id for Road to Vostok. Hard-coded since only one game is
/// targeted; multi-game support will thread this through state /
/// settings.
pub const RTV_GAME_ID: u64 = 864;

/// Top-level message type. Each variant either:
/// - Triggers global state change (tab switch, theme toggle)
/// - Forwards into a tab-specific message via tag
#[derive(Debug, Clone)]
pub enum Message {
    /// Tab button clicked.
    TabSelected(Tab),
    /// Theme toggle clicked in the tab bar.
    ToggleTheme,
    /// Granular theme axis changed via the Appearance sub-view or the
    /// floating quick-theme panel. App applies + persists.
    ThemeChanged(ThemeChange),
    /// Floating quick-theme pill clicked.
    ToggleQuickTheme,
    /// Conflicts-panel header clicked on a mod's detail page.
    /// Toggles the collapsed-vs-expanded state for that specific mod
    /// and persists to launcher.toml so it survives restarts.
    ToggleConflictPanel(String),
    /// Refresh button - rescan game dir + reload manifest. Also
    /// re-runs the analyzer (cheap), so fresh conflict data lands
    /// without needing a separate Rescan action.
    Refresh,
    /// "Browse" clicked on the missing-game-dir banner. Triggers a
    /// native folder picker via [`rfd`].
    PickGameDir,
    /// Folder-picker resolved. `None` if cancelled or an invalid
    /// directory was picked.
    GameDirPicked(Option<PathBuf>),
    /// Install / Rebake button clicked. Kicks the bake pipeline on a
    /// background thread so the UI stays responsive.
    InstallStart,
    /// Install finished. Carries the human-readable status string.
    InstallFinished(InstallOutcome),
    /// Background mod-cache rebuild finished. Carries `Ok(count)` for
    /// the number of archives (re)written or `Err(msg)` on failure.
    /// Fires-and-forgets; UI only logs and the next render reads cache
    /// state from disk via `mod_index.cfg`'s `cache_path` field, so
    /// nothing here needs `&mut self` except diagnostics.
    ModCacheRebuildFinished(Result<usize, String>),
    /// Background boot scan finished. Carries the consolidated scan
    /// artifacts (discovered mods, analyzer intents, bake status) so
    /// the apply path can fan them out to per-tab state without
    /// re-walking archives. `None` when no game dir was set when the
    /// scan kicked off.
    BootScanFinished(Option<Box<BootScanResult>>),
    /// Launch button clicked. Spawns the game executable detached so
    /// the launcher stays usable while RTV is open.
    LaunchGame,
    /// Adaptive Launch button clicked when the bake was out of date.
    /// Chains an install, then on success kicks the launch.
    BakeAndLaunch,
    /// Launch finished. Either the process spawned successfully or an
    /// error surfaced (binary missing, etc.).
    LaunchFinished(LaunchOutcome),
    /// Forwarded mod-tab message.
    Mods(mods::Message),
    /// Forwarded profiles-tab message.
    Profiles(profiles::Message),
    /// Forwarded diagnostics-tab message.
    Diagnostics(diagnostics::Message),
    /// Forwarded logs-tab message.
    Logs(logs::Message),
    /// Forwarded saves-tab message.
    Saves(saves::Message),
    /// Forwarded settings-tab message (rail + sub-view nav).
    Settings(settings::Message),
    /// Modpack export/import workflow message.
    Modpack(crate::modpack_ui::Message),
    /// Apply multiple messages in order - used when an async task
    /// needs to fire several state updates (e.g. a fan-out of
    /// dep-name lookups).
    Batch(Vec<Message>),
    /// Title bar pressed - initiate an OS drag-move loop.
    WindowDragStart,
    /// Minimize the window.
    WindowMinimize,
    /// Toggle maximize / restore.
    WindowToggleMaximize,
    /// Close the window (gracefully - saves whatever needs saving
    /// before the process exits).
    WindowClose,
    /// Edge/corner resize handle pressed. Fires
    /// `iced::window::drag_resize` with the matching direction.
    WindowResize(iced::window::Direction),
}

/// Top-level application state.
#[derive(Debug)]
pub struct App {
    /// Resolved game directory. `None` until first auto-detect or
    /// override; UI surfaces a helpful prompt instead of crashing.
    pub game_dir: Option<PathBuf>,
    /// Persisted launcher prefs. Updated when a game directory is
    /// picked via the first-run dialog.
    pub launcher_config: LauncherConfig,
    /// Currently-selected tab.
    pub active_tab: Tab,
    /// Theme (mode + accent + density). Mutable so the quick-theme
    /// panel and the tab-bar toggle can edit it.
    pub theme: CrabbyTheme,
    /// Mods-tab state.
    pub mods: mods::State,
    /// Profiles-tab state.
    pub profiles: profiles::State,
    /// Diagnostics-tab state.
    pub diagnostics: diagnostics::State,
    /// Logs-tab state. Holds the last-loaded entries + filter state.
    pub logs: logs::State,
    /// Saves-tab state - slot list + per-slot snapshots.
    pub saves: saves::State,
    /// Settings-tab shell state (which sub-view is active).
    pub settings: settings::State,
    /// Floating quick-theme panel - collapsed by default to a pill at
    /// the bottom-right; expanded by clicking the pill.
    pub quick_theme_open: bool,
    /// Modpack export/import workflow state. None = idle (nothing
    /// in progress, no surface visible). Some(...) = an export or
    /// import is being prepared / previewed / running.
    pub modpack: ModpackState,
    /// Status of the install/rebake operation. Drives the install
    /// button's label and any inline message under the profile bar.
    pub install: InstallStatus,
    /// Status of the most recent launch attempt. Drives the inline
    /// message in the status bar and disables the Launch button while
    /// a spawn is in flight.
    pub launch: LaunchStatus,
    /// Long-lived ModWorkshop API client. Cheap to clone - internal
    /// state (HTTP pool, in-flight dedupe map) is shared via Arc.
    pub mw: crabby_modworkshop::Client,
    /// True once the bulk version-probe pass has been kicked for the
    /// current row set. Reset on Refresh.
    pub mw_bulk_started: bool,
    /// Per-mod analyzer findings for the active profile, refreshed
    /// after each install/bake. Mod-list rows + detail panel read
    /// from this; empty until the first bake completes (or Refresh
    /// kicks an analyzer-only pass).
    pub mod_intents: Vec<crabby_mod_analyzer::ModIntent>,
    /// Cross-mod conflicts derived from `mod_intents`. Kept alongside
    /// (rather than re-derived per render) since conflict detection
    /// is O(N·M) and the data is read by every row + the detail pane.
    pub conflicts: Vec<crabby_mod_analyzer::Conflict>,
    /// Loose `*.tres` save files detected at the user-data dir root
    /// (vanilla / VML layout). `Some` when present so the Saves tab
    /// can render an "import to profile" affordance; `None` when the
    /// dir is clean. Refreshed on boot, Rescan, and post-Launch.
    pub vanilla_save_set: Option<crabby_config::saves::VanillaSaveSet>,
    /// Cached "is the baked PCK in sync with current enabled-mods?"
    /// signal. Drives the Launch button's adaptive label
    /// (Launch vs Bake & Launch). Refreshed on boot, after each
    /// mutation that could shift the bake key (toggle, profile
    /// switch, install completion), and on Rescan.
    pub bake_status: crabby_install::BakeStatus,
    /// True when Bake & Launch was clicked; InstallFinished will
    /// chain into LaunchGame on success rather than just settling.
    /// Reset after the chain fires (or on bake failure).
    pub launch_after_bake: bool,
    /// True while a background boot scan is running. Set when Refresh
    /// dispatches `run_boot_scan`, cleared on `BootScanFinished`.
    /// Drives the splash overlay so the app reads as alive vs frozen
    /// during the heavy archive walk.
    pub boot_scan_in_flight: bool,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    /// Build the initial app state. Resolves the game directory in
    /// priority order:
    ///
    /// 1. `--game-dir` CLI flag (handled by `main.rs` via env var
    ///    forwarding - see [`Self::new_with_override`])
    /// 2. `CRABBY_GAME_DIR` env var
    /// 3. Persisted `launcher.toml` from a previous session
    /// 4. Auto-detect via `crabby_install::detect_game_dir`
    ///
    /// On miss, `game_dir` stays `None` and the UI shows the
    /// first-run picker banner.
    #[must_use]
    pub fn new() -> Self {
        Self::new_with_override(None)
    }

    /// Like [`Self::new`] but with an explicit game-dir override
    /// (typically from a CLI flag). The override jumps the
    /// resolution chain - useful for testing and CI.
    #[must_use]
    pub fn new_with_override(cli_override: Option<PathBuf>) -> Self {
        let launcher_config = LauncherConfig::load();
        let game_dir = resolve_game_dir(cli_override.as_deref(), &launcher_config);
        if let Some(ref dir) = game_dir {
            tracing::info!(path = %dir.display(), "ui: game dir resolved");
        } else {
            tracing::warn!("ui: game dir not detected - user will be prompted");
        }
        let theme = CrabbyTheme::from_prefs(&launcher_config.theme);
        let mut me = Self {
            game_dir,
            launcher_config,
            active_tab: Tab::default(),
            theme,
            mods: mods::State::default(),
            profiles: profiles::State::default(),
            diagnostics: diagnostics::State::default(),
            logs: logs::State::default(),
            saves: saves::State::default(),
            settings: settings::State::default(),
            quick_theme_open: false,
            modpack: ModpackState::default(),
            install: InstallStatus::Idle,
            launch: LaunchStatus::Idle,
            mw: crabby_modworkshop::Client::new(),
            mw_bulk_started: false,
            mod_intents: Vec::new(),
            conflicts: Vec::new(),
            vanilla_save_set: None,
            bake_status: crabby_install::BakeStatus::Unknown {
                reason: "not yet checked".into(),
            },
            launch_after_bake: false,
            boot_scan_in_flight: false,
        };
        // Light-weight refreshes: a single config read each, no
        // archive walks. Heavy work (mod scan, analyzer, bake status,
        // mod-index) is deferred to the initial Refresh Task fired by
        // main.rs after first paint, so the window can paint immediately.
        me.profiles.refresh(me.game_dir.as_deref());
        me.diagnostics.refresh(me.game_dir.as_deref());
        me.saves.refresh(me.profiles.active_profile_name());
        me.refresh_vanilla_save_set();
        // One-shot pass: rewrite every MCM config so any Float-typed
        // [Int]-section values left by the in-game MCM (its slider
        // SpinBox always returns float) get coerced to Int. Without
        // this, mods like Faction Warfare silently fail in their
        // `func _spawn_pool() -> int` math against Float-typed
        // Resource properties.
        let (rewrote, err) = crabby_config::mcm::normalize_all_mcm_configs();
        if rewrote > 0 || err > 0 {
            tracing::info!(rewrote, errors = err, "mcm: normalized configs at boot");
        }
        me
    }

    /// Window title. iced 0.13 polls this each frame; keep it cheap.
    #[must_use]
    pub fn title(&self) -> String {
        String::from("Crabby - Road to Vostok mod manager")
    }

    /// Route a message. Returns a [`Task`] for messages that need to
    /// kick off async work (e.g. the file picker); other branches
    /// return `Task::none()`.
    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::TabSelected(tab) => {
                self.active_tab = tab;
                // Logs auto-refresh on enter so the freshest tail is
                // visible without needing a Refresh click.
                if tab == Tab::Logs {
                    self.logs.refresh(crate::tabs::logs::AnalyzerView {
                        intents: &self.mod_intents,
                        conflicts: &self.conflicts,
                    });
                }
                // Saves rescans slot dirs on enter so anything that
                // landed since launch surfaces (e.g., game just wrote
                // new save files).
                if tab == Tab::Saves {
                    self.saves.invalidate();
                    let active = self.profiles.active_profile_name().to_string();
                    self.saves.refresh(&active);
                }
                Task::none()
            }
            Message::ToggleTheme => {
                self.theme.toggle_mode();
                let overrides = crate::theme::PillOverrides::from_prefs(
                    &self.launcher_config.theme.pill_overrides,
                );
                self.theme.refresh(&overrides);
                self.persist_theme();
                Task::none()
            }
            Message::ThemeChanged(change) => {
                self.apply_theme_change(change);
                Task::none()
            }
            Message::ToggleQuickTheme => {
                self.quick_theme_open = !self.quick_theme_open;
                Task::none()
            }
            Message::ToggleConflictPanel(mod_id) => {
                // Compute the "would be" default - the flip should go
                // around the *currently visible* state, not just write
                // `true` blindly. If no override exists, the toggle is
                // FROM the default; record the opposite.
                let has_hard = crabby_mod_analyzer::mod_has_hard_conflict(&self.conflicts, &mod_id);
                let current_collapsed = self
                    .launcher_config
                    .mod_conflict_panel_collapsed
                    .get(&mod_id)
                    .copied()
                    .unwrap_or(!has_hard);
                self.launcher_config
                    .mod_conflict_panel_collapsed
                    .insert(mod_id, !current_collapsed);
                self.launcher_config.save();
                Task::none()
            }
            Message::Refresh => {
                // Lightweight refreshes (config reads, no archive walks)
                // run synchronously so profile + saves updates land
                // instantly. Heavy work (mod discovery, analyzer, bake
                // status, mod-index) gets dispatched to a worker thread
                // so the UI stays interactive while it runs.
                //
                // Network calls (MW probe, listings) are NOT included;
                // those stay automatic / on-demand to keep Rescan
                // fast. MW data refreshes after each bake or via the
                // Browse tab's own refresh button.
                self.do_lightweight_refresh();
                self.boot_scan_in_flight = true;
                Task::perform(
                    run_boot_scan(self.game_dir.clone()),
                    Message::BootScanFinished,
                )
            }
            Message::BootScanFinished(result) => {
                self.boot_scan_in_flight = false;
                self.apply_boot_scan(result);
                // Kick the background cache rebuild now that the
                // scan settled and the on-disk mod_index reflects
                // the latest archives.
                self.spawn_cache_rebuild_task()
            }
            Message::PickGameDir => Task::perform(pick_game_dir(), Message::GameDirPicked),
            Message::InstallStart => {
                let Some(dir) = self.game_dir.clone() else {
                    self.install = InstallStatus::Failed("Set game directory first".into());
                    return Task::none();
                };
                if matches!(self.install, InstallStatus::Running) {
                    return Task::none();
                }
                self.install = InstallStatus::Running;
                Task::perform(run_install(dir), Message::InstallFinished)
            }
            Message::ModCacheRebuildFinished(result) => {
                match result {
                    Ok(0) => {
                        tracing::debug!("ui: mod cache rebuild - already current");
                    }
                    Ok(n) => {
                        tracing::info!(rebuilt = n, "ui: mod cache rebuild finished");
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "ui: mod cache rebuild failed");
                    }
                }
                Task::none()
            }
            Message::InstallFinished(outcome) => {
                tracing::info!(?outcome, "ui: install finished");
                self.install = match outcome {
                    InstallOutcome::Success(msg) => InstallStatus::Succeeded(msg),
                    InstallOutcome::Failed(msg) => InstallStatus::Failed(msg),
                };
                // Mod list and diagnostics may have shifted - re-scan.
                self.mods.refresh_rows(self.game_dir.as_deref());
                self.diagnostics.refresh(self.game_dir.as_deref());
                // Successful bake → re-run analyzer to surface conflicts
                // for the row-pill + detail panel, and re-evaluate the
                // bake-status so the Launch button drops "Bake &" prefix.
                if matches!(self.install, InstallStatus::Succeeded(_)) {
                    self.refresh_mod_analysis();
                    self.refresh_bake_status();
                    // If "Bake & Launch" kicked this off, chain
                    // straight into LaunchGame now that the bake is
                    // current. The cache rebuild needs to finish
                    // FIRST (the runtime mounts from the cache, not
                    // the source archive), so don't bypass it.
                    let chain_launch = std::mem::take(&mut self.launch_after_bake);
                    let cache_task = self.spawn_cache_rebuild_task();
                    if chain_launch {
                        // Sequence: cache rebuild, then launch. The
                        // cache rebuild's done-message handler is a
                        // no-op so chaining via `.then` is overkill;
                        // launch_after_bake is cleared and we kick
                        // launch directly. The cache rebuild
                        // completes in the background; if launch is
                        // faster than the rebuild the runtime falls
                        // back to mounting the source archive.
                        return Task::batch([cache_task, Task::done(Message::LaunchGame)]);
                    }
                    return cache_task;
                }
                // Bake failed - drop the chained-launch intent so
                // a future plain "Launch" click doesn't kick
                // another bake unexpectedly.
                self.launch_after_bake = false;
                Task::none()
            }
            Message::BakeAndLaunch => {
                let Some(_) = self.game_dir.clone() else {
                    self.launch = LaunchStatus::Failed("Set game directory first".into());
                    return Task::none();
                };
                if matches!(self.install, InstallStatus::Running) {
                    return Task::none();
                }
                // Set the chain flag so InstallFinished knows to fire
                // LaunchGame on success, then kick the install via the
                // existing handler so all the install-side bookkeeping
                // (status, refresh, etc.) stays in one place.
                self.launch_after_bake = true;
                return self.update(Message::InstallStart);
            }
            Message::LaunchGame => {
                let Some(dir) = self.game_dir.clone() else {
                    self.launch = LaunchStatus::Failed("Set game directory first".into());
                    return Task::none();
                };
                if matches!(self.install, InstallStatus::Running) {
                    self.launch = LaunchStatus::Failed("Wait for bake to finish".into());
                    return Task::none();
                }
                self.launch = LaunchStatus::Launching;
                Task::perform(launch_game(dir), Message::LaunchFinished)
            }
            Message::LaunchFinished(outcome) => {
                tracing::info!(?outcome, "ui: launch finished");
                self.launch = match outcome {
                    LaunchOutcome::Spawned => {
                        // Tick the active profile's last-played stamp.
                        // Failed launches don't update it - surfacing
                        // those would mislead about whether the run
                        // actually happened.
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0);
                        let active = self.profiles.active_profile_name().to_string();
                        self.launcher_config.last_played.insert(active, now);
                        self.launcher_config.save();
                        LaunchStatus::Launched
                    }
                    LaunchOutcome::Failed(msg) => LaunchStatus::Failed(msg),
                };
                Task::none()
            }
            Message::GameDirPicked(picked) => {
                if let Some(path) = picked {
                    self.game_dir = Some(path.clone());
                    self.launcher_config.game_dir = Some(path);
                    self.launcher_config.save();
                    self.mods.refresh_rows(self.game_dir.as_deref());
                }
                Task::none()
            }
            Message::Mods(msg) => {
                // Capture pre-update state so we know which downstream
                // fetches to fire after the message is applied.
                let selecting = if let mods::Message::SelectMod(id) = &msg {
                    Some(id.clone())
                } else {
                    None
                };
                let refreshing_one = if let mods::Message::RefreshOne(id) = &msg {
                    Some(id.clone())
                } else {
                    None
                };
                let just_fetched = if let mods::Message::MwFetched(id, Ok(_)) = &msg {
                    Some(id.clone())
                } else {
                    None
                };
                let refreshing_listings = matches!(msg, mods::Message::RefreshListings);
                let installing_remote = if let mods::Message::InstallRemote(id) = &msg {
                    Some(id.clone())
                } else {
                    None
                };
                // Add-mod intercepts. AddModClicked fires the picker;
                // AddModPicked(Some) copies the file into Mods/. Both
                // routed before mods.update so the tab's no-op handler
                // doesn't mask App-side work.
                let kicked_add_mod = matches!(msg, mods::Message::AddModClicked);
                let kicked_rescan = matches!(msg, mods::Message::RescanClicked);
                let toggled_enabled = matches!(msg, mods::Message::ToggleEnabled(_));
                let toggled_conflict_panel = if let mods::Message::ToggleConflictPanel(id) = &msg {
                    Some(id.clone())
                } else {
                    None
                };
                let add_mod_picked = if let mods::Message::AddModPicked(p) = &msg {
                    Some(p.clone())
                } else {
                    None
                };
                // The mod-list footer's Import/Export buttons are
                // bridges into the modpack flow. Translate here so
                // tabs/mods.rs doesn't need to know about ModpackUi.
                let routed_modpack: Option<crate::modpack_ui::Message> = match &msg {
                    mods::Message::ImportPackClicked => {
                        Some(crate::modpack_ui::Message::ImportClicked)
                    }
                    mods::Message::ExportPackClicked => {
                        Some(crate::modpack_ui::Message::ExportClicked)
                    }
                    _ => None,
                };
                // For UpdateMod we need to look up the row before
                // mods.update borrows self.mods mutably. Carry the
                // mw_id + archive path forward so the post-update
                // block can fire the download Task.
                let updating_target: Option<(String, u64, std::path::PathBuf)> = match &msg {
                    mods::Message::UpdateMod(local_id) => self.mods.update_target_for(local_id),
                    _ => None,
                };
                self.mods.update(msg, self.game_dir.as_deref());

                // Toggling a mod shifts the enabled-mods digest in the
                // bake key. Re-evaluate so the Launch button label
                // updates to "Bake & Launch" without needing a Rescan.
                // Also kick a background cache rebuild so any newly-
                // enabled archive's pre-rewrite is ready before launch.
                let toggle_cache_task: Task<Message> = if toggled_enabled {
                    self.refresh_bake_status();
                    self.spawn_cache_rebuild_task()
                } else {
                    Task::none()
                };

                if kicked_rescan {
                    // Rescan is the unified local-state refresh now -
                    // mods + profiles + diagnostics + saves + analyzer.
                    return self.update(Message::Refresh);
                }
                if let Some(mod_id) = toggled_conflict_panel {
                    return self.update(Message::ToggleConflictPanel(mod_id));
                }
                if kicked_add_mod {
                    return Task::perform(pick_mod_archive_path(), |opt| {
                        Message::Mods(mods::Message::AddModPicked(opt))
                    });
                }
                if let Some(picked) = add_mod_picked {
                    if let Some(path) = picked {
                        match copy_mod_into_mods_dir(&path, self.game_dir.as_deref()) {
                            Ok(dest) => {
                                tracing::info!(
                                    target = "crabby_ui::add_mod",
                                    src = %path.display(),
                                    dest = %dest.display(),
                                    "added mod from local file",
                                );
                                self.mods.refresh_rows(self.game_dir.as_deref());
                            }
                            Err(e) => {
                                tracing::error!(
                                    target = "crabby_ui::add_mod",
                                    src = %path.display(),
                                    err = %e,
                                    "add mod failed",
                                );
                            }
                        }
                    }
                    return Task::none();
                }
                if let Some(modpack_msg) = routed_modpack {
                    return self.handle_modpack_message(modpack_msg);
                }

                if let Some(listing_id) = installing_remote {
                    let Some(game_dir) = self.game_dir.clone() else {
                        return Task::done(Message::Mods(mods::Message::InstallFinished(
                            listing_id,
                            Err("No game directory set".into()),
                        )));
                    };
                    let Ok(mw_id) = listing_id.parse::<u64>() else {
                        return Task::done(Message::Mods(mods::Message::InstallFinished(
                            listing_id,
                            Err("Listing id is not a MW mod id".into()),
                        )));
                    };
                    let client = self.mw.clone();
                    let listing_id_clone = listing_id.clone();
                    return Task::batch(vec![
                        Task::done(Message::Mods(mods::Message::InstallStarted(listing_id))),
                        Task::perform(
                            async move { install_remote_mod(&client, mw_id, &game_dir).await },
                            move |result| {
                                Message::Mods(mods::Message::InstallFinished(
                                    listing_id_clone.clone(),
                                    result,
                                ))
                            },
                        ),
                    ]);
                }

                if let Some((local_id, mw_id, existing_path)) = updating_target {
                    let client = self.mw.clone();
                    let local_id_clone = local_id.clone();
                    return Task::batch(vec![
                        Task::done(Message::Mods(mods::Message::UpdateStarted(local_id))),
                        Task::perform(
                            async move { update_installed_mod(&client, mw_id, &existing_path).await },
                            move |result| {
                                Message::Mods(mods::Message::UpdateFinished(
                                    local_id_clone.clone(),
                                    result,
                                ))
                            },
                        ),
                    ]);
                }

                if refreshing_listings {
                    // Force-bypass cache by clearing the listing
                    // entry on disk before re-fetching. Per-mod caches
                    // remain so individual records stay warm.
                    if let Some(p) =
                        crabby_modworkshop::cache_path("listing", crate::app::RTV_GAME_ID)
                    {
                        let _ = std::fs::remove_file(p);
                    }
                    return self.maybe_start_listings_fetch(true);
                }

                // SelectMod and RefreshOne both want the same fetch:
                // pull MW data for the active mod if it has an mw_id
                // and we don't already have it cached in memory.
                if let Some(id) = selecting.clone().or(refreshing_one.clone()) {
                    if let Some((mod_id, mw_id, local_v)) = self.mods.pending_mw_fetch(&id) {
                        let client = self.mw.clone();
                        return Task::perform(
                            async move { fetch_mw_one(&client, mod_id.clone(), mw_id, local_v).await },
                            |outcome| Message::Mods(mods::Message::MwFetched(outcome.0, outcome.1)),
                        );
                    }
                    // No fresh fetch needed (data already cached
                    // in memory). Still kick the gallery the first
                    // time this mod is selected since galleries
                    // aren't pre-fetched from the bulk probe.
                    if !self.mods.is_gallery_kicked(&id) {
                        let files = self.mods.pending_gallery_image_files(&id);
                        if !files.is_empty() {
                            self.mods.mark_gallery_kicked(&id);
                            let client = self.mw.clone();
                            return Task::perform(
                                async move { fetch_gallery(&client, files).await },
                                |pairs| {
                                    Message::Batch(
                                        pairs
                                            .into_iter()
                                            .map(|(f, r)| {
                                                Message::Mods(mods::Message::MwImageLoaded(f, r))
                                            })
                                            .collect(),
                                    )
                                },
                            );
                        }
                    }
                }

                // Just got a primary MW fetch back - fan out the
                // dep-name lookups (so Requires shows real names) and
                // the thumbnail fetch (so the hero image appears).
                // Gallery is deferred to when the Images section opens
                // to keep the bandwidth honest.
                if let Some(parent_id) = just_fetched {
                    let mut tasks: Vec<Task<Message>> = Vec::new();

                    let dep_ids = self.mods.pending_dep_name_lookups(&parent_id);
                    if !dep_ids.is_empty() {
                        let client = self.mw.clone();
                        tasks.push(Task::perform(
                            async move { fetch_dep_names(&client, dep_ids).await },
                            |pairs| {
                                Message::Batch(
                                    pairs
                                        .into_iter()
                                        .map(|(id, name)| {
                                            Message::Mods(mods::Message::MwDepName(id, name))
                                        })
                                        .collect(),
                                )
                            },
                        ));
                    }

                    if let Some(file) = self.mods.pending_thumbnail_file(&parent_id) {
                        let client = self.mw.clone();
                        let file_clone = file.clone();
                        tasks.push(Task::perform(
                            async move { fetch_image(&client, file_clone).await },
                            |(file, result)| {
                                Message::Mods(mods::Message::MwImageLoaded(file, result))
                            },
                        ));
                    }

                    // Gallery fan-out - kicked once per mod per
                    // session. The fetch loop is rate-limited inside
                    // fetch_gallery to stay polite.
                    if !self.mods.is_gallery_kicked(&parent_id) {
                        let files = self.mods.pending_gallery_image_files(&parent_id);
                        if !files.is_empty() {
                            self.mods.mark_gallery_kicked(&parent_id);
                            let client = self.mw.clone();
                            tasks.push(Task::perform(
                                async move { fetch_gallery(&client, files).await },
                                |pairs| {
                                    Message::Batch(
                                        pairs
                                            .into_iter()
                                            .map(|(f, r)| {
                                                Message::Mods(mods::Message::MwImageLoaded(f, r))
                                            })
                                            .collect(),
                                    )
                                },
                            ));
                        }
                    }

                    if !tasks.is_empty() {
                        return Task::batch(tasks);
                    }
                }
                toggle_cache_task
            }
            Message::Profiles(msg) => {
                // Capture before-state so a profile switch can be
                // reacted to by re-pointing the active save target to
                // the new profile's default slot.
                let switching_to: Option<String> =
                    if let profiles::Message::SwitchProfile(name) = &msg {
                        Some(name.clone())
                    } else {
                        None
                    };
                self.profiles.update(msg, self.game_dir.as_deref());
                // Switching profiles changes which mods show as enabled
                // - force the mod list to repopulate.
                self.mods.refresh_rows(self.game_dir.as_deref());
                if let Some(new_profile) = switching_to {
                    // Re-target active_slot.txt to the new profile.
                    // Carry the current slot name forward when possible
                    // (so a "default" slot in profile A maps to
                    // "default" in B when present); otherwise fall
                    // back to "default".
                    let target = crabby_config::saves::active_target();
                    let candidate_slot = target.slot.as_str();
                    let resolved_slot = match crabby_config::saves::list_slots(&new_profile) {
                        Ok(slots) if slots.iter().any(|s| s.name == candidate_slot) => {
                            candidate_slot.to_string()
                        }
                        _ => crabby_config::saves::DEFAULT_NAME.to_string(),
                    };
                    if let Err(e) =
                        crabby_config::saves::set_active_target(&new_profile, &resolved_slot)
                    {
                        tracing::warn!(profile = %new_profile, error = %e, "saves: set_active_target on profile switch failed");
                    }
                    self.saves.invalidate();
                    self.saves.refresh(&new_profile);
                    // Profile switch changes the enabled-mods set →
                    // re-cache to keep the runtime shim's mount path
                    // serving pre-rewritten archives for the new
                    // profile's mods.
                    return self.spawn_cache_rebuild_task();
                }
                Task::none()
            }
            Message::Diagnostics(msg) => {
                // PickRoot needs to fire the async folder picker; the
                // diagnostics tab can't kick a Task itself. Intercept it
                // here and route the result back as AddRoot.
                if matches!(msg, diagnostics::Message::PickRoot) {
                    return Task::perform(pick_mod_source_folder(), |picked| {
                        Message::Diagnostics(diagnostics::Message::AddRoot(picked))
                    });
                }
                // ToggleConfirmDestructive lives on launcher_config -
                // diagnostics::State doesn't own it. Persist here.
                if let diagnostics::Message::ToggleConfirmDestructive(v) = &msg {
                    self.launcher_config.confirm_destructive_actions = *v;
                    self.launcher_config.save();
                    return Task::none();
                }
                let needs_mods_refresh = matches!(
                    msg,
                    diagnostics::Message::AddRoot(_)
                        | diagnostics::Message::ToggleRootDev(_)
                        | diagnostics::Message::RemoveRoot(_)
                );
                self.diagnostics.update(msg, self.game_dir.as_deref());
                // Mod-source changes affect what the Mods tab shows.
                if needs_mods_refresh {
                    self.mods.refresh_rows(self.game_dir.as_deref());
                }
                Task::none()
            }
            Message::Logs(msg) => {
                self.logs.update(
                    msg,
                    crate::tabs::logs::AnalyzerView {
                        intents: &self.mod_intents,
                        conflicts: &self.conflicts,
                    },
                );
                Task::none()
            }
            Message::Saves(msg) => {
                let active = self.profiles.active_profile_name().to_string();
                // Intercept the vanilla-import dispatch - app owns the
                // VanillaSaveSet so the actual filesystem call lives
                // here. Everything else routes through the sub-tab's
                // own update.
                if let saves::Message::ImportVanilla { profile, slot } = &msg {
                    let profile = profile.clone();
                    let slot = slot.clone();
                    if let Some(set) = self.vanilla_save_set.as_ref() {
                        match crabby_config::saves::import_vanilla_to_slot(set, &profile, &slot) {
                            Ok(report) => {
                                tracing::info!(
                                    target = "crabby_ui::saves",
                                    profile = %profile,
                                    slot = %slot,
                                    moved = report.moved.len(),
                                    "imported vanilla saves",
                                );
                                self.saves.clear_vanilla_import_form();
                                // Refresh both the slot list (new
                                // files now visible there) and the
                                // vanilla scan (root should be empty
                                // for this set now, hiding the
                                // section).
                                self.saves.invalidate();
                                self.saves.refresh(&active);
                                self.refresh_vanilla_save_set();
                            }
                            Err(e) => {
                                tracing::error!(target = "crabby_ui::saves", err = %e, "import_vanilla");
                                self.saves.set_error(format!("{e}"));
                            }
                        }
                    } else {
                        // Race: the set scan returned None between
                        // render and click. Just hide the form.
                        self.saves.clear_vanilla_import_form();
                    }
                    return Task::none();
                }
                self.saves
                    .update(
                        msg,
                        &active,
                        self.launcher_config.confirm_destructive_actions,
                    )
                    .map(Message::Saves)
            }
            Message::Modpack(msg) => {
                return self.handle_modpack_message(msg);
            }
            Message::Settings(msg) => {
                // Settings shell produces three message kinds: rail
                // navigation (handled by `settings.update`), theme
                // changes (route through the existing apply path so
                // persistence happens), and General/Diagnostics CRUD
                // (route through the diagnostics handler - same
                // PickRoot folder picker, same mutate_roots paths).
                match msg {
                    settings::Message::SelectSection(_) => {
                        self.settings.update(msg);
                        Task::none()
                    }
                    settings::Message::Theme(change) => {
                        self.apply_theme_change(change);
                        Task::none()
                    }
                    settings::Message::General(diag_msg) => {
                        // Wrap and re-enter so PickRoot's folder-picker
                        // dispatch (in the Diagnostics handler) reaches
                        // the same code path it always did.
                        self.update(Message::Diagnostics(diag_msg))
                    }
                }
            }
            Message::Batch(msgs) => {
                let mut tasks: Vec<Task<Message>> = Vec::new();
                for m in msgs {
                    tasks.push(self.update(m));
                }
                Task::batch(tasks)
            }
            Message::WindowDragStart => {
                // Drag-move the OS window. We resolve the active
                // window id via get_latest and chain into the drag
                // task; iced wires this through winit's
                // start_window_drag on platforms that support it.
                iced::window::latest().and_then(iced::window::drag)
            }
            Message::WindowMinimize => {
                iced::window::latest().and_then(|id| iced::window::minimize(id, true))
            }
            Message::WindowToggleMaximize => {
                // Resolve the window id (Task<Option<Id>>; `and_then`),
                // query the current maximized state (Task<bool>; `then`),
                // then flip it. The chain shape mirrors how the iced
                // examples wire `get_*` queries.
                iced::window::latest().and_then(|id| {
                    iced::window::is_maximized(id)
                        .then(move |maxed| iced::window::maximize(id, !maxed))
                })
            }
            Message::WindowClose => iced::window::latest().and_then(iced::window::close),
            Message::WindowResize(dir) => {
                iced::window::latest().and_then(move |id| iced::window::drag_resize(id, dir))
            }
        }
    }

    /// Render the window.
    #[must_use]
    pub fn view(&self) -> Element<'_, Message> {
        let palette = self.theme.palette;

        let tab_bar = self.tab_bar();
        let profile_bar = self.profile_bar();
        let body = self.body();
        let status_bar = self.status_bar();

        let layout = column![tab_bar, profile_bar, body, status_bar]
            .spacing(0)
            .width(Length::Fill)
            .height(Length::Fill);

        let main = container(layout)
            .style(surface_style(palette, SurfaceKind::Bg0))
            .width(Length::Fill)
            .height(Length::Fill);

        // Borderless mode means we lose the OS's edge-resize affordance.
        // Reconstruct it with eight thin transparent mouse_areas
        // overlaid on the window edges + corners. Each fires
        // `WindowResize(direction)` on press; iced's drag_resize then
        // hands off to winit's start_window_resize.
        let resize_handles = self.resize_overlay();

        // Floating quick-theme panel - anchored bottom-right, layered
        // above the main UI but below the edge-resize handles so the
        // window stays resizable while the panel is open.
        let quick_theme = crate::quick_theme::overlay(
            &self.theme,
            &self.launcher_config.theme.saved_colors,
            self.quick_theme_open,
            &palette,
        );

        // Modpack overlay - modal centered, above main but below the
        // quick-theme panel so theme tweaks remain available during a
        // long import. `Idle` renders an empty space (click-through).
        let modpack =
            crate::modpack_ui::overlay(&self.modpack, self.profiles.all_profiles(), &palette);

        // Profile modal: Create / Edit overlays. Same stack position
        // as modpack: above main, below the floating quick-theme.
        let profile_modal = crate::profile_modal::overlay(&self.profiles, &palette);

        iced::widget::stack![main, modpack, profile_modal, quick_theme, resize_handles]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// Build the resize-handle overlay: a Length::Fill container with
    /// eight mouse_area edges/corners arranged like a 3×3 grid.
    fn resize_overlay(&self) -> Element<'_, Message> {
        use iced::widget::{Space, mouse_area, row as row_widget};
        use iced::window::Direction as Dir;

        // Edge thickness in pixels. Matches what most native window
        // managers use; keeps the hit zone large enough to grab
        // without being visually obvious.
        const EDGE: f32 = 6.0;

        let make_area = |dir: Dir, w: Length, h: Length| -> Element<'_, Message> {
            // Match the cursor to the resize direction so the
            // affordance reads correctly. iced's mouse_area sets the
            // cursor whenever the pointer enters the area.
            use iced::mouse::Interaction as I;
            let cursor = match dir {
                Dir::North | Dir::South => I::ResizingVertically,
                Dir::East | Dir::West => I::ResizingHorizontally,
                Dir::NorthWest | Dir::SouthEast => I::ResizingDiagonallyDown,
                Dir::NorthEast | Dir::SouthWest => I::ResizingDiagonallyUp,
            };
            mouse_area(Space::new().width(w).height(h))
                .on_press(Message::WindowResize(dir))
                .interaction(cursor)
                .into()
        };
        let center_filler: Element<'_, Message> =
            Space::new().width(Length::Fill).height(Length::Fill).into();

        // Top row: NW corner, N edge, NE corner.
        let top = row_widget![
            make_area(Dir::NorthWest, Length::Fixed(EDGE), Length::Fixed(EDGE)),
            make_area(Dir::North, Length::Fill, Length::Fixed(EDGE)),
            make_area(Dir::NorthEast, Length::Fixed(EDGE), Length::Fixed(EDGE)),
        ];
        // Middle row: W edge, transparent center (lets clicks fall
        // through to the underlying widgets), E edge.
        let middle = row_widget![
            make_area(Dir::West, Length::Fixed(EDGE), Length::Fill),
            center_filler,
            make_area(Dir::East, Length::Fixed(EDGE), Length::Fill),
        ]
        .height(Length::Fill);
        let bottom = row_widget![
            make_area(Dir::SouthWest, Length::Fixed(EDGE), Length::Fixed(EDGE)),
            make_area(Dir::South, Length::Fill, Length::Fixed(EDGE)),
            make_area(Dir::SouthEast, Length::Fixed(EDGE), Length::Fixed(EDGE)),
        ];
        column![top, middle, bottom]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// Theme - keep iced on its `Dark` built-in; widget styles are
    /// overridden per-instance through closures rather than wholesale.
    /// (Future: switch to `Theme::custom_with_fn` for top-level
    /// menus / tooltips that don't accept per-instance styles.)
    #[must_use]
    pub fn theme(&self) -> Theme {
        match self.theme.mode {
            crate::theme::Mode::Dark => Theme::Dark,
            crate::theme::Mode::Light => Theme::Light,
        }
    }

    /// Kick the bulk MW probe if there are mods to fetch and one
    /// hasn't already started this refresh cycle. Fetches the full
    /// mod record per row (rate-limited inside the helper) so
    /// authors, sizes, tags, and update statuses all populate at
    /// once instead of via cheap-but-narrow version-only probes.
    /// Kick a catalog-listing fetch when one isn't already in flight.
    /// Skipped when the cache is fresh (the catalog-level cache TTL
    /// guards that internally) unless `force` is `true`. Called from
    /// Refresh and from the chip's manual refresh button.
    pub fn maybe_start_listings_fetch(&mut self, force: bool) -> Task<Message> {
        // Skip if we already have data and aren't forcing.
        let listings_loaded = matches!(
            self.mods.listings_state,
            mods::ListingsState::Ready | mods::ListingsState::Loading
        );
        if listings_loaded && !force {
            return Task::none();
        }
        let client = self.mw.clone();
        let game_id = RTV_GAME_ID;
        Task::batch(vec![
            Task::done(Message::Mods(mods::Message::ListingsLoading)),
            Task::perform(
                async move {
                    use crabby_modworkshop::{GameFilter, MwCatalog, RemoteCatalog};
                    let catalog = MwCatalog::new(client);
                    catalog
                        .list(GameFilter::GameId(game_id.to_string()))
                        .await
                        .map_err(|e| format!("{e}"))
                },
                |result| Message::Mods(mods::Message::ListingsLoaded(result)),
            ),
        ])
    }

    /// Kick the bulk per-mod fetch when one isn't already in flight.
    /// Idempotent within a refresh cycle - clears the gate on every
    /// Refresh so newly-discovered mods get probed too.
    pub fn maybe_start_mw_bulk_probe(&mut self) -> Task<Message> {
        if self.mw_bulk_started {
            return Task::none();
        }
        let targets = self.mods.pending_mw_bulk_probe();
        if targets.is_empty() {
            return Task::none();
        }
        self.mw_bulk_started = true;
        let client = self.mw.clone();
        Task::perform(
            async move { fetch_mw_bulk(&client, targets).await },
            |pairs| {
                Message::Batch(
                    pairs
                        .into_iter()
                        .map(|(id, result)| Message::Mods(mods::Message::MwFetched(id, result)))
                        .collect(),
                )
            },
        )
    }

    /// Modpack export/import dispatcher. Returns the Task to run
    /// (often `Task::none()`).
    fn handle_modpack_message(&mut self, msg: crate::modpack_ui::Message) -> Task<Message> {
        use crate::modpack_ui::Message as M;
        match msg {
            M::ExportClicked => {
                match self.build_modpack_manifest() {
                    Ok(m) => match crabby_modpack::encode_string(&m) {
                        Ok(code) => {
                            // Best-effort clipboard via iced's primitive.
                            // Falls back to "saved string available, click
                            // Save to file" if clipboard isn't reachable.
                            self.modpack = ModpackState::ExportToast {
                                text: code,
                                copied: false,
                            };
                        }
                        Err(e) => {
                            tracing::error!(target = "crabby_ui::modpack", err = %e, "encode pack");
                            self.modpack = ModpackState::ExportToast {
                                text: format!("export failed: {e}"),
                                copied: false,
                            };
                        }
                    },
                    Err(e) => {
                        tracing::error!(target = "crabby_ui::modpack", err = %e, "build manifest");
                        self.modpack = ModpackState::ExportToast {
                            text: format!("export failed: {e}"),
                            copied: false,
                        };
                    }
                }
                Task::none()
            }
            M::ExportSaveToFile => Task::perform(pick_modpack_save_path(), |opt| {
                Message::Modpack(M::ExportFilePicked(opt))
            }),
            M::ExportFilePicked(Some(path)) => {
                if let Err(e) = self.save_modpack_to_file(&path) {
                    tracing::error!(target = "crabby_ui::modpack", err = %e, "save file");
                    self.modpack = ModpackState::ExportToast {
                        text: format!("save failed: {e}"),
                        copied: false,
                    };
                } else {
                    self.modpack = ModpackState::ExportToast {
                        text: format!("Saved to {}", path.display()),
                        copied: false,
                    };
                }
                Task::none()
            }
            M::ExportFilePicked(None) => Task::none(),
            M::CopyToClipboard(s) => {
                // Fire the clipboard write Task and chain a synthetic
                // "copied" message so the toast flips its label.
                Task::batch(vec![
                    iced::clipboard::write(s),
                    Task::done(Message::Modpack(M::ClipboardCopied)),
                ])
            }
            M::ClipboardCopied => {
                if let ModpackState::ExportToast { copied, .. } = &mut self.modpack {
                    *copied = true;
                }
                Task::none()
            }
            M::ImportClicked => {
                self.modpack = ModpackState::ImportPaste {
                    input: String::new(),
                    error: None,
                };
                Task::none()
            }
            M::ImportFromFile => Task::perform(pick_modpack_open_path(), |opt| {
                Message::Modpack(M::ImportFilePicked(opt))
            }),
            M::ImportFilePicked(Some(path)) => {
                match std::fs::read(&path) {
                    Ok(bytes) => match crabby_modpack::decode_any(&bytes) {
                        Ok(m) => {
                            self.modpack = self.build_import_preview(m);
                        }
                        Err(e) => {
                            self.modpack = ModpackState::ImportPaste {
                                input: String::new(),
                                error: Some(format!("decode: {e}")),
                            };
                        }
                    },
                    Err(e) => {
                        self.modpack = ModpackState::ImportPaste {
                            input: String::new(),
                            error: Some(format!("read {}: {e}", path.display())),
                        };
                    }
                }
                Task::none()
            }
            M::ImportFilePicked(None) => Task::none(),
            M::PasteInputChanged(s) => {
                if let ModpackState::ImportPaste { input, .. } = &mut self.modpack {
                    *input = s;
                }
                Task::none()
            }
            M::PasteSubmit => {
                let input = match &self.modpack {
                    ModpackState::ImportPaste { input, .. } => input.clone(),
                    _ => return Task::none(),
                };
                match crabby_modpack::decode_any(input.as_bytes()) {
                    Ok(m) => {
                        self.modpack = self.build_import_preview(m);
                    }
                    Err(e) => {
                        self.modpack = ModpackState::ImportPaste {
                            input,
                            error: Some(format!("decode: {e}")),
                        };
                    }
                }
                Task::none()
            }
            M::SelectNewProfileTarget => {
                if let ModpackState::ImportPreview { target, .. } = &mut self.modpack {
                    *target = ImportTarget::NewProfile;
                }
                Task::none()
            }
            M::SelectExistingProfileTarget(name) => {
                if let ModpackState::ImportPreview { target, .. } = &mut self.modpack {
                    *target = ImportTarget::ExistingProfile(name);
                }
                Task::none()
            }
            M::NewProfileNameChanged(s) => {
                if let ModpackState::ImportPreview {
                    new_profile_name, ..
                } = &mut self.modpack
                {
                    *new_profile_name = s;
                }
                Task::none()
            }
            M::ToggleDeactivateExisting => {
                if let ModpackState::ImportPreview {
                    deactivate_existing,
                    ..
                } = &mut self.modpack
                {
                    *deactivate_existing = !*deactivate_existing;
                }
                Task::none()
            }
            M::ToggleOverwriteMcm => {
                if let ModpackState::ImportPreview { overwrite_mcm, .. } = &mut self.modpack {
                    *overwrite_mcm = !*overwrite_mcm;
                }
                Task::none()
            }
            M::ConfirmImport => {
                // Pull the preview state out, then invoke the runner.
                let (manifest, target_profile, deactivate, overwrite) =
                    match std::mem::replace(&mut self.modpack, ModpackState::Idle) {
                        ModpackState::ImportPreview {
                            manifest,
                            new_profile_name,
                            target,
                            deactivate_existing,
                            overwrite_mcm,
                        } => {
                            let target_profile = match target {
                                ImportTarget::NewProfile => new_profile_name,
                                ImportTarget::ExistingProfile(s) => s,
                            };
                            (manifest, target_profile, deactivate_existing, overwrite_mcm)
                        }
                        other => {
                            self.modpack = other;
                            return Task::none();
                        }
                    };
                self.run_import(manifest, target_profile, deactivate, overwrite)
            }
            M::ImportProgress { index, outcome } => {
                if let ModpackState::ImportRunning { statuses, .. } = &mut self.modpack {
                    if let Some(s) = statuses.get_mut(index) {
                        s.outcome = Some(outcome);
                    }
                }
                Task::none()
            }
            M::ImportComplete => {
                let (statuses, target_profile) =
                    match std::mem::replace(&mut self.modpack, ModpackState::Idle) {
                        ModpackState::ImportRunning {
                            statuses,
                            target_profile,
                            ..
                        } => (statuses, target_profile),
                        other => {
                            self.modpack = other;
                            return Task::none();
                        }
                    };
                self.modpack = ModpackState::ImportDone {
                    statuses,
                    target_profile,
                };
                // Refresh the rest of the UI to reflect the new mods/profile.
                self.mods.refresh_rows(self.game_dir.as_deref());
                self.profiles.invalidate();
                self.profiles.refresh(self.game_dir.as_deref());
                Task::none()
            }
            M::Dismiss => {
                self.modpack = ModpackState::Idle;
                Task::none()
            }
        }
    }

    /// Build the export manifest from the active profile + MCM configs.
    fn build_modpack_manifest(&self) -> Result<crabby_modpack::Manifest, String> {
        let game_dir = self
            .game_dir
            .as_ref()
            .ok_or_else(|| "no game directory".to_string())?;
        let cfg = crabby_config::ModConfig::load_or_default(game_dir)
            .map_err(|e| format!("read mod_config.cfg: {e}"))?;
        let active_profile_name = cfg.active_profile.clone();
        let profile = cfg
            .profiles
            .get(&active_profile_name)
            .ok_or_else(|| format!("profile `{active_profile_name}` missing"))?;

        let mut mods: Vec<crabby_modpack::PackMod> = Vec::new();
        for snap in self.mods.installed_snapshot() {
            // Only include enabled mods - that's the "active set" the
            // pack reproduces.
            if !profile
                .mods
                .get(&snap.id)
                .map(|e| e.enabled)
                .unwrap_or(false)
            {
                continue;
            }
            let mcm_config = crabby_config::mcm::find_config_for_mod(&snap.id, &snap.name)
                .and_then(|p| std::fs::read(&p).ok());
            mods.push(crabby_modpack::PackMod {
                id: snap.id,
                name: snap.name,
                version: snap.version,
                mw_id: snap.mw_id,
                mcm_config,
            });
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Ok(crabby_modpack::Manifest {
            schema: crabby_modpack::Manifest::SCHEMA_VERSION,
            name: active_profile_name,
            description: String::new(),
            crabby_version: env!("CARGO_PKG_VERSION").to_string(),
            created_at: now,
            mods,
        })
    }

    fn save_modpack_to_file(&self, path: &std::path::Path) -> Result<(), String> {
        let m = self.build_modpack_manifest()?;
        crabby_modpack::write_pack_file(path, &m).map_err(|e| format!("{e}"))
    }

    /// Compute the import preview state from a decoded manifest.
    fn build_import_preview(&self, manifest: crabby_modpack::Manifest) -> ModpackState {
        // Statuses are recomputed in `run_import` from the same logic;
        // the preview screen renders from `manifest` directly so no
        // stale copy is stored here.
        ModpackState::ImportPreview {
            new_profile_name: manifest.name.clone(),
            target: ImportTarget::NewProfile,
            deactivate_existing: true,
            overwrite_mcm: true,
            manifest,
        }
    }

    /// Kick the import pipeline - resolves each mod, installs missing
    /// ones from MW, drops MCM configs, then updates the active set.
    fn run_import(
        &mut self,
        manifest: crabby_modpack::Manifest,
        target_profile: String,
        deactivate_existing: bool,
        overwrite_mcm: bool,
    ) -> Task<Message> {
        // Recompute statuses (the ImportPreview consumed its copy when
        // we extracted the manifest). Same logic as build_import_preview.
        let snapshot = self.mods.installed_snapshot();
        let mut statuses: Vec<ModImportStatus> = manifest
            .mods
            .iter()
            .map(|pm| {
                let local = snapshot.iter().find(|s| s.id == pm.id);
                let resolution = match (local, pm.mw_id) {
                    (Some(s), _) if s.version == pm.version => ModResolution::Activate,
                    (Some(s), _) => ModResolution::KeepInstalledVersion {
                        local_version: s.version.clone(),
                    },
                    (None, Some(mw_id)) => ModResolution::InstallFromMw { mw_id },
                    (None, None) => ModResolution::SkippedNoSource,
                };
                ModImportStatus {
                    id: pm.id.clone(),
                    name: pm.name.clone(),
                    pack_version: pm.version.clone(),
                    mw_id: pm.mw_id,
                    resolution,
                    outcome: None,
                }
            })
            .collect();

        // Synchronous pre-work: ensure target profile exists, optionally
        // deactivate existing mods, write MCM configs, flip activations.
        // Mod installs are async - they fan out below.
        let game_dir = match self.game_dir.clone() {
            Some(d) => d,
            None => {
                self.modpack = ModpackState::ImportDone {
                    statuses: statuses
                        .into_iter()
                        .map(|mut s| {
                            s.outcome = Some(ModOutcome::Failed("no game directory".into()));
                            s
                        })
                        .collect(),
                    target_profile,
                };
                return Task::none();
            }
        };

        // Apply profile + MCM changes synchronously (cheap file IO).
        if let Err(e) = apply_modpack_profile_changes(
            &game_dir,
            &target_profile,
            &manifest,
            deactivate_existing,
            overwrite_mcm,
            &mut statuses,
        ) {
            tracing::error!(target = "crabby_ui::modpack", err = %e, "profile changes failed");
        }

        // For each mod that needs an install, spawn an async fetch.
        // For everything else, mark Ok now.
        let mut tasks: Vec<Task<Message>> = Vec::new();
        for (idx, status) in statuses.iter_mut().enumerate() {
            match &status.resolution {
                ModResolution::InstallFromMw { mw_id } => {
                    let client = self.mw.clone();
                    let game_dir = game_dir.clone();
                    let mw_id = *mw_id;
                    tasks.push(Task::perform(
                        async move { install_remote_mod(&client, mw_id, &game_dir).await },
                        move |res| {
                            let outcome = match res {
                                Ok(_) => ModOutcome::Ok,
                                Err(e) => ModOutcome::Failed(format!("install: {e}")),
                            };
                            Message::Modpack(crate::modpack_ui::Message::ImportProgress {
                                index: idx,
                                outcome,
                            })
                        },
                    ));
                }
                ModResolution::SkippedNoSource => {
                    status.outcome = Some(ModOutcome::Skipped("not on MW".into()));
                }
                _ => {
                    // Activation already happened in apply_modpack_profile_changes.
                    if status.outcome.is_none() {
                        status.outcome = Some(ModOutcome::Ok);
                    }
                }
            }
        }

        // Add a terminating task that fires Complete after all the
        // install tasks resolve. Task::batch finishes when all child
        // tasks finish; chaining a `.chain(Task::done(Complete))`
        // would do it, but Task doesn't have that. The user-facing
        // summary handles this instead: when all statuses have
        // outcomes set, Complete is surfaced via the ImportProgress
        // handler. For simplicity here, emit Complete at the end of
        // the batch.
        let n_pending = tasks.len();
        self.modpack = ModpackState::ImportRunning {
            manifest,
            target_profile,
            statuses,
        };
        if n_pending == 0 {
            return Task::done(Message::Modpack(crate::modpack_ui::Message::ImportComplete));
        }
        // Append a "complete" trigger after all per-mod tasks land.
        // iced's Task::batch resolves children independently; a
        // separate Task::done at the tail fires Complete. Order isn't
        // strict but the runtime processes them in roughly arrival
        // order - close enough for the summary screen.
        tasks.push(Task::done(Message::Modpack(
            crate::modpack_ui::Message::ImportComplete,
        )));
        Task::batch(tasks)
    }

    /// Re-run the mod analyzer over the active profile and stash the
    /// results on App state. Best-effort - analyzer failures log a
    /// warn and leave the previous results in place. No vanilla
    /// schema is passed (the install path feeds that for schema-aware
    /// grading); UI-side refreshes use baseline severity, which is
    /// fine for the UI - Hard/Warn/Info still split reasonably
    /// without the function-set comparison.
    fn refresh_mod_analysis(&mut self) {
        let Some(dir) = self.game_dir.as_deref() else {
            self.mod_intents.clear();
            self.conflicts.clear();
            return;
        };
        match crabby_mod_analyzer::analyze_active_profile(dir) {
            Ok(intents) => {
                self.conflicts = crabby_mod_analyzer::detect_conflicts(&intents);
                self.mod_intents = intents;
            }
            Err(e) => {
                tracing::warn!(error = %e, "ui: mod analysis refresh failed");
            }
        }
    }

    /// Synchronous lightweight refreshes that complete instantly:
    /// profile config read, diagnostics read, saves rescan, vanilla
    /// save set probe. Heavy work (mod discovery, analyzer, bake
    /// status, mod-index) is dispatched separately via `run_boot_scan`
    /// and applied via `apply_boot_scan`.
    fn do_lightweight_refresh(&mut self) {
        self.profiles.invalidate();
        self.profiles.refresh(self.game_dir.as_deref());
        self.diagnostics.refresh(self.game_dir.as_deref());
        self.saves.invalidate();
        let active = self.profiles.active_profile_name().to_string();
        self.saves.refresh(&active);
        self.refresh_vanilla_save_set();
    }

    /// Apply a worker-thread boot scan result to per-tab state.
    /// `None` means the scan ran without a game dir; clear analyzer
    /// state so the conflict surface doesn't show stale results.
    fn apply_boot_scan(&mut self, result: Option<Box<BootScanResult>>) {
        let Some(scan) = result else {
            self.mod_intents.clear();
            self.conflicts.clear();
            self.bake_status = crabby_install::BakeStatus::Unknown {
                reason: "no game dir set".into(),
            };
            self.mods.refresh_rows(self.game_dir.as_deref());
            return;
        };
        let BootScanResult {
            game_dir,
            cfg,
            discovered,
            intents,
            enabled_intents_idx: _,
            conflicts,
            bake_status,
        } = *scan;
        self.mods
            .refresh_rows_from_discovered(Some(&game_dir), &cfg, discovered);
        self.mod_intents = intents;
        self.conflicts = conflicts;
        self.bake_status = bake_status;
    }

    /// Re-scan `<user>/` root for loose vanilla save files. Cheap
    /// (one read_dir + a handful of metadata calls); safe to run on
    /// every Refresh. Errors leave the previous result in place
    /// rather than blanking the UI - a transient FS hiccup shouldn't
    /// hide an import affordance about to be clicked.
    fn refresh_vanilla_save_set(&mut self) {
        match crabby_config::saves::scan_vanilla_root_saves() {
            Ok(set) => self.vanilla_save_set = set,
            Err(e) => tracing::warn!(error = %e, "ui: vanilla-save scan failed"),
        }
    }

    /// Spawn a background mod-cache rebuild for the current game dir.
    /// Returns `Task::none()` when no dir is set (nothing to cache).
    /// Caller is expected to chain the result into `update`'s return.
    fn spawn_cache_rebuild_task(&self) -> Task<Message> {
        let Some(dir) = self.game_dir.clone() else {
            return Task::none();
        };
        Task::perform(rebuild_mod_cache(dir), Message::ModCacheRebuildFinished)
    }

    fn refresh_bake_status(&mut self) {
        let Some(dir) = self.game_dir.as_deref() else {
            self.bake_status = crabby_install::BakeStatus::Unknown {
                reason: "no game dir set".into(),
            };
            return;
        };
        match crabby_install::bake_status(dir, env!("CARGO_PKG_VERSION")) {
            Ok(s) => self.bake_status = s,
            Err(e) => {
                tracing::warn!(error = %e, "ui: bake status check failed");
                self.bake_status = crabby_install::BakeStatus::Unknown {
                    reason: format!("{e}"),
                };
            }
        }
    }

    fn apply_theme_change(&mut self, change: ThemeChange) {
        match change {
            ThemeChange::Mode(m) => {
                self.theme.mode = m;
            }
            ThemeChange::AccentL(v) => {
                self.theme.accent_l = v.clamp(0.0, 1.0);
            }
            ThemeChange::AccentC(v) => {
                self.theme.accent_c = v.clamp(0.0, 0.4);
            }
            ThemeChange::AccentHue(v) => {
                self.theme.accent_hue = v.rem_euclid(360.0);
            }
            ThemeChange::BgTintHue(v) => {
                self.theme.bg_tint_hue = v.rem_euclid(360.0);
            }
            ThemeChange::ApplySaved([l, c, h]) => {
                self.theme.accent_l = l.clamp(0.0, 1.0);
                self.theme.accent_c = c.clamp(0.0, 0.4);
                self.theme.accent_hue = h.rem_euclid(360.0);
            }
            ThemeChange::SaveCurrent => {
                let entry = [
                    self.theme.accent_l,
                    self.theme.accent_c,
                    self.theme.accent_hue,
                ];
                let saved = &mut self.launcher_config.theme.saved_colors;
                let is_dup = saved.iter().any(|s| {
                    (s[0] - entry[0]).abs() < 0.005
                        && (s[1] - entry[1]).abs() < 0.005
                        && (s[2] - entry[2]).abs() < 1.0
                });
                if !is_dup {
                    if saved.len() >= crate::launcher_config::MAX_SAVED_COLORS {
                        saved.remove(0); // FIFO eviction
                    }
                    saved.push(entry);
                }
            }
            ThemeChange::RemoveSaved(idx) => {
                let saved = &mut self.launcher_config.theme.saved_colors;
                if idx < saved.len() {
                    saved.remove(idx);
                }
            }
            ThemeChange::SetPillOverride { tone_key, lch } => {
                // tone_key validated by the picker; store as-is.
                self.launcher_config
                    .theme
                    .pill_overrides
                    .insert(tone_key, lch);
            }
            ThemeChange::ClearPillOverride { tone_key } => {
                self.launcher_config.theme.pill_overrides.remove(&tone_key);
            }
        }
        let overrides =
            crate::theme::PillOverrides::from_prefs(&self.launcher_config.theme.pill_overrides);
        self.theme.refresh(&overrides);
        self.persist_theme();
    }

    /// Snapshot the live theme + saved colors + pill overrides back
    /// into launcher_config and write to disk. Best-effort.
    fn persist_theme(&mut self) {
        let saved = self.launcher_config.theme.saved_colors.clone();
        let pills = self.launcher_config.theme.pill_overrides.clone();
        self.launcher_config.theme = self.theme.to_prefs(saved, pills);
        self.launcher_config.save();
    }

    /// Global event subscription. Active only while an MCM keycode
    /// field is in capture mode; the next `KeyPressed` /
    /// `MouseButtonPressed` becomes the new binding. Esc cancels,
    /// Backspace/Delete clears (writes `0`).
    #[must_use]
    pub fn subscription(&self) -> iced::Subscription<Message> {
        if self.mods.mcm_capture.is_none() {
            return iced::Subscription::none();
        }
        iced::event::listen_with(|event, _status, _window| {
            use iced::event::Event;
            use iced::keyboard::Event as KbEvent;
            use iced::mouse::{Button as MouseButton, Event as MouseEvent};
            match event {
                Event::Keyboard(KbEvent::KeyPressed { key, .. }) => {
                    let outcome = key_event_to_mods_message(&key);
                    Some(Message::Mods(outcome))
                }
                Event::Mouse(MouseEvent::ButtonPressed(button)) => {
                    let mb = match button {
                        MouseButton::Left => crabby_config::keycode::MB_LEFT,
                        MouseButton::Right => crabby_config::keycode::MB_RIGHT,
                        MouseButton::Middle => crabby_config::keycode::MB_MIDDLE,
                        MouseButton::Back => crabby_config::keycode::MB_XBUTTON1,
                        MouseButton::Forward => crabby_config::keycode::MB_XBUTTON2,
                        MouseButton::Other(n) => i64::from(n),
                    };
                    Some(Message::Mods(mods::Message::McmKeyCaptured(
                        crabby_config::keycode::encode_mouse(mb),
                    )))
                }
                _ => None,
            }
        })
    }

    // ---- private layout helpers ----

    /// Tab bar - logo + 4 tabs + theme toggle. Tabs sit attached to
    /// the body below: the active one fills with bg-2 (the body's
    /// surface) so it visually merges into the content, while
    /// inactive tabs stay transparent on the bg-1 chrome.
    fn tab_bar(&self) -> Element<'_, Message> {
        let palette = self.theme.palette;

        let logo = container(text("c").size(12))
            .padding([2, 6])
            .style(move |_t| iced::widget::container::Style {
                background: Some(iced::Background::Color(palette.accent)),
                text_color: Some(palette.accent_ink),
                border: iced::Border {
                    color: palette.accent_edge,
                    width: 1.0,
                    radius: 5.0.into(),
                },
                ..Default::default()
            });

        let logo_section = row![logo, text("Crabby").size(13).color(palette.fg_0),]
            .spacing(8)
            .align_y(iced::Alignment::Center);

        // Tab label only - design uses small SVG icons but iced's
        // default font lacks the unicode glyphs that were tried, so
        // they rendered as tofu. A real icon font can layer in later.
        let mk_tab = |label: &'static str, tab: Tab| -> Element<'_, Message> {
            let active = tab == self.active_tab;
            let p = palette;
            let label_color = if active { p.fg_0 } else { p.fg_2 };
            let inner = row![text(label).size(12).color(label_color)]
                .spacing(7)
                .align_y(iced::Alignment::Center);
            let mut btn =
                button(inner)
                    .padding([7, 14])
                    .style(move |_t, _s| iced::widget::button::Style {
                        background: Some(iced::Background::Color(if active {
                            p.bg_2
                        } else {
                            iced::Color::TRANSPARENT
                        })),
                        text_color: label_color,
                        border: iced::Border {
                            // No border on the active tab so its bg_2 fill
                            // visually merges with the body below.
                            color: iced::Color::TRANSPARENT,
                            width: 0.0,
                            radius: iced::border::Radius::new(0).top_left(7).top_right(7),
                        },
                        ..Default::default()
                    });
            if !active {
                btn = btn.on_press(Message::TabSelected(tab));
            }
            btn.into()
        };

        let tabs = row![
            mk_tab("Mods", Tab::Mods),
            mk_tab("Saves", Tab::Saves),
            mk_tab("Logs", Tab::Logs),
            mk_tab("Settings", Tab::Settings),
        ]
        .spacing(2);

        // Window control buttons - folded into this strip now that
        // the title bar is gone.
        let p = palette;
        let mk_ctrl = |glyph: &'static str, msg: Message, danger: bool| {
            let p_local = p;
            let fg = if danger { p.err } else { p.fg_2 };
            iced::widget::button(
                text(glyph)
                    .size(13)
                    .color(fg)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(iced::Alignment::Center)
                    .align_y(iced::Alignment::Center),
            )
            .padding(0)
            .width(Length::Fixed(30.0))
            .height(Length::Fixed(24.0))
            .style(move |_t, status| {
                let bg = match status {
                    iced::widget::button::Status::Hovered
                    | iced::widget::button::Status::Pressed => {
                        if danger {
                            iced::Color {
                                a: 0.18,
                                ..p_local.err
                            }
                        } else {
                            p_local.bg_3
                        }
                    }
                    _ => iced::Color::TRANSPARENT,
                };
                iced::widget::button::Style {
                    background: Some(iced::Background::Color(bg)),
                    text_color: fg,
                    border: iced::Border {
                        color: iced::Color::TRANSPARENT,
                        width: 0.0,
                        radius: 4.0.into(),
                    },
                    ..Default::default()
                }
            })
            .on_press(msg)
        };
        let controls = row![
            mk_ctrl("–", Message::WindowMinimize, false),
            mk_ctrl("□", Message::WindowToggleMaximize, false),
            mk_ctrl("×", Message::WindowClose, true),
        ]
        .spacing(2)
        .align_y(iced::Alignment::Center);

        // Vertical divider after the logo.
        let logo_divider = container(text(""))
            .width(Length::Fixed(1.0))
            .height(Length::Fixed(20.0))
            .style(move |_t| iced::widget::container::Style {
                background: Some(iced::Background::Color(palette.line_soft)),
                ..Default::default()
            });

        // The "drag handle" is the empty space between the tabs and
        // the window controls. Pressing in there starts the OS drag
        // loop. The buttons sit outside the mouse_area so button
        // clicks don't double-fire as drag initiation.
        let drag_spacer =
            iced::widget::mouse_area(container(text("")).width(Length::Fill).height(Length::Fill))
                .on_press(Message::WindowDragStart);

        let bar = row![
            logo_section,
            container(text("")).width(Length::Fixed(8.0)),
            logo_divider,
            container(text("")).width(Length::Fixed(6.0)),
            tabs,
            drag_spacer,
            controls,
        ]
        .spacing(8)
        .padding([0, 10])
        .align_y(iced::Alignment::End);

        container(bar)
            .style(band_style(palette, SurfaceKind::Bg1))
            .width(Length::Fill)
            .height(Length::Fixed(40.0))
            .into()
    }

    /// Profile bar - second band. Avatar + name dropdown stub +
    /// stats + Edit + Launch.
    fn profile_bar(&self) -> Element<'_, Message> {
        let palette = self.theme.palette;
        let active_profile = self.profiles.active_profile_name();

        // Avatar - small accent-filled square with the profile's
        // first letter. Uses padding (not center_x/y on a fixed
        // container - those expand to fill the parent).
        let avatar_letter = active_profile
            .chars()
            .next()
            .unwrap_or('?')
            .to_ascii_uppercase()
            .to_string();
        let avatar = container(text(avatar_letter).size(14).color(palette.accent_ink))
            .padding([6, 11])
            .style(move |_t| iced::widget::container::Style {
                background: Some(iced::Background::Color(palette.accent)),
                text_color: Some(palette.accent_ink),
                border: iced::Border {
                    color: palette.accent_edge,
                    width: 1.0,
                    radius: 7.0.into(),
                },
                ..Default::default()
            });

        // Eyebrow + styled pick_list for the active-profile name.
        // iced 0.13's pick_list takes its own style so it picks up
        // our palette rather than the system look.
        let profile_options: Vec<String> = self.profiles.all_profiles().to_vec();
        let p = palette;
        let picker = pick_list(profile_options, Some(active_profile.to_string()), |s| {
            Message::Profiles(profiles::Message::SwitchProfile(s))
        })
        .text_size(14)
        .padding([4, 10])
        .style(move |_t, _s| iced::widget::pick_list::Style {
            background: iced::Background::Color(iced::Color::TRANSPARENT),
            text_color: p.fg_0,
            placeholder_color: p.fg_3,
            handle_color: p.fg_2,
            border: iced::Border {
                color: iced::Color::TRANSPARENT,
                width: 0.0,
                radius: 6.0.into(),
            },
        });
        // Adjacent profile-action buttons. Live next to the dropdown
        // since iced 0.14's `pick_list` doesn't allow custom footer
        // entries - "+ Create new" and "✎ Edit" surface as small
        // buttons instead. Keeps the dropdown a pure switcher.
        let new_profile_btn = button(text("+").size(13))
            .padding(iced::Padding {
                top: 2.0,
                right: 8.0,
                bottom: 2.0,
                left: 8.0,
            })
            .style(button_style(palette, ButtonKind::Ghost))
            .on_press(Message::Profiles(profiles::Message::OpenCreateModal));
        let edit_profile_btn = button(text("✎").size(13))
            .padding(iced::Padding {
                top: 2.0,
                right: 8.0,
                bottom: 2.0,
                left: 8.0,
            })
            .style(button_style(palette, ButtonKind::Ghost))
            .on_press(Message::Profiles(profiles::Message::OpenEditModal));
        let picker_row = row![picker, new_profile_btn, edit_profile_btn]
            .spacing(4)
            .align_y(iced::Alignment::Center);
        let name_block = column![
            text("ACTIVE PROFILE").size(10).color(palette.fg_2),
            picker_row,
        ]
        .spacing(0);

        // Stats - `N mods · 1.2 GB · last played -`. Size is summed
        // from currently-enabled rows; last-played stays as a
        // placeholder until save-file timestamps are wired.
        let stats_dim_a = text("·").size(11).color(palette.fg_3);
        let stats_dim_b = text("·").size(11).color(palette.fg_3);
        let stats = row![
            text(format!("{} mods", self.profiles.active_mod_count()))
                .size(11)
                .color(palette.fg_1),
            stats_dim_a,
            text(crate::tabs::mods::fmt_size(self.mods.enabled_size_bytes()))
                .size(11)
                .color(palette.fg_2),
            stats_dim_b,
            text(format!(
                "last played {}",
                fmt_relative_time(
                    self.launcher_config
                        .last_played
                        .get(self.profiles.active_profile_name())
                        .copied(),
                )
            ))
            .size(11)
            .color(palette.fg_2),
        ]
        .spacing(8)
        .align_y(iced::Alignment::Center);

        // Launch is gated on a small state machine. Disabled states
        // surface a label that tells *why* - first-run testers were
        // trying to launch before baking and getting cryptic failures.
        //
        // The button is adaptive: when the baked PCK matches the
        // current enabled-mods set it just launches; when it's out of
        // date (or absent) it bakes first, then launches.
        let baked = self.diagnostics.is_baked();
        let snapshot_ready = self.diagnostics.snapshot_populated();
        let needs_bake = self.bake_status.needs_bake();
        let (launch_label, launch_msg, launch_enabled): (&str, Message, bool) =
            match (&self.install, &self.launch) {
                (_, LaunchStatus::Launching) => ("Launching…", Message::LaunchGame, false),
                (InstallStatus::Running, _) => ("Baking…", Message::LaunchGame, false),
                (InstallStatus::Failed(_), _) => ("Fix bake first", Message::LaunchGame, false),
                _ if !snapshot_ready => ("Loading…", Message::LaunchGame, false),
                _ if !baked => ("Install first", Message::LaunchGame, false),
                _ if self.game_dir.is_none() => ("Set game dir", Message::LaunchGame, false),
                _ if needs_bake => ("Bake & Launch", Message::BakeAndLaunch, true),
                _ => ("Launch game", Message::LaunchGame, true),
            };
        let launch_kind = if launch_enabled {
            ButtonKind::Primary
        } else {
            ButtonKind::Default
        };
        let mut launch_btn = button(text(launch_label).size(13))
            .padding([6, 14])
            .style(button_style(palette, launch_kind));
        if launch_enabled {
            launch_btn = launch_btn.on_press(launch_msg);
        }

        // Install button - kicks the bake pipeline. Disabled while a
        // bake is already in flight; label changes to reflect status.
        let (install_label, install_kind, install_enabled) = match &self.install {
            InstallStatus::Idle => ("Install / Rebake", ButtonKind::Default, true),
            InstallStatus::Running => ("Baking...", ButtonKind::Default, false),
            InstallStatus::Succeeded(_) => ("Re-bake", ButtonKind::Default, true),
            InstallStatus::Failed(_) => ("Retry", ButtonKind::Default, true),
        };
        let mut install_btn = button(text(install_label).size(11))
            .padding([4, 10])
            .style(button_style(palette, install_kind));
        if install_enabled && self.game_dir.is_some() {
            install_btn = install_btn.on_press(Message::InstallStart);
        }

        let bar = row![
            avatar,
            name_block,
            stats,
            style::hspace(),
            install_btn,
            launch_btn,
        ]
        .spacing(14)
        .padding([10, 18])
        .align_y(iced::Alignment::Center);

        container(bar)
            .style(band_style(palette, SurfaceKind::Bg2))
            .width(Length::Fill)
            .into()
    }

    /// Body - active tab's contents, or the first-run picker banner
    /// when no game directory has been resolved.
    fn body(&self) -> Element<'_, Message> {
        let palette = self.theme.palette;
        if self.game_dir.is_none() {
            return container(self.first_run_picker())
                .style(surface_style(palette, SurfaceKind::Bg2))
                .width(Length::Fill)
                .height(Length::Fill)
                .into();
        }
        let body_inner: Element<'_, Message> = match self.active_tab {
            Tab::Mods => self
                .mods
                .view(
                    self.game_dir.as_deref(),
                    &self.conflicts,
                    &self.launcher_config.mod_conflict_panel_collapsed,
                    &palette,
                )
                .map(Message::Mods),
            Tab::Saves => self
                .saves
                .view(
                    self.profiles.active_profile_name(),
                    self.vanilla_save_set.as_ref(),
                    self.profiles.all_profiles().to_vec(),
                    &palette,
                )
                .map(Message::Saves),
            Tab::Logs => self.logs.view(&palette).map(Message::Logs),
            Tab::Settings => self
                .settings
                .view(
                    self.game_dir.as_deref(),
                    &self.theme,
                    &self.launcher_config.theme.saved_colors,
                    &self.launcher_config.theme.pill_overrides,
                    &self.diagnostics,
                    self.launcher_config.confirm_destructive_actions,
                    &palette,
                )
                .map(Message::Settings),
        };

        // First-run banner - only when a game dir is set but the
        // bake hasn't run yet. Sits above the active tab so the
        // path-to-success is unambiguous: bake → launch.
        let needs_first_bake = self.diagnostics.snapshot_populated()
            && !self.diagnostics.is_baked()
            && !matches!(self.install, InstallStatus::Running);
        let body_with_banner: Element<'_, Message> = if needs_first_bake {
            let banner = self.first_run_banner(palette);
            iced::widget::column![banner, body_inner].spacing(0).into()
        } else {
            body_inner
        };

        container(body_with_banner)
            .style(surface_style(palette, SurfaceKind::Bg2))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// Banner shown when a game dir is set but the PCK hasn't been
    /// baked yet. Surfaces the same Install action as the profile-bar
    /// button so it's discoverable inline.
    fn first_run_banner(&self, palette: crate::theme::Palette) -> Element<'_, Message> {
        let install_btn = button(text("Install / Bake").size(12))
            .padding([5, 14])
            .style(button_style(palette, ButtonKind::Primary))
            .on_press(Message::InstallStart);
        let body = iced::widget::row![
            text("Click ").size(12).color(palette.fg_1),
            text("Install").size(12).color(palette.accent),
            text(" to bake the PCK before launching the game. This processes RTV's scripts so mods can hook in.")
                .size(12)
                .color(palette.fg_1),
            style::hspace(),
            install_btn,
        ]
        .spacing(0)
        .padding([10, 18])
        .align_y(iced::Alignment::Center);
        container(body)
            .style(surface_style(palette, SurfaceKind::Bg3))
            .width(Length::Fill)
            .into()
    }

    /// First-run picker shown when no game directory has been
    /// resolved. Centered card with copy + a Browse button.
    fn first_run_picker(&self) -> Element<'_, Message> {
        let p = self.theme.palette;
        let title = text("Select your Road to Vostok install")
            .size(20)
            .color(p.fg_0);
        let body = text(
            "We couldn't find the game automatically. \
             Pick the folder that contains RTV.exe (or RTV.x86_64) and RTV.pck.",
        )
        .size(13)
        .color(p.fg_1);
        let hint = text("This choice is saved, you only need to pick once.")
            .size(11)
            .color(p.fg_3);
        let browse = button(text("Browse for game folder").size(13))
            .padding([8, 16])
            .style(button_style(p, ButtonKind::Primary))
            .on_press(Message::PickGameDir);

        let card_inner = column![title, body, hint, browse]
            .spacing(12)
            .align_x(iced::Alignment::Center)
            .padding(24);

        let card = container(card_inner)
            .style(move |_t| iced::widget::container::Style {
                background: Some(iced::Background::Color(p.bg_3)),
                text_color: Some(p.fg_0),
                border: iced::Border {
                    color: p.line_soft,
                    width: 1.0,
                    radius: 8.0.into(),
                },
                ..Default::default()
            })
            .max_width(520);

        container(card)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }

    /// Status bar - bottom band. Game path on the left, install
    /// status (when interesting) in the middle, crabby version on
    /// the right.
    fn status_bar(&self) -> Element<'_, Message> {
        let palette = self.theme.palette;
        // Green dot - RTV install detected and validated; tinted red
        // when no game dir is present for a quick read.
        let (dot_color, status_word) = if self.game_dir.is_some() {
            (palette.ok, "ready")
        } else {
            (palette.err, "no game dir")
        };
        let ok_dot = container(text(""))
            .width(Length::Fixed(6.0))
            .height(Length::Fixed(6.0))
            .style(move |_t| iced::widget::container::Style {
                background: Some(iced::Background::Color(dot_color)),
                border: iced::Border {
                    color: dot_color,
                    width: 0.0,
                    radius: 999.0.into(),
                },
                ..Default::default()
            });

        let game_path = match &self.game_dir {
            Some(p) => p.display().to_string(),
            None => "—".to_string(),
        };

        // Status label precedence (highest first):
        //   1. install in flight ("baking…") - explicit user action
        //   2. launch in flight ("launching…") - explicit user action
        //   3. boot scan in flight ("scanning mods…") - background work
        //   4. last terminal install/launch outcome (✓/✗)
        // The scan only wins when no explicit user-driven action is
        // running, so a Bake or Launch button press always supersedes
        // the background-scan label.
        let (install_label, install_tone) = match (&self.install, &self.launch) {
            (InstallStatus::Running, _) => ("baking…".to_string(), TextTone::Fg2),
            (_, LaunchStatus::Launching) => ("launching…".to_string(), TextTone::Fg2),
            (_, LaunchStatus::Launched) => ("✓ game launched".to_string(), TextTone::Ok),
            (_, LaunchStatus::Failed(msg)) => (format!("✗ {msg}"), TextTone::Err),
            (InstallStatus::Succeeded(msg), _) => (format!("✓ {msg}"), TextTone::Ok),
            (InstallStatus::Failed(msg), _) => (format!("✗ {msg}"), TextTone::Err),
            (InstallStatus::Idle, LaunchStatus::Idle) if self.boot_scan_in_flight => {
                ("scanning mods…".to_string(), TextTone::Fg2)
            }
            (InstallStatus::Idle, LaunchStatus::Idle) => (String::new(), TextTone::Fg2),
        };

        let dim = text("·").size(11).color(palette.fg_3);
        let dim2 = text("·").size(11).color(palette.fg_3);

        let theme_label = match self.theme.mode {
            crate::theme::Mode::Dark => "🌙",
            crate::theme::Mode::Light => "☀",
        };
        let theme_btn = button(text(theme_label).size(11))
            .padding(style::ButtonSize::Sm.padding())
            .style(button_style(palette, ButtonKind::Ghost))
            .on_press(Message::ToggleTheme);

        let bar = row![
            row![
                ok_dot,
                text(format!("game {status_word}"))
                    .size(11)
                    .color(palette.fg_2),
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center),
            dim,
            text(game_path)
                .size(11)
                .style(text_style(palette, TextTone::Fg2)),
            dim2,
            text(install_label)
                .size(11)
                .color(text_color(palette, install_tone)),
            style::hspace(),
            text(format!("crabby v{APP_VERSION}"))
                .size(11)
                .color(text_color(palette, TextTone::Fg3)),
            theme_btn,
        ]
        .spacing(10)
        .padding([5, 14])
        .align_y(iced::Alignment::Center);

        container(bar)
            .style(band_style(palette, SurfaceKind::Bg1))
            .width(Length::Fill)
            .into()
    }
}

/// Granular theme changes from the Appearance sub-view or floating
/// quick-theme panel. App applies the change to `self.theme`, rebuilds
/// the palette, and persists to `launcher_config`.
#[derive(Debug, Clone)]
pub enum ThemeChange {
    /// Set dark or light mode.
    Mode(crate::theme::Mode),
    /// Set accent OKLCH lightness [0.0, 1.0]. Practical: 0.40–0.85.
    AccentL(f32),
    /// Set accent OKLCH chroma [0.0, 0.40]. Practical: 0.0–0.30.
    AccentC(f32),
    /// Set accent hue in degrees [0, 360).
    AccentHue(f32),
    /// Set background-tint hue in degrees [0, 360).
    BgTintHue(f32),
    /// Apply a saved swatch (L, C, H) - drops it into the live accent.
    ApplySaved([f32; 3]),
    /// Save the current accent into the saved-colors row. Caps at
    /// `MAX_SAVED_COLORS`; oldest entries are evicted.
    SaveCurrent,
    /// Remove a saved color by index in the persisted vec.
    RemoveSaved(usize),
    /// Override the named pill tone (`"ok"`, `"warn"`, `"err"`) with
    /// a specific OKLCH triple.
    SetPillOverride {
        /// Tone identifier - one of `"ok"`, `"warn"`, `"err"`.
        tone_key: String,
        /// Override color as `(L, C, H)`.
        lch: [f32; 3],
    },
    /// Drop a per-tone override, returning the tone to its design default.
    ClearPillOverride {
        /// Tone identifier.
        tone_key: String,
    },
}

/// Install/rebake status, owned by [`App`] and surfaced via the
/// install button + status bar. Transitions: `Idle` →
/// `Running` → either `Succeeded(message)` or `Failed(message)`.
/// Another bake can fire from any non-`Running` state.
#[derive(Debug, Clone)]
pub enum InstallStatus {
    /// No bake has been triggered this session.
    Idle,
    /// Bake is in flight on a background thread.
    Running,
    /// Last bake completed; carries a one-line summary.
    Succeeded(String),
    /// Last bake failed; carries the error message.
    Failed(String),
}

/// Outcome of a background-spawned install. Plain `String`s rather
/// than threading `CrabbyError` through `Message::Clone` (errors
/// aren't `Clone`).
#[derive(Debug, Clone)]
pub enum InstallOutcome {
    /// Bake succeeded; `String` is the human-readable summary.
    Success(String),
    /// Bake failed; `String` is the error message.
    Failed(String),
}

/// Launch state machine. Transitions: `Idle` → `Launching` →
/// `Launched` | `Failed(message)`. The running process is not
/// tracked - launches can repeat; whether a previous instance
/// still exists is the OS's problem.
#[derive(Debug, Clone)]
pub enum LaunchStatus {
    /// No launch attempted yet.
    Idle,
    /// Spawning the binary on a worker thread.
    Launching,
    /// Process spawned successfully (not tracked after that).
    Launched,
    /// Spawn failed; carries the error message.
    Failed(String),
}

/// Outcome of a launch attempt. `Spawned` is fire-and-forget - once
/// the process is alive control hands off to the OS.
#[derive(Debug, Clone)]
pub enum LaunchOutcome {
    /// Process spawned successfully.
    Spawned,
    /// Spawn failed with this message.
    Failed(String),
}

/// Consolidated boot-scan result. Carries everything the apply path
/// needs to fan out to per-tab state: the mod-tab rows source, the
/// analyzer artifacts, and the bake-status check, all derived from a
/// single archive walk + single per-mod GDScript pass.
///
/// The `intents` and `enabled_intents_idx` fields together support the
/// conflict surface (uses all intents) and the bake-status digest
/// (uses the enabled subset) without re-running the analyzer.
#[derive(Debug, Clone)]
pub struct BootScanResult {
    /// Game dir the scan ran against. Stored back so the apply path
    /// doesn't pick up a stale `self.game_dir` if it changed
    /// mid-scan (rare, but the apply would silently target the old dir).
    pub game_dir: PathBuf,
    /// Pre-loaded mod config, reused by the mod-tab rows path.
    pub cfg: crabby_config::ModConfig,
    /// Discovered mods (one archive walk's worth). Consumed by the
    /// mod-tab rows path; the mod-index rebuild already happened on
    /// the worker thread.
    pub discovered: Vec<crabby_manifest::DiscoveredMod>,
    /// Every discovered mod's analyzer intent, enabled or not.
    pub intents: Vec<crabby_mod_analyzer::ModIntent>,
    /// Indices into `intents` for mods enabled in the active profile.
    /// Pre-computed so the apply path doesn't re-derive the subset.
    pub enabled_intents_idx: Vec<usize>,
    /// Conflict surface, derived from `intents` on the worker thread.
    pub conflicts: Vec<crabby_mod_analyzer::Conflict>,
    /// Bake-status verdict, computed on the worker thread using the
    /// enabled-mod intents subset.
    pub bake_status: crabby_install::BakeStatus,
}

/// Run the heavy boot work on a worker thread. One ModConfig load,
/// one archive walk via `discover_mods_for_config`, one per-mod
/// GDScript walk via the analyzer, plus the mod-index rebuild and
/// bake-status digest, all sharing the same in-memory state.
///
/// Spawned from `Refresh` (and the boot-time auto-Refresh main.rs
/// fires after first paint) so the UI thread never blocks on archive
/// IO. Returns `None` if no game dir is set.
async fn run_boot_scan(game_dir: Option<PathBuf>) -> Option<Box<BootScanResult>> {
    let dir = game_dir?;
    let res = tokio::task::spawn_blocking(move || -> Option<Box<BootScanResult>> {
        let cfg = match crabby_config::ModConfig::load_or_default(&dir) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "ui: boot ModConfig load failed");
                return None;
            }
        };
        let discovered = crabby_config::discover_mods_for_config(&dir, &cfg)
            .inspect_err(|e| tracing::warn!(error = %e, "ui: boot mod discovery failed"))
            .unwrap_or_default();

        // NB: defer the mod_index rebuild until AFTER the analyzer
        // scan so we can thread per-mod overlay source paths into
        // each ModIndexEntry. The mod-cache rebuild (chained from
        // the launcher elsewhere) reads those paths and strips the
        // overlay sources from the runtime cache, preventing
        // duplicate `class_name` bindings on overlays that ship a
        // class_name script.

        let scan = match crabby_mod_analyzer::scan_active_profile(&dir) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "ui: boot analyzer scan failed");
                // Without analyzer data the apply path falls back to
                // a synchronous bake-status check on the UI thread,
                // which is the same behavior the pre-async path had.
                return Some(Box::new(BootScanResult {
                    game_dir: dir,
                    cfg,
                    discovered,
                    intents: Vec::new(),
                    enabled_intents_idx: Vec::new(),
                    conflicts: Vec::new(),
                    bake_status: crabby_install::BakeStatus::Unknown {
                        reason: "analyzer scan failed".into(),
                    },
                }));
            }
        };

        let conflicts = crabby_mod_analyzer::detect_conflicts(&scan.all_intents);

        // Mod-index rebuild now that the analyzer told us which
        // overlay sources each mod ships. Threading the source
        // paths through to ModIndexEntry lets the mod-cache rebuild
        // strip them from the runtime mount, avoiding duplicate
        // class_name bindings on overlays.
        let overlay_sources_by_mod_id: std::collections::BTreeMap<String, Vec<String>> = scan
            .all_intents
            .iter()
            .filter(|i| !i.overlay_writes.is_empty())
            .map(|i| {
                let sources: Vec<String> = i
                    .overlay_writes
                    .iter()
                    .filter_map(|w| w.source_path.clone())
                    .collect();
                (i.mod_id.clone(), sources)
            })
            .collect();
        if let Err(e) =
            crabby_config::mod_index::rebuild_and_save_from_discovered_with_overlays(
                &dir,
                &cfg,
                &discovered,
                &overlay_sources_by_mod_id,
            )
        {
            tracing::warn!(error = %e, "ui: mod_index refresh failed");
        }

        // Map enabled IDs back to indices into `all_intents`. Cheaper
        // than threading two parallel Vecs through the message and
        // re-stitching on the apply side.
        let enabled_set: std::collections::BTreeSet<&str> =
            scan.enabled_ids.iter().map(String::as_str).collect();
        let enabled_intents_idx: Vec<usize> = scan
            .all_intents
            .iter()
            .enumerate()
            .filter(|(_, i)| enabled_set.contains(i.mod_id.as_str()))
            .map(|(idx, _)| idx)
            .collect();

        let enabled_intents_iter = enabled_intents_idx.iter().map(|&i| &scan.all_intents[i]);
        let bake_status = match crabby_install::bake_status_from_intents(
            &dir,
            env!("CARGO_PKG_VERSION"),
            enabled_intents_iter,
        ) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "ui: bake status check failed");
                crabby_install::BakeStatus::Unknown {
                    reason: format!("{e}"),
                }
            }
        };

        Some(Box::new(BootScanResult {
            game_dir: dir,
            cfg,
            discovered,
            intents: scan.all_intents,
            enabled_intents_idx,
            conflicts,
            bake_status,
        }))
    })
    .await;
    match res {
        Ok(r) => r,
        Err(join_err) => {
            tracing::warn!(error = %join_err, "ui: boot scan task panicked");
            None
        }
    }
}

/// Background pre-rewrite of every enabled vmz/zip archive into the
/// `.crabby/mod_cache/` cache. Spawned via `Task::perform` after every
/// action that touches the enabled-mods set so the runtime shim never
/// sees a stale cache. Returns the number of archives (re)written.
async fn rebuild_mod_cache(game_dir: PathBuf) -> Result<usize, String> {
    let res = tokio::task::spawn_blocking(move || -> Result<usize, String> {
        // Re-load the index from disk rather than threading it through
        // - `rebuild_and_save` always runs synchronously before this
        // task is spawned, so the index file on disk is current.
        let index = crabby_config::mod_index::ModIndex::load_or_default(&game_dir)
            .map_err(|e| format!("load index: {e}"))?;
        crabby_config::mod_cache::rebuild_for_enabled(&game_dir, &index)
            .map_err(|e| format!("rebuild cache: {e}"))
    })
    .await;
    match res {
        Ok(r) => r,
        Err(join_err) => Err(format!("cache task panicked: {join_err}")),
    }
}

async fn run_install(game_dir: PathBuf) -> InstallOutcome {
    let res = tokio::task::spawn_blocking(move || {
        crabby_install::install(&crabby_install::InstallOptions {
            game_dir: &game_dir,
            crabby_version: env!("CARGO_PKG_VERSION"),
            force: true,
        })
    })
    .await;
    match res {
        Ok(Ok(report)) => {
            let label = match report.action {
                crabby_install::InstallAction::AlreadyCurrent => "already current",
                crabby_install::InstallAction::FreshInstall => "fresh install",
                crabby_install::InstallAction::RebakedStale => "rebaked",
                crabby_install::InstallAction::RepairedPlacement => "repaired placement",
            };
            let msg = match report.bake {
                Some(b) => format!(
                    "{label} - {} script(s) rewritten, {} hooks",
                    b.scripts_rewritten, b.stats.total_hooks,
                ),
                None => label.to_string(),
            };
            InstallOutcome::Success(msg)
        }
        Ok(Err(e)) => InstallOutcome::Failed(format!("{e}")),
        Err(join_err) => InstallOutcome::Failed(format!("install task panicked: {join_err}")),
    }
}

/// Format a Unix timestamp as a coarse-grained relative time
/// (`"just now"`, `"5m ago"`, `"3h ago"`, `"2d ago"`, `"never"`).
/// `None` → `"never"`. Used by the profile bar's last-played stat.
fn fmt_relative_time(secs: Option<u64>) -> String {
    let Some(t) = secs else { return "never".into() };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if now < t {
        return "just now".into(); // clock skew; don't show "in 3s"
    }
    let age = now - t;
    match age {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m ago", age / 60),
        3600..=86_399 => format!("{}h ago", age / 3600),
        _ => format!("{}d ago", age / 86_400),
    }
}

fn resolve_game_dir(
    cli_override: Option<&std::path::Path>,
    cfg: &LauncherConfig,
) -> Option<PathBuf> {
    if let Some(p) = cli_override
        && crabby_install::validate_game_dir(p).is_ok()
    {
        return Some(p.to_path_buf());
    }
    if let Ok(env) = std::env::var("CRABBY_GAME_DIR") {
        let p = PathBuf::from(env);
        if crabby_install::validate_game_dir(&p).is_ok() {
            return Some(p);
        }
        tracing::warn!(path = %p.display(), "ui: CRABBY_GAME_DIR points at a non-RTV directory; ignoring");
    }
    if let Some(p) = cfg.game_dir.as_deref() {
        if crabby_install::validate_game_dir(p).is_ok() {
            return Some(p.to_path_buf());
        }
        tracing::warn!(path = %p.display(), "ui: persisted game_dir no longer valid; falling back to auto-detect");
    }
    crabby_install::detect_game_dir().ok()
}

/// Map an iced `Key` event to the matching `mods::Message`. Returns
/// `Cancel` for Escape, `Captured(0)` for Backspace/Delete (clears
/// the binding), and `Captured(godot_keycode)` for everything else
/// we can translate. Unrecognised keys produce `Captured(0)` to keep
/// the user from getting stuck in capture mode on a weird key.
fn key_event_to_mods_message(key: &iced::keyboard::Key) -> mods::Message {
    use crabby_config::keycode::KEY_SPECIAL;
    use iced::keyboard::Key;
    use iced::keyboard::key::Named;

    if let Key::Named(Named::Escape) = key {
        return mods::Message::McmKeyCaptureCancel;
    }
    let code: i64 = match key {
        Key::Named(named) => match named {
            Named::Tab => KEY_SPECIAL | 0x02,
            Named::Backspace | Named::Delete => 0,
            Named::Enter => KEY_SPECIAL | 0x05,
            Named::Insert => KEY_SPECIAL | 0x07,
            Named::Pause => KEY_SPECIAL | 0x09,
            Named::PrintScreen => KEY_SPECIAL | 0x0A,
            Named::Home => KEY_SPECIAL | 0x0D,
            Named::End => KEY_SPECIAL | 0x0E,
            Named::ArrowLeft => KEY_SPECIAL | 0x0F,
            Named::ArrowUp => KEY_SPECIAL | 0x10,
            Named::ArrowRight => KEY_SPECIAL | 0x11,
            Named::ArrowDown => KEY_SPECIAL | 0x12,
            Named::PageUp => KEY_SPECIAL | 0x13,
            Named::PageDown => KEY_SPECIAL | 0x14,
            Named::Shift => KEY_SPECIAL | 0x15,
            Named::Control => KEY_SPECIAL | 0x16,
            Named::Meta => KEY_SPECIAL | 0x17,
            Named::Alt => KEY_SPECIAL | 0x18,
            Named::CapsLock => KEY_SPECIAL | 0x19,
            Named::NumLock => KEY_SPECIAL | 0x1A,
            Named::ScrollLock => KEY_SPECIAL | 0x1B,
            Named::F1 => KEY_SPECIAL | 0x1C,
            Named::F2 => KEY_SPECIAL | 0x1D,
            Named::F3 => KEY_SPECIAL | 0x1E,
            Named::F4 => KEY_SPECIAL | 0x1F,
            Named::F5 => KEY_SPECIAL | 0x20,
            Named::F6 => KEY_SPECIAL | 0x21,
            Named::F7 => KEY_SPECIAL | 0x22,
            Named::F8 => KEY_SPECIAL | 0x23,
            Named::F9 => KEY_SPECIAL | 0x24,
            Named::F10 => KEY_SPECIAL | 0x25,
            Named::F11 => KEY_SPECIAL | 0x26,
            Named::F12 => KEY_SPECIAL | 0x27,
            Named::Space => 0x20,
            _ => 0,
        },
        Key::Character(c) => {
            // Take the first char and uppercase it so 'a' and 'A'
            // both map to Godot's `Key::A == 0x41`.
            c.chars()
                .next()
                .map(|ch| {
                    let upper = ch.to_ascii_uppercase();
                    i64::from(upper as u32)
                })
                .unwrap_or(0)
        }
        Key::Unidentified => 0,
    };
    mods::Message::McmKeyCaptured(code)
}

/// Spawn the RTV game binary as a detached child process. Since the
/// crabby shim is baked into the PCK by the install step, vanilla's
/// own executable launches the modded game - no wrapper needed.
///
/// The child is fully detached: stdio is redirected to null and the
/// [`Child`] handle is dropped without waiting, so the launcher can
/// exit (or rebake) without affecting the running game.
async fn launch_game(game_dir: PathBuf) -> LaunchOutcome {
    let res = tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        let bin = match crabby_install::find_game_binary(&game_dir) {
            Ok(p) => p,
            Err(e) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("{e}"),
                ));
            }
        };
        let mut cmd = std::process::Command::new(&bin);
        cmd.current_dir(&game_dir)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let _child = cmd.spawn()?;
        // Drop the handle - child runs detached, OS reaps it when it exits.
        Ok(())
    })
    .await;
    match res {
        Ok(Ok(())) => LaunchOutcome::Spawned,
        Ok(Err(e)) => LaunchOutcome::Failed(format!("{e}")),
        Err(join_err) => LaunchOutcome::Failed(format!("launch task panicked: {join_err}")),
    }
}

/// File picker for "Save modpack as…" (export-to-file). Returns the
/// chosen path (with `.crabbypack` extension forced if missing) or
/// `None` if cancelled.
async fn pick_modpack_save_path() -> Option<PathBuf> {
    let picked = rfd::AsyncFileDialog::new()
        .set_title("Save modpack as")
        .add_filter("Crabby modpack", &[crabby_modpack::FILE_EXTENSION])
        .set_file_name(format!("modpack.{}", crabby_modpack::FILE_EXTENSION))
        .save_file()
        .await?;
    let mut p = picked.path().to_path_buf();
    if p.extension().and_then(|x| x.to_str()) != Some(crabby_modpack::FILE_EXTENSION) {
        let mut s = p.into_os_string();
        s.push(format!(".{}", crabby_modpack::FILE_EXTENSION));
        p = PathBuf::from(s);
    }
    Some(p)
}

/// File picker for "Open modpack file…" (import-from-file).
async fn pick_modpack_open_path() -> Option<PathBuf> {
    let picked = rfd::AsyncFileDialog::new()
        .set_title("Open modpack")
        .add_filter("Crabby modpack", &[crabby_modpack::FILE_EXTENSION])
        .pick_file()
        .await?;
    Some(picked.path().to_path_buf())
}

/// Apply modpack-driven profile + MCM changes to disk. Synchronous -
/// just file IO. Mod installs (download from MW) are handled by the
/// async pipeline that calls this; only the local config side is
/// handled here.
///
/// Steps:
/// 1. Ensure `target_profile` exists in `mod_config.cfg` (create if needed).
/// 2. Set it as the active profile.
/// 3. If `deactivate_existing` is true, mark every mod in the target
///    profile as disabled before applying the pack's activations.
/// 4. For each pack mod that's already installed locally, set
///    `enabled=true` in the target profile.
/// 5. Drop MCM configs (when `overwrite_mcm` is true OR no existing
///    config). Skipped if the mod isn't installed yet - the post-install
///    pass will re-enter for those.
fn apply_modpack_profile_changes(
    game_dir: &std::path::Path,
    target_profile: &str,
    manifest: &crabby_modpack::Manifest,
    deactivate_existing: bool,
    overwrite_mcm: bool,
    statuses: &mut [ModImportStatus],
) -> Result<(), String> {
    use crabby_config::{ModConfig, ModEntry, Profile};

    let mut cfg =
        ModConfig::load_or_default(game_dir).map_err(|e| format!("read mod_config.cfg: {e}"))?;

    // Ensure profile exists.
    if !cfg.profiles.contains_key(target_profile) {
        cfg.profiles
            .insert(target_profile.to_string(), Profile::default());
    }
    cfg.active_profile = target_profile.to_string();

    let profile = cfg.profiles.get_mut(target_profile).expect("just inserted");

    // Optionally deactivate everything currently in the profile.
    if deactivate_existing {
        for entry in profile.mods.values_mut() {
            entry.enabled = false;
        }
    }

    // Activate every pack mod we already have locally; the rest will
    // be enabled by the install pipeline once the file lands.
    for pm in &manifest.mods {
        if let Some(entry) = profile.mods.get_mut(&pm.id) {
            entry.enabled = true;
        } else {
            // Install pipeline will fill version after the download.
            profile.mods.insert(
                pm.id.clone(),
                ModEntry {
                    enabled: true,
                    version: pm.version.clone(),
                    priority_override: None,
                },
            );
        }
    }

    cfg.save(game_dir)
        .map_err(|e| format!("write mod_config.cfg: {e}"))?;
    // Don't refresh mod_index here. The next bake handles it with
    // analyzer overlay-source info. The modpack import will trigger
    // a bake on its own (the user is expected to bake after import).

    // MCM configs.
    let mcm_root = match crabby_config::mcm::mcm_root() {
        Some(p) => p,
        None => return Ok(()), // unsupported platform - nothing more to do
    };
    for (pm, status) in manifest.mods.iter().zip(statuses.iter_mut()) {
        let Some(bytes) = pm.mcm_config.as_ref() else {
            continue;
        };
        let dest_dir = mcm_root.join(&pm.id);
        let dest = dest_dir.join("config.ini");
        let exists = dest.exists();
        if exists && !overwrite_mcm {
            continue;
        }
        if let Err(e) = std::fs::create_dir_all(&dest_dir) {
            tracing::warn!(target = "crabby_ui::modpack", err = %e, mod_id = %pm.id, "mkdir mcm dir");
            status.outcome = Some(ModOutcome::Failed(format!("mkdir mcm: {e}")));
            continue;
        }
        if let Err(e) = std::fs::write(&dest, bytes) {
            tracing::warn!(target = "crabby_ui::modpack", err = %e, path = %dest.display(), "write mcm");
            status.outcome = Some(ModOutcome::Failed(format!("write mcm: {e}")));
        }
    }

    Ok(())
}

/// File picker for "Add mod" - accepts vmz/zip files. Returns the
/// chosen path or `None` if cancelled.
async fn pick_mod_archive_path() -> Option<PathBuf> {
    let picked = rfd::AsyncFileDialog::new()
        .set_title("Add mod (vmz or zip)")
        .add_filter("Mod archive", &["vmz", "zip"])
        .pick_file()
        .await?;
    Some(picked.path().to_path_buf())
}

/// Copy a user-picked mod archive into `<game-dir>/Mods/`. Refuses
/// when no game dir is set, when the file's extension isn't vmz/zip,
/// or when a same-named file already exists at the destination
/// (treat name collisions as an error rather than silently
/// overwriting - pick a different file or rename instead).
fn copy_mod_into_mods_dir(
    src: &std::path::Path,
    game_dir: Option<&std::path::Path>,
) -> Result<PathBuf, String> {
    let game_dir = game_dir.ok_or_else(|| "no game directory".to_string())?;
    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());
    if !matches!(ext.as_deref(), Some("vmz") | Some("zip")) {
        return Err("only .vmz or .zip files are supported".into());
    }
    let file_name = src
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| "source path has no usable file name".to_string())?;
    let mods_dir = game_dir.join("Mods");
    std::fs::create_dir_all(&mods_dir).map_err(|e| format!("mkdir {}: {e}", mods_dir.display()))?;
    let dest = mods_dir.join(file_name);
    if dest.exists() {
        return Err(format!(
            "{} already exists, remove or rename the existing file first",
            dest.display()
        ));
    }
    std::fs::copy(src, &dest).map_err(|e| format!("copy: {e}"))?;
    Ok(dest)
}

async fn pick_mod_source_folder() -> Option<PathBuf> {
    let picked = rfd::AsyncFileDialog::new()
        .set_title("Add a folder to scan for mods")
        .pick_folder()
        .await?;
    Some(picked.path().to_path_buf())
}

/// Async folder picker. Validates the chosen directory before
/// returning it; an invalid pick yields `None` so the caller stays
/// in the picker state.
async fn pick_game_dir() -> Option<PathBuf> {
    let picked = rfd::AsyncFileDialog::new()
        .set_title("Select your Road to Vostok install")
        .pick_folder()
        .await?;
    let path = picked.path().to_path_buf();
    match crabby_install::validate_game_dir(&path) {
        Ok(()) => {
            tracing::info!(path = %path.display(), "ui: user picked game dir");
            Some(path)
        }
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "ui: picked dir doesn't look like an RTV install");
            None
        }
    }
}

/// Resolve the canonical download for a mod. Prefers `mod.download`
/// (the root-level field MW sets on single-file mods); falls back to
/// `/api/mods/{id}/files` and picks the first entry when MW left
/// `download` null on a multi-file mod (`has_download` is the hint).
/// Returns a parser-friendly error when nothing usable is found.
async fn resolve_mod_download(
    client: &crabby_modworkshop::Client,
    m: &crabby_modworkshop::Mod,
) -> Result<crabby_modworkshop::ModDownload, String> {
    if let Some(d) = m.download.clone() {
        if !d.download_url.is_empty() {
            return Ok(d);
        }
    }
    if !m.has_download {
        return Err("Mod has no download attached".into());
    }
    let files = client
        .get_mod_files(m.id)
        .await
        .map_err(|e| format!("fetch files: {e}"))?;
    files
        .into_iter()
        .find(|f| !f.download_url.is_empty())
        .ok_or_else(|| "Mod has no usable download in /files".into())
}

async fn install_remote_mod(
    client: &crabby_modworkshop::Client,
    mw_id: u64,
    game_dir: &std::path::Path,
) -> Result<std::path::PathBuf, String> {
    let m = client
        .get_mod(mw_id)
        .await
        .map_err(|e| format!("fetch mod: {e}"))?;
    let download = resolve_mod_download(client, &m).await?;
    if !is_supported_archive_kind(&download.kind) {
        return Err(format!(
            "Crabby can only install .vmz or .zip mods (this one is .{}). Open the mod's page \
             to grab it manually.",
            download.kind
        ));
    }
    let bytes = client
        .download(&download.download_url)
        .await
        .map_err(|e| format!("download: {e}"))?;

    // Pick the on-disk filename. MW's `download.download_url` ends in
    // a content-hash blob name like `55936_..._abc.vmz`; that's not
    // friendly. Reuse the download's storage filename only if there's
    // no nicer choice - typically MW's `download.name` is something
    // human like "RealGunAndAttachmentNames" but the URL also carries
    // a `?filename=...` hint. Parse that first.
    let filename = filename_from_download(&download.download_url, &download.kind, &m.name);

    let mods_dir = game_dir.join("Mods");
    if let Err(e) = std::fs::create_dir_all(&mods_dir) {
        return Err(format!("mkdir Mods/: {e}"));
    }
    let path = mods_dir.join(&filename);
    std::fs::write(&path, &bytes).map_err(|e| format!("write {}: {e}", path.display()))?;
    tracing::info!(
        target = "crabby_ui::install",
        path = %path.display(),
        bytes = bytes.len(),
        "installed mod from MW",
    );
    Ok(path)
}

/// Update one already-installed mod by overwriting its archive in
/// place. Same content-type guard as install - only vmz/zip are
/// handled. Mod_config.cfg keeps its existing entry; the new bytes
/// carry the new version, which load_rows will pick up on the next
/// refresh.
async fn update_installed_mod(
    client: &crabby_modworkshop::Client,
    mw_id: u64,
    existing_path: &std::path::Path,
) -> Result<(), String> {
    let m = client
        .get_mod(mw_id)
        .await
        .map_err(|e| format!("fetch mod: {e}"))?;
    let download = resolve_mod_download(client, &m).await?;
    if !is_supported_archive_kind(&download.kind) {
        return Err(format!(
            "Crabby can only update .vmz or .zip mods (this one is .{}).",
            download.kind
        ));
    }
    let bytes = client
        .download(&download.download_url)
        .await
        .map_err(|e| format!("download: {e}"))?;

    // Write to a sibling temp path first so a partial download
    // doesn't clobber the working archive. Atomically rename on
    // success.
    let parent = existing_path.parent().unwrap_or(std::path::Path::new("."));
    let tmp_path = parent.join(format!(
        ".{}.crabby-update.tmp",
        existing_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("mod")
    ));
    std::fs::write(&tmp_path, &bytes).map_err(|e| format!("write {}: {e}", tmp_path.display()))?;
    if let Err(e) = std::fs::rename(&tmp_path, existing_path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(format!("replace {}: {e}", existing_path.display()));
    }
    tracing::info!(
        target = "crabby_ui::install",
        path = %existing_path.display(),
        bytes = bytes.len(),
        "updated mod from MW",
    );
    Ok(())
}

/// Pick a sensible on-disk filename for a downloaded mod.
/// Strategy: prefer the URL's `?filename=` query hint (MW sets it to
/// the original upload name), fall back to `<mod_name>.<ext>`.
fn filename_from_download(url: &str, kind: &str, mod_name: &str) -> String {
    if let Some(qs_start) = url.find('?') {
        for kv in url[qs_start + 1..].split('&') {
            if let Some(rest) = kv.strip_prefix("filename=") {
                if !rest.is_empty() {
                    let decoded = percent_decode(rest);
                    return sanitize_filename(&decoded);
                }
            }
        }
    }
    let ext = if kind.is_empty() { "vmz" } else { kind };
    let base = if mod_name.trim().is_empty() {
        "mod"
    } else {
        mod_name.trim()
    };
    sanitize_filename(&format!("{base}.{ext}"))
}

/// Decode `%XX` hex escapes. Implements the small subset of percent
/// decoding URLs need; pulling in `percent-encoding` would be heavy
/// for one call site.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (
                (bytes[i + 1] as char).to_digit(16),
                (bytes[i + 2] as char).to_digit(16),
            ) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// True for the file kinds crabby knows how to load. The shim mounts
/// .vmz and .zip via Godot's resource-pack loader; anything else
/// (executables, source archives, dlls) is not supported.
fn is_supported_archive_kind(kind: &str) -> bool {
    matches!(kind.to_ascii_lowercase().as_str(), "vmz" | "zip")
}

/// Strip path-traversal characters from a filename so a malicious or
/// malformed download URL can't write outside Mods/.
fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| {
            if matches!(
                c,
                '/' | '\\' | '\0' | ':' | '*' | '?' | '"' | '<' | '>' | '|'
            ) {
                '_'
            } else {
                c
            }
        })
        .collect()
}

/// Fetch full MW records for many mods. Rate-limited (~2/sec when
/// hitting the network; cache hits skip the throttle). Returns
/// `(local_id, fetch_result)` pairs in the same order as `targets`.
/// Failures land as `Err` entries and don't stall the rest.
async fn fetch_mw_bulk(
    client: &crabby_modworkshop::Client,
    targets: Vec<(String, u64, String)>,
) -> Vec<(String, mods::MwFetchResult)> {
    let mut out = Vec::with_capacity(targets.len());
    let last = targets.len().saturating_sub(1);
    for (i, (mod_id, mw_id, local_v)) in targets.into_iter().enumerate() {
        let started = std::time::Instant::now();
        let pair = fetch_mw_one(client, mod_id.clone(), mw_id, local_v).await;
        let was_network = started.elapsed() > std::time::Duration::from_millis(50);
        out.push(pair);
        if was_network && i < last {
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        }
    }
    out
}

async fn fetch_mw_one(
    client: &crabby_modworkshop::Client,
    mod_id: String,
    mw_id: u64,
    local_version: String,
) -> (String, mods::MwFetchResult) {
    let mod_res = client.get_mod(mw_id).await;
    let mod_ = match mod_res {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(mod_id = %mod_id, mw_id, error = %e, "mw: get_mod failed");
            return (mod_id, Err(format!("{e}")));
        }
    };
    // User lookup is best-effort - failures fall back to "user #N".
    let user = if mod_.user_id != 0 {
        client.get_user(mod_.user_id).await.ok()
    } else {
        None
    };
    let update = client.check_update(mw_id, &local_version).await;
    let data = mods::MwData { mod_, user, update };
    (mod_id, Ok(data))
}

/// Fetch a single image's bytes. Returns `(filename, Result)` so the
/// resulting message can route by filename without threading the key
/// through the Future. The client's disk cache makes repeats free.
async fn fetch_image(
    client: &crabby_modworkshop::Client,
    file: String,
) -> (String, mods::MwImageResult) {
    let res = client
        .get_image_bytes(&file)
        .await
        .map_err(|e| format!("{e}"));
    (file, res)
}

/// Fetch many gallery images sequentially, with a small inter-call
/// delay when calls actually hit the network. Disk-cache hits return
/// instantly and skip the throttle.
async fn fetch_gallery(
    client: &crabby_modworkshop::Client,
    files: Vec<String>,
) -> Vec<(String, mods::MwImageResult)> {
    let mut out = Vec::with_capacity(files.len());
    let last = files.len().saturating_sub(1);
    for (i, file) in files.into_iter().enumerate() {
        let started = std::time::Instant::now();
        let res = client
            .get_image_bytes(&file)
            .await
            .map_err(|e| format!("{e}"));
        out.push((file, res));
        let was_network = started.elapsed() > std::time::Duration::from_millis(50);
        if was_network && i < last {
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        }
    }
    out
}

/// Resolve display names for a list of MW mod ids. Used by the
/// Requires section when a dep entry came back with `name == ""`.
/// Failures yield empty strings; the caller filters those out and
/// shows a `mod #N` fallback.
async fn fetch_dep_names(client: &crabby_modworkshop::Client, ids: Vec<u64>) -> Vec<(u64, String)> {
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        match client.get_mod(id).await {
            Ok(m) => out.push((id, m.name)),
            Err(e) => {
                tracing::warn!(mw_id = id, error = %e, "mw: dep name lookup failed");
            }
        }
    }
    out
}
