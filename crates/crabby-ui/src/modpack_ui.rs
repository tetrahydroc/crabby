//! Modpack export/import UI state + messages + the import-preview
//! overlay.
//!
//! Lives outside `tabs/` because the surface isn't a tab - it's a
//! profile-bar action plus a modal overlay that sits above the rest
//! of the UI when an import is in progress.

use std::path::PathBuf;

use iced::widget::{button, column, container, row, scrollable, text, text_input};
use iced::{Alignment, Element, Length};

use crabby_modpack::Manifest;

use crate::style::{ButtonKind, SurfaceKind, button_style, surface_style};
use crate::theme::Palette;

/// Current modpack workflow state. The App stores one of these and
/// drives the UI off it.
#[derive(Debug, Default)]
pub enum ModpackState {
    /// Nothing in progress.
    #[default]
    Idle,
    /// Just exported - surface the encoded pack code (or status text
    /// for save-to-file) plus a copied marker so the Copy button can
    /// flip to "Copied!" briefly.
    ExportToast {
        /// The code (or status string) to show.
        text: String,
        /// True after a successful clipboard write - UI shows "Copied!"
        /// in place of the Copy button until the toast is dismissed.
        copied: bool,
    },
    /// Import requested - modal showing the paste box (waiting for
    /// input or showing a decode error).
    ImportPaste {
        /// Current contents of the paste textbox.
        input: String,
        /// Decode error from the last submit attempt, if any.
        error: Option<String>,
    },
    /// Pack decoded successfully - show the preview screen with the
    /// target picker + the two destructive prompts.
    ImportPreview {
        /// Decoded manifest.
        manifest: Manifest,
        /// Typed name when target = NewProfile. Defaults to
        /// `manifest.name`.
        new_profile_name: String,
        /// Active selection: new profile or merge into existing.
        target: ImportTarget,
        /// "Deactivate currently-active mods in target?" prompt
        /// (only relevant for ExistingProfile target).
        deactivate_existing: bool,
        /// "Overwrite MCM configs that already exist?" prompt.
        overwrite_mcm: bool,
    },
    /// Import running - async work in progress. Per-mod outcomes
    /// accumulate as messages land.
    ImportRunning {
        /// Pack being imported (carried for display + completion).
        manifest: Manifest,
        /// Profile being installed into.
        target_profile: String,
        /// Per-mod status, in pack order.
        statuses: Vec<ModImportStatus>,
    },
    /// Import finished - show the summary screen.
    ImportDone {
        /// Outcomes per mod, for the summary table.
        statuses: Vec<ModImportStatus>,
        /// Profile installed into.
        target_profile: String,
    },
}

/// Selected import target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportTarget {
    /// Create a fresh profile (name on `ImportPreview`).
    NewProfile,
    /// Merge into an existing profile (named here).
    ExistingProfile(String),
}

/// Per-mod resolution + final outcome. Passes through the import
/// pipeline gathering state.
#[derive(Debug, Clone)]
pub struct ModImportStatus {
    /// Local mod id (the key the install/activate flow keys on).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Pack-author's version.
    pub pack_version: String,
    /// Pack's MW id, if any.
    pub mw_id: Option<u64>,
    /// Resolution decision the preview computed.
    pub resolution: ModResolution,
    /// Final outcome after the import ran. None = not yet processed.
    pub outcome: Option<ModOutcome>,
}

/// What the import flow plans to do for a given mod, computed at
/// preview time from the recipient's current state.
#[derive(Debug, Clone)]
pub enum ModResolution {
    /// Mod present locally at the same version - just activate.
    Activate,
    /// Mod present locally at a different version - keep installed,
    /// activate, warn in summary.
    KeepInstalledVersion {
        /// Version the recipient has locally.
        local_version: String,
    },
    /// Mod not installed; install from MW first, then activate.
    InstallFromMw {
        /// MW numeric id to fetch.
        mw_id: u64,
    },
    /// Mod isn't on MW (no `mw_id`), and the recipient doesn't have
    /// it locally. Skipped - the preview surfaces the name so the
    /// recipient can install manually.
    SkippedNoSource,
}

/// Final outcome per mod.
#[derive(Debug, Clone)]
pub enum ModOutcome {
    /// Activated successfully (whether or not install was needed).
    Ok,
    /// Failed at some step, with a human-readable reason.
    Failed(String),
    /// Skipped per [`ModResolution::SkippedNoSource`].
    Skipped(String),
}

