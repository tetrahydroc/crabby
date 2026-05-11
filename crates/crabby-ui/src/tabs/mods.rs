//! Mods tab - split-pane layout per the design mockup.
//!
//! Left pane (380px): search + filter chips + scrollable mod row list.
//! Right pane (flex): detail view for the selected mod.
//!
//! v0 scope: real mod discovery + enable/disable wired to
//! `mod_config.cfg`. The detail pane is a stub showing the manifest's
//! basic fields; richer content (screenshots, settings, conflict
//! callouts, requires panel) lands as real metadata gets layered in.

use std::path::{Path, PathBuf};

use iced::widget::{button, column, container, row, scrollable, text, text_input};
use iced::{Alignment, Element, Length};

use crabby_config::{
    ModConfig, ModEntry, discover_mods_for_config,
    mcm::{self, McmConfig, McmValue},
};
use crabby_manifest::ModSource;

use crate::style::{ButtonKind, SurfaceKind, button_style, surface_style};
use crate::theme::Palette;

/// Filter chip - narrows the list to a subset of discovered mods.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Filter {
    /// All discovered mods.
    #[default]
    All,
    /// Only mods present on disk (regardless of enabled state).
    Installed,
    /// Only mods enabled in the active profile.
    Enabled,
    /// Only mods present on disk but not enabled in the active profile.
    Disabled,
    /// Folder-source mods (the dev-loop subset).
    Folder,
    /// Mods with available updates. Empty until update-check data exists.
    Updates,
    /// Mods flagged as conflicting with another. Empty until conflict
    /// detection lands.
    Conflicts,
    /// Remote catalog listings the user hasn't installed yet. Each
    /// row replaces the toggle with an Install button.
    Browse,
}

impl Filter {
    fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Installed => "Installed",
            Self::Enabled => "Enabled",
            Self::Disabled => "Disabled",
            Self::Folder => "Folder",
            Self::Updates => "Updates",
            Self::Conflicts => "Conflicts",
            Self::Browse => "Browse",
        }
    }

    /// Tone for the inactive-state chip. Matches the design's count
    /// coloring: updates are accent-tinted, conflicts are err-tinted.
    fn tone(self) -> crate::style::PillTone {
        match self {
            Self::Updates => crate::style::PillTone::Accent,
            Self::Conflicts => crate::style::PillTone::Err,
            Self::Browse => crate::style::PillTone::Accent,
            _ => crate::style::PillTone::Neutral,
        }
    }
}

/// Per-tab message.
#[derive(Debug, Clone)]
pub enum Message {
    /// Filter chip clicked.
    FilterSelected(Filter),
    /// Search box edited.
    SearchChanged(String),
    /// Row selected.
    SelectMod(String),
    /// Enable switch toggled on a row.
    ToggleEnabled(String),
    /// MCM-backed config field edited. `(section, key, new_value)` -
    /// the value gets written back to the mod's `config.ini`.
    McmFieldChanged(String, String, McmValue),
    /// MCM string/int input edited. Pre-commit (just updates the
    /// in-memory buffer); commit happens on submit/blur.
    McmInputBuffer(String, String, String),
    /// Commit the buffered text input for `(section, key)` to the file.
    McmInputCommit(String, String),
    /// Begin keycode capture for `(section, key)`. Subsequent key/mouse
    /// events while capture is active land via [`McmKeyCaptured`].
    McmKeyCaptureStart(String, String),
    /// Cancel an in-flight keycode capture.
    McmKeyCaptureCancel,
    /// A keystroke or mouse-button arrived during capture; commit it.
    /// `0` clears the binding (Backspace/Delete during capture).
    McmKeyCaptured(i64),
    /// ModWorkshop fetch finished for the mod with this id. Carries
    /// the rich data on success, or an error string on failure.
    MwFetched(String, MwFetchResult),
    /// Open a URL in the default browser. Routed via Message
    /// (rather than fired inline) so the same code path can show a
    /// failure notice later without a refactor.
    OpenUrl(String),
    /// Reveal a path in the OS file manager. Auto-routes to "open
    /// folder" for directories and "select file in parent" for files.
    OpenPath(PathBuf),
    /// A dependency-name lookup landed. `(mw_mod_id, name)`.
    MwDepName(u64, String),
    /// Re-scan a single mod: drop its cached rows + MW data so the
    /// next render refetches manifest + ModWorkshop + dep names. The
    /// App layer fires the actual async work after this lands.
    RefreshOne(String),
    /// Mark the catalog-listing fetch as in flight.
    ListingsLoading,
    /// Listings finished loading.
    ListingsLoaded(Result<Vec<crabby_modworkshop::RemoteListing>, String>),
    /// Refresh-listings button clicked next to the Browse chip. App
    /// handles by clearing the disk cache and re-fetching.
    RefreshListings,
    /// Install clicked on a remote-listing row. Carries the
    /// catalog-issued listing id; App resolves it to a download URL.
    InstallRemote(String),
    /// Mark a listing's install as starting (UI-only state flip).
    InstallStarted(String),
    /// Install finished - `Ok(installed_path)` for success (path is
    /// used to stamp the new local row's `mw_id` from the listing id
    /// so the Browse-dedupe + on-select MW fetch both work without
    /// needing the mod's manifest to declare `[updates] modworkshop`),
    /// `Err` for a failure message to surface inline.
    InstallFinished(String, Result<std::path::PathBuf, String>),
    /// Update clicked for an installed mod. Carries the local mod id;
    /// App resolves it to the existing archive path + mw_id and fires
    /// the download flow.
    UpdateMod(String),
    /// Update download started.
    UpdateStarted(String),
    /// Update finished - same shape as InstallFinished.
    UpdateFinished(String, Result<(), String>),
    /// An image fetch landed. Keyed by storage filename so the same
    /// image (e.g. used as thumbnail by mod A and gallery item by
    /// mod B) only gets fetched once.
    MwImageLoaded(String, MwImageResult),
    /// "Add mod" clicked in the mod-list footer. App handles by
    /// firing the file picker and copying the chosen vmz/zip into
    /// `<game-dir>/Mods/`. No state change here.
    AddModClicked,
    /// File-picker resolved with a path. App copies to Mods/ and
    /// triggers `refresh_rows`. `None` = cancelled.
    AddModPicked(Option<PathBuf>),
    /// "Import pack" clicked in the mod-list footer. App intercepts
    /// and routes to the modpack import flow.
    ImportPackClicked,
    /// "Export pack" clicked in the mod-list footer. App intercepts
    /// and routes to the modpack export flow.
    ExportPackClicked,
    /// "More ▾" footer button clicked. Toggles the inline menu strip
    /// that houses secondary actions.
    ToggleMoreMenu,
    /// Priority-override input edited on a mod's detail pane.
    /// `(mod_id, raw_text)`. Buffer-only - commit happens on
    /// `PriorityCommit` (Enter / blur).
    PriorityBuffer(String, String),
    /// Commit the buffered priority text for `mod_id` to mod_config.cfg.
    /// Empty buffer = clear the override (inherit the manifest's value).
    /// Non-integer text reverts the buffer to the persisted value.
    PriorityCommit(String),
    /// Rescan clicked in the More menu. Bridges to the App's
    /// analyzer refresh.
    RescanClicked,
    /// Conflicts panel header clicked on the detail page. Bridges to
    /// App's `ToggleConflictPanel` so the toggle persists.
    ToggleConflictPanel(String),
}

/// Outcome of an MW image fetch - Ok(bytes) or stringly error.
pub type MwImageResult = Result<Vec<u8>, String>;

/// Cached ModWorkshop data for one mod. Combines mod + author so the
/// detail view can render byline without a follow-up fetch.
#[derive(Debug, Clone)]
pub struct MwData {
    /// Full mod record from `GET /mods/{id}`.
    pub mod_: crabby_modworkshop::Mod,
    /// Author display data, when fetched. None if the user lookup
    /// failed; UI falls back to "user #N" in that case.
    pub user: Option<crabby_modworkshop::User>,
    /// Update comparison vs the local mod.txt version.
    pub update: crabby_modworkshop::UpdateStatus,
}

/// Outcome of an MW fetch - Ok(data) or a stringly error for display.
pub type MwFetchResult = Result<MwData, String>;

