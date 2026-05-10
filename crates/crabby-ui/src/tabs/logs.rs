//! Logs tab - view + filter the launcher's own log file.
//!
//! `tracing_appender` writes JSON-lines to
//! `<user-config>/crabby/logs/launcher.log.YYYY-MM-DD`. This tab tails
//! that file: load on enter, refresh on demand, filter by level +
//! free-text search.
//!
//! v1 reads the *current day's* file only. Older days exist on disk
//! (7-day retention) but the picker for them is deferred - the most
//! common case is "what just went wrong".

use std::fs;
use std::path::Path;

use iced::widget::{button, column, container, row, scrollable, text, text_input};
use iced::{Alignment, Element, Length};

use crate::launcher_config;
use crate::style::{ButtonKind, SurfaceKind, button_style, surface_style};
use crate::theme::Palette;

/// One parsed log line. The raw text is kept around for the unparseable
/// case so non-JSON garbage in the file still surfaces.
#[derive(Debug, Clone)]
pub struct LogEntry {
    /// Original line in the file. Used for display fallback when JSON
    /// parsing fails (rare - only if a write was truncated).
    pub raw: String,
    /// `INFO` / `WARN` / `ERROR` / `DEBUG` / `TRACE`.
    pub level: String,
    /// RFC3339 timestamp string (`tracing_appender` JSON's default
    /// "timestamp" field). Stored as-is for display; sort relies on
    /// file order, not parsing.
    pub timestamp: String,
    /// Source target (module path).
    pub target: String,
    /// Log message body.
    pub message: String,
    /// Structured key/value pairs from the JSON `fields` block other
    /// than `message` (e.g. `path`, `error`, `mod_id`). Sorted by key
    /// when shown in the expanded detail.
    pub extras: std::collections::BTreeMap<String, String>,
}

/// Per-tab message.
#[derive(Debug, Clone)]
pub enum Message {
    /// Re-read the log file from disk.
    Refresh,
    /// Filter by level. `None` = all.
    LevelSelected(Option<LevelFilter>),
    /// Free-text search.
    SearchChanged(String),
    /// Toggle the expanded state of one row, identified by its index
    /// in the parsed-entries vec (file order). Expanded rows show
    /// the full message + extras key/value list.
    ToggleExpanded(usize),
    /// Switch which log file feeds the view. Re-reads from disk.
    SourceSelected(Source),
}

/// Which log feed the tab is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Source {
    /// Launcher's own JSON-lines log under `<user-config>/crabby/logs/`.
    #[default]
    Launcher,
    /// Game's stdout log Godot writes to
    /// `<user-data>/Road to Vostok/logs/godot.log`.
    Godot,
    /// Synthetic feed: every finding the mod analyzer captured for the
    /// active profile, rendered as log lines. Severity maps to level
    /// (Hard → ERROR, Warn → WARN, Info → INFO). Doesn't read from
    /// disk - consumes the App's cached `Vec<ModIntent>`.
    Analyzer,
}

/// Level filter chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LevelFilter {
    /// INFO and above.
    Info,
    /// WARN and above.
    Warn,
    /// ERROR only.
    Error,
}

impl LevelFilter {
    fn matches(self, level: &str) -> bool {
        let level = level.to_ascii_uppercase();
        match self {
            // Treat OK / TRACE / DEBUG as "info-like" so the chip
            // doesn't hide useful lines.
            Self::Info => true,
            Self::Warn => matches!(level.as_str(), "WARN" | "WARNING" | "ERROR"),
            Self::Error => matches!(level.as_str(), "ERROR"),
        }
    }
}

/// Per-tab state.
#[derive(Debug, Default)]
pub struct State {
    /// Cached log entries, oldest-first (file order).
    entries: Vec<LogEntry>,
    /// Path actually loaded from on the last refresh - surfaced in the
    /// UI footer so users can find the file.
    last_path: Option<std::path::PathBuf>,
    /// Active level filter. `None` = all.
    pub level: Option<LevelFilter>,
    /// Free-text search query.
    pub search: String,
    /// Set once at least one load has been attempted, so the view can
    /// distinguish "not read yet" from "log is empty".
    populated: bool,
    /// Row indices currently expanded. Cleared on Refresh since
    /// indices won't line up with re-read entries.
    expanded: std::collections::BTreeSet<usize>,
    /// Currently-displayed log feed.
    pub source: Source,
}