/// Per-tab message - emitted by buttons in the profile bar + the
/// import overlay. App routes to the modpack handler.
#[derive(Debug, Clone)]
pub enum Message {
    /// "Export pack" clicked in the profile bar.
    ExportClicked,
    /// "Import pack" clicked in the profile bar.
    ImportClicked,
    /// Dismiss whatever modpack surface is currently showing.
    Dismiss,
    /// Paste box edited.
    PasteInputChanged(String),
    /// Submit the paste box - decode the pack.
    PasteSubmit,
    /// New Profile target chosen.
    SelectNewProfileTarget,
    /// Existing Profile target chosen.
    SelectExistingProfileTarget(String),
    /// New-profile-name input edited.
    NewProfileNameChanged(String),
    /// Toggle "deactivate existing".
    ToggleDeactivateExisting,
    /// Toggle "overwrite MCM".
    ToggleOverwriteMcm,
    /// "Import" clicked on the preview screen - kick the async
    /// pipeline.
    ConfirmImport,
    /// One mod's import finished. Carries the index in the statuses
    /// vec + the outcome.
    ImportProgress {
        /// Index in the statuses vec.
        index: usize,
        /// Outcome from the worker.
        outcome: ModOutcome,
    },
    /// All mods done.
    ImportComplete,
    /// Async export-to-file picker resolved. `Some(path)` = picked,
    /// `None` = cancelled.
    ExportFilePicked(Option<PathBuf>),
    /// Async import-from-file picker resolved.
    ImportFilePicked(Option<PathBuf>),
    /// "Save to file" clicked on the export toast.
    ExportSaveToFile,
    /// "Open file picker" clicked on the import paste screen.
    ImportFromFile,
    /// "Copy code" clicked on the export toast - App handles by firing
    /// `iced::clipboard::write` with the carried string.
    CopyToClipboard(String),
    /// Internal: clipboard write succeeded; flip the export toast's
    /// state to show "Copied!" briefly.
    ClipboardCopied,
}

/// Build the modpack overlay. Returns a `Length::Fill` element to be
/// stacked on top of the main UI. Returns an empty space when state
/// is `Idle` so the stack lets clicks through.
#[must_use]
pub fn overlay<'a>(
    state: &'a ModpackState,
    profile_names: &'a [String],
    palette: &Palette,
) -> Element<'a, crate::app::Message> {
    let p = *palette;
    match state {
        ModpackState::Idle => empty(),
        ModpackState::ExportToast { text, copied } => modal(p, export_toast_view(text, *copied, p)),
        ModpackState::ImportPaste { input, error } => {
            modal(p, import_paste_view(input, error.as_deref(), p))
        }
        ModpackState::ImportPreview {
            manifest,
            new_profile_name,
            target,
            deactivate_existing,
            overwrite_mcm,
        } => modal(
            p,
            import_preview_view(
                manifest,
                new_profile_name,
                target,
                *deactivate_existing,
                *overwrite_mcm,
                profile_names,
                p,
            ),
        ),
        ModpackState::ImportRunning {
            statuses,
            target_profile,
            ..
        } => modal(p, import_progress_view(statuses, target_profile, p)),
        ModpackState::ImportDone {
            statuses,
            target_profile,
        } => modal(p, import_done_view(statuses, target_profile, p)),
    }
}

fn empty<'a>() -> Element<'a, crate::app::Message> {
    iced::widget::Space::new()
        .width(Length::Fixed(0.0))
        .height(Length::Fixed(0.0))
        .into()
}

/// Wrap `body` in a centered modal panel with a dim backdrop.
fn modal<'a>(
    p: Palette,
    body: Element<'a, crate::app::Message>,
) -> Element<'a, crate::app::Message> {
    // Backdrop = full-fill semi-opaque container that swallows clicks
    // outside the panel. Clicking it dismisses.
    let panel = container(body)
        .style(move |_t| iced::widget::container::Style {
            background: Some(iced::Background::Color(p.bg_1)),
            text_color: Some(p.fg_0),
            border: iced::Border {
                color: p.line,
                width: 1.0,
                radius: 10.0.into(),
            },
            ..Default::default()
        })
        .padding(0)
        .max_width(560.0);

    let backdrop_color = iced::Color { a: 0.55, ..p.bg_0 };
    container(panel)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .style(move |_t| iced::widget::container::Style {
            background: Some(iced::Background::Color(backdrop_color)),
            ..Default::default()
        })
        .into()
}