/// One item in the unified row list - either an installed local mod
/// or a not-yet-installed remote listing.
#[derive(Debug, Clone, Copy)]
enum DisplayItem<'a> {
    /// Installed mod from `<game-dir>/Mods/`.
    Local(&'a Row),
    /// Catalog listing the user could install.
    Remote(&'a crabby_modworkshop::RemoteListing),
}

/// Slim view of an installed mod handed out via [`State::installed_snapshot`].
/// Used by cross-module consumers (modpack export/import) so they don't
/// have to know about the full `Row` representation.
#[derive(Debug, Clone)]
pub struct InstalledModSnapshot {
    /// Local mod id from the manifest.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Installed version string.
    pub version: String,
    /// ModWorkshop numeric id when known; `None` for non-MW mods.
    pub mw_id: Option<u64>,
}

/// Cached mod row for display + lookup.
#[derive(Debug, Clone)]
pub(crate) struct Row {
    id: String,
    name: String,
    version: String,
    source: ModSource,
    /// ModWorkshop numeric id from `[updates] modworkshop=`. None if
    /// the mod's mod.txt didn't declare one.
    mw_id: Option<u64>,
    archive_path: PathBuf,
    enabled: bool,
    /// On-disk size in bytes. For vmz/zip it's the archive file size;
    /// for folder mods every file under the folder is summed. `0`
    /// means the size couldn't be read (filesystem error, mod not on
    /// local disk, etc.) - `fmt_size` renders that as the em-dash.
    size_bytes: u64,
    /// Synthesized for remote-listing display. When true, this row
    /// isn't backed by a local install; the detail pane swaps the
    /// toggle for an Install button and hides install-only sections.
    is_remote: bool,
    /// Mod author's declared `[mod] priority=` (default 0). What the
    /// shim uses for load order absent any user override.
    priority_manifest: i64,
    /// Per-profile user override on `priority`. `None` = inherit
    /// from manifest. Persisted via mod_config.cfg's `priority` field.
    priority_override: Option<i64>,
}

/// Per-tab state.
#[derive(Debug, Default)]
pub struct State {
    /// Bumped via [`invalidate`] to force re-fetch on next view.
    pub generation: u64,
    /// Currently-active filter chip.
    pub filter: Filter,
    /// Currently-selected mod id (mirrors the design's "click row to
    /// open detail" interaction).
    pub selected: Option<String>,
    /// Cached row list. Refreshed on `invalidate` or when a toggle
    /// rewrites `mod_config.cfg`.
    rows: Vec<Row>,
    /// Generation that produced `rows`. When this lags behind
    /// `generation`, view rebuilds the cache.
    rows_gen: Option<u64>,
    /// MCM config for the currently-selected mod, if found. Loaded
    /// lazily on selection; cleared on selection change or refresh.
    mcm_cfg: Option<McmConfig>,
    /// Mod id that `mcm_cfg` belongs to - guards against showing a
    /// stale config when the selection switches between mods that
    /// both have MCM data.
    mcm_for: Option<String>,
    /// In-progress text-input buffers, keyed by `(section, key)`. Lets
    /// the user type freely without re-rendering the file on every
    /// keystroke; commits on Enter/blur.
    mcm_buffers: std::collections::BTreeMap<(String, String), String>,
    /// `(section, key)` currently in keycode-capture mode. The
    /// subscription up at the App level reads this to decide whether
    /// to swallow keystrokes; the next captured event writes here.
    pub mcm_capture: Option<(String, String)>,
    /// Free-text search query. Filters rows by name/id substring,
    /// case-insensitive. Empty = no filter.
    pub search: String,
    /// Per-mod ModWorkshop fetch results. Keyed by local mod id.
    /// Inserted on `MwFetched`; checked by the detail pane.
    pub mw_data: std::collections::BTreeMap<String, MwFetchResult>,
    /// Footer "More ▾" menu open? Toggled by the button. Houses
    /// secondary panel actions (currently just "Add mod"; future
    /// overflow lands here too).
    pub more_menu_open: bool,
    /// Cached parsed-markdown items per mod id. The markdown widget
    /// borrows from these, so they need a stable home outside view().
    pub mw_desc_md: std::collections::BTreeMap<String, Vec<iced::widget::markdown::Item>>,
    /// Resolved dependency names, keyed by MW mod id. Populated as
    /// `MwDepName` messages land. The dep-row renderer checks here
    /// when the dependency entry's own `name` came back empty.
    pub mw_dep_names: std::collections::BTreeMap<u64, String>,
    /// Per-mod update statuses, populated as a side effect of every
    /// successful `MwFetched`. Drives the row-level "update" pill +
    /// the Updates filter-chip count.
    pub mw_status: std::collections::BTreeMap<String, crabby_modworkshop::UpdateStatus>,
    /// Decoded image handles keyed by storage filename. Decoding
    /// happens once when bytes land (in `MwImageLoaded`); subsequent
    /// renders just clone the handle, which is cheap. Errors stash
    /// the failure string so the UI can show it instead of refetching.
    pub mw_images: std::collections::BTreeMap<String, MwImageState>,
    /// Mod ids whose gallery fan-out we've already kicked. Stops the
    /// refetch loop when the user re-selects a mod inside the same
    /// session. Cleared per-mod on RefreshOne.
    pub mw_gallery_kicked: std::collections::BTreeSet<String>,
    /// Remote-catalog listings (Browse filter source). Populated by
    /// the App's first listing fetch; refreshable via the chip's
    /// refresh button.
    pub listings: Vec<crabby_modworkshop::RemoteListing>,
    /// Status of the most-recent listing fetch.
    pub listings_state: ListingsState,
    /// Listing ids currently being installed (download in flight).
    /// The remote-row renderer uses this to show "Installing…".
    pub installing: std::collections::BTreeSet<String>,
    /// Per-listing install errors. Keys are listing ids.
    pub install_errors: std::collections::BTreeMap<String, String>,
    /// Local mod ids currently being updated (download in flight).
    pub updating: std::collections::BTreeSet<String>,
    /// Per-mod update errors. Keys are local mod ids.
    pub update_errors: std::collections::BTreeMap<String, String>,
    /// Synthesized `Row` for the currently-selected remote listing.
    /// Lives here (not in `view`) so its lifetime is bound to State,
    /// which the detail-pane render can borrow from. Rebuilt on
    /// every `SelectMod` for a remote id.
    pub(crate) remote_row_cache: Option<Row>,
    /// Live text-input buffer for the priority-override field, keyed
    /// by mod id. Allows typing freely (incl. transient invalid
    /// states like "-" or "" while editing) without a re-render per
    /// keystroke; commits on blur / submit. Empty string commits as
    /// "no override" → inherit manifest value.
    pub(crate) priority_buffers: std::collections::BTreeMap<String, String>,
}

/// State of the catalog-listing fetch backing the Browse chip.
#[derive(Debug, Clone, Default)]
pub enum ListingsState {
    /// Haven't tried yet - prompts the user to hit Refresh.
    #[default]
    Idle,
    /// Fetch in flight. UI shows a placeholder.
    Loading,
    /// Last fetch succeeded. Carries no data - the listings live on
    /// `State.listings` and stay there even on re-fetch failure.
    Ready,
    /// Last fetch failed. Carries a stringly error for inline display.
    Failed(String),
}

/// Per-image state tracked alongside the bytes/handle.
#[derive(Debug, Clone)]
pub enum MwImageState {
    /// Bytes are loaded and decoded into an iced handle.
    Loaded(iced::widget::image::Handle),
    /// Decode or fetch failed; carries a human-readable reason.
    Failed(String),
}

impl State {
    /// Drop cached state so the next view re-reads from disk.
    pub fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.rows_gen = None;
    }

    /// Force a re-scan from disk. Called by [`App::update`] in
    /// response to the global Refresh action; view itself is `&self`
    /// so the cache can't be populated lazily there.
    pub fn refresh_rows(&mut self, game_dir: Option<&Path>) {
        // Persist any in-flight priority edits BEFORE invalidating;
        // refresh_rows runs on tab focus / mod-list updates / explicit
        // Refresh, all of which would otherwise discard a typed-but-
        // not-Enter'd value.
        self.flush_priority_buffers(game_dir);
        self.invalidate();
        self.ensure_rows(game_dir);
    }

    /// Refresh from a pre-discovered mod list and ModConfig. Used by
    /// the boot path so the four expensive helpers it calls share one
    /// archive walk instead of opening every `.vmz` four separate times.
    pub fn refresh_rows_from_discovered(
        &mut self,
        game_dir: Option<&Path>,
        cfg: &ModConfig,
        discovered: Vec<crabby_manifest::DiscoveredMod>,
    ) {
        self.flush_priority_buffers(game_dir);
        self.invalidate();
        self.rows = load_rows_from_discovered(cfg, discovered);
        self.rows_gen = Some(self.generation);
    }

    /// Force-commit any in-flight priority-input buffers to disk.
    ///
    /// `text_input` fires `on_submit` only on Enter - losing focus by
    /// clicking another mod, switching filters, or navigating tabs
    /// would otherwise leave the typed value unsaved. Call this at
    /// every "leaving the field" boundary so edits stick without
    /// requiring an explicit Enter press.
    ///
    /// Mirrors `PriorityCommit`'s parse semantics: empty buffer clears
    /// the override, valid integers persist, invalid input is silently
    /// discarded (the persisted value remains canonical, so the next
    /// render reads what was on disk).
    fn flush_priority_buffers(&mut self, game_dir: Option<&Path>) {
        if self.priority_buffers.is_empty() {
            return;
        }
        let buffers = std::mem::take(&mut self.priority_buffers);
        let dir = match game_dir {
            Some(d) => d,
            None => return,
        };
        for (id, raw) in buffers {
            let trimmed = raw.trim();
            let new_override: Option<Option<i64>> = if trimmed.is_empty() {
                Some(None)
            } else {
                trimmed.parse::<i64>().ok().map(Some)
            };
            if let Some(new_override) = new_override
                && let Err(e) = set_mod_priority_override(dir, &id, new_override)
            {
                tracing::warn!(
                    mod_id = %id,
                    error = %e,
                    "ui: failed to flush priority override on focus loss",
                );
            }
        }
    }

    /// Apply a message.
    pub fn update(&mut self, message: Message, game_dir: Option<&Path>) {
        match message {
            Message::FilterSelected(f) => {
                // Switching filters can hide the row whose priority
                // input was just being typed in. Flush first so that
                // edit lands.
                self.flush_priority_buffers(game_dir);
                self.filter = f;
            }
            Message::SearchChanged(s) => {
                // Same rationale as FilterSelected - typing in search
                // re-filters rows and can hide the active mod's input.
                self.flush_priority_buffers(game_dir);
                self.search = s;
            }
            Message::SelectMod(id) => {
                // Clicking a different mod is the most common
                // "leave the field" path; persist before swapping.
                self.flush_priority_buffers(game_dir);
                self.selected = Some(id.clone());
                self.mcm_buffers.clear();
                self.load_mcm_for(&id);
                // If the selection isn't a local row, build a
                // synthesized one from the listings so the detail
                // pane has something to render.
                self.remote_row_cache = if self.rows.iter().any(|r| r.id == id) {
                    None
                } else {
                    self.synthesize_remote_row(&id)
                };
            }
            Message::McmFieldChanged(section, key, new_value) => {
                self.write_mcm_value(&section, &key, new_value);
            }
            Message::McmInputBuffer(section, key, buf) => {
                self.mcm_buffers.insert((section, key), buf);
            }
            Message::McmInputCommit(section, key) => {
                self.commit_mcm_buffer(&section, &key);
            }
            Message::McmKeyCaptureStart(section, key) => {
                self.mcm_capture = Some((section, key));
            }
            Message::McmKeyCaptureCancel => {
                self.mcm_capture = None;
            }
            Message::McmKeyCaptured(code) => {
                if let Some((section, key)) = self.mcm_capture.take() {
                    self.write_mcm_value(&section, &key, McmValue::Int(code));
                }
            }
            Message::MwFetched(id, result) => {
                // If the fetch succeeded, also stash the per-row update
                // status so the bulk-probe shape stays consistent.
                if let Ok(data) = &result {
                    self.mw_status.insert(id.clone(), data.update);
                    // Pre-parse markdown so the view layer can borrow
                    // a long-lived slice without re-parsing per render.
                    let raw = if !data.mod_.desc.trim().is_empty() {
                        &data.mod_.desc
                    } else {
                        &data.mod_.short_desc
                    };
                    if !raw.trim().is_empty() {
                        let cleaned = preprocess_mw_markdown(raw);
                        let with_breaks = inject_hard_line_breaks(&cleaned);
                        let items: Vec<iced::widget::markdown::Item> =
                            iced::widget::markdown::parse(&with_breaks).collect();
                        self.mw_desc_md.insert(id.clone(), items);
                    }
                }
                self.mw_data.insert(id.clone(), result);
                // If this fetched mod is also the active remote
                // selection, refresh the synthesized row so size
                // (from download.size) shows.
                if self.selected.as_deref() == Some(id.as_str())
                    && !self.rows.iter().any(|r| r.id == id)
                {
                    self.remote_row_cache = self.synthesize_remote_row(&id);
                }
            }
            Message::OpenUrl(url) => crate::open::open_url(&url),
            Message::OpenPath(path) => {
                if path.is_dir() {
                    crate::open::open_dir(&path);
                } else {
                    crate::open::reveal(&path);
                }
            }
            Message::MwDepName(mw_id, name) => {
                if !name.is_empty() {
                    self.mw_dep_names.insert(mw_id, name);
                }
            }
            Message::ListingsLoading => self.listings_state = ListingsState::Loading,
            Message::ListingsLoaded(res) => match res {
                Ok(list) => {
                    self.listings = list;
                    self.listings_state = ListingsState::Ready;
                }
                Err(e) => self.listings_state = ListingsState::Failed(e),
            },
            Message::RefreshListings => {
                // App layer handles the actual async work; this just
                // marks loading so the UI feedback is immediate.
                self.listings_state = ListingsState::Loading;
            }
            Message::InstallRemote(_id) => {
                // No state change here - App captures this message
                // pre-update and kicks the actual download Task.
            }
            Message::InstallStarted(id) => {
                self.install_errors.remove(&id);
                self.installing.insert(id);
            }
            Message::InstallFinished(id, result) => {
                self.installing.remove(&id);
                match result {
                    Ok(installed_path) => {
                        self.install_errors.remove(&id);
                        // Pull the freshly-installed mod into the row
                        // list so it moves from Browse to Installed
                        // without a manual Refresh.
                        self.refresh_rows(game_dir);
                        // Stamp `mw_id` on the new row by archive-path
                        // match. Without this the Browse filter still
                        // shows the listing (dedupe is by mw_id) and
                        // the detail pane can't fetch MW data because
                        // pending_mw_fetch needs the row's mw_id.
                        if let Ok(mw_id) = id.parse::<u64>() {
                            for row in &mut self.rows {
                                if row.archive_path == installed_path {
                                    row.mw_id = Some(mw_id);
                                    break;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(target = "crabby_ui::install", id = %id, err = %e, "install failed");
                        self.install_errors.insert(id, e);
                    }
                }
            }
            Message::UpdateMod(_id) => {
                // App captures this pre-update and kicks the actual
                // download Task; nothing to do here.
            }
            Message::UpdateStarted(id) => {
                self.update_errors.remove(&id);
                self.updating.insert(id);
            }
            Message::UpdateFinished(id, result) => {
                self.updating.remove(&id);
                match result {
                    Ok(()) => {
                        self.update_errors.remove(&id);
                        // Force the per-mod MW cache to evict so the
                        // detail pane picks up the new download +
                        // version on the next render.
                        self.mw_data.remove(&id);
                        self.mw_status.remove(&id);
                        self.refresh_rows(game_dir);
                    }
                    Err(e) => {
                        tracing::error!(target = "crabby_ui::install", id = %id, err = %e, "update failed");
                        self.update_errors.insert(id, e);
                    }
                }
            }
            Message::MwImageLoaded(file, result) => {
                let entry = match result {
                    Ok(bytes) => match decode_image_to_handle(&bytes) {
                        Ok(h) => MwImageState::Loaded(h),
                        Err(e) => {
                            tracing::warn!(file = %file, error = %e, "ui: image decode failed");
                            MwImageState::Failed(e)
                        }
                    },
                    Err(e) => MwImageState::Failed(e),
                };
                self.mw_images.insert(file, entry);
            }
            Message::AddModClicked => {
                // App handles via the file picker; refresh runs when
                // the result lands. Close the More menu since one of
                // its items was picked.
                self.more_menu_open = false;
            }
            Message::AddModPicked(path) => {
                // App copies the file into Mods/ and calls
                // refresh_rows; nothing to do tab-side except let the
                // App's refresh repaint. Swallowed here so the
                // exhaustive match passes.
                let _ = path;
            }
            Message::ImportPackClicked | Message::ExportPackClicked => {
                // App-side intercepts route these to the modpack flow;
                // tab has no state to update.
            }
            Message::ToggleMoreMenu => {
                self.more_menu_open = !self.more_menu_open;
            }
            Message::RescanClicked => {
                // App-side intercept runs the analyzer; close the menu.
                self.more_menu_open = false;
            }
            Message::ToggleConflictPanel(_) => {
                // App-side intercept persists the toggle; tab has no
                // local state to update.
            }
            Message::RefreshOne(id) => {
                // Drop the in-memory caches that key off this mod id.
                // Manifest re-scan happens via refresh_rows; the MW
                // disk cache eviction is best-effort (purges only the
                // entries known for this mod, leaves shared dep-name
                // lookups alone since they're useful for siblings too).
                self.mw_data.remove(&id);
                self.mw_desc_md.remove(&id);
                self.mw_status.remove(&id);
                self.mw_gallery_kicked.remove(&id);
                self.refresh_rows(game_dir);
                if let Some(row) = self.rows.iter().find(|r| r.id == id) {
                    if let Some(mw_id) = row.mw_id {
                        let _ = crabby_modworkshop::cache_path("mod", mw_id)
                            .map(|p| std::fs::remove_file(p).ok());
                        let _ = crabby_modworkshop::cache_path("version", mw_id)
                            .map(|p| std::fs::remove_file(p).ok());
                    }
                }
            }
            Message::ToggleEnabled(id) => {
                if let Some(dir) = game_dir {
                    if let Err(e) = toggle_mod(dir, &id) {
                        tracing::warn!(mod_id = %id, error = %e, "ui: failed to toggle mod");
                    }
                }
                // Re-scan immediately so the row list reflects the new
                // enabled state without an explicit Refresh click. The
                // view is `&self` so it can't repopulate the cache itself.
                self.refresh_rows(game_dir);
            }
            Message::PriorityBuffer(id, raw) => {
                // Reject non-numeric keystrokes at the buffer level so
                // the input visibly refuses letters, spaces, dots, etc.
                // Allow: digits, optional leading minus, OR an empty
                // buffer (= "clear the override on commit"). Non-empty
                // strings that don't fit are simply not stored, leaving
                // the previous valid buffer state intact (the on_input
                // event fires post-keystroke so the rejected character
                // never lands).
                if raw.is_empty() || is_valid_priority_input(&raw) {
                    self.priority_buffers.insert(id, raw);
                }
            }
            Message::PriorityCommit(id) => {
                let raw = self.priority_buffers.remove(&id).unwrap_or_default();
                let trimmed = raw.trim();
                // Empty input = clear override. Non-integer = silent
                // revert (we just discard the buffer; next render reads
                // the persisted value).
                let new_override: Option<Option<i64>> = if trimmed.is_empty() {
                    Some(None)
                } else {
                    trimmed.parse::<i64>().ok().map(Some)
                };
                if let (Some(new_override), Some(dir)) = (new_override, game_dir) {
                    if let Err(e) = set_mod_priority_override(dir, &id, new_override) {
                        tracing::warn!(
                            mod_id = %id,
                            error = %e,
                            "ui: failed to write priority override",
                        );
                    }
                }
                self.refresh_rows(game_dir);
            }
        }
    }

    /// If the row exists and declares a ModWorkshop id and we don't
    /// already have data (or it's a stale failure), return
    /// `Some((mod_id, mw_id, local_version))` so the App layer can
    /// kick an async fetch. The first call after selection wins; the
    /// returned tuple's `mod_id` is what the resulting `MwFetched`
    /// message must use.
    pub fn pending_mw_fetch(&self, mod_id: &str) -> Option<(String, u64, String)> {
        if matches!(self.mw_data.get(mod_id), Some(Ok(_))) {
            return None;
        }
        // Local row first; fall back to the synthesized remote row so
        // a Browse-tab selection still kicks an MW fetch even when no
        // local row exists.
        let (mw_id, version) = if let Some(row) = self.rows.iter().find(|r| r.id == mod_id) {
            (row.mw_id?, row.version.clone())
        } else {
            let row = self.remote_row_cache.as_ref().filter(|r| r.id == mod_id)?;
            (row.mw_id?, row.version.clone())
        };
        Some((mod_id.to_string(), mw_id, version))
    }

    /// Total bytes for all currently-enabled rows. Drives the
    /// profile-bar "size" stat. Disabled mods aren't counted since
    /// they don't ship to the bake.
    #[must_use]
    pub fn enabled_size_bytes(&self) -> u64 {
        self.rows
            .iter()
            .filter(|r| r.enabled)
            .map(|r| r.size_bytes)
            .sum()
    }

    /// Snapshot of installed mods for cross-module consumers (the
    /// modpack export / import pipeline). Skips remote-only synthetic
    /// rows - the caller wants real mods, not Browse-tab placeholders.
    #[must_use]
    pub fn installed_snapshot(&self) -> Vec<InstalledModSnapshot> {
        self.rows
            .iter()
            .filter(|r| !r.is_remote)
            .map(|r| InstalledModSnapshot {
                id: r.id.clone(),
                name: r.name.clone(),
                version: r.version.clone(),
                mw_id: r.mw_id,
            })
            .collect()
    }

    /// Storage filenames of MW images we want to fetch for the given
    /// mod but haven't yet. Currently scopes to the thumbnail; the
    /// gallery uses [`pending_gallery_image_files`] so callers can
    /// decide when to fan out the bigger batch.
    pub fn pending_thumbnail_file(&self, mod_id: &str) -> Option<String> {
        let data = self.mw_data.get(mod_id)?.as_ref().ok()?;
        let thumb = data.mod_.thumbnail.as_ref()?;
        if thumb.file.is_empty() || self.mw_images.contains_key(&thumb.file) {
            return None;
        }
        Some(thumb.file.clone())
    }

    /// Resolve a local mod id into the (id, mw_id, archive_path)
    /// triple the App needs to fire an update download. Returns
    /// `None` when the mod isn't found, has no MW id, or is a folder
    /// mod (no clean update path for those).
    pub fn update_target_for(&self, local_id: &str) -> Option<(String, u64, std::path::PathBuf)> {
        let row = self.rows.iter().find(|r| r.id == local_id)?;
        if row.source == ModSource::Folder {
            return None;
        }
        let mw_id = row.mw_id?;
        Some((row.id.clone(), mw_id, row.archive_path.clone()))
    }

    /// Mark the gallery fan-out as kicked so subsequent selections of
    /// the same mod don't re-enqueue all the fetches. The App layer
    /// calls this when it dispatches the gallery-fetch task.
    pub fn mark_gallery_kicked(&mut self, mod_id: &str) {
        self.mw_gallery_kicked.insert(mod_id.to_string());
    }

    /// True when we've already kicked the gallery fan-out this session.
    #[must_use]
    pub fn is_gallery_kicked(&self, mod_id: &str) -> bool {
        self.mw_gallery_kicked.contains(mod_id)
    }

    /// All gallery image filenames that haven't been fetched yet.
    /// Sorted by display_order.
    pub fn pending_gallery_image_files(&self, mod_id: &str) -> Vec<String> {
        let Some(Ok(data)) = self.mw_data.get(mod_id) else {
            return Vec::new();
        };
        let mut imgs: Vec<&crabby_modworkshop::Image> = data
            .mod_
            .images
            .iter()
            .filter(|i| i.visible && !i.file.is_empty())
            .collect();
        imgs.sort_by_key(|i| i.display_order);
        imgs.into_iter()
            .map(|i| i.file.clone())
            .filter(|f| !self.mw_images.contains_key(f))
            .collect()
    }

    /// MW ids of dependencies we should resolve names for. Excludes
    /// any whose name was returned inline by the parent fetch and any
    /// we've already cached. Caller fans these out to one fetch each.
    pub fn pending_dep_name_lookups(&self, mod_id: &str) -> Vec<u64> {
        let Some(Ok(data)) = self.mw_data.get(mod_id) else {
            return Vec::new();
        };
        data.mod_
            .dependencies
            .iter()
            .filter(|d| d.name.trim().is_empty() && d.mod_id != 0)
            .map(|d| d.mod_id)
            .filter(|id| !self.mw_dep_names.contains_key(id))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    /// Snapshot all (mod_id, mw_id, local_version) tuples for the
    /// bulk-probe. Excludes mods whose full MW record is already
    /// loaded (success only - failed entries get retried on the next
    /// pass since the in-memory cache holds an `Err` state).
    pub fn pending_mw_bulk_probe(&self) -> Vec<(String, u64, String)> {
        self.rows
            .iter()
            .filter_map(|r| {
                let mw_id = r.mw_id?;
                if matches!(self.mw_data.get(&r.id), Some(Ok(_))) {
                    return None;
                }
                Some((r.id.clone(), mw_id, r.version.clone()))
            })
            .collect()
    }

    /// Try to find + load an MCM config for `mod_id`. Searches for the
    /// row to grab the display name (better fuzzy match), then asks
    /// the MCM module to find the matching folder. Silently no-ops if
    /// nothing matches - many mods don't ship MCM config.
    fn load_mcm_for(&mut self, mod_id: &str) {
        self.mcm_cfg = None;
        self.mcm_for = None;
        let Some(row) = self.rows.iter().find(|r| r.id == mod_id) else {
            return;
        };
        let Some(path) = mcm::find_config_for_mod(&row.id, &row.name) else {
            return;
        };
        match McmConfig::load(&path) {
            Ok(Some(cfg)) => {
                self.mcm_cfg = Some(cfg);
                self.mcm_for = Some(mod_id.to_string());
            }
            Ok(None) => {}
            Err(e) => tracing::warn!(error = %e, "ui: MCM config load failed"),
        }
    }

    /// Write a value to the active MCM config and update the in-memory copy.
    fn write_mcm_value(&mut self, section: &str, key: &str, value: McmValue) {
        let Some(cfg) = self.mcm_cfg.as_mut() else {
            return;
        };
        if let Err(e) = cfg.set_value(section, key, value) {
            tracing::warn!(section, key, error = %e, "ui: MCM write failed");
        }
    }

    /// Convert a buffered text-input string to the right McmValue kind
    /// (matches the field's existing kind) and write it. Bad input is
    /// silently dropped - the field's display reverts.
    fn commit_mcm_buffer(&mut self, section: &str, key: &str) {
        let Some(buf) = self
            .mcm_buffers
            .remove(&(section.to_string(), key.to_string()))
        else {
            return;
        };
        let Some(cfg) = self.mcm_cfg.as_ref() else {
            return;
        };
        let Some(field) = cfg.find(section, key) else {
            return;
        };
        let new_value = match &field.value {
            McmValue::Bool(_) => return, // bools don't go through input buffers
            McmValue::Int(_) => match buf.trim().parse::<i64>() {
                Ok(n) => McmValue::Int(n),
                Err(_) => return,
            },
            McmValue::Float(_) => match buf.trim().parse::<f64>() {
                Ok(f) => McmValue::Float(f),
                Err(_) => return,
            },
            McmValue::Str(_) => McmValue::Str(buf),
        };
        self.write_mcm_value(section, key, new_value);
    }

    /// Refresh `rows` from disk if stale.
    fn ensure_rows(&mut self, game_dir: Option<&Path>) {
        if self.rows_gen == Some(self.generation) {
            return;
        }
        self.rows = match game_dir {
            Some(dir) => load_rows(dir).unwrap_or_else(|e| {
                tracing::warn!(error = %e, "ui: mod discovery failed");
                Vec::new()
            }),
            None => Vec::new(),
        };
        self.rows_gen = Some(self.generation);
    }

    /// Render the tab body.
    ///
    /// `conflicts` is the App-side analyzer aggregate; rows decorate
    /// themselves with a red conflict pill when the mod participates,
    /// and the detail pane gets a Conflicts panel listing each one.
    pub fn view<'a>(
        &'a self,
        game_dir: Option<&Path>,
        conflicts: &'a [crabby_mod_analyzer::Conflict],
        conflict_panel_overrides: &'a std::collections::BTreeMap<String, bool>,
        palette: &Palette,
    ) -> Element<'a, Message> {
        // The view is `&self` so the cache can't be refreshed mutably
        // here. Refresh happens lazily on the next `update` cycle -
        // for now just render whatever's cached. This is fine because
        // the tab is invalidated on app-level Refresh, which also
        // fires an update before the next view.
        let p = *palette;

        let list_pane = self.list_pane(game_dir, conflicts, &p);
        let detail_pane = self.detail_pane(conflicts, conflict_panel_overrides, &p);

        row![list_pane, detail_pane]
            .spacing(0)
            .height(Length::Fill)
            .into()
    }

    fn list_pane<'a>(
        &'a self,
        _game_dir: Option<&Path>,
        conflicts: &'a [crabby_mod_analyzer::Conflict],
        palette: &Palette,
    ) -> Element<'a, Message> {
        let p = *palette;

        // ---- Counts (drive the chips and the footer line) ----
        let total = self.rows.len();
        let enabled_count = self.rows.iter().filter(|r| r.enabled).count();
        let disabled_count = total - enabled_count;
        let folder_count = self
            .rows
            .iter()
            .filter(|r| r.source == ModSource::Folder)
            .count();
        // Updates / Conflicts have no real source yet - surface 0 so
        // the chip exists but doesn't lie about counts.
        let updates_count = self
            .mw_status
            .iter()
            .filter(|(_, s)| matches!(**s, crabby_modworkshop::UpdateStatus::UpdateAvailable))
            .count();
        // Conflict count = mods that participate in at least one
        // conflict. Distinct from the *count of conflicts* - a single
        // collision involving 3 mods contributes 3 to this.
        let conflicts_count = self
            .rows
            .iter()
            .filter(|r| crabby_mod_analyzer::mod_has_conflicts(conflicts, &r.id))
            .count();
        // Browse delta used by both the chip and the All count.
        let local_mw_ids: std::collections::HashSet<u64> =
            self.rows.iter().filter_map(|r| r.mw_id).collect();
        let browse_count = self
            .listings
            .iter()
            .filter(|l| {
                l.id.parse::<u64>()
                    .ok()
                    .map(|id| !local_mw_ids.contains(&id))
                    .unwrap_or(true)
            })
            .count();

        // ---- Search input ----
        // Placeholder count tracks the active filter so the search
        // hint reflects the narrowed set - "Search 40 mods" while
        // looking at the Updates chip would be misleading.
        let active_filter_count = match self.filter {
            Filter::All => total + browse_count,
            Filter::Installed => total,
            Filter::Enabled => enabled_count,
            Filter::Disabled => disabled_count,
            Filter::Folder => folder_count,
            Filter::Updates => updates_count,
            Filter::Conflicts => conflicts_count,
            Filter::Browse => browse_count,
        };
        let search_box = text_input(&format!("Search {active_filter_count} mods"), &self.search)
            .on_input(Message::SearchChanged)
            .padding([5, 10])
            .size(12)
            .style(move |_t, _s| iced::widget::text_input::Style {
                background: iced::Background::Color(p.bg_2),
                border: iced::Border {
                    color: p.line,
                    width: 1.0,
                    radius: 6.0.into(),
                },
                icon: p.fg_3,
                placeholder: p.fg_3,
                value: p.fg_0,
                selection: p.accent_soft,
            });

        // ---- Filter chips ----
        let mk_chip = |f: Filter, count: usize| -> Element<'_, Message> {
            let active = self.filter == f;
            let label_text = text(format!("{} {}", f.label(), count)).size(11);
            button(label_text)
                .padding([3, 9])
                .style(crate::style::filter_chip_style(p, active, f.tone()))
                .on_press(Message::FilterSelected(f))
                .into()
        };

        let chips = iced::widget::Row::with_children(vec![
            mk_chip(Filter::All, total + browse_count),
            mk_chip(Filter::Installed, total),
            mk_chip(Filter::Enabled, enabled_count),
            mk_chip(Filter::Disabled, disabled_count),
            mk_chip(Filter::Folder, folder_count),
            mk_chip(Filter::Updates, updates_count),
            mk_chip(Filter::Conflicts, conflicts_count),
            mk_chip(Filter::Browse, browse_count),
        ])
        .spacing(5)
        .wrap();

        // Refresh-listings button - only visible when Browse is the
        // active filter so it doesn't clutter the rest of the time.
        let refresh_listings_btn: Element<'_, Message> = if self.filter == Filter::Browse {
            button(text("↻").size(11))
                .padding([3, 8])
                .style(button_style(p, ButtonKind::Ghost))
                .on_press(Message::RefreshListings)
                .into()
        } else {
            iced::widget::Space::new()
                .width(Length::Fixed(0.0))
                .height(Length::Fixed(0.0))
                .into()
        };
        let chips_row = row![chips, crate::style::hspace(), refresh_listings_btn]
            .spacing(8)
            .align_y(Alignment::Center);

        let top = column![search_box, chips_row].spacing(10).padding([14, 14]);

        // ---- Build display items: locals first, then deduped remotes ----
        let q = self.search.trim().to_ascii_lowercase();
        let want_locals = !matches!(self.filter, Filter::Browse);
        let want_remotes = matches!(self.filter, Filter::All | Filter::Browse);

        let local_items: Vec<DisplayItem<'_>> = if want_locals {
            self.rows
                .iter()
                .filter(|r| match self.filter {
                    Filter::All | Filter::Browse | Filter::Installed => true,
                    Filter::Enabled => r.enabled,
                    Filter::Disabled => !r.enabled,
                    Filter::Folder => r.source == ModSource::Folder,
                    Filter::Updates => matches!(
                        self.mw_status.get(&r.id),
                        Some(crabby_modworkshop::UpdateStatus::UpdateAvailable)
                    ),
                    Filter::Conflicts => crabby_mod_analyzer::mod_has_conflicts(conflicts, &r.id),
                })
                .filter(|r| {
                    if q.is_empty() {
                        return true;
                    }
                    r.name.to_ascii_lowercase().contains(&q)
                        || r.id.to_ascii_lowercase().contains(&q)
                })
                .map(DisplayItem::Local)
                .collect()
        } else {
            Vec::new()
        };

        let remote_items: Vec<DisplayItem<'_>> = if want_remotes {
            self.listings
                .iter()
                .filter(|l| {
                    // Skip remotes that are already installed locally.
                    if let Ok(mw_id) = l.id.parse::<u64>() {
                        if local_mw_ids.contains(&mw_id) {
                            return false;
                        }
                    }
                    if q.is_empty() {
                        return true;
                    }
                    l.name.to_ascii_lowercase().contains(&q)
                        || l.id.to_ascii_lowercase().contains(&q)
                })
                .map(DisplayItem::Remote)
                .collect()
        } else {
            Vec::new()
        };

        let visible: Vec<DisplayItem<'_>> = local_items
            .into_iter()
            .chain(remote_items.into_iter())
            .collect();

        // ---- Header strip: "INSTALLED · N shown" + sort button ----
        // ---- Body: rows or empty state ----
        let listings_loading = matches!(self.listings_state, ListingsState::Loading);
        let listings_failed = matches!(self.listings_state, ListingsState::Failed(_));
        let body: Element<'_, Message> = if self.rows_gen.is_none() {
            container(text("Click Refresh to scan").size(12).color(p.fg_3))
                .padding(20)
                .into()
        } else if visible.is_empty() {
            let msg = match self.filter {
                Filter::Updates => "Everything's up to date.",
                Filter::Conflicts => "No conflicts in the active profile.",
                Filter::Browse if listings_loading => "Loading catalog…",
                Filter::Browse if listings_failed => {
                    if let ListingsState::Failed(e) = &self.listings_state {
                        return container(
                            text(format!("Catalog fetch failed: {e}"))
                                .size(12)
                                .color(p.err),
                        )
                        .padding(24)
                        .center_x(Length::Fill)
                        .into();
                    }
                    "Catalog fetch failed."
                }
                Filter::Browse => "No installable mods found. Hit refresh to check again.",
                _ if !q.is_empty() => "Nothing matches your search.",
                _ => "Nothing here. Try a different filter.",
            };
            container(text(msg).size(12).color(p.fg_3))
                .padding(24)
                .center_x(Length::Fill)
                .into()
        } else {
            let rows: Vec<Element<'_, Message>> = visible
                .iter()
                .enumerate()
                .map(|(idx, item)| match item {
                    DisplayItem::Local(r) => self.mod_row(r, idx + 1, conflicts, &p),
                    DisplayItem::Remote(l) => self.remote_row(l, idx + 1, &p),
                })
                .collect();
            scrollable(column(rows).spacing(0))
                .height(Length::Fill)
                .into()
        };

        // ---- Footer: action buttons only ----
        // Counts/stats live in the chips above + the conflict pill on
        // each row. Primary row: [Rescan] [Import] [Export] [More ▾].
        // More opens an inline menu strip with secondary actions
        // (Add mod today; future overflow lands there too).
        let rescan_label = if conflicts.is_empty() {
            "Rescan".to_string()
        } else {
            format!("Rescan ({})", conflicts.len())
        };
        let rescan_btn = button(text(rescan_label).size(11))
            .padding(crate::style::ButtonSize::Sm.padding())
            .style(button_style(p, ButtonKind::Default))
            .on_press(Message::RescanClicked);
        let import_btn = button(text("Import").size(11))
            .padding(crate::style::ButtonSize::Sm.padding())
            .style(button_style(p, ButtonKind::Default))
            .on_press(Message::ImportPackClicked);
        let export_btn = button(text("Export").size(11))
            .padding(crate::style::ButtonSize::Sm.padding())
            .style(button_style(p, ButtonKind::Default))
            .on_press(Message::ExportPackClicked);
        let more_label = if self.more_menu_open {
            "More ▴"
        } else {
            "More ▾"
        };
        let more_btn = button(text(more_label).size(11))
            .padding(crate::style::ButtonSize::Sm.padding())
            .style(button_style(p, ButtonKind::Default))
            .on_press(Message::ToggleMoreMenu);

        let footer_row = row![
            rescan_btn,
            crate::style::hspace(),
            import_btn,
            export_btn,
            more_btn,
        ]
        .spacing(8)
        .padding([8, 14])
        .align_y(Alignment::Center);

        // Inline menu strip - appears between the body and the footer
        // row when "More" is open. Iced 0.14 doesn't ship a real popup
        // primitive; this is the pragmatic alternative. Items are
        // small left-aligned ghost buttons so the menu reads as a
        // dropdown without overlay positioning gymnastics.
        let footer: Element<'_, Message> = if self.more_menu_open {
            let add_btn = button(text("+ Add mod").size(11))
                .padding(crate::style::ButtonSize::Sm.padding())
                .style(button_style(p, ButtonKind::Ghost))
                .on_press(Message::AddModClicked);
            let menu_strip = container(
                row![add_btn]
                    .spacing(6)
                    .padding([6, 14])
                    .align_y(Alignment::Center),
            )
            .style(surface_style(p, SurfaceKind::Bg2))
            .width(Length::Fill);
            container(column![menu_strip, footer_row].spacing(0))
                .style(surface_style(p, SurfaceKind::Bg1))
                .width(Length::Fill)
                .into()
        } else {
            container(footer_row)
                .style(surface_style(p, SurfaceKind::Bg1))
                .width(Length::Fill)
                .into()
        };

        // Header strip dropped - `INSTALLED · N shown` was redundant
        // with the active filter chip + its count up top. Body sits
        // directly under the search/chips area now.
        let inner = column![top, body, footer].spacing(0).height(Length::Fill);

        container(inner)
            .style(surface_style(p, SurfaceKind::Bg2))
            .width(Length::Fixed(380.0))
            .height(Length::Fill)
            .into()
    }

    fn mod_row<'a>(
        &'a self,
        r: &'a Row,
        _idx: usize,
        conflicts: &'a [crabby_mod_analyzer::Conflict],
        palette: &Palette,
    ) -> Element<'a, Message> {
        let p = *palette;
        let selected = self.selected.as_deref() == Some(&r.id);

        // Name + status pill. Width::Fill on the name lets iced wrap
        // it instead of pushing siblings out of the row.
        let title_color = if r.enabled { p.fg_0 } else { p.fg_2 };
        let name = text(r.name.clone())
            .size(12)
            .color(title_color)
            .width(Length::Fill);

        // Single status pill - priority: conflict > update > installed > disabled.
        // Conflict trumps everything because the user needs to know
        // something's off about this mod regardless of update state.
        // Conflict severity drives the pill tone: Hard = red,
        // Warn = amber. Info-level findings don't surface as conflicts
        // (filtered earlier in detect_conflicts).
        let conflict_severity = crabby_mod_analyzer::mod_max_conflict_severity(conflicts, &r.id);
        let has_update = matches!(
            self.mw_status.get(&r.id),
            Some(crabby_modworkshop::UpdateStatus::UpdateAvailable)
        );
        let (status_label, status_tone) = match conflict_severity {
            Some(crabby_mod_analyzer::Severity::Hard) => ("conflict", crate::style::PillTone::Err),
            Some(crabby_mod_analyzer::Severity::Warn) => ("conflict", crate::style::PillTone::Warn),
            Some(crabby_mod_analyzer::Severity::Info) | None if has_update => {
                ("update", crate::style::PillTone::Accent)
            }
            Some(crabby_mod_analyzer::Severity::Info) | None if r.enabled => {
                ("installed", crate::style::PillTone::Ok)
            }
            _ => ("disabled", crate::style::PillTone::Neutral),
        };
        // Pill lives outside the title-text row now so it can align
        // vertically-centered with the toggle on the right; this also
        // lets us add real spacing between name / pill / toggle.
        let status_pill = crate::style::pill(p, status_label, status_tone);
        let title_row = iced::widget::Row::with_children(vec![name.into()])
            .spacing(8)
            .align_y(Alignment::Center);

        // Byline: version · author · source · size. Author resolves
        // from MW data when present; missing pieces collapse so the
        // separators don't dangle.
        let author_name = self
            .mw_data
            .get(&r.id)
            .and_then(|res| res.as_ref().ok())
            .and_then(|d| d.user.as_ref())
            .map(|u| u.name.clone())
            .filter(|n| !n.trim().is_empty());
        let mut sub_pieces: Vec<Element<'_, Message>> =
            vec![text(fmt_version(&r.version)).size(10).color(p.fg_2).into()];
        if let Some(name) = author_name {
            sub_pieces.push(text("·").size(10).color(p.fg_3).into());
            sub_pieces.push(text(name).size(10).color(p.fg_2).into());
        }
        sub_pieces.push(text("·").size(10).color(p.fg_3).into());
        sub_pieces.push(text(r.source.label()).size(10).color(p.fg_2).into());
        if r.size_bytes > 0 {
            sub_pieces.push(text("·").size(10).color(p.fg_3).into());
            sub_pieces.push(text(fmt_size(r.size_bytes)).size(10).color(p.fg_2).into());
        }
        let sub = iced::widget::Row::with_children(sub_pieces)
            .spacing(6)
            .align_y(Alignment::Center);

        // The info column takes the row's flexible space. Long names
        // wrap to a second line rather than squishing the toggle.
        let info = column![title_row, sub].spacing(2).width(Length::Fill);

        // Toggle - small ON/OFF pill button that flips colors.
        let id_for_toggle = r.id.clone();
        let toggle_kind = if r.enabled {
            ButtonKind::Primary
        } else {
            ButtonKind::Default
        };
        let toggle_label = if r.enabled { "ON" } else { "OFF" };
        let toggle = button(text(toggle_label).size(10))
            .padding([3, 9])
            .style(button_style(p, toggle_kind))
            .on_press(Message::ToggleEnabled(id_for_toggle));

        // Layout: [info column (name + byline)] [pill] [toggle].
        // Wider spacing between the three so they read as separate
        // columns rather than a clump.
        let id_for_select = r.id.clone();
        let row_inner = row![info, status_pill, toggle]
            .spacing(14)
            .align_y(Alignment::Center)
            .padding([9, 14]);

        let row_bg = if selected { p.accent_soft } else { p.bg_2 };
        let border_color = if selected {
            p.accent
        } else {
            iced::Color::TRANSPARENT
        };
        button(row_inner)
            .padding(0)
            .width(Length::Fill)
            .style(move |_t, _s| iced::widget::button::Style {
                background: Some(iced::Background::Color(row_bg)),
                text_color: p.fg_0,
                border: iced::Border {
                    color: border_color,
                    width: 0.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            })
            .on_press(Message::SelectMod(id_for_select))
            .into()
    }

    /// Render one row for an uninstalled remote listing. Mirrors
    /// `mod_row`'s shape so Browse and All look uniform; key
    /// difference is the right column shows an `Install` button
    /// instead of an enabled-toggle.
    fn remote_row<'a>(
        &'a self,
        l: &'a crabby_modworkshop::RemoteListing,
        _idx: usize,
        palette: &Palette,
    ) -> Element<'a, Message> {
        let p = *palette;

        let name = text(l.name.clone())
            .size(12)
            .color(p.fg_0)
            .width(Length::Fill);

        // Pill lifted out of the title row so it can sit in its own
        // column aligned with the install button - same pattern as
        // the local rows.
        let status_pill = crate::style::pill(p, "available", crate::style::PillTone::Accent);
        let title_row = iced::widget::Row::with_children(vec![name.into()])
            .spacing(8)
            .align_y(Alignment::Center);

        // Sublabel: version · author · downloads.
        let mut sub_pieces: Vec<Element<'_, Message>> =
            vec![text(fmt_version(&l.version)).size(10).color(p.fg_2).into()];
        if !l.author.trim().is_empty() {
            sub_pieces.push(text("·").size(10).color(p.fg_3).into());
            sub_pieces.push(text(l.author.clone()).size(10).color(p.fg_2).into());
        }
        if l.downloads > 0 {
            sub_pieces.push(text("·").size(10).color(p.fg_3).into());
            sub_pieces.push(
                text(format!("{}↓", fmt_thousands(l.downloads)))
                    .size(10)
                    .color(p.fg_2)
                    .into(),
            );
        }
        let sub = iced::widget::Row::with_children(sub_pieces)
            .spacing(6)
            .align_y(Alignment::Center);

        let info = column![title_row, sub].spacing(2).width(Length::Fill);

        // Install button. Three visual states: idle / in-flight /
        // failed. Failures show a tiny error-tinted retry button.
        let installing = self.installing.contains(&l.id);
        let install_btn: Element<'_, Message> = if installing {
            button(text("Installing…").size(10))
                .padding([3, 9])
                .style(button_style(p, ButtonKind::Default))
                .into()
        } else if let Some(err) = self.install_errors.get(&l.id) {
            let id_clone = l.id.clone();
            let _ = err; // shown via tooltip-equivalent in the future
            button(text("Retry").size(10))
                .padding([3, 9])
                .style(button_style(p, ButtonKind::Default))
                .on_press(Message::InstallRemote(id_clone))
                .into()
        } else {
            let id_clone = l.id.clone();
            button(text("Install").size(10))
                .padding([3, 9])
                .style(button_style(p, ButtonKind::Primary))
                .on_press(Message::InstallRemote(id_clone))
                .into()
        };

        let id_for_select = l.id.clone();
        let row_inner = row![info, status_pill, install_btn]
            .spacing(14)
            .align_y(Alignment::Center)
            .padding([9, 14]);

        let selected = self
            .selected
            .as_deref()
            .map(|s| s == l.id.as_str())
            .unwrap_or(false);
        let row_bg = if selected { p.accent_soft } else { p.bg_2 };
        let border_color = if selected {
            p.accent
        } else {
            iced::Color::TRANSPARENT
        };
        button(row_inner)
            .padding(0)
            .width(Length::Fill)
            .style(move |_t, _s| iced::widget::button::Style {
                background: Some(iced::Background::Color(row_bg)),
                text_color: p.fg_0,
                border: iced::Border {
                    color: border_color,
                    width: 0.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            })
            .on_press(Message::SelectMod(id_for_select))
            .into()
    }

    fn detail_pane<'a>(
        &'a self,
        conflicts: &'a [crabby_mod_analyzer::Conflict],
        conflict_panel_overrides: &'a std::collections::BTreeMap<String, bool>,
        palette: &Palette,
    ) -> Element<'a, Message> {
        let p = *palette;

        // Selection resolves locally first (an installed mod), then
        // falls back to the synthesized remote row built when the
        // user selected a Browse listing. The synth lives on State
        // so its lifetime works with hero_and_body's borrow.
        let body: Element<'_, Message> = match self.selected.as_deref() {
            Some(id) => {
                if let Some(r) = self.rows.iter().find(|r| r.id == id) {
                    self.hero_and_body(r, conflicts, conflict_panel_overrides, p)
                } else if let Some(r) = self.remote_row_cache.as_ref().filter(|r| r.id == id) {
                    self.hero_and_body(r, conflicts, conflict_panel_overrides, p)
                } else {
                    detail_empty_state(p)
                }
            }
            None => detail_empty_state(p),
        };

        container(body)
            .style(surface_style(p, SurfaceKind::Bg2))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// Build a synthetic [`Row`] from a remote listing + cached MW
    /// data so `hero_and_body` can render the Browse selection.
    /// Returns `None` when the id doesn't match any listing.
    fn synthesize_remote_row(&self, id: &str) -> Option<Row> {
        let listing = self.listings.iter().find(|l| l.id == id)?;
        // Pull size from MW data when available (download.size from
        // the per-mod fetch), zero otherwise.
        let size_bytes = self
            .mw_data
            .get(id)
            .and_then(|res| res.as_ref().ok())
            .and_then(|d| d.mod_.download.as_ref())
            .map(|dl| dl.size)
            .unwrap_or(0);
        let mw_id = listing.id.parse::<u64>().ok();
        Some(Row {
            id: listing.id.clone(),
            name: listing.name.clone(),
            version: listing.version.clone(),
            // Default to vmz; the install path validates the kind so
            // this is just a display-shape choice. Folder/zip stays
            // wrong here, but we don't render source-derived UI for
            // remotes anyway (no Open-folder button etc).
            source: ModSource::Vmz,
            mw_id,
            archive_path: std::path::PathBuf::new(),
            enabled: false,
            size_bytes,
            is_remote: true,
            // Remote listings have no manifest data + no per-profile
            // record, so default both to "no priority info." The detail
            // pane skips the priority row when remote (see view_mod).
            priority_manifest: 0,
            priority_override: None,
        })
    }

    /// Render the full mod-detail view: hero strip + 2-col body.
    fn hero_and_body<'a>(
        &'a self,
        r: &'a Row,
        conflicts: &'a [crabby_mod_analyzer::Conflict],
        conflict_panel_overrides: &'a std::collections::BTreeMap<String, bool>,
        p: Palette,
    ) -> Element<'a, Message> {
        // ---- Hero strip ----
        // Hero thumbnail. Three states:
        //   - We have a decoded handle for the mod's MW thumbnail file → render it.
        //   - Fetch in flight or no MW data → patterned placeholder.
        //   - MW says no thumbnail at all → patterned placeholder too.
        let thumbnail: Element<'_, Message> = match self
            .mw_data
            .get(&r.id)
            .and_then(|res| res.as_ref().ok())
            .and_then(|d| d.mod_.thumbnail.as_ref())
            .filter(|t| !t.file.is_empty())
        {
            Some(thumb) => match self.mw_images.get(&thumb.file) {
                Some(MwImageState::Loaded(handle)) => iced::widget::image(handle.clone())
                    .width(Length::Fixed(168.0))
                    .height(Length::Fixed(104.0))
                    .content_fit(iced::ContentFit::Cover)
                    .into(),
                _ => crate::style::thumb(p, "loading", 168.0, 104.0),
            },
            None => crate::style::thumb(p, "screenshot", 168.0, 104.0),
        };

        // Status pill row - eyebrow with source label + status pill.
        // Status data isn't real yet; render a "Healthy" pill for
        // enabled mods so the design shape is visible.
        let status_pill: Element<'_, Message> = if r.enabled {
            crate::style::pill(p, "Active", crate::style::PillTone::Ok)
        } else {
            crate::style::pill(p, "Disabled", crate::style::PillTone::Neutral)
        };
        // Source is shown once, as the second pill. The eyebrow + the
        // byline duplicate of the source label both got dropped.
        let source_pill = crate::style::pill(p, r.source.label(), crate::style::PillTone::Neutral);
        let mut pill_pieces: Vec<Element<'_, Message>> = vec![status_pill, source_pill];
        // Update / version pill from the bulk probe (or per-mod fetch).
        if let Some(status) = self.mw_status.get(&r.id) {
            use crabby_modworkshop::UpdateStatus as US;
            let pill_el: Option<Element<'_, Message>> = match status {
                US::UpdateAvailable => Some(crate::style::pill(
                    p,
                    "Update available",
                    crate::style::PillTone::Accent,
                )),
                US::Differs => Some(crate::style::pill(
                    p,
                    "Version differs",
                    crate::style::PillTone::Warn,
                )),
                US::LocalNewer => Some(crate::style::pill(
                    p,
                    "Local newer",
                    crate::style::PillTone::Neutral,
                )),
                US::UpToDate | US::Unknown => None,
            };
            if let Some(pe) = pill_el {
                pill_pieces.push(pe);
            }
        }
        let pill_row = iced::widget::Row::with_children(pill_pieces)
            .spacing(8)
            .align_y(Alignment::Center);

        let title = text(r.name.clone()).size(22).color(p.fg_0);

        // Byline: id · version · author · file basename. Author comes
        // from the MW user lookup; omitted when MW data isn't loaded
        // yet so the row doesn't ghost-shift when the fetch lands.
        let path_str = r
            .archive_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string();
        let author_name = self
            .mw_data
            .get(&r.id)
            .and_then(|res| res.as_ref().ok())
            .and_then(|d| d.user.as_ref())
            .map(|u| u.name.clone())
            .filter(|n| !n.trim().is_empty());
        let mut byline_pieces: Vec<Element<'_, Message>> = vec![
            text(r.id.clone()).size(12).color(p.fg_1).into(),
            text("·").size(12).color(p.fg_3).into(),
            text(fmt_version(&r.version)).size(12).color(p.fg_2).into(),
        ];
        if let Some(name) = author_name {
            byline_pieces.push(text("·").size(12).color(p.fg_3).into());
            byline_pieces.push(text(name).size(12).color(p.fg_1).into());
        }
        if r.size_bytes > 0 {
            byline_pieces.push(text("·").size(12).color(p.fg_3).into());
            byline_pieces.push(text(fmt_size(r.size_bytes)).size(12).color(p.fg_2).into());
        }
        byline_pieces.push(text("·").size(12).color(p.fg_3).into());
        byline_pieces.push(text(path_str).size(12).color(p.fg_2).into());
        let byline = iced::widget::Row::with_children(byline_pieces)
            .spacing(8)
            .align_y(Alignment::Center);

        // Hero action buttons. Remote (uninstalled) selections show
        // an Install button; installed selections show Enable/Enabled
        // (and an optional Update button when MW says one's available).
        let install_err: Option<String> = self.install_errors.get(&r.id).cloned();
        let primary_btn: Element<'_, Message> = if r.is_remote {
            let installing = self.installing.contains(&r.id);
            let label = if installing {
                "Installing…"
            } else if install_err.is_some() {
                "Retry install"
            } else {
                "Install"
            };
            let kind = if installing {
                ButtonKind::Default
            } else {
                ButtonKind::Primary
            };
            let mut btn = button(text(label).size(12))
                .padding(crate::style::ButtonSize::Md.padding())
                .style(button_style(p, kind));
            if !installing {
                btn = btn.on_press(Message::InstallRemote(r.id.clone()));
            }
            btn.into()
        } else {
            let id_for_toggle = r.id.clone();
            let primary_label = if r.enabled { "✓ Enabled" } else { "Enable" };
            let primary_kind = if r.enabled {
                ButtonKind::Primary
            } else {
                ButtonKind::Default
            };
            button(text(primary_label).size(12))
                .padding(crate::style::ButtonSize::Md.padding())
                .style(button_style(p, primary_kind))
                .on_press(Message::ToggleEnabled(id_for_toggle))
                .into()
        };

        // Optional Update button - only on installed, non-folder mods
        // with an MW id when MW reports an update available. Skipped
        // entirely for remote rows since "Update" is implicit in
        // re-install for those.
        let archive_ok = r.source != ModSource::Folder;
        let has_update = matches!(
            self.mw_status.get(&r.id),
            Some(crabby_modworkshop::UpdateStatus::UpdateAvailable)
        );
        let updating = self.updating.contains(&r.id);
        let update_err = self.update_errors.get(&r.id).cloned();
        let mut hero_actions_pieces: Vec<Element<'_, Message>> = vec![primary_btn];
        if !r.is_remote
            && archive_ok
            && r.mw_id.is_some()
            && (has_update || update_err.is_some() || updating)
        {
            let label = if updating {
                "Updating…".to_string()
            } else if update_err.is_some() {
                "Retry update".to_string()
            } else {
                "Update".to_string()
            };
            let kind = if updating {
                ButtonKind::Default
            } else {
                ButtonKind::Primary
            };
            let mut btn = button(text(label).size(12))
                .padding(crate::style::ButtonSize::Md.padding())
                .style(button_style(p, kind));
            if !updating {
                btn = btn.on_press(Message::UpdateMod(r.id.clone()));
            }
            hero_actions_pieces.push(btn.into());
        } else if !r.is_remote && !archive_ok && r.mw_id.is_some() && has_update {
            // Folder mod with an upstream update - there's no archive
            // to swap, so surface the explanation instead of pretending
            // the button isn't relevant.
            hero_actions_pieces.push(
                text("Folder mod, update manually")
                    .size(11)
                    .color(p.fg_3)
                    .into(),
            );
        }
        let hero_actions_row = iced::widget::Row::with_children(hero_actions_pieces)
            .spacing(8)
            .align_y(Alignment::Center);
        // Surface install/update failures right under the button so
        // *why* the action bounced is visible - otherwise it just
        // flicks back to "Retry" with no signal.
        let action_error: Option<String> = if r.is_remote {
            install_err.clone()
        } else {
            update_err.clone()
        };
        let hero_actions: Element<'_, Message> = if let Some(e) = action_error {
            column![
                hero_actions_row,
                text(e).size(11).color(p.err).width(Length::Shrink),
            ]
            .spacing(6)
            .align_x(Alignment::End)
            .into()
        } else {
            hero_actions_row.into()
        };

        // Optional tag chip row, populated from MW data when present.
        // Each chip uses the MW-supplied color so curated tags look
        // distinct (Quality of Life green, Feedback Needed blue, etc.)
        let tags: Vec<Element<'_, Message>> = self
            .mw_data
            .get(&r.id)
            .and_then(|res| res.as_ref().ok())
            .map(|data| {
                data.mod_
                    .tags
                    .iter()
                    .filter(|t| !t.name.is_empty())
                    .map(|t| tag_chip(&t.name, &t.color, p))
                    .collect()
            })
            .unwrap_or_default();
        let hero_text = if tags.is_empty() {
            column![pill_row, title, byline].spacing(8)
        } else {
            let tag_row = iced::widget::Row::with_children(tags)
                .spacing(6)
                .align_y(Alignment::Center)
                .wrap();
            column![pill_row, title, byline, tag_row].spacing(8)
        };
        let hero_row = row![thumbnail, hero_text, crate::style::hspace(), hero_actions,]
            .spacing(18)
            .align_y(Alignment::Start)
            .padding([22, 28]);

        let hero_band = container(hero_row).width(Length::Fill).style(move |_t| {
            iced::widget::container::Style {
                background: Some(iced::Background::Color(p.bg_2)),
                text_color: Some(p.fg_0),
                border: iced::Border {
                    color: p.line_soft,
                    width: 0.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            }
        });

        // ---- Body (2 cols) ----
        // Left: About + Settings (MCM editor).
        let about_eyebrow = text("ABOUT").size(11).color(p.fg_2);
        // Pre-parsed markdown items live on `self.mw_desc_md` so the
        // widget can borrow them without re-parsing per render and
        // without lifetime trouble in this view.
        let about_body: Element<'_, Message> = match self.mw_desc_md.get(&r.id) {
            Some(items) if !items.is_empty() => {
                use iced::widget::markdown;
                let iced_palette = iced::theme::Palette {
                    background: p.bg_2,
                    text: p.fg_0,
                    primary: p.accent,
                    success: p.ok,
                    warning: p.warn,
                    danger: p.err,
                };
                // iced 0.14 folded the Style param into Settings.
                let style = markdown::Style::from_palette(iced_palette);
                let settings = markdown::Settings::with_style(style);
                let view = markdown::view(items, settings);
                view.map(|url| Message::OpenUrl(url.to_string())).into()
            }
            _ => text("No description available.")
                .size(13)
                .color(p.fg_3)
                .into(),
        };

        let config_section: Element<'_, Message> = match (
            self.mcm_for.as_deref() == Some(&r.id),
            self.mcm_cfg.as_ref(),
        ) {
            (true, Some(cfg)) => column![
                text("SETTINGS").size(11).color(p.fg_2),
                self.mcm_editor(cfg, p),
            ]
            .spacing(10)
            .into(),
            _ => column![
                text("SETTINGS").size(11).color(p.fg_2),
                text("This mod doesn't expose configuration via MCM.")
                    .size(11)
                    .color(p.fg_3),
            ]
            .spacing(10)
            .into(),
        };

        // Gallery - strip of thumbnails for the mod's MW images.
        // Empty when MW reports no gallery (or fetches haven't fired);
        // the eyebrow renders only when there's actually content so
        // the panel doesn't dangle a "GALLERY" header over nothing.
        let gallery_section: Element<'_, Message> = self.gallery_strip(r, p);

        let left_col =
            column![about_eyebrow, about_body, gallery_section, config_section].spacing(14);

        // Right sidebar: Metadata table, Requires (placeholder), Actions.
        let metadata_eyebrow = text("METADATA").size(11).color(p.fg_2);
        let mut meta_col = column![].spacing(4);
        // Common fields. Remote rows skip Source (we don't know the
        // archive kind without fetching detail) and Size (no value
        // until detail's downloaded).
        let mut meta_pairs: Vec<(&str, String)> = vec![
            ("Mod ID", r.id.clone()),
            ("Version", fmt_version(&r.version)),
        ];
        if !r.is_remote {
            meta_pairs.push(("Source", r.source.label().to_string()));
        }
        if r.size_bytes > 0 {
            meta_pairs.push(("Size", fmt_size(r.size_bytes)));
        }
        for (k, v) in meta_pairs {
            meta_col = meta_col.push(
                row![
                    text(k.to_string())
                        .size(11)
                        .color(p.fg_2)
                        .width(Length::Fixed(72.0)),
                    text(v).size(11).color(p.fg_0),
                ]
                .spacing(8),
            );
        }
        if !r.is_remote {
            // Path row - clickable link to reveal in file manager.
            let path_value: Element<'_, Message> = link_button(
                r.archive_path.display().to_string(),
                Message::OpenPath(r.archive_path.clone()),
                p,
            );
            meta_col = meta_col.push(
                row![
                    text("Path")
                        .size(11)
                        .color(p.fg_2)
                        .width(Length::Fixed(72.0)),
                    path_value,
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            );
            meta_col = meta_col.push(
                row![
                    text("Enabled")
                        .size(11)
                        .color(p.fg_2)
                        .width(Length::Fixed(72.0)),
                    text(if r.enabled { "yes" } else { "no" })
                        .size(11)
                        .color(p.fg_0),
                ]
                .spacing(8),
            );
            // Priority row - number-style input bound to mod_config's
            // per-profile override. Empty buffer = inherit manifest's
            // declared priority. Auto-commits on Enter / blur via the
            // PriorityCommit message; mid-edit text lives only in
            // `priority_buffers` so transient "-" or "" states don't
            // re-render the file. Hint text shows the manifest's
            // declared value so the override target is visible.
            let priority_id_buf = r.id.clone();
            let priority_id_commit = r.id.clone();
            let priority_buffer_text: String = self
                .priority_buffers
                .get(&r.id)
                .cloned()
                .unwrap_or_else(|| match r.priority_override {
                    Some(n) => n.to_string(),
                    None => String::new(),
                });
            let priority_hint = match r.priority_override {
                Some(_) => format!("manifest: {}", r.priority_manifest),
                None => format!("manifest default ({})", r.priority_manifest),
            };
            let p_input = text_input("inherit", &priority_buffer_text)
                .on_input(move |t| Message::PriorityBuffer(priority_id_buf.clone(), t))
                .on_submit(Message::PriorityCommit(priority_id_commit))
                .padding([4, 8])
                .size(11)
                .width(Length::Fixed(80.0))
                .style(move |_t, _s| iced::widget::text_input::Style {
                    background: iced::Background::Color(p.bg_2),
                    border: iced::Border {
                        color: p.line_soft,
                        width: 1.0,
                        radius: 0.0.into(),
                    },
                    icon: p.fg_2,
                    placeholder: p.fg_3,
                    value: p.fg_0,
                    selection: p.accent_soft,
                });
            meta_col = meta_col.push(
                row![
                    text("Priority")
                        .size(11)
                        .color(p.fg_2)
                        .width(Length::Fixed(72.0)),
                    p_input,
                    text(priority_hint).size(11).color(p.fg_3),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
            );
        }

        let requires_eyebrow = text("REQUIRES").size(11).color(p.fg_2);
        let requires_body: Element<'_, Message> =
            match self.mw_data.get(&r.id).and_then(|res| res.as_ref().ok()) {
                Some(data) if !data.mod_.dependencies.is_empty() => {
                    let mut col = column![].spacing(4);
                    for dep in &data.mod_.dependencies {
                        col = col.push(dep_row(dep, &self.mw_dep_names, p));
                    }
                    col.into()
                }
                _ => text("None declared.").size(11).color(p.fg_3).into(),
            };

        // For folder mods the archive_path *is* the folder; for vmz/zip
        // it's the archive file. Either way we want to surface the
        // containing directory for "Open mod folder" so the user can
        // see siblings.
        let folder_target = if r.archive_path.is_dir() {
            r.archive_path.clone()
        } else {
            r.archive_path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| r.archive_path.clone())
        };
        // Action column. Local-only buttons (Open mod folder, Refresh
        // this mod) are skipped on remote selections - they target a
        // file path that doesn't exist yet.
        let actions_eyebrow = text("ACTIONS").size(11).color(p.fg_2);
        let actions_col: Element<'_, Message> = if r.is_remote {
            // Nothing actionable here for now; "Install" is on the
            // hero. Render an empty placeholder so the eyebrow doesn't
            // dangle over a void.
            text("(install from the hero above)")
                .size(11)
                .color(p.fg_3)
                .into()
        } else {
            column![
                button(text("Open mod folder").size(11))
                    .padding(crate::style::ButtonSize::Sm.padding())
                    .style(button_style(p, ButtonKind::Default))
                    .width(Length::Fill)
                    .on_press(Message::OpenPath(folder_target)),
                button(text("Refresh this mod").size(11))
                    .padding(crate::style::ButtonSize::Sm.padding())
                    .style(button_style(p, ButtonKind::Default))
                    .width(Length::Fill)
                    .on_press(Message::RefreshOne(r.id.clone())),
            ]
            .spacing(6)
            .into()
        };

        // ModWorkshop section - only shown when the mod declares an
        // mw id. Renders fetch state explicitly (loading / error /
        // data) so progress is visible.
        let mw_section: Element<'_, Message> = match (r.mw_id, self.mw_data.get(&r.id)) {
            (None, _) => column![].into(),
            (Some(_mw_id), None) => column![
                text("MODWORKSHOP").size(11).color(p.fg_2),
                text("Loading…").size(11).color(p.fg_3),
            ]
            .spacing(10)
            .into(),
            (Some(_), Some(Err(e))) => column![
                text("MODWORKSHOP").size(11).color(p.fg_2),
                text(format!("Failed: {e}")).size(11).color(p.err),
            ]
            .spacing(10)
            .into(),
            (Some(mw_id), Some(Ok(data))) => mw_section_view(mw_id, data, p),
        };

        let right_col = column![
            metadata_eyebrow,
            meta_col,
            mw_section,
            requires_eyebrow,
            requires_body,
            actions_eyebrow,
            actions_col,
        ]
        .spacing(14);

        let body_row = row![
            container(left_col).width(Length::Fill),
            container(right_col).width(Length::Fixed(260.0)),
        ]
        .spacing(28)
        .padding([20, 28]);

        // Conflicts panel - appears between the hero and the main
        // body when this mod participates in any conflict. Empty
        // element collapses cleanly when the mod is clean.
        let conflicts_panel = render_conflicts_panel(&r.id, conflicts, conflict_panel_overrides, p);

        // Stack hero + body in a column. Width::Fill matters so the
        // scrollable below has a finite content width and the body's
        // Fill column has something to size against.
        let inner = column![hero_band, conflicts_panel, body_row]
            .spacing(0)
            .width(Length::Fill);

        scrollable(inner)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    /// Render the MCM editor for the selected mod. Groups fields by
    /// category and emits a kind-appropriate widget per row.
    // mw_section_view defined as a free function below so it doesn't
    // need to borrow &self.

    /// Render the gallery strip - a horizontally-scrollable row of
    /// image thumbnails. Each image opens its full-size MW page on
    /// click (no built-in lightbox in v1; clicking the thumbnail
    /// jumps to the MW gallery).
    fn gallery_strip<'a>(&'a self, r: &'a Row, p: Palette) -> Element<'a, Message> {
        let Some(Ok(data)) = self.mw_data.get(&r.id) else {
            return iced::widget::Space::new().height(Length::Fixed(0.0)).into();
        };
        if data.mod_.images.is_empty() {
            return iced::widget::Space::new().height(Length::Fixed(0.0)).into();
        }
        // Sort by display_order; visible images only.
        let mut imgs: Vec<&crabby_modworkshop::Image> = data
            .mod_
            .images
            .iter()
            .filter(|i| i.visible && !i.file.is_empty())
            .collect();
        imgs.sort_by_key(|i| i.display_order);
        if imgs.is_empty() {
            return iced::widget::Space::new().height(Length::Fixed(0.0)).into();
        }

        let mw_id = r.mw_id.unwrap_or(0);
        let mut strip = iced::widget::Row::with_children(Vec::<Element<'_, Message>>::new())
            .spacing(8)
            .align_y(Alignment::Center);
        for img in &imgs {
            let url = if mw_id != 0 {
                format!("https://modworkshop.net/mod/{mw_id}?tab=images")
            } else {
                crabby_modworkshop::image_url(&img.file)
            };
            let thumb_el: Element<'_, Message> = match self.mw_images.get(&img.file) {
                Some(MwImageState::Loaded(handle)) => iced::widget::button(
                    iced::widget::image(handle.clone())
                        .width(Length::Fixed(120.0))
                        .height(Length::Fixed(75.0))
                        .content_fit(iced::ContentFit::Cover),
                )
                .padding(0)
                .style(crate::style::link_button_style(p))
                .on_press(Message::OpenUrl(url))
                .into(),
                Some(MwImageState::Failed(_)) => crate::style::thumb(p, "(failed)", 120.0, 75.0),
                None => crate::style::thumb(p, "loading", 120.0, 75.0),
            };
            strip = strip.push(thumb_el);
        }

        iced::widget::column![
            text("GALLERY").size(11).color(p.fg_2),
            iced::widget::scrollable(strip).direction(
                iced::widget::scrollable::Direction::Horizontal(
                    iced::widget::scrollable::Scrollbar::default(),
                )
            ),
        ]
        .spacing(10)
        .into()
    }

    fn mcm_editor<'a>(&'a self, cfg: &'a McmConfig, p: Palette) -> Element<'a, Message> {
        let by_cat = cfg.fields_by_category();
        let mut col = column![].spacing(14);
        for (category, fields) in &by_cat {
            if !category.is_empty() {
                col = col.push(text(category.clone()).size(13).color(p.fg_1));
            }
            let mut group = column![].spacing(8);
            for f in fields {
                group = group.push(self.mcm_field_row(f, p));
            }
            col = col.push(container(group).padding([10, 14]).style(move |_t| {
                iced::widget::container::Style {
                    background: Some(iced::Background::Color(p.bg_3)),
                    text_color: Some(p.fg_0),
                    border: iced::Border {
                        color: p.line_soft,
                        width: 1.0,
                        radius: 0.0.into(),
                    },
                    ..Default::default()
                }
            }));
        }
        col.push(
            text(format!("MCM file: {}", cfg.path.display()))
                .size(10)
                .color(p.fg_3),
        )
        .into()
    }

    /// One field row: label + tooltip caption + kind-appropriate editor.
    fn mcm_field_row<'a>(
        &'a self,
        f: &'a crabby_config::mcm::McmField,
        p: Palette,
    ) -> Element<'a, Message> {
        let label = column![
            text(f.name.clone()).size(12).color(p.fg_0),
            text(f.tooltip.clone()).size(10).color(p.fg_3),
        ]
        .spacing(2);

        let editor: Element<'_, Message> = self.mcm_editor_widget(f, p);

        row![
            label,
            crate::style::hspace(),
            container(editor).width(Length::Fixed(220.0)),
        ]
        .spacing(12)
        .align_y(Alignment::Center)
        .into()
    }

    /// Keycode-section widget. Shows the current key/mouse label as a
    /// button; click → "Press a key…" capture mode; the App-level key
    /// subscription routes the next event back via `McmKeyCaptured`.
    fn mcm_keycode_widget<'a>(
        &'a self,
        f: &'a crabby_config::mcm::McmField,
        p: Palette,
    ) -> Element<'a, Message> {
        let section = f.section.clone();
        let key = f.key.clone();
        let is_capturing = self
            .mcm_capture
            .as_ref()
            .is_some_and(|(s, k)| s == &section && k == &key);

        let code = match &f.value {
            McmValue::Int(n) => *n,
            McmValue::Float(x) => *x as i64,
            _ => 0,
        };
        let label = if is_capturing {
            "Press a key…  (Esc cancels)".to_string()
        } else {
            crabby_config::keycode::keycode_label(code)
        };

        let kind = if is_capturing {
            ButtonKind::Primary
        } else {
            ButtonKind::Default
        };
        let mut btn = button(text(label).size(12))
            .padding([4, 12])
            .style(button_style(p, kind));
        if is_capturing {
            btn = btn.on_press(Message::McmKeyCaptureCancel);
        } else {
            btn = btn.on_press(Message::McmKeyCaptureStart(section, key));
        }
        btn.into()
    }

    fn mcm_editor_widget<'a>(
        &'a self,
        f: &'a crabby_config::mcm::McmField,
        p: Palette,
    ) -> Element<'a, Message> {
        use iced::widget::{checkbox, pick_list, text_input};

        let section = f.section.clone();
        let key = f.key.clone();

        // Keycode section is special: numbers are Godot keycode ints,
        // render as labels and capture next keystroke on click.
        if f.section == "Keycode" {
            return self.mcm_keycode_widget(f, p);
        }

        match (&f.value, !f.extras.options.is_empty()) {
            (McmValue::Bool(b), _) => {
                let s = section.clone();
                let k = key.clone();
                checkbox(*b)
                    .on_toggle(move |new_b| {
                        Message::McmFieldChanged(s.clone(), k.clone(), McmValue::Bool(new_b))
                    })
                    .into()
            }
            (McmValue::Int(idx), true) => {
                // Dropdown - value is the index into options.
                let options: Vec<String> = f.extras.options.clone();
                let current = options
                    .get(*idx as usize)
                    .cloned()
                    .unwrap_or_else(|| format!("(invalid index {idx})"));
                let s = section.clone();
                let k = key.clone();
                let opts_for_lookup = options.clone();
                pick_list(options, Some(current), move |picked| {
                    let new_idx = opts_for_lookup
                        .iter()
                        .position(|o| o == &picked)
                        .unwrap_or(0) as i64;
                    Message::McmFieldChanged(s.clone(), k.clone(), McmValue::Int(new_idx))
                })
                .text_size(12)
                .into()
            }
            (McmValue::Int(_) | McmValue::Float(_) | McmValue::Str(_), _) => {
                let buf_key = (section.clone(), key.clone());
                let displayed = self
                    .mcm_buffers
                    .get(&buf_key)
                    .cloned()
                    .unwrap_or_else(|| match &f.value {
                        McmValue::Int(n) => n.to_string(),
                        McmValue::Float(x) => format!("{x}"),
                        McmValue::Str(s) => s.clone(),
                        _ => String::new(),
                    });
                let s_buf = section.clone();
                let k_buf = key.clone();
                let s_commit = section.clone();
                let k_commit = key.clone();
                text_input("", &displayed)
                    .on_input(move |t| Message::McmInputBuffer(s_buf.clone(), k_buf.clone(), t))
                    .on_submit(Message::McmInputCommit(s_commit, k_commit))
                    .padding([4, 8])
                    .size(12)
                    .style(move |_t, _s| iced::widget::text_input::Style {
                        background: iced::Background::Color(p.bg_2),
                        border: iced::Border {
                            color: p.line_soft,
                            width: 1.0,
                            radius: 0.0.into(),
                        },
                        icon: p.fg_2,
                        placeholder: p.fg_3,
                        value: p.fg_0,
                        selection: p.accent_soft,
                    })
                    .into()
            }
        }
    }
}

