//! Settings/diagnostics tab.
//!
//! v0 surfaces the last-bake snapshot (manifest fields + PCK hashes)
//! plus the resolved game dir / launcher config path so testers can
//! report bug states accurately. Settings (theme picker, etc.) layer
//! in once the floating quick-theme panel is wired.

use std::path::{Path, PathBuf};

use iced::widget::{button, column, container, row, scrollable, text};
use iced::{Element, Length};

use crabby_config::{ModConfig, RootEntry};
use crabby_install::{InstallManifest, classify_pck};

use crate::style::{ButtonKind, SurfaceKind, button_style, surface_style};
use crate::theme::Palette;

/// Per-tab message.
#[derive(Debug, Clone)]
pub enum Message {
    /// Re-read on-disk state - manifest, PCK hash, etc.
    Refresh,
    /// "Add folder" clicked in the Mod sources section. The App shell
    /// handles the async folder picker and routes the result back via
    /// [`AddRoot`].
    PickRoot,
    /// Result of the folder picker. `None` = cancelled.
    AddRoot(Option<PathBuf>),
    /// Toggle the dev flag on the root with this path.
    ToggleRootDev(PathBuf),
    /// Remove the root with this path.
    RemoveRoot(PathBuf),
    /// "Confirm destructive actions" checkbox toggled in Settings →
    /// General. App owns persistence (writes `launcher.toml`).
    ToggleConfirmDestructive(bool),
}

/// Cached snapshot. Computing the PCK hash is cheap (the file is
/// already on disk; sha256 over a few hundred MB takes <1s on SSD).
#[derive(Debug, Default)]
struct Snapshot {
    /// Path to the resolved game dir, displayed verbatim.
    game_dir: Option<String>,
    /// Manifest schema version.
    schema_version: Option<u32>,
    /// Bake key string.
    bake_key: Option<String>,
    /// `installed_at` Unix seconds.
    installed_at: Option<u64>,
    /// Vanilla PCK SHA-256, lowercase hex.
    vanilla_hash: Option<String>,
    /// Last-baked PCK SHA-256, lowercase hex.
    last_baked_hash: Option<String>,
    /// Current PCK classification - typed so other parts of the App
    /// (launch button gating, first-run banner) can inspect without
    /// string-matching the debug repr.
    pck_state: Option<crabby_install::PckState>,
    /// Stringified `pck_state` for the diagnostics-tab kv display.
    /// Kept alongside the typed value so the view layer doesn't have
    /// to re-format on every render.
    pck_state_label: Option<String>,
    /// Number of files crabby placed during the last install.
    placed_files: Option<usize>,
    /// Path to the launcher config (`launcher.toml`).
    launcher_config_path: Option<String>,
    /// Extra mod-source roots from `mod_config.cfg`. Empty list is
    /// the default state (just `<game-dir>/Mods/`).
    extra_roots: Vec<RootEntry>,
    /// Set to `true` once `refresh()` has run, so the view can tell
    /// "no data yet" from "data really is empty."
    populated: bool,
}

/// Per-tab state.
#[derive(Debug, Default)]
pub struct State {
    /// Bumped via [`invalidate`] to force re-fetch.
    pub generation: u64,
    snapshot: Snapshot,
    snapshot_gen: Option<u64>,
}