fn modal_header<'a>(title: String, p: Palette) -> Element<'a, crate::app::Message> {
    row![
        text(title).size(14).color(p.fg_0),
        crate::style::hspace(),
        button(text("×").size(14).color(p.fg_2))
            .padding(iced::Padding {
                top: 0.0,
                right: 8.0,
                bottom: 0.0,
                left: 8.0
            })
            .style(button_style(p, ButtonKind::Ghost))
            .on_press(crate::app::Message::Modpack(Message::Dismiss)),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .padding([12, 16])
    .into()
}

fn export_toast_view<'a>(
    code: &'a str,
    copied: bool,
    p: Palette,
) -> Element<'a, crate::app::Message> {
    let header = modal_header("Export pack".into(), p);

    // Read-only-feeling text_input that allows click in, select all
    // (Ctrl+A), copy (Ctrl+C). The value is rebound on every input
    // event - typed characters land on the text but the next render
    // overwrites them with the canonical code, so the field reads as
    // "non-editable" without needing a real read-only widget (iced
    // 0.14 doesn't ship one with selectable text).
    let preview_input = text_input("", code)
        .on_input(|_| crate::app::Message::Modpack(Message::PasteInputChanged(String::new())))
        .padding([6, 10])
        .size(11)
        .width(Length::Fill);

    let copy_btn: Element<'a, crate::app::Message> = if copied {
        text("Copied!").size(11).color(p.ok).into()
    } else {
        button(text("Copy code").size(11))
            .padding([4, 12])
            .style(button_style(p, ButtonKind::Primary))
            .on_press(crate::app::Message::Modpack(Message::CopyToClipboard(
                code.to_string(),
            )))
            .into()
    };

    let body = column![
        text("Share this code as text, or save to a `.crabbypack` file for big packs.")
            .size(12)
            .color(p.fg_2),
        preview_input,
        row![
            crate::style::hspace(),
            copy_btn,
            button(text("Save to file").size(11))
                .padding([4, 12])
                .style(button_style(p, ButtonKind::Default))
                .on_press(crate::app::Message::Modpack(Message::ExportSaveToFile)),
            button(text("Done").size(11))
                .padding([4, 12])
                .style(button_style(p, ButtonKind::Default))
                .on_press(crate::app::Message::Modpack(Message::Dismiss)),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    ]
    .spacing(12)
    .padding(iced::Padding {
        top: 0.0,
        right: 16.0,
        bottom: 16.0,
        left: 16.0,
    });

    column![header, Element::<crate::app::Message>::from(body)]
        .spacing(0)
        .into()
}

fn import_paste_view<'a>(
    input: &'a str,
    error: Option<&'a str>,
    p: Palette,
) -> Element<'a, crate::app::Message> {
    let header = modal_header("Import pack".into(), p);
    let paste = text_input("Paste a crabby:pack:… code, or use a file…", input)
        .on_input(|s| crate::app::Message::Modpack(Message::PasteInputChanged(s)))
        .on_submit(crate::app::Message::Modpack(Message::PasteSubmit))
        .padding([6, 10])
        .size(12);

    let err: Element<'a, crate::app::Message> = if let Some(e) = error {
        text(e.to_string()).size(11).color(p.err).into()
    } else {
        empty()
    };

    let body = column![
        text("Paste the pack code, or open a `.crabbypack` file.")
            .size(12)
            .color(p.fg_2),
        paste,
        err,
        row![
            crate::style::hspace(),
            button(text("Open file…").size(11))
                .padding([4, 12])
                .style(button_style(p, ButtonKind::Default))
                .on_press(crate::app::Message::Modpack(Message::ImportFromFile)),
            button(text("Continue").size(11))
                .padding([4, 12])
                .style(button_style(p, ButtonKind::Primary))
                .on_press(crate::app::Message::Modpack(Message::PasteSubmit)),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    ]
    .spacing(10)
    .padding(iced::Padding {
        top: 0.0,
        right: 16.0,
        bottom: 16.0,
        left: 16.0,
    });

    column![header, Element::<crate::app::Message>::from(body)]
        .spacing(0)
        .into()
}