/// Read all discovered mods + the active profile's enabled set, fold
/// into [`Row`]s sorted by display name.
fn load_rows(game_dir: &Path) -> Result<Vec<Row>, crabby_error::CrabbyError> {
    let cfg = ModConfig::load_or_default(game_dir)?;
    let discovered = discover_mods_for_config(game_dir, &cfg)?;
    Ok(load_rows_from_discovered(&cfg, discovered))
}

/// Variant of [`load_rows`] that consumes a pre-loaded `ModConfig` and
/// pre-discovered mod list. Boot paths share one discovery walk across
/// the mod tab, the conflict scan, the bake-status check, and the
/// mod-index rebuild instead of redoing the work four times.
pub(crate) fn load_rows_from_discovered(
    cfg: &ModConfig,
    discovered: Vec<crabby_manifest::DiscoveredMod>,
) -> Vec<Row> {
    let active_name = cfg.active_profile.clone();
    let active_profile = cfg.profiles.get(&active_name);
    let enabled_set: std::collections::HashSet<String> = active_profile
        .map(|p| {
            p.mods
                .iter()
                .filter(|(_, e)| e.enabled)
                .map(|(id, _)| id.clone())
                .collect()
        })
        .unwrap_or_default();
    let priority_overrides: std::collections::HashMap<String, i64> = active_profile
        .map(|p| {
            p.mods
                .iter()
                .filter_map(|(id, e)| e.priority_override.map(|po| (id.clone(), po)))
                .collect()
        })
        .unwrap_or_default();
    tracing::debug!(
        active_profile = %active_name,
        overrides = ?priority_overrides,
        "ui: loading rows with priority overrides",
    );

    let mut rows: Vec<Row> = discovered
        .into_iter()
        .map(|d| {
            let mw_id = d
                .manifest
                .extra_sections
                .get("updates")
                .and_then(|s| s.get("modworkshop"))
                .and_then(|v| v.parse::<u64>().ok());
            let size_bytes = compute_mod_size(&d.archive_path);
            let priority_override = priority_overrides.get(&d.manifest.id).copied();
            Row {
                enabled: enabled_set.contains(&d.manifest.id),
                id: d.manifest.id.clone(),
                name: if d.manifest.name.is_empty() {
                    d.manifest.id.clone()
                } else {
                    d.manifest.name.clone()
                },
                version: d.manifest.version.clone(),
                source: d.source,
                archive_path: d.archive_path,
                mw_id,
                size_bytes,
                is_remote: false,
                priority_manifest: d.manifest.priority,
                priority_override,
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        a.name
            .to_ascii_lowercase()
            .cmp(&b.name.to_ascii_lowercase())
    });
    rows
}

/// Toggle the mod's enabled state in the active profile and persist
/// `mod_config.cfg`. If the mod isn't yet in the profile it gets added
/// as `enabled=true`.
/// True when `s` is a valid in-progress priority input: digits, with
/// an optional leading `-`. The lone `-` (mid-edit, before the user
/// types a digit) is allowed so users can type a negative number
/// naturally. The actual integer parse happens on commit.
fn is_valid_priority_input(s: &str) -> bool {
    let mut chars = s.chars();
    let first = match chars.next() {
        Some(c) => c,
        None => return true,
    };
    if first == '-' {
        // After a leading '-' the rest must be digits (or empty -
        // the lone '-' case).
        return chars.all(|c| c.is_ascii_digit());
    }
    if !first.is_ascii_digit() {
        return false;
    }
    chars.all(|c| c.is_ascii_digit())
}

/// Persist a per-profile priority override for `mod_id`.
///
/// `Some(n)` writes the override; `None` clears it (so the manifest's
/// declared priority wins again). Refreshes mod_index.cfg afterward so
/// the runtime shim picks up the new ordering on the next launch
/// without needing a re-bake.
fn set_mod_priority_override(
    game_dir: &Path,
    mod_id: &str,
    new_override: Option<i64>,
) -> Result<(), crabby_error::CrabbyError> {
    let mut cfg = ModConfig::load_or_default(game_dir)?;
    let cfg_snapshot = cfg.clone();
    let active = cfg.active_profile.clone();
    let profile = cfg.profiles.entry(active).or_default();
    match profile.mods.get_mut(mod_id) {
        Some(entry) => entry.priority_override = new_override,
        None => {
            // Mod isn't enabled in this profile yet. Materialize a
            // disabled entry so the override sticks; toggling the mod
            // on later preserves the override.
            let discovered = discover_mods_for_config(game_dir, &cfg_snapshot)?;
            let d = discovered
                .into_iter()
                .find(|d| d.manifest.id == mod_id)
                .ok_or_else(|| crabby_error::CrabbyError::Bake {
                    context: format!("set_priority_override: no discovered mod with id {mod_id:?}"),
                    source: "mod not found".into(),
                })?;
            profile.mods.insert(
                mod_id.to_string(),
                ModEntry {
                    enabled: false,
                    version: d.manifest.version.clone(),
                    priority_override: new_override,
                },
            );
        }
    }
    cfg.save(game_dir)?;
    // Don't refresh mod_index here. The index gets rewritten by
    // `crabby_install::install` (the bake), which has full analyzer
    // state including overlay-source paths. Refreshing here would
    // overwrite that overlay metadata with a no-overlay-info copy
    // and break the runtime cache strip for overlay-bearing mods.
    Ok(())
}

fn toggle_mod(game_dir: &Path, mod_id: &str) -> Result<(), crabby_error::CrabbyError> {
    let mut cfg = ModConfig::load_or_default(game_dir)?;
    // Snapshot for the None-arm discovery call before we take a
    // mutable borrow of cfg below.
    let cfg_snapshot = cfg.clone();
    let active = cfg.active_profile.clone();
    let profile = cfg.profiles.entry(active).or_default();
    match profile.mods.get_mut(mod_id) {
        Some(entry) => entry.enabled = !entry.enabled,
        None => {
            let discovered = discover_mods_for_config(game_dir, &cfg_snapshot)?;
            let d = discovered
                .into_iter()
                .find(|d| d.manifest.id == mod_id)
                .ok_or_else(|| crabby_error::CrabbyError::Bake {
                    context: format!("toggle: no discovered mod with id {mod_id:?}"),
                    source: "mod not found".into(),
                })?;
            profile.mods.insert(
                mod_id.to_string(),
                ModEntry {
                    enabled: true,
                    version: d.manifest.version.clone(),
                    priority_override: None,
                },
            );
        }
    }
    cfg.save(game_dir)?;
    // Don't refresh mod_index here, the bake does it with full
    // overlay-source info. See the priority-edit branch above for
    // the rationale.
    Ok(())
}

/// Render the per-mod conflicts panel for the detail pane. Returns an
/// empty zero-sized element when this mod has no conflicts, so the
/// caller can unconditionally splice it into the detail column.
///
/// Header is clickable to toggle expanded/collapsed. Default state
/// is open when the mod has any Hard conflict, closed otherwise;
/// `panel_overrides` carries explicit user toggles which override
/// the default. State is persisted via `Message::ToggleConflictPanel`.
///
/// Each conflict card carries:
/// - severity-tinted border (red Hard, amber Warn)
/// - kind-specific icon glyph + headline
/// - per-participant detail rows (skipped for self-patterns where
///   the only participant IS the selected mod - its name + the
///   headline already say what's needed)
fn render_conflicts_panel<'a>(
    mod_id: &str,
    all_conflicts: &'a [crabby_mod_analyzer::Conflict],
    panel_overrides: &'a std::collections::BTreeMap<String, bool>,
    p: Palette,
) -> Element<'a, Message> {
    use crabby_mod_analyzer::{ConflictKind, Severity};

    let mine = crabby_mod_analyzer::conflicts_involving(all_conflicts, mod_id);
    if mine.is_empty() {
        return iced::widget::Space::new()
            .width(Length::Fixed(0.0))
            .height(Length::Fixed(0.0))
            .into();
    }

    let has_hard = mine.iter().any(|c| {
        matches!(c.kind, ConflictKind::DuplicateVanillaSwap { .. })
            || matches!(
                c.kind,
                ConflictKind::SelfPattern {
                    severity: Severity::Hard,
                    ..
                }
            )
    });
    let collapsed = panel_overrides.get(mod_id).copied().unwrap_or(!has_hard);

    let count_color = if has_hard { p.err } else { p.warn };
    let chevron = if collapsed { "▸" } else { "▾" };
    let header_btn = iced::widget::button(
        iced::widget::row![
            iced::widget::text(chevron).size(11).color(p.fg_2),
            iced::widget::text("CONFLICTS").size(11).color(p.fg_2),
            iced::widget::text(format!("{}", mine.len()))
                .size(11)
                .color(count_color),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    )
    .padding(0)
    .style(button_style(p, ButtonKind::Ghost))
    .on_press(Message::ToggleConflictPanel(mod_id.to_string()))
    .width(Length::Shrink);

    if collapsed {
        let body = iced::widget::column![header_btn]
            .spacing(8)
            .padding([14, 28]);
        return iced::widget::container(body)
            .style(surface_style(p, SurfaceKind::Bg2))
            .width(Length::Fill)
            .into();
    }

    let mut rows: Vec<Element<'_, Message>> = Vec::with_capacity(mine.len());
    for c in &mine {
        let (glyph, severity) = match &c.kind {
            ConflictKind::RegistryCollision { .. } => ("⚠", Severity::Warn),
            ConflictKind::ReplaceHookCollision { .. } => ("⚠", Severity::Warn),
            ConflictKind::DuplicateVanillaSwap { .. } => ("✖", Severity::Hard),
            ConflictKind::FileReplaceCollision { .. } => ("✖", Severity::Hard),
            ConflictKind::AddFileCollision { .. } => ("✖", Severity::Hard),
            ConflictKind::SelfPattern { severity, .. } => (
                if *severity == Severity::Hard {
                    "✖"
                } else {
                    "⚠"
                },
                *severity,
            ),
        };
        let accent = match severity {
            Severity::Hard => p.err,
            Severity::Warn => p.warn,
            Severity::Info => p.fg_2,
        };
        // Trim leading "<mod_id>: " from self-pattern headlines so
        // the card doesn't repeat the mod name we're already on.
        let headline = strip_self_mod_prefix(&c.headline, mod_id);
        let mut row_col = iced::widget::column![
            iced::widget::row![
                iced::widget::text(glyph).size(13).color(accent),
                iced::widget::text(headline).size(12).color(p.fg_0),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        ]
        .spacing(4);
        // Participant detail rows. For self-patterns (one participant,
        // and that participant IS this mod), drop the redundant mod-id
        // column - just show callsite + verdict on a single line.
        let is_self_pattern = matches!(c.kind, ConflictKind::SelfPattern { .. });
        for participant in &c.participants {
            if is_self_pattern && participant.mod_id == mod_id {
                let line = iced::widget::row![
                    iced::widget::text(participant.callsite.clone())
                        .size(10)
                        .color(p.fg_3)
                        .width(Length::Fixed(160.0)),
                    iced::widget::text(participant.detail.clone())
                        .size(11)
                        .color(p.fg_2)
                        .width(Length::Fill),
                ]
                .spacing(8);
                row_col = row_col.push(line);
            } else {
                let line = iced::widget::row![
                    iced::widget::text(format!("· {}", participant.mod_id))
                        .size(11)
                        .color(p.fg_1)
                        .width(Length::Fixed(160.0)),
                    iced::widget::text(participant.callsite.clone())
                        .size(10)
                        .color(p.fg_3)
                        .width(Length::Fixed(160.0)),
                    iced::widget::text(participant.detail.clone())
                        .size(11)
                        .color(p.fg_2)
                        .width(Length::Fill),
                ]
                .spacing(8);
                row_col = row_col.push(line);
            }
        }
        let card = iced::widget::container(row_col)
            .padding([8, 12])
            .style(move |_t| iced::widget::container::Style {
                background: Some(iced::Background::Color(p.bg_3)),
                text_color: Some(p.fg_0),
                border: iced::Border {
                    color: accent,
                    width: 1.0,
                    radius: 6.0.into(),
                },
                ..Default::default()
            })
            .width(Length::Fill);
        rows.push(card.into());
    }

    let body = iced::widget::column![
        header_btn,
        iced::widget::Column::with_children(rows).spacing(6),
    ]
    .spacing(8)
    .padding([14, 28]);
    iced::widget::container(body)
        .style(surface_style(p, SurfaceKind::Bg2))
        .width(Length::Fill)
        .into()
}

/// Strip "<mod_id>: " from the start of a headline if present. Used
/// for self-pattern conflict cards so the card body doesn't repeat
/// the mod name being viewed.
fn strip_self_mod_prefix(headline: &str, mod_id: &str) -> String {
    let prefix = format!("{mod_id}: ");
    if let Some(rest) = headline.strip_prefix(&prefix) {
        rest.to_string()
    } else {
        headline.to_string()
    }
}

/// Centered "no selection" panel, used by the detail pane when the
/// `selected` id resolves to neither a local row nor a remote listing.
fn detail_empty_state<'a>(p: Palette) -> Element<'a, Message> {
    iced::widget::container(
        iced::widget::column![
            iced::widget::text("Select a mod").size(14).color(p.fg_2),
            iced::widget::text("Click a row in the list to open its detail.")
                .size(11)
                .color(p.fg_3),
        ]
        .spacing(6)
        .align_x(Alignment::Center),
    )
    .center_x(Length::Fill)
    .center_y(Length::Fill)
    .into()
}

fn tag_chip<'a>(label: &str, hex: &str, p: Palette) -> Element<'a, Message> {
    let tone = parse_hex_color(hex).unwrap_or(p.fg_1);
    // 14% / 40% alpha mirrors the design's pill tone math.
    let bg = iced::Color { a: 0.14, ..tone };
    let border = iced::Color { a: 0.4, ..tone };
    iced::widget::container(text(label.to_string()).size(10).color(tone))
        .padding([2, 8])
        .style(move |_t| iced::widget::container::Style {
            background: Some(iced::Background::Color(bg)),
            text_color: Some(tone),
            border: iced::Border {
                color: border,
                width: 1.0,
                radius: 999.0.into(),
            },
            ..Default::default()
        })
        .into()
}

/// Render one Requires row: name + optional/required pill + page link.
/// Falls back to "mod #N" when MW didn't supply a name and we haven't
/// resolved one via a dep-name lookup yet.
fn dep_row<'a>(
    dep: &'a crabby_modworkshop::Dependency,
    resolved_names: &std::collections::BTreeMap<u64, String>,
    p: Palette,
) -> Element<'a, Message> {
    let label = if !dep.name.trim().is_empty() {
        dep.name.clone()
    } else if let Some(n) = resolved_names
        .get(&dep.mod_id)
        .filter(|n| !n.trim().is_empty())
    {
        n.clone()
    } else if dep.mod_id != 0 {
        format!("mod #{}", dep.mod_id)
    } else if !dep.url.is_empty() {
        dep.url.clone()
    } else {
        "(unnamed dep)".to_string()
    };
    let tone = if dep.optional {
        crate::style::PillTone::Neutral
    } else {
        crate::style::PillTone::Accent
    };
    let pill_label = if dep.optional { "optional" } else { "required" };
    let url = if !dep.url.is_empty() {
        dep.url.clone()
    } else if dep.mod_id != 0 {
        format!("https://modworkshop.net/mod/{}", dep.mod_id)
    } else {
        String::new()
    };
    let name_el: Element<'_, Message> = if url.is_empty() {
        text(label).size(11).color(p.fg_0).into()
    } else {
        link_button(label, Message::OpenUrl(url), p)
    };
    iced::widget::row![
        name_el,
        crate::style::hspace(),
        crate::style::pill(p, pill_label, tone),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

/// Parse a `#rrggbb` (or `rrggbb`) hex color into an iced `Color`.
/// Returns `None` for malformed input so callers can fall back to a
/// theme color.
fn parse_hex_color(s: &str) -> Option<iced::Color> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(iced::Color::from_rgb(
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
    ))
}