impl State {
    /// Force a re-read from disk (or re-synthesize from analyzer data).
    /// Called on tab-enter and on Refresh.
    ///
    /// The analyzer view is consulted only when the active source is
    /// `Source::Analyzer`. Pass `AnalyzerView::default()` when the App
    /// doesn't have data yet - the empty source produces an empty
    /// entry list, which the view renders as a "click Rescan to
    /// populate" empty state.
    pub fn refresh(&mut self, analyzer: AnalyzerView<'_>) {
        self.populated = true;
        // Indices in `expanded` are positional; nuke them on refresh
        // so a row that's no longer at the same position doesn't end
        // up spookily expanded.
        self.expanded.clear();
        if matches!(self.source, Source::Analyzer) {
            // Synthetic source: no file path, no IO; build entries
            // from the analyzer's cached intents + conflicts.
            self.last_path = None;
            self.entries = synthesize_analyzer_entries(analyzer);
            return;
        }
        let path = match self.source {
            Source::Launcher => launcher_config::current_log_path(),
            Source::Godot => godot_log_path(),
            Source::Analyzer => unreachable!("handled above"),
        };
        let Some(path) = path else {
            self.entries.clear();
            self.last_path = None;
            return;
        };
        self.last_path = Some(path.clone());
        self.entries = match fs::read_to_string(&path) {
            Ok(text) => match self.source {
                Source::Launcher => parse_log(&text),
                Source::Godot => parse_godot_log(&text),
                Source::Analyzer => unreachable!(),
            },
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!(path = %path.display(), error = %e, "logs: read failed");
                }
                Vec::new()
            }
        };
    }

    /// Apply a message. `analyzer` is consulted only by the Analyzer
    /// source on Refresh / SourceSelected paths; pass
    /// `AnalyzerView::default()` when not available.
    pub fn update(&mut self, message: Message, analyzer: AnalyzerView<'_>) {
        match message {
            Message::Refresh => self.refresh(analyzer),
            Message::LevelSelected(f) => self.level = f,
            Message::SearchChanged(s) => self.search = s,
            Message::ToggleExpanded(idx) => {
                if !self.expanded.insert(idx) {
                    self.expanded.remove(&idx);
                }
            }
            Message::SourceSelected(s) => {
                if self.source != s {
                    self.source = s;
                    self.refresh(analyzer);
                }
            }
        }
    }

    /// Counts by level for the chip badges.
    fn counts(&self) -> (usize, usize, usize) {
        let mut info = 0_usize;
        let mut warn = 0_usize;
        let mut err = 0_usize;
        for e in &self.entries {
            let lvl = e.level.to_ascii_uppercase();
            match lvl.as_str() {
                "ERROR" => err += 1,
                "WARN" | "WARNING" => warn += 1,
                _ => info += 1,
            }
        }
        (info, warn, err)
    }

    /// Render the tab body.
    pub fn view<'a>(&'a self, palette: &Palette) -> Element<'a, Message> {
        let p = *palette;

        let (info_count, warn_count, err_count) = self.counts();

        let refresh_btn = button(text("Refresh").size(11))
            .padding([4, 10])
            .style(button_style(p, ButtonKind::Default))
            .on_press(Message::Refresh);

        // Source toggle - Launcher vs Godot. Pill-button pair styled
        // like the level filter chips.
        let mk_source_btn = |label: &'static str, target: Source| -> Element<'_, Message> {
            let active = self.source == target;
            button(text(label).size(11))
                .padding([3, 10])
                .style(crate::style::filter_chip_style(
                    p,
                    active,
                    crate::style::PillTone::Neutral,
                ))
                .on_press(Message::SourceSelected(target))
                .into()
        };
        let source_pills = row![
            mk_source_btn("Launcher", Source::Launcher),
            mk_source_btn("Godot", Source::Godot),
            mk_source_btn("Analyzer", Source::Analyzer),
        ]
        .spacing(6)
        .align_y(Alignment::Center);

        let title = match self.source {
            Source::Launcher => "Launcher logs",
            Source::Godot => "Godot logs",
            Source::Analyzer => "Analyzer findings",
        };
        let header = row![
            text(title).size(20).color(p.fg_0),
            text(format!("{} entries", self.entries.len()))
                .size(11)
                .color(p.fg_2),
            crate::style::hspace(),
            source_pills,
            refresh_btn,
        ]
        .spacing(12)
        .align_y(Alignment::Center);

        // Level chips.
        let mk_chip = |label: &'static str,
                       count: usize,
                       filter: Option<LevelFilter>,
                       tone: iced::Color|
         -> Element<'_, Message> {
            let active = self.level == filter;
            let lbl = text(format!("{label} {count}")).size(11);
            button(lbl)
                .padding([3, 9])
                .style(crate::style::filter_chip_style(
                    p,
                    active,
                    match tone {
                        c if c == p.warn => crate::style::PillTone::Warn,
                        c if c == p.err => crate::style::PillTone::Err,
                        _ => crate::style::PillTone::Neutral,
                    },
                ))
                .on_press(Message::LevelSelected(filter))
                .into()
        };

        let chips = row![
            mk_chip("All", self.entries.len(), None, p.fg_1),
            mk_chip("Info", info_count, Some(LevelFilter::Info), p.fg_1),
            mk_chip("Warn", warn_count, Some(LevelFilter::Warn), p.warn),
            mk_chip("Error", err_count, Some(LevelFilter::Error), p.err),
        ]
        .spacing(6)
        .align_y(Alignment::Center);

        let search_box = text_input("Filter messages", &self.search)
            .on_input(Message::SearchChanged)
            .padding([4, 10])
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

        let toolbar = row![chips, crate::style::hspace(), search_box]
            .spacing(12)
            .align_y(Alignment::Center);

        // Filter the entries. Carry the original index alongside each
        // entry so the expand toggle can route to the right row even
        // when the visible set is filtered.
        let q = self.search.trim().to_ascii_lowercase();
        let visible: Vec<(usize, &LogEntry)> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| match self.level {
                Some(l) => l.matches(&e.level),
                None => true,
            })
            .filter(|(_, e)| {
                if q.is_empty() {
                    return true;
                }
                e.message.to_ascii_lowercase().contains(&q)
                    || e.target.to_ascii_lowercase().contains(&q)
            })
            .collect();

        let body: Element<'_, Message> = if !self.populated {
            text("Click Refresh to load.").size(12).color(p.fg_3).into()
        } else if self.entries.is_empty() {
            column![
                text("Log file is empty or hasn't been written yet.")
                    .size(12)
                    .color(p.fg_3),
                if let Some(path) = &self.last_path {
                    text(format!("Path: {}", path.display()))
                        .size(10)
                        .color(p.fg_3)
                } else {
                    text("No log path resolved on this platform.")
                        .size(10)
                        .color(p.fg_3)
                },
            ]
            .spacing(6)
            .into()
        } else if visible.is_empty() {
            text("Nothing matches the current filter.")
                .size(12)
                .color(p.fg_3)
                .into()
        } else {
            // Render rows newest-first so the latest line is on top.
            let mut rows: Vec<Element<'_, Message>> = Vec::with_capacity(visible.len());
            for (idx, e) in visible.iter().rev() {
                let expanded = self.expanded.contains(idx);
                rows.push(log_row(*idx, e, expanded, p));
            }
            scrollable(column(rows).spacing(2))
                .height(Length::Fill)
                .into()
        };

        let path_line = self
            .last_path
            .as_ref()
            .map(|p| format!("{}", p.display()))
            .unwrap_or_else(|| "—".into());
        let footer = row![
            text("file").size(10).color(p.fg_3),
            text("·").size(10).color(p.fg_3),
            text(path_line).size(10).color(p.fg_2),
        ]
        .spacing(6)
        .align_y(Alignment::Center);

        // Header + toolbar are sticky at the top; the body owns its
        // own scrollable so only the rows scroll. The footer pins to
        // the bottom.
        let top = column![header, toolbar].spacing(14);
        let body_pane = container(body).width(Length::Fill).height(Length::Fill);
        let layout = column![top, body_pane, footer]
            .spacing(14)
            .padding(20)
            .height(Length::Fill);

        container(layout)
            .style(surface_style(p, SurfaceKind::Bg2))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}