#[allow(clippy::too_many_arguments)]
fn import_preview_view<'a>(
    manifest: &'a Manifest,
    new_profile_name: &'a str,
    target: &'a ImportTarget,
    deactivate_existing: bool,
    overwrite_mcm: bool,
    profile_names: &'a [String],
    p: Palette,
) -> Element<'a, crate::app::Message> {
    let header = modal_header(format!("Import: {}", manifest.name), p);

    let mod_count = manifest.mods.len();
    let summary = text(format!("{} mod(s) in pack.", mod_count))
        .size(12)
        .color(p.fg_2);

    // Target picker.
    let new_radio = button(
        text(if matches!(target, ImportTarget::NewProfile) {
            "● New profile"
        } else {
            "○ New profile"
        })
        .size(11),
    )
    .padding([3, 10])
    .style(button_style(p, ButtonKind::Ghost))
    .on_press(crate::app::Message::Modpack(
        Message::SelectNewProfileTarget,
    ));
    let new_input = text_input("New profile name", new_profile_name)
        .on_input(|s| crate::app::Message::Modpack(Message::NewProfileNameChanged(s)))
        .padding([5, 10])
        .size(12)
        .width(Length::Fill);

    let mut existing_chips: Vec<Element<'a, crate::app::Message>> = vec![];
    for name in profile_names {
        let active = matches!(target, ImportTarget::ExistingProfile(n) if n == name);
        let label = if active {
            format!("● {name}")
        } else {
            format!("○ {name}")
        };
        existing_chips.push(
            button(text(label).size(11))
                .padding([3, 10])
                .style(button_style(p, ButtonKind::Ghost))
                .on_press(crate::app::Message::Modpack(
                    Message::SelectExistingProfileTarget(name.clone()),
                ))
                .into(),
        );
    }

    let target_section = column![
        text("TARGET").size(11).color(p.fg_2),
        row![new_radio, new_input]
            .spacing(8)
            .align_y(Alignment::Center),
        text("Or merge into existing:").size(11).color(p.fg_3),
        iced::widget::Row::with_children(existing_chips)
            .spacing(6)
            .wrap(),
    ]
    .spacing(6);

    // Two prompts shown only when relevant (existing profile target).
    let is_existing = matches!(target, ImportTarget::ExistingProfile(_));
    let prompts: Element<'a, crate::app::Message> = if is_existing {
        let deact = checkbox_btn(
            "Deactivate currently-active mods",
            deactivate_existing,
            crate::app::Message::Modpack(Message::ToggleDeactivateExisting),
            p,
        );
        let mcm = checkbox_btn(
            "Overwrite existing MCM configs",
            overwrite_mcm,
            crate::app::Message::Modpack(Message::ToggleOverwriteMcm),
            p,
        );
        column![text("ON IMPORT").size(11).color(p.fg_2), deact, mcm,]
            .spacing(6)
            .into()
    } else {
        // For new profiles: only the MCM prompt is interesting (still
        // applies if the same mod ids exist in the MCM dir from
        // another profile). Default = overwrite for "use my exact
        // setup" intent; override available.
        let mcm = checkbox_btn(
            "Overwrite existing MCM configs (if any)",
            overwrite_mcm,
            crate::app::Message::Modpack(Message::ToggleOverwriteMcm),
            p,
        );
        column![text("ON IMPORT").size(11).color(p.fg_2), mcm]
            .spacing(6)
            .into()
    };

    // Mods list.
    let rows: Vec<Element<'a, crate::app::Message>> = manifest
        .mods
        .iter()
        .map(|pm| {
            let mw_chip = if let Some(id) = pm.mw_id {
                text(format!("MW#{id}")).size(10).color(p.fg_3)
            } else {
                text("non-MW").size(10).color(p.warn)
            };
            container(
                row![
                    text(pm.name.clone())
                        .size(12)
                        .color(p.fg_0)
                        .width(Length::Fill),
                    text(pm.version.clone()).size(11).color(p.fg_2),
                    mw_chip,
                ]
                .spacing(10)
                .align_y(Alignment::Center)
                .padding([4, 10]),
            )
            .style(surface_style(p, SurfaceKind::Bg3))
            .width(Length::Fill)
            .into()
        })
        .collect();
    let mods_box = container(
        scrollable(
            iced::widget::Column::with_children(rows)
                .spacing(2)
                .width(Length::Fill),
        )
        .height(Length::Fixed(160.0)),
    )
    .style(surface_style(p, SurfaceKind::Bg2))
    .width(Length::Fill);

    let actions = row![
        crate::style::hspace(),
        button(text("Cancel").size(11))
            .padding([4, 12])
            .style(button_style(p, ButtonKind::Default))
            .on_press(crate::app::Message::Modpack(Message::Dismiss)),
        button(text("Import").size(11))
            .padding([4, 14])
            .style(button_style(p, ButtonKind::Primary))
            .on_press(crate::app::Message::Modpack(Message::ConfirmImport)),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let body = column![summary, target_section, prompts, mods_box, actions]
        .spacing(12)
        .padding(iced::Padding {
            top: 0.0,
            right: 16.0,
            bottom: 16.0,
            left: 16.0,
        });

    column![header, Element::<crate::app::Message>::from(body)]
        .spacing(0)
        .into()
}