fn link_button<'a>(label: String, msg: Message, p: Palette) -> Element<'a, Message> {
    iced::widget::button(text(label).size(11))
        .padding([2, 4])
        .style(crate::style::link_button_style(p))
        .on_press(msg)
        .into()
}

/// Render the populated MW section: stats + author + last updated +
/// repo link. Free function (not a method) so it doesn't need access
/// to `&self`.
fn mw_section_view<'a>(mw_id: u64, data: &'a MwData, p: Palette) -> Element<'a, Message> {
    let author = data
        .user
        .as_ref()
        .map(|u| u.name.clone())
        .unwrap_or_else(|| format!("user #{}", data.mod_.user_id));
    let stats = format!(
        "{} downloads · {} likes · {} views",
        fmt_thousands(data.mod_.downloads),
        fmt_thousands(data.mod_.likes),
        fmt_thousands(data.mod_.views),
    );
    let last_updated = if data.mod_.bumped_at.is_empty() {
        "—".into()
    } else {
        // Trim to YYYY-MM-DD; full RFC3339 is too long for the panel.
        data.mod_.bumped_at.chars().take(10).collect::<String>()
    };
    // Prefer the curated top-level version. Some downloads carry
    // sentinel strings like "latest" (e.g. trader-improvements points
    // at a github redirect), which look bad in the UI and break
    // version comparison. Fall back to the download's version only
    // when the top-level is empty.
    let remote_version = if !data.mod_.version.trim().is_empty() {
        data.mod_.version.clone()
    } else {
        data.mod_
            .download
            .as_ref()
            .map(|dl| dl.version.clone())
            .unwrap_or_default()
    };

    let mut col = column![text("MODWORKSHOP").size(11).color(p.fg_2),].spacing(6);

    let kv = |k: &'static str, v: String| -> Element<'_, Message> {
        row![
            text(k).size(11).color(p.fg_2).width(Length::Fixed(80.0)),
            text(v).size(11).color(p.fg_0),
        ]
        .spacing(8)
        .into()
    };
    let kv_link = |k: &'static str, url: String| -> Element<'_, Message> {
        row![
            text(k).size(11).color(p.fg_2).width(Length::Fixed(80.0)),
            link_button(url.clone(), Message::OpenUrl(url), p),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .into()
    };
    col = col
        .push(kv("Author", author))
        .push(kv("Latest", fmt_version(&remote_version)))
        .push(kv("Updated", last_updated))
        .push(kv("Stats", stats));

    if !data.mod_.repo_url.is_empty() {
        col = col.push(kv_link("Repo", data.mod_.repo_url.clone()));
    }
    // Always show the MW page URL - handy for "open page in browser".
    col = col.push(kv_link(
        "Page",
        format!("https://modworkshop.net/mod/{mw_id}"),
    ));

    col.into()
}