/// One log row - colored level chip + timestamp + target + message,
/// plus a chevron column. Click anywhere on the row to expand and
/// see the full message + structured fields.
fn log_row<'a>(idx: usize, e: &'a LogEntry, expanded: bool, p: Palette) -> Element<'a, Message> {
    let level_upper = e.level.to_ascii_uppercase();
    let level_color = match level_upper.as_str() {
        "ERROR" => p.err,
        "WARN" | "WARNING" => p.warn,
        "INFO" => p.ok,
        _ => p.fg_2,
    };
    let bg = match level_upper.as_str() {
        "ERROR" => with_alpha(p.err, 0.06),
        "WARN" | "WARNING" => with_alpha(p.warn, 0.06),
        _ => p.bg_3,
    };

    // Whether the row has expandable content. Pure-message rows with
    // no extras still get the chevron so the un-truncated message can
    // be read; long messages also benefit. Trivial rows (short
    // message, no extras) just no-op the chevron rather than hide it
    // - keeps the column alignment stable.
    let chevron_glyph = if expanded { "v" } else { ">" };

    let header_inner = row![
        text(level_upper)
            .size(10)
            .color(level_color)
            .width(Length::Fixed(48.0)),
        text(short_ts(&e.timestamp))
            .size(10)
            .color(p.fg_3)
            .width(Length::Fixed(80.0)),
        text(short_target(&e.target))
            .size(10)
            .color(p.fg_2)
            .width(Length::Fixed(160.0)),
        // Truncate message visually to a single line; the expanded
        // detail shows the full text. iced's text widget wraps by
        // default - capping height works around that.
        text(message_preview(&e.message))
            .size(11)
            .color(p.fg_0)
            .width(Length::Fill),
        text(chevron_glyph).size(11).color(p.fg_3),
    ]
    .spacing(10)
    .padding([4, 10])
    .align_y(Alignment::Center);

    // Tap target spans the whole header row.
    let header_btn = iced::widget::button(header_inner)
        .padding(0)
        .width(Length::Fill)
        .style(move |_t, _s| iced::widget::button::Style {
            background: Some(iced::Background::Color(bg)),
            text_color: p.fg_0,
            border: iced::Border {
                color: p.line_soft,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        })
        .on_press(Message::ToggleExpanded(idx));

    if !expanded {
        return header_btn.into();
    }

    // Detail panel: full message + extras.
    // Left-pad past the level/ts/target columns so the detail block
    // visually nests under the message text.
    let detail_padding = iced::Padding {
        top: 8.0,
        right: 14.0,
        bottom: 10.0,
        left: 138.0,
    };
    let mut detail_col = column![].spacing(4).padding(detail_padding);
    detail_col = detail_col.push(text(e.message.clone()).size(11).color(p.fg_0));
    if !e.extras.is_empty() {
        detail_col = detail_col.push(iced::widget::Space::new().height(Length::Fixed(4.0)));
        // Size the key column to the widest key so long names like
        // `wrappers_skipped_aot` get full breathing room. Cap at a
        // reasonable max so an outlier key doesn't push values
        // off-screen on small windows.
        let widest_key_chars = e.extras.keys().map(|k| k.len()).max().unwrap_or(0);
        let key_col_width = (widest_key_chars as f32 * 6.5 + 16.0).clamp(96.0, 220.0);
        for (k, v) in &e.extras {
            detail_col = detail_col.push(
                row![
                    text(format!("{k}:"))
                        .size(10)
                        .color(p.fg_2)
                        .width(Length::Fixed(key_col_width)),
                    text(v.clone()).size(10).color(p.fg_1).width(Length::Fill),
                ]
                .spacing(12),
            );
        }
    }
    let detail =
        container(detail_col)
            .width(Length::Fill)
            .style(move |_t| iced::widget::container::Style {
                background: Some(iced::Background::Color(bg)),
                text_color: Some(p.fg_0),
                border: iced::Border {
                    color: p.line_soft,
                    width: 1.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            });

    column![header_btn, detail].into()
}

/// Shorten a log message for the collapsed row. Cuts at the first
/// newline (so multi-line messages don't blow up the row height) and
/// caps at 240 chars so very long single-line messages stay readable.
fn message_preview(msg: &str) -> String {
    let first_line = msg.split_once('\n').map(|(a, _)| a).unwrap_or(msg);
    if first_line.chars().count() > 240 {
        let truncated: String = first_line.chars().take(237).collect();
        format!("{truncated}…")
    } else {
        first_line.to_string()
    }
}

/// Trim the timestamp to `HH:MM:SS` so the row fits - full date is in
/// the file path. Falls back to the first 8 chars if the format
/// doesn't look like RFC3339.
fn short_ts(ts: &str) -> String {
    // Look for `T` and extract `HH:MM:SS`.
    if let Some(t_idx) = ts.find('T') {
        let after = &ts[t_idx + 1..];
        if after.len() >= 8 {
            return after[..8].to_string();
        }
    }
    ts.chars().take(8).collect()
}

/// Crop the target to its last `::` segment so space isn't wasted on
/// `crabby_install::install` when `install` says enough.
fn short_target(target: &str) -> String {
    target
        .rsplit_once("::")
        .map(|(_, last)| last.to_string())
        .unwrap_or_else(|| target.to_string())
}

fn with_alpha(c: iced::Color, a: f32) -> iced::Color {
    iced::Color { a, ..c }
}

/// Resolve the active Godot log path. Godot writes to
/// `<user-data>/Road to Vostok/logs/godot.log` (the file rotates per
/// session, with archived siblings named `godot<timestamp>.log`).
/// Reuses the user-data resolver from the MCM module since that
/// already targets the right per-platform directory.
fn godot_log_path() -> Option<std::path::PathBuf> {
    crabby_config::mcm::user_data_dir().map(|p| p.join("logs").join("godot.log"))
}

/// Bundle of analyzer-derived data the Logs tab consumes when its
/// active source is `Source::Analyzer`. Bundling keeps the
/// `refresh`/`update` signatures stable as we add more derived views.
#[derive(Debug, Default, Clone, Copy)]
pub struct AnalyzerView<'a> {
    /// Per-mod findings (hooks, registry writes, classic patterns).
    pub intents: &'a [crabby_mod_analyzer::ModIntent],
    /// Cross-mod conflicts derived from intents (registry collisions,
    /// replace-hook collisions, duplicate vanilla swaps, surfaced
    /// self-patterns).
    pub conflicts: &'a [crabby_mod_analyzer::Conflict],
}