impl State {
    /// Drop cached state.
    pub fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.snapshot_gen = None;
    }

    /// True when the live PCK matches the last-baked hash - i.e. the
    /// game would launch with crabby's bake. Used by the App's launch
    /// button to gate the first-run "not baked yet" case. Returns
    /// `false` when the snapshot hasn't been built yet (block launch
    /// on stale data rather than allow it).
    #[must_use]
    pub fn is_baked(&self) -> bool {
        matches!(
            self.snapshot.pck_state,
            Some(crabby_install::PckState::OursCurrent { .. })
        )
    }

    /// True when the snapshot has been populated at least once. Used
    /// by the launch button to distinguish "unknown" from "known, and
    /// not baked".
    #[must_use]
    pub fn snapshot_populated(&self) -> bool {
        self.snapshot.populated
    }

    /// Force a re-scan from disk. App calls this on Refresh and when
    /// the install button finishes a bake.
    pub fn refresh(&mut self, game_dir: Option<&Path>) {
        self.invalidate();
        self.snapshot = build_snapshot(game_dir);
        self.snapshot_gen = Some(self.generation);
    }

    /// Apply a message.
    pub fn update(&mut self, message: Message, game_dir: Option<&Path>) {
        match message {
            Message::Refresh => self.refresh(game_dir),
            Message::PickRoot => {
                // Folder picker is dispatched by the App shell; this
                // arm exists so the routing in app.rs has somewhere to
                // dispatch when reusing the same Message enum, but the
                // actual work is in App::update's PickRoot handler.
            }
            Message::AddRoot(picked) => {
                if let (Some(path), Some(dir)) = (picked, game_dir) {
                    if let Err(e) = mutate_roots(dir, |roots| {
                        if !roots.iter().any(|r| r.path == path) {
                            roots.push(RootEntry { path, dev: false });
                        }
                    }) {
                        tracing::warn!(error = %e, "diagnostics: add root failed");
                    }
                    self.refresh(game_dir);
                }
            }
            Message::ToggleRootDev(path) => {
                if let Some(dir) = game_dir {
                    if let Err(e) = mutate_roots(dir, |roots| {
                        for r in roots.iter_mut() {
                            if r.path == path {
                                r.dev = !r.dev;
                                break;
                            }
                        }
                    }) {
                        tracing::warn!(error = %e, "diagnostics: toggle dev failed");
                    }
                    self.refresh(game_dir);
                }
            }
            Message::RemoveRoot(path) => {
                if let Some(dir) = game_dir {
                    if let Err(e) = mutate_roots(dir, |roots| {
                        roots.retain(|r| r.path != path);
                    }) {
                        tracing::warn!(error = %e, "diagnostics: remove root failed");
                    }
                    self.refresh(game_dir);
                }
            }
            Message::ToggleConfirmDestructive(_) => {
                // App-owned state - no-op here. App handles persistence
                // via launcher_config.save() in its Settings → General
                // routing. The variant is listed so message dispatch
                // exhausts the enum cleanly even though this state
                // doesn't react.
            }
        }
    }

    /// Render the General sub-view: game dir, mod sources. (Other
    /// "settings" land here over time - cache dir, autodetect knobs,
    /// launcher behavior toggles.)
    pub fn general_view<'a>(
        &'a self,
        _game_dir: Option<&Path>,
        confirm_destructive_actions: bool,
        palette: &Palette,
    ) -> Element<'a, Message> {
        let p = *palette;
        let s = &self.snapshot;

        let eyebrow = |label: &'a str| text(label).size(11).color(p.fg_2);

        let kv_row = |label: &'a str, value: String| -> Element<'a, Message> {
            kv_row_widget(label, value, p)
        };

        let refresh_btn = button(text("Refresh").size(11))
            .padding([4, 10])
            .style(button_style(p, ButtonKind::Default))
            .on_press(Message::Refresh);
        let header = row![
            text("General").size(20).color(p.fg_0),
            crate::style::hspace(),
            refresh_btn,
        ]
        .align_y(iced::Alignment::Center);

        let body: Element<'a, Message> = if !s.populated {
            text("Click Refresh to load.").size(12).color(p.fg_3).into()
        } else {
            let mut col = column![
                eyebrow("PATHS"),
                kv_row("Game dir", s.game_dir.clone().unwrap_or_else(|| "—".into())),
                kv_row(
                    "Launcher config",
                    s.launcher_config_path.clone().unwrap_or_else(|| "—".into()),
                ),
                eyebrow("MOD SOURCES"),
            ]
            .spacing(8);

            let add_btn = button(text("Add folder").size(11))
                .padding([4, 10])
                .style(button_style(p, ButtonKind::Default))
                .on_press(Message::PickRoot);
            col = col.push(
                row![
                    text("Scan these folders for mods, in addition to the game's Mods/ dir.")
                        .size(11)
                        .color(p.fg_2),
                    crate::style::hspace(),
                    add_btn,
                ]
                .spacing(12)
                .align_y(iced::Alignment::Center),
            );
            if s.extra_roots.is_empty() {
                col = col.push(
                    text("No extra roots configured. Click \"Add folder\" to point at a dev checkout.")
                        .size(11)
                        .color(p.fg_3),
                );
            } else {
                for root in &s.extra_roots {
                    col = col.push(root_row(root, p));
                }
                col = col.push(
                    text("Dev roots take precedence over the game's Mods/ for matching mod ids.")
                        .size(10)
                        .color(p.fg_3),
                );
            }

            // Confirmations section. Single bool today (gates the
            // Move-to-profile flow on the Saves tab); future destructive
            // actions reuse the same toggle.
            col = col.push(eyebrow("CONFIRMATIONS"));
            let confirm_box = iced::widget::checkbox(confirm_destructive_actions)
                .size(14)
                .on_toggle(Message::ToggleConfirmDestructive);
            col = col.push(
                row![
                    confirm_box,
                    text("Confirm destructive actions (slot move, delete, …)")
                        .size(11)
                        .color(p.fg_0),
                ]
                .spacing(8)
                .align_y(iced::Alignment::Center),
            );
            col = col.push(
                text("When off, destructive actions execute immediately without a confirm step.")
                    .size(10)
                    .color(p.fg_3),
            );

            col.into()
        };

        scrollable(column![header, body].spacing(14).padding(20))
            .height(Length::Fill)
            .into()
    }

    /// Render the Diagnostics sub-view: read-only install/PCK/manifest
    /// snapshot. No mutations - that's General's job.
    pub fn diagnostics_view<'a>(&'a self, palette: &Palette) -> Element<'a, Message> {
        let p = *palette;
        let s = &self.snapshot;

        let eyebrow = |label: &'a str| text(label).size(11).color(p.fg_2);
        let kv_row = |label: &'a str, value: String| -> Element<'a, Message> {
            kv_row_widget(label, value, p)
        };

        let refresh_btn = button(text("Refresh").size(11))
            .padding([4, 10])
            .style(button_style(p, ButtonKind::Default))
            .on_press(Message::Refresh);
        let header = row![
            text("Diagnostics").size(20).color(p.fg_0),
            crate::style::hspace(),
            refresh_btn,
        ]
        .align_y(iced::Alignment::Center);

        let body: Element<'a, Message> = if !s.populated {
            text("Click Refresh to load diagnostics.")
                .size(12)
                .color(p.fg_3)
                .into()
        } else {
            column![
                eyebrow("BUILD"),
                kv_row("Launcher version", crate::app::APP_VERSION.to_string()),
                kv_row("Git commit", crate::app::BUILD_GIT_SHA.to_string()),
                kv_row("Build time", crate::app::BUILD_TIME.to_string()),
                eyebrow("INSTALL"),
                kv_row("Schema version", as_or_dash(s.schema_version)),
                kv_row("Bake key", s.bake_key.clone().unwrap_or_else(|| "—".into())),
                kv_row("Installed at", format_unix(s.installed_at)),
                kv_row("Placed files", as_or_dash(s.placed_files)),
                eyebrow("PCK"),
                kv_row(
                    "Current state",
                    s.pck_state_label.clone().unwrap_or_else(|| "—".into())
                ),
                kv_row("Vanilla hash", short_hash(s.vanilla_hash.as_deref())),
                kv_row("Last baked hash", short_hash(s.last_baked_hash.as_deref())),
            ]
            .spacing(8)
            .into()
        };

        scrollable(column![header, body].spacing(14).padding(20))
            .height(Length::Fill)
            .into()
    }

    /// Legacy single-pane view, kept until the Settings shell is
    /// fully wired in app.rs. Delete once the shell is the only
    /// entry point.
    pub fn view<'a>(&'a self, _game_dir: Option<&Path>, palette: &Palette) -> Element<'a, Message> {
        let p = *palette;
        let s = &self.snapshot;

        // Section heading style - used for both Diagnostics and
        // Launcher sections; iced doesn't support reusable widget
        // closures cleanly so it's inlined as a helper here.
        let eyebrow = |label: &'a str| text(label).size(11).color(p.fg_2);

        let kv_row = |label: &'a str, value: String| -> Element<'a, Message> {
            let p = p;
            container(
                row![
                    text(label)
                        .size(12)
                        .color(p.fg_2)
                        .width(Length::Fixed(180.0)),
                    text(value).size(12).color(p.fg_0),
                ]
                .spacing(12)
                .padding([6, 14]),
            )
            .style(move |_t| iced::widget::container::Style {
                background: Some(iced::Background::Color(p.bg_3)),
                text_color: Some(p.fg_0),
                border: iced::Border {
                    color: p.line_soft,
                    width: 1.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            })
            .width(Length::Fill)
            .into()
        };

        let refresh_btn = button(text("Refresh diagnostics").size(11))
            .padding([4, 10])
            .style(button_style(p, ButtonKind::Default))
            .on_press(Message::Refresh);

        let header = row![
            text("Diagnostics").size(20).color(p.fg_0),
            crate::style::hspace(),
            refresh_btn,
        ]
        .align_y(iced::Alignment::Center);

        let body: Element<'a, Message> = if !s.populated {
            text("Click Refresh to load diagnostics.")
                .size(12)
                .color(p.fg_3)
                .into()
        } else {
            let mut col = column![
                eyebrow("INSTALL"),
                kv_row("Schema version", as_or_dash(s.schema_version)),
                kv_row("Bake key", s.bake_key.clone().unwrap_or_else(|| "—".into())),
                kv_row("Installed at", format_unix(s.installed_at)),
                kv_row("Placed files", as_or_dash(s.placed_files)),
                eyebrow("PCK"),
                kv_row(
                    "Current state",
                    s.pck_state_label.clone().unwrap_or_else(|| "—".into())
                ),
                kv_row("Vanilla hash", short_hash(s.vanilla_hash.as_deref())),
                kv_row("Last baked hash", short_hash(s.last_baked_hash.as_deref())),
                eyebrow("LAUNCHER"),
                kv_row("Game dir", s.game_dir.clone().unwrap_or_else(|| "—".into())),
                kv_row(
                    "Launcher config",
                    s.launcher_config_path.clone().unwrap_or_else(|| "—".into()),
                ),
            ]
            .spacing(8);

            // MOD SOURCES - extra roots configured via [crabby.roots].
            // The canonical <game-dir>/Mods/ is implicit and always
            // scanned; only user-added roots are listed.
            col = col.push(eyebrow("MOD SOURCES"));
            let add_btn = button(text("Add folder").size(11))
                .padding([4, 10])
                .style(button_style(p, ButtonKind::Default))
                .on_press(Message::PickRoot);
            col = col.push(
                row![
                    text("Scan these folders for mods, in addition to the game's Mods/ dir.")
                        .size(11)
                        .color(p.fg_2),
                    crate::style::hspace(),
                    add_btn,
                ]
                .spacing(12)
                .align_y(iced::Alignment::Center),
            );
            if s.extra_roots.is_empty() {
                col = col.push(
                    text("No extra roots configured. Click \"Add folder\" to point at a dev checkout.")
                        .size(11)
                        .color(p.fg_3),
                );
            } else {
                for root in &s.extra_roots {
                    col = col.push(root_row(root, p));
                }
                col = col.push(
                    text("Dev roots take precedence over the game's Mods/ for matching mod ids.")
                        .size(10)
                        .color(p.fg_3),
                );
            }

            col.into()
        };

        container(scrollable(column![header, body].spacing(14).padding(20)))
            .style(surface_style(p, SurfaceKind::Bg2))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}