/// Strip ModWorkshop-flavour markdown extensions that the
/// `pulldown-cmark`-based widget can't render. MW uses inline color
/// directives like `{#e74c3c}` and a parenthesized-text variant after
/// headings, e.g. `# {#e74c3c}(Section Title)`. Without this the
/// directive shows up as literal text.
///
/// Strategy: drop `{#hex}` directives and unwrap `{#hex}(text)` into
/// just `text`. Color is lost (the widget can't paint per-span colors
/// from our level), but the text reads correctly.
fn preprocess_mw_markdown(input: &str) -> String {
    // Strategy: walk byte indices but advance by full UTF-8 codepoints
    // so non-ASCII runs (▎, emoji, accented chars) copy through intact.
    // The lookahead-based directive detection only ever inspects ASCII
    // bytes (`{`, `#`, hex digits, `(`, `)`), so reading them as `u8`
    // is safe - the bug was the fallback `out.push(bytes[i] as char)`
    // which truncated multi-byte sequences into individual Latin-1 chars.
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' && i + 8 < bytes.len() && bytes[i + 1] == b'#' {
            let close = i + 8;
            if bytes[close] == b'}' && bytes[i + 2..close].iter().all(|b| b.is_ascii_hexdigit()) {
                if close + 1 < bytes.len() && bytes[close + 1] == b'(' {
                    if let Some(paren_end) = find_matching_paren(bytes, close + 1) {
                        let inner = std::str::from_utf8(&bytes[close + 2..paren_end]).unwrap_or("");
                        out.push_str(inner);
                        i = paren_end + 1;
                        continue;
                    }
                }
                i = close + 1;
                continue;
            }
        }
        // Copy one UTF-8 codepoint through. utf8_char_width returns 1
        // for ASCII, 2-4 for higher planes; falling back to 1 keeps us
        // moving on a malformed input rather than panicking.
        let width = utf8_char_width(bytes[i]).max(1);
        let end = (i + width).min(bytes.len());
        if let Ok(s) = std::str::from_utf8(&bytes[i..end]) {
            out.push_str(s);
        }
        i = end;
    }
    out
}