/// Synthesize log entries from the analyzer's per-mod findings AND
/// cross-mod conflicts. One entry per finding/conflict so the user
/// can scan / filter / search just like a real log. Conflicts emit
/// first so the worst stuff floats to the top of the unsorted list.
///
/// Mapping per finding:
/// - **target** = mod id (or `crabby` for cross-mod conflicts)
/// - **level** = INFO for hooks/registry writes, WARN/ERROR for
///   classic patterns + conflicts based on severity
/// - **message** = short headline
/// - **extras** = file/line/verdict so the expander has detail
fn synthesize_analyzer_entries(view: AnalyzerView<'_>) -> Vec<LogEntry> {
    use crabby_mod_analyzer::{ConflictKind, Severity};
    let mut out: Vec<LogEntry> = Vec::new();

    // Cross-mod conflicts first. SelfPattern conflicts are
    // already represented in the per-mod ClassicPattern entries
    // below, so skip them here to avoid double-counting.
    for c in view.conflicts {
        if matches!(c.kind, ConflictKind::SelfPattern { .. }) {
            continue;
        }
        let (level, kind_label) = match &c.kind {
            ConflictKind::RegistryCollision { .. } => ("WARN", "registry collision"),
            ConflictKind::ReplaceHookCollision { .. } => ("WARN", "replace hook collision"),
            ConflictKind::DuplicateVanillaSwap { .. } => ("ERROR", "duplicate vanilla swap"),
            ConflictKind::SelfPattern { .. } => unreachable!("filtered above"),
        };
        let mut extras = std::collections::BTreeMap::new();
        extras.insert("kind".into(), kind_label.into());
        extras.insert(
            "participants".into(),
            c.participants
                .iter()
                .map(|p| p.mod_id.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        );
        for (i, p) in c.participants.iter().enumerate() {
            extras.insert(format!("p{i}_callsite"), p.callsite.clone());
            if !p.detail.is_empty() {
                extras.insert(format!("p{i}_detail"), p.detail.clone());
            }
        }
        out.push(LogEntry {
            raw: c.headline.clone(),
            level: level.into(),
            timestamp: String::new(),
            // `crabby` as the synthetic target signals "this is a
            // cross-mod finding the analyzer noticed, not something
            // any single mod logged."
            target: "crabby".into(),
            message: c.headline.clone(),
            extras,
        });
    }

    for intent in view.intents {
        for h in &intent.hooks {
            let mut extras = std::collections::BTreeMap::new();
            extras.insert("file".into(), h.filename.clone());
            extras.insert("line".into(), h.line.to_string());
            extras.insert("kind".into(), format!("{:?}", h.kind));
            extras.insert("callable".into(), h.callable_text.clone());
            extras.insert("resolvability".into(), format!("{:?}", h.resolvability));
            let msg = format!(
                "hook {} -> {}",
                h.hook_name.as_deref().unwrap_or("<dynamic>"),
                h.callable_text,
            );
            out.push(LogEntry {
                raw: msg.clone(),
                level: "INFO".into(),
                timestamp: String::new(),
                target: intent.mod_id.clone(),
                message: msg,
                extras,
            });
        }
        for w in &intent.registry_writes {
            let mut extras = std::collections::BTreeMap::new();
            extras.insert("file".into(), w.filename.clone());
            extras.insert("line".into(), w.line.to_string());
            extras.insert("verb".into(), format!("{:?}", w.verb));
            if !w.payload_text.is_empty() {
                extras.insert("payload".into(), w.payload_text.clone());
            }
            extras.insert("resolvability".into(), format!("{:?}", w.resolvability));
            let msg = format!(
                "{:?} {}/{}",
                w.verb,
                w.registry.as_deref().unwrap_or("<dynamic>"),
                w.key.as_deref().unwrap_or("<dynamic>"),
            );
            out.push(LogEntry {
                raw: msg.clone(),
                level: "INFO".into(),
                timestamp: String::new(),
                target: intent.mod_id.clone(),
                message: msg,
                extras,
            });
        }
        for c in &intent.classic_patterns {
            let level = match c.severity {
                Severity::Hard => "ERROR",
                Severity::Warn => "WARN",
                Severity::Info => "INFO",
            };
            let mut extras = std::collections::BTreeMap::new();
            extras.insert("file".into(), c.filename.clone());
            extras.insert("line".into(), c.line.to_string());
            extras.insert("pattern".into(), format!("{:?}", c.pattern));
            extras.insert("severity".into(), format!("{:?}", c.severity));
            if let Some(t) = &c.target {
                extras.insert("target".into(), t.clone());
            }
            if !c.verdict.is_empty() {
                extras.insert("verdict".into(), c.verdict.clone());
            }
            let kind_label = match c.pattern {
                crabby_mod_analyzer::ClassicPatternKind::TakeOverPath => "take_over_path",
                crabby_mod_analyzer::ClassicPatternKind::SetScript => "set_script",
                crabby_mod_analyzer::ClassicPatternKind::LoadResourcePack => "load_resource_pack",
                crabby_mod_analyzer::ClassicPatternKind::ExtendsVanilla => "extends vanilla",
                crabby_mod_analyzer::ClassicPatternKind::PreloadVanillaScript => "preload vanilla",
            };
            let msg = format!("{kind_label} on {}", c.target.as_deref().unwrap_or("?"),);
            out.push(LogEntry {
                raw: msg.clone(),
                level: level.into(),
                timestamp: String::new(),
                target: intent.mod_id.clone(),
                message: msg,
                extras,
            });
        }
    }
    out
}

/// Parse Godot's stdout log. Plain text with `[Tag]` prefixes for mod
/// chatter and `WARNING:` / `ERROR:` / `SCRIPT ERROR:` prefixes for
/// engine messages. We infer level heuristically; everything else
/// shows as info-level. Multi-line entries (e.g. Godot's `at:`
/// continuation lines) get folded into the previous entry's message.
fn parse_godot_log(text: &str) -> Vec<LogEntry> {
    let mut out: Vec<LogEntry> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }
        // `at: ...` lines are continuations of the previous Godot
        // error/warning. Append to the last entry rather than emit a
        // new row so the chevron expand shows the full context.
        if trimmed.starts_with("at:") || trimmed.starts_with("   at:") {
            if let Some(last) = out.last_mut() {
                last.message.push('\n');
                last.message.push_str(line.trim_end());
                continue;
            }
        }
        let upper = trimmed.to_ascii_uppercase();
        let level = if upper.starts_with("ERROR:") || upper.starts_with("SCRIPT ERROR:") {
            "ERROR"
        } else if upper.starts_with("WARNING:") || upper.starts_with("USER WARNING:") {
            "WARN"
        } else {
            "INFO"
        };
        // Strip the level prefix so the message reads cleanly.
        let message = match level {
            "ERROR" => strip_prefix_ci(trimmed, &["SCRIPT ERROR:", "ERROR:"]),
            "WARN" => strip_prefix_ci(trimmed, &["USER WARNING:", "WARNING:"]),
            _ => trimmed.to_string(),
        };
        // Pull the `[Tag]` prefix out into `target` if present; lets
        // users filter by mod name via the search box later.
        let (target, message) = if let Some(rest) = message.strip_prefix('[') {
            if let Some(close) = rest.find(']') {
                let tag = &rest[..close];
                let body = rest[close + 1..].trim_start();
                (tag.to_string(), body.to_string())
            } else {
                (String::new(), message)
            }
        } else {
            (String::new(), message)
        };
        out.push(LogEntry {
            raw: line.to_string(),
            level: level.to_string(),
            timestamp: String::new(),
            target,
            message,
            extras: std::collections::BTreeMap::new(),
        });
    }
    out
}