fn import_progress_view<'a>(
    statuses: &'a [ModImportStatus],
    target_profile: &'a str,
    p: Palette,
) -> Element<'a, crate::app::Message> {
    let header = modal_header(format!("Importing into `{target_profile}`"), p);
    let total = statuses.len();
    let done = statuses.iter().filter(|s| s.outcome.is_some()).count();
    let summary = text(format!("{done}/{total} processed"))
        .size(12)
        .color(p.fg_2);
    let rows: Vec<Element<'a, crate::app::Message>> =
        statuses.iter().map(|s| status_row(s, p)).collect();
    let body = column![
        summary,
        container(
            scrollable(
                iced::widget::Column::with_children(rows)
                    .spacing(2)
                    .width(Length::Fill),
            )
            .height(Length::Fixed(220.0)),
        )
        .style(surface_style(p, SurfaceKind::Bg2))
        .width(Length::Fill),
    ]
    .spacing(12)
    .padding(iced::Padding {
        top: 0.0,
        right: 16.0,
        bottom: 16.0,
        left: 16.0,
    });
    column![header, Element::<crate::app::Message>::from(body)]
        .spacing(0)
        .into()
}

fn import_done_view<'a>(
    statuses: &'a [ModImportStatus],
    target_profile: &'a str,
    p: Palette,
) -> Element<'a, crate::app::Message> {
    let header = modal_header(format!("Imported into `{target_profile}`"), p);
    let ok = statuses
        .iter()
        .filter(|s| matches!(s.outcome, Some(ModOutcome::Ok)))
        .count();
    let failed = statuses
        .iter()
        .filter(|s| matches!(s.outcome, Some(ModOutcome::Failed(_))))
        .count();
    let skipped = statuses
        .iter()
        .filter(|s| matches!(s.outcome, Some(ModOutcome::Skipped(_))))
        .count();
    let summary = text(format!("{ok} ok • {failed} failed • {skipped} skipped"))
        .size(12)
        .color(p.fg_2);
    let rows: Vec<Element<'a, crate::app::Message>> =
        statuses.iter().map(|s| status_row(s, p)).collect();
    let body = column![
        summary,
        container(
            scrollable(
                iced::widget::Column::with_children(rows)
                    .spacing(2)
                    .width(Length::Fill),
            )
            .height(Length::Fixed(220.0)),
        )
        .style(surface_style(p, SurfaceKind::Bg2))
        .width(Length::Fill),
        row![
            crate::style::hspace(),
            button(text("Close").size(11))
                .padding([4, 12])
                .style(button_style(p, ButtonKind::Primary))
                .on_press(crate::app::Message::Modpack(Message::Dismiss)),
        ]
        .spacing(8),
    ]
    .spacing(12)
    .padding(iced::Padding {
        top: 0.0,
        right: 16.0,
        bottom: 16.0,
        left: 16.0,
    });
    column![header, Element::<crate::app::Message>::from(body)]
        .spacing(0)
        .into()
}

fn status_row<'a>(s: &'a ModImportStatus, p: Palette) -> Element<'a, crate::app::Message> {
    let (label, color) = match &s.outcome {
        Some(ModOutcome::Ok) => ("ok".to_string(), p.ok),
        Some(ModOutcome::Failed(e)) => (format!("failed: {e}"), p.err),
        Some(ModOutcome::Skipped(why)) => (format!("skipped: {why}"), p.warn),
        None => match &s.resolution {
            ModResolution::Activate => ("queued: activate".to_string(), p.fg_3),
            ModResolution::KeepInstalledVersion { local_version } => {
                (format!("queued: keep local v{local_version}"), p.warn)
            }
            ModResolution::InstallFromMw { .. } => ("queued: install".to_string(), p.fg_3),
            ModResolution::SkippedNoSource => ("skip: not on MW".to_string(), p.warn),
        },
    };
    container(
        row![
            text(s.name.clone())
                .size(12)
                .color(p.fg_0)
                .width(Length::Fill),
            text(label).size(11).color(color),
        ]
        .spacing(10)
        .padding([4, 10]),
    )
    .style(surface_style(p, SurfaceKind::Bg3))
    .width(Length::Fill)
    .into()
}

fn checkbox_btn<'a>(
    label: &'a str,
    on: bool,
    msg: crate::app::Message,
    p: Palette,
) -> Element<'a, crate::app::Message> {
    let glyph = if on { "[x]" } else { "[ ]" };
    button(
        row![
            text(glyph).size(11).color(p.fg_1),
            text(label).size(11).color(p.fg_0),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .padding([3, 8])
    .style(button_style(p, ButtonKind::Ghost))
    .on_press(msg)
    .into()
}