/// Convert "soft" single newlines into Markdown hard breaks so the
/// rendered description matches what MW shows on its website.
///
/// CommonMark normally collapses single `\n` inside a paragraph to a
/// single space - the spec calls these "soft breaks". MW's web renderer
/// preserves them as visible breaks, which is why descriptions like
/// the coop mod's WARNING block otherwise render as a wall of text.
///
/// Strategy: walk line-by-line. If a line is non-empty and the next
/// line is also non-empty (i.e. they belong to the same paragraph),
/// suffix the current line with two spaces + `\n` (the CommonMark
/// hard-break sequence). Lines inside fenced code blocks (between
/// triple backticks) are left alone since their newlines already
/// render literally.
fn inject_hard_line_breaks(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let lines: Vec<&str> = input.split('\n').collect();
    let mut in_fence = false;
    for (i, line) in lines.iter().enumerate() {
        // Detect fence open/close (don't be too clever about indented
        // fences; MW descriptions don't use them).
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            out.push_str(line);
            if i + 1 < lines.len() {
                out.push('\n');
            }
            in_fence = !in_fence;
            continue;
        }

        if in_fence {
            out.push_str(line);
            if i + 1 < lines.len() {
                out.push('\n');
            }
            continue;
        }

        let is_last = i + 1 >= lines.len();
        let next_blank = !is_last && lines[i + 1].trim().is_empty();
        let this_blank = line.trim().is_empty();

        out.push_str(line);
        if is_last {
            continue;
        }
        // Add a hard break when this line and the next both have
        // content. Otherwise keep just the original `\n` so paragraph
        // breaks (`\n\n`) and trailing blank lines still work.
        if !this_blank && !next_blank {
            // Only add the two spaces if the line doesn't already end
            // in whitespace - pulldown-cmark needs literally two
            // trailing spaces before the newline.
            if !line.ends_with("  ") {
                out.push_str("  ");
            }
        }
        out.push('\n');
    }
    out
}