/// Case-insensitive prefix strip. Returns the remainder after the
/// first matching prefix (whitespace-trimmed), or the input as-is.
fn strip_prefix_ci(s: &str, prefixes: &[&str]) -> String {
    let upper = s.to_ascii_uppercase();
    for prefix in prefixes {
        if upper.starts_with(prefix) {
            return s[prefix.len()..].trim_start().to_string();
        }
    }
    s.to_string()
}

/// Render a JSON value as a one-line string for the extras map.
/// Strings drop their quotes; numbers/bools render as themselves;
/// objects/arrays fall back to compact JSON.
fn stringify_json_value(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => "null".into(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => s.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// Parse a JSON-lines log file. Tolerates bad lines (returns them as
/// raw entries with level=`""`). `tracing_subscriber`'s json layer
/// emits one object per line with `timestamp` / `level` / `target` /
/// `fields.message` keys - lifted out by string fishing rather than
/// depending on serde_json types so unexpected schema changes don't
/// break parsing.
fn parse_log(text: &str) -> Vec<LogEntry> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<serde_json::Value>(line) {
            Ok(v) => {
                let level = v
                    .get("level")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                let timestamp = v
                    .get("timestamp")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                let target = v
                    .get("target")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                let message = v
                    .get("fields")
                    .and_then(|f| f.get("message"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_string();
                // Walk `fields.*` (everything except `message`) plus
                // any top-level keys not already claimed, and
                // stringify them. Numbers / bools / strings serialize
                // sensibly; objects keep their JSON form so unusual
                // structured payloads still surface, not vanish.
                let mut extras = std::collections::BTreeMap::new();
                if let Some(serde_json::Value::Object(map)) = v.get("fields") {
                    for (k, val) in map {
                        if k == "message" {
                            continue;
                        }
                        extras.insert(k.clone(), stringify_json_value(val));
                    }
                }
                if let serde_json::Value::Object(map) = &v {
                    for (k, val) in map {
                        if matches!(k.as_str(), "level" | "timestamp" | "target" | "fields") {
                            continue;
                        }
                        // Top-level extras are rare but harmless to keep.
                        extras
                            .entry(k.clone())
                            .or_insert_with(|| stringify_json_value(val));
                    }
                }
                out.push(LogEntry {
                    raw: line.to_string(),
                    level,
                    timestamp,
                    target,
                    message,
                    extras,
                });
            }
            Err(_) => out.push(LogEntry {
                raw: line.to_string(),
                level: String::new(),
                timestamp: String::new(),
                target: String::new(),
                message: line.to_string(),
                extras: std::collections::BTreeMap::new(),
            }),
        }
    }
    out
}

// `Path` import is kept for forward-compat (per-day file selection
// may land here). Suppress the unused warning until then.
#[allow(dead_code)]
fn _path_marker(_p: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tracing_json_lines() {
        let text = r#"{"timestamp":"2026-05-01T10:42:13.123Z","level":"INFO","fields":{"message":"hello"},"target":"crabby_install::install"}
{"timestamp":"2026-05-01T10:42:14.000Z","level":"WARN","fields":{"message":"oops"},"target":"crabby_ui::tabs::mods"}"#;
        let entries = parse_log(text);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].level, "INFO");
        assert_eq!(entries[0].message, "hello");
        assert_eq!(short_ts(&entries[0].timestamp), "10:42:13");
        assert_eq!(short_target(&entries[1].target), "mods");
    }

    #[test]
    fn falls_back_to_raw_for_bad_lines() {
        let text =
            "not json\n{\"level\":\"ERROR\",\"fields\":{\"message\":\"crash\"},\"target\":\"x\"}\n";
        let entries = parse_log(text);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].level, "");
        assert_eq!(entries[0].message, "not json");
        assert_eq!(entries[1].level, "ERROR");
    }

    #[test]
    fn godot_parser_infers_levels_and_extracts_tag() {
        let text = "[CrabbyShim] mod loaded: GunsmithsBunker v1.1.0\n\
                    WARNING: invalid UID for resource\n\
                       at: load (some/path.cpp:503)\n\
                    SCRIPT ERROR: Parse Error: ...\n\
                    [GlobalEconomy] hooks registered";
        let entries = parse_godot_log(text);
        // 4 entries - the `at:` line folds into the WARNING.
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].level, "INFO");
        assert_eq!(entries[0].target, "CrabbyShim");
        assert_eq!(entries[1].level, "WARN");
        assert!(entries[1].message.contains("invalid UID"));
        assert!(entries[1].message.contains("at: load"));
        assert_eq!(entries[2].level, "ERROR");
        assert_eq!(entries[3].target, "GlobalEconomy");
    }

    #[test]
    fn captures_extra_fields() {
        // Manifest-discovery warns shape: archive=path, error=string.
        let line = r#"{"timestamp":"2026-05-01T10:42:13.123Z","level":"WARN","fields":{"archive":"/games/x.vmz","error":"mod.txt missing","message":"skipping archive: mod.txt missing or unparseable"},"target":"crabby_config"}"#;
        let entries = parse_log(line);
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].extras.get("archive").map(String::as_str),
            Some("/games/x.vmz")
        );
        assert_eq!(
            entries[0].extras.get("error").map(String::as_str),
            Some("mod.txt missing")
        );
        // Make sure the message itself didn't leak into extras.
        assert!(!entries[0].extras.contains_key("message"));
    }
}