fn build_snapshot(game_dir: Option<&Path>) -> Snapshot {
    let mut s = Snapshot {
        populated: true,
        ..Snapshot::default()
    };
    s.launcher_config_path = crate::launcher_config::config_path().map(|p| p.display().to_string());

    let Some(dir) = game_dir else {
        return s;
    };
    s.game_dir = Some(dir.display().to_string());

    if let Ok(cfg) = ModConfig::load_or_default(dir) {
        s.extra_roots = cfg.extra_roots;
    }

    match InstallManifest::load(dir) {
        Ok(Some(m)) => {
            s.schema_version = Some(m.schema_version);
            s.bake_key = Some(format!("{}", m.bake_key));
            s.installed_at = Some(m.installed_at);
            s.placed_files = Some(m.placed_files.len());
            s.vanilla_hash = m.vanilla_pck_hash.clone();
            s.last_baked_hash = m.last_baked_pck_hash.clone();
            // Classify the live PCK against the recorded hashes.
            match classify_pck(
                dir,
                m.vanilla_pck_hash.as_deref(),
                m.last_baked_pck_hash.as_deref(),
            ) {
                Ok(state) => {
                    s.pck_state_label = Some(format!("{state:?}"));
                    s.pck_state = Some(state);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "diagnostics: classify_pck failed");
                }
            }
        }
        Ok(None) => {
            s.pck_state_label = Some("not installed yet".into());
        }
        Err(e) => {
            tracing::warn!(error = %e, "diagnostics: manifest load failed");
        }
    }
    s
}