/// UTF-8 byte-sequence length given the leading byte. Mirrors
/// `core::str::utf8_char_width` (which is unstable). Returns 0 for
/// invalid leading bytes; callers should clamp to ≥1 to avoid stalls.
fn utf8_char_width(b: u8) -> usize {
    match b {
        0x00..=0x7F => 1,
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => 0,
    }
}

/// Find the index of the `)` that matches the `(` at `open`.
/// Returns `None` if the parens don't balance before EOF.
fn find_matching_paren(bytes: &[u8], open: usize) -> Option<usize> {
    debug_assert_eq!(bytes[open], b'(');
    let mut depth = 0;
    for (i, &b) in bytes.iter().enumerate().skip(open) {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Render a version string with a single `v` prefix. Some authors
/// already include `v` (or `V`) in the manifest, others don't -
/// `format!("v{}", v)` blindly produces `vv1.0.3`. Returns `"—"`
/// for empty input so the caller doesn't have to special-case it.
fn fmt_version(v: &str) -> String {
    let v = v.trim();
    if v.is_empty() {
        return "—".into();
    }
    if v.starts_with('v') || v.starts_with('V') {
        v.to_string()
    } else {
        format!("v{v}")
    }
}

/// Decode raw image bytes into an `iced::widget::image::Handle` ready
/// to render. Uses the `image` crate's auto-format detection so we
/// don't have to look at the MW `kind` field. RGBA8 conversion happens
/// here since iced's wgpu renderer expects RGBA byte buffers.
fn decode_image_to_handle(bytes: &[u8]) -> Result<iced::widget::image::Handle, String> {
    let img = image::load_from_memory(bytes).map_err(|e| format!("decode: {e}"))?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    Ok(iced::widget::image::Handle::from_rgba(
        w,
        h,
        rgba.into_raw(),
    ))
}

/// On-disk size for a mod entry. Files (vmz/zip) report their own
/// length; folders sum every regular-file descendant. Errors return
/// `0` - `fmt_size` then renders "—" so the row stays clean.
fn compute_mod_size(path: &Path) -> u64 {
    let Ok(meta) = std::fs::metadata(path) else {
        return 0;
    };
    if meta.is_file() {
        return meta.len();
    }
    if meta.is_dir() {
        return walk_dir_size(path);
    }
    0
}

/// Recursively sum file sizes under `dir`. Skips on read errors so a
/// permissions-denied subdir doesn't make the whole mod size zero.
fn walk_dir_size(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut total: u64 = 0;
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_file() {
            if let Ok(m) = entry.metadata() {
                total += m.len();
            }
        } else if ft.is_dir() {
            total += walk_dir_size(&entry.path());
        }
    }
    total
}

/// Render a byte count as a short human string. `0` → `"—"` so the
/// caller doesn't have to special-case "no data". Picks the largest
/// unit where the value is ≥1 and renders one decimal for KB+ to keep
/// the visual length stable. Mirrors the format MW shows on its own
/// pages (e.g. `"3.4 MB"`).
pub fn fmt_size(bytes: u64) -> String {
    if bytes == 0 {
        return "—".into();
    }
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b < KB {
        return format!("{bytes} B");
    }
    if b < KB * KB {
        return format!("{:.1} KB", b / KB);
    }
    if b < KB * KB * KB {
        return format!("{:.1} MB", b / (KB * KB));
    }
    format!("{:.2} GB", b / (KB * KB * KB))
}

/// Add thousands separators to a u64 - `12345` → `"12,345"`.
fn fmt_thousands(n: u64) -> String {
    let s = n.to_string();
    let chars: Vec<char> = s.chars().rev().collect();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in chars.iter().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(*c);
    }
    out.chars().rev().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preprocess_strips_mw_color_directives() {
        // Heading with parenthesized colored text - the MCM mod uses
        // this in their actual description.
        let input = "# {#e74c3c}(Metro Mod Loader Users and Priority)\nbody text";
        assert_eq!(
            preprocess_mw_markdown(input),
            "# Metro Mod Loader Users and Priority\nbody text"
        );
    }

    #[test]
    fn preprocess_drops_bare_color_tag() {
        let input = "{#abcdef}rest of the line";
        assert_eq!(preprocess_mw_markdown(input), "rest of the line");
    }

    #[test]
    fn preprocess_passes_normal_text_through() {
        let input = "**bold** and [link](https://x) and `code`";
        assert_eq!(preprocess_mw_markdown(input), input);
    }

    #[test]
    fn inject_hard_breaks_keeps_paragraph_breaks() {
        let input = "first paragraph\n\nsecond paragraph";
        // Paragraph break (\n\n) preserved as-is.
        let out = inject_hard_line_breaks(input);
        assert!(out.contains("first paragraph\n\nsecond paragraph"));
    }

    #[test]
    fn inject_hard_breaks_adds_breaks_within_paragraph() {
        let input = "line one\nline two\nline three";
        let out = inject_hard_line_breaks(input);
        // Two trailing spaces + \n marks a hard break in CommonMark.
        assert_eq!(out, "line one  \nline two  \nline three");
    }

    #[test]
    fn inject_hard_breaks_skips_fenced_code() {
        let input = "intro\n```\ncode line one\ncode line two\n```\noutro";
        let out = inject_hard_line_breaks(input);
        // Inside the fence the newlines should not get hard-break spaces.
        assert!(out.contains("code line one\ncode line two"));
    }

    #[test]
    fn preprocess_preserves_multibyte_characters() {
        // Regression: previous impl pushed bytes as Latin-1 chars,
        // splitting `▎` (3 bytes) into three garbage codepoints.
        let input = "▎ section ▎ with emoji 😀 and é";
        assert_eq!(preprocess_mw_markdown(input), input);
    }

    #[test]
    fn preprocess_leaves_non_hex_braces_alone() {
        // Random `{...}` content (e.g. code samples) shouldn't get eaten.
        let input = "use std::{io, fs}; fn main() {}";
        assert_eq!(preprocess_mw_markdown(input), input);
    }

    #[test]
    fn fmt_size_picks_largest_unit() {
        assert_eq!(fmt_size(0), "—");
        assert_eq!(fmt_size(512), "512 B");
        assert_eq!(fmt_size(1500), "1.5 KB");
        assert_eq!(fmt_size(3 * 1024 * 1024), "3.0 MB");
        assert_eq!(
            fmt_size(2 * 1024 * 1024 * 1024 + 100 * 1024 * 1024),
            "2.10 GB"
        );
    }

    #[test]
    fn fmt_version_handles_v_prefix() {
        assert_eq!(fmt_version("1.0.3"), "v1.0.3");
        assert_eq!(fmt_version("v1.0.3"), "v1.0.3");
        assert_eq!(fmt_version("V2.0"), "V2.0");
        assert_eq!(fmt_version(" 1.0 "), "v1.0");
        assert_eq!(fmt_version(""), "—");
    }
}