/// One row in the Mod sources list: path + DEV toggle + Remove button.
fn root_row<'a>(root: &'a RootEntry, palette: Palette) -> Element<'a, Message> {
    let p = palette;
    let dev_label = if root.dev { "DEV ✓" } else { "DEV" };
    let dev_kind = if root.dev {
        ButtonKind::Primary
    } else {
        ButtonKind::Ghost
    };
    let dev_btn = button(text(dev_label).size(10))
        .padding([3, 8])
        .style(button_style(p, dev_kind))
        .on_press(Message::ToggleRootDev(root.path.clone()));
    let remove_btn = button(text("Remove").size(10))
        .padding([3, 8])
        .style(button_style(p, ButtonKind::Ghost))
        .on_press(Message::RemoveRoot(root.path.clone()));
    container(
        row![
            text(root.path.display().to_string()).size(12).color(p.fg_0),
            crate::style::hspace(),
            dev_btn,
            remove_btn,
        ]
        .spacing(8)
        .padding([6, 14])
        .align_y(iced::Alignment::Center),
    )
    .style(move |_t| iced::widget::container::Style {
        background: Some(iced::Background::Color(p.bg_3)),
        text_color: Some(p.fg_0),
        border: iced::Border {
            color: p.line_soft,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    })
    .width(Length::Fill)
    .into()
}

/// Load `mod_config.cfg`, hand the `extra_roots` vec to `f` for in-place
/// editing, and persist. Centralizes the load/save bookkeeping for the
/// add/remove/toggle handlers.
fn mutate_roots(
    game_dir: &Path,
    f: impl FnOnce(&mut Vec<RootEntry>),
) -> Result<(), crabby_error::CrabbyError> {
    let mut cfg = ModConfig::load_or_default(game_dir)?;
    f(&mut cfg.extra_roots);
    cfg.save(game_dir)?;
    Ok(())
}

/// Two-column key/value row used by both the General and Diagnostics
/// sub-views. Background is `bg_3` to read as an inset surface against
/// the parent `bg_2` body.
fn kv_row_widget<'a>(label: &'a str, value: String, p: Palette) -> Element<'a, Message> {
    container(
        row![
            text(label)
                .size(12)
                .color(p.fg_2)
                .width(Length::Fixed(180.0)),
            text(value).size(12).color(p.fg_0),
        ]
        .spacing(12)
        .padding([6, 14]),
    )
    .style(move |_t| iced::widget::container::Style {
        background: Some(iced::Background::Color(p.bg_3)),
        text_color: Some(p.fg_0),
        border: iced::Border {
            color: p.line_soft,
            width: 1.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    })
    .width(Length::Fill)
    .into()
}

fn as_or_dash<T: std::fmt::Display>(v: Option<T>) -> String {
    v.map(|x| x.to_string()).unwrap_or_else(|| "—".into())
}

fn short_hash(h: Option<&str>) -> String {
    match h {
        Some(s) if s.len() >= 12 => format!("{}…", &s[..12]),
        Some(s) => s.to_string(),
        None => "—".into(),
    }
}

fn format_unix(secs: Option<u64>) -> String {
    let Some(s) = secs else {
        return "—".into();
    };
    // Approximate "minutes/hours/days ago" - chrono isn't needed for
    // this; integer arithmetic is fine.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if now < s {
        return format!("epoch {s}");
    }
    let delta = now - s;
    if delta < 60 {
        format!("{delta}s ago")
    } else if delta < 3600 {
        format!("{}m ago", delta / 60)
    } else if delta < 86_400 {
        format!("{}h ago", delta / 3600)
    } else {
        format!("{}d ago", delta / 86_400)
    }
}
