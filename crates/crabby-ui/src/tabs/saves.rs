//! Save-slot + snapshot management - profile-scoped.
//!
//! Slots live under `<user>/saves/<profile>/<slot>/`. The default
//! view shows only slots belonging to the active mod profile, since
//! a save built against profile X is meaningful only when profile X
//! is loaded. A "Show all profiles" toggle reveals foreign slots
//! tagged with their owning profile, for power users who know what
//! they're doing.
//!
//! Restoring across profiles is intentionally not supported in v1 -
//! restores happen in-place into the snapshot's owning slot.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use iced::widget::{button, column, container, row, scrollable, text, text_input, tooltip};
use iced::{Alignment, Element, Length, Task};

use crabby_config::saves::{self, SavesError, SlotInfo, SnapshotInfo};

use crate::style::{ButtonKind, SurfaceKind, button_style, surface_style};
use crate::theme::Palette;

/// Per-tab message.
#[derive(Debug, Clone)]
pub enum Message {
    /// Re-scan slots + snapshots from disk.
    Refresh,
    /// Toggle the "Show all profiles" filter.
    ToggleShowAllProfiles,
    /// New-slot input edited.
    NewSlotInputChanged(String),
    /// Create a slot under the active mod profile.
    CreateSlot,
    /// Switch clicked on a slot row. Writes `active_slot.txt` with
    /// this slot's `(profile, slot)`.
    SwitchSlot {
        /// Owning profile of the row.
        profile: String,
        /// Slot name within that profile.
        slot: String,
    },
    /// Snapshot clicked on a slot row.
    SnapshotSlot {
        /// Owning profile of the row.
        profile: String,
        /// Slot name within that profile.
        slot: String,
    },
    /// Delete clicked on a slot row.
    DeleteSlot {
        /// Owning profile of the row.
        profile: String,
        /// Slot name within that profile.
        slot: String,
    },
    /// Destination profile picked in a slot row's Move pick_list.
    /// When `confirm_destructive_actions` is on this opens an inline
    /// confirm strip; off, it dispatches `MoveSlotToProfile` directly.
    RequestMoveSlotToProfile {
        /// Owning profile of the row.
        src_profile: String,
        /// Slot name within that profile.
        src_slot: String,
        /// Destination profile picked from the row's pick_list.
        dst_profile: String,
    },
    /// Clear any transient action state for `(profile, slot)`. Used
    /// by the cancel-confirm `[✗]` button on the move flow AND by
    /// the timed dispatch that auto-clears the snapshot success
    /// indicator after a couple of seconds.
    ClearTransientAction {
        /// Owning profile of the row.
        profile: String,
        /// Slot name within that profile.
        slot: String,
    },
    /// Confirmed the move (either via the inline strip's Confirm
    /// button or directly from `RequestMoveSlotToProfile` when
    /// confirmations are off). Performs the move; collisions surface
    /// via the existing error banner.
    MoveSlotToProfile {
        /// Owning profile of the row.
        src_profile: String,
        /// Slot name within that profile.
        src_slot: String,
        /// Destination profile picked from the row's pick_list.
        dst_profile: String,
    },
    /// Slot selected to expand its snapshot list. `Some((profile, slot))`.
    SelectSlot(Option<(String, String)>),
    /// Per-slot snapshot label input edited.
    SnapshotLabelChanged(String),
    /// Restore clicked on a snapshot. Restore stays within the
    /// snapshot's owning (profile, slot) - both come from the row.
    RestoreSnapshot {
        /// Owning profile.
        profile: String,
        /// Owning slot.
        slot: String,
        /// Absolute path to the snapshot zip.
        path: PathBuf,
    },
    /// Delete clicked on a snapshot zip.
    DeleteSnapshot(PathBuf),
    /// Destination profile picked in the vanilla-import form.
    VanillaImportProfileChanged(String),
    /// Vanilla-import target slot input edited.
    VanillaImportSlotChanged(String),
    /// "Import vanilla save" button clicked. App.rs holds the actual
    /// VanillaSaveSet, so this is a signal to perform the import via
    /// crabby_config::saves::import_vanilla_to_slot.
    ImportVanilla {
        /// Destination profile name from the form.
        profile: String,
        /// Destination slot name from the form (created if missing).
        slot: String,
    },
}

/// Per-tab state.
#[derive(Debug, Default)]
pub struct State {
    /// Bumped via [`invalidate`] to force re-fetch on next view.
    pub generation: u64,
    /// Discovered slots - either active-profile-only or all profiles
    /// depending on `show_all_profiles`.
    slots: Vec<SlotInfo>,
    /// Snapshots for the currently-selected slot.
    selected_snapshots: Vec<SnapshotInfo>,
    /// `(profile, slot)` the user is inspecting; `None` = list view only.
    selected_slot: Option<(String, String)>,
    /// New-slot input buffer.
    new_slot_input: String,
    /// Optional snapshot label override.
    snapshot_label: String,
    /// Show every profile's slots, not just the active one.
    pub show_all_profiles: bool,
    /// Last error from a slot/snapshot mutation.
    error: Option<String>,
    /// Generation that produced the cached `slots` vec.
    cache_gen: Option<u64>,
    /// Vanilla-import form: target profile (defaults to active when
    /// None at render time).
    vanilla_import_profile: Option<String>,
    /// Vanilla-import form: target slot name. Defaults to "default"
    /// at render time when empty.
    vanilla_import_slot: String,
    /// Per-row transient action state. Drives in-place button mutation
    /// for actions that need either a confirm step (move) or async
    /// feedback (snapshot). Keyed by `(profile, slot)`. Cleared by the
    /// owning message handler when the action completes (or by a
    /// timeout for the success/failure feedback states).
    transient_actions: std::collections::BTreeMap<(String, String), TransientAction>,
}

/// Per-row transient state covering the destructive-action confirm
/// flow + the snapshot async feedback flow. Lives in the saves tab's
/// `transient_actions` map keyed by `(profile, slot)`.
#[derive(Debug, Clone)]
pub enum TransientAction {
    /// Move-to-profile awaiting confirm. The pick_list cell is replaced
    /// by `[✗] [Confirm]` with the destination shown in the Confirm
    /// button's tooltip. Triggered by `RequestMoveSlotToProfile` when
    /// the launcher's confirm-destructive setting is on.
    PendingMove {
        /// Destination profile picked from the row's pick_list.
        dst_profile: String,
    },
    /// Snapshot operation in flight. Snapshot button shows `[Saving…]`
    /// disabled. Set on dispatch; cleared by `SnapshotSlot` finishing
    /// (which transitions to `SnapshotJustSucceeded` or
    /// `SnapshotJustFailed`).
    SnapshotInFlight,
    /// Snapshot just landed successfully. Button shows `[Snapshotted ✓]`
    /// for ~2.5s, then auto-clears via `ClearTransientAction`.
    SnapshotJustSucceeded,
    /// Snapshot just failed. Button shows `[Failed ✗]`. Persists until
    /// any other action is taken (no auto-clear) - the error banner
    /// already carries the detailed message.
    SnapshotJustFailed,
}

impl State {
    /// Drop cached state.
    pub fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.cache_gen = None;
    }

    /// Re-read slots + (if a slot is selected) its snapshots. The
    /// active mod profile name is passed in by the App, avoiding a
    /// round-trip through `mod_config.cfg`.
    pub fn refresh(&mut self, active_profile: &str) {
        if self.cache_gen == Some(self.generation) {
            return;
        }
        let result = if self.show_all_profiles {
            saves::list_all_slots()
        } else {
            saves::list_slots(active_profile)
        };
        match result {
            Ok(slots) => {
                self.slots = slots;
                self.error = None;
            }
            Err(e) => {
                tracing::warn!(error = %e, "saves: list failed");
                self.error = Some(format!("{e}"));
                self.slots = Vec::new();
            }
        }
        if let Some((p, s)) = self.selected_slot.clone() {
            if !self.slots.iter().any(|x| x.profile == p && x.name == s) {
                self.selected_slot = None;
                self.selected_snapshots.clear();
            } else {
                self.refresh_selected_snapshots();
            }
        }
        self.cache_gen = Some(self.generation);
    }

    fn refresh_selected_snapshots(&mut self) {
        let Some((profile, slot)) = self.selected_slot.as_ref() else {
            self.selected_snapshots.clear();
            return;
        };
        match saves::list_snapshots(profile, slot) {
            Ok(snaps) => self.selected_snapshots = snaps,
            Err(e) => {
                tracing::warn!(error = %e, profile = %profile, slot = %slot, "saves: list_snapshots failed");
                self.selected_snapshots = Vec::new();
            }
        }
    }

    /// Apply a message. `active_profile` is the launcher's view of the
    /// active mod profile (= the directory under `saves/` we operate
    /// inside for new-slot creation and the default-view filter).
    pub fn update(
        &mut self,
        msg: Message,
        active_profile: &str,
        confirm_destructive_actions: bool,
    ) -> Task<Message> {
        match msg {
            Message::Refresh => {
                self.invalidate();
                self.refresh(active_profile);
            }
            Message::ToggleShowAllProfiles => {
                self.show_all_profiles = !self.show_all_profiles;
                self.invalidate();
                self.refresh(active_profile);
            }
            Message::NewSlotInputChanged(s) => {
                self.new_slot_input = s;
            }
            Message::CreateSlot => {
                let name = self.new_slot_input.trim().to_string();
                if name.is_empty() {
                    return Task::none();
                }
                match saves::create_slot(active_profile, &name) {
                    Ok(_) => {
                        self.new_slot_input.clear();
                        self.error = None;
                    }
                    Err(e) => {
                        tracing::error!(target = "crabby_ui::saves", err = %e, "create_slot");
                        self.error = Some(format!("{e}"));
                    }
                }
                self.invalidate();
                self.refresh(active_profile);
            }
            Message::SwitchSlot { profile, slot } => {
                match saves::set_active_target(&profile, &slot) {
                    Ok(()) => {
                        self.error = None;
                    }
                    Err(e) => {
                        tracing::error!(target = "crabby_ui::saves", profile = %profile, slot = %slot, err = %e, "switch_slot");
                        self.error = Some(format!("{e}"));
                    }
                }
                self.invalidate();
                self.refresh(active_profile);
            }
            Message::SnapshotSlot { profile, slot } => {
                let label = if self.snapshot_label.trim().is_empty() {
                    saves::default_snapshot_name()
                } else {
                    self.snapshot_label.trim().to_string()
                };
                let key = (profile.clone(), slot.clone());
                let succeeded = match saves::snapshot_slot(&profile, &slot, &label) {
                    Ok(p) => {
                        tracing::info!(target = "crabby_ui::saves", profile = %profile, slot = %slot, path = %p.display(), "snapshot");
                        self.snapshot_label.clear();
                        self.error = None;
                        true
                    }
                    Err(e) => {
                        tracing::error!(target = "crabby_ui::saves", profile = %profile, slot = %slot, err = %e, "snapshot");
                        self.error = Some(format!("{e}"));
                        false
                    }
                };
                self.invalidate();
                self.refresh(active_profile);
                if succeeded {
                    // Show "Snapshotted ✓" for ~2.5s, then auto-clear
                    // back to the default Snapshot button.
                    self.transient_actions.insert(key.clone(), TransientAction::SnapshotJustSucceeded);
                    return Task::perform(
                        tokio::time::sleep(std::time::Duration::from_millis(2500)),
                        move |_| Message::ClearTransientAction { profile: key.0.clone(), slot: key.1.clone() },
                    );
                }
                // Failure persists in-place - the banner has the
                // detailed message; cleared by any subsequent action.
                self.transient_actions.insert(key, TransientAction::SnapshotJustFailed);
            }
            Message::DeleteSlot { profile, slot } => {
                match saves::delete_slot(&profile, &slot) {
                    Ok(()) => {
                        if self.selected_slot.as_ref().is_some_and(|(p, s)| p == &profile && s == &slot) {
                            self.selected_slot = None;
                            self.selected_snapshots.clear();
                        }
                        self.error = None;
                    }
                    Err(e) => {
                        tracing::error!(target = "crabby_ui::saves", profile = %profile, slot = %slot, err = %e, "delete_slot");
                        self.error = Some(format!("{e}"));
                    }
                }
                self.invalidate();
                self.refresh(active_profile);
            }
            Message::RequestMoveSlotToProfile { src_profile, src_slot, dst_profile } => {
                if confirm_destructive_actions {
                    // Park the requested move so the slot row replaces
                    // its pick_list cell with [✗][Confirm] in-place.
                    self.transient_actions.insert(
                        (src_profile, src_slot),
                        TransientAction::PendingMove { dst_profile },
                    );
                } else {
                    // Confirmations off - fire the move directly.
                    return self.update(
                        Message::MoveSlotToProfile { src_profile, src_slot, dst_profile },
                        active_profile,
                        confirm_destructive_actions,
                    );
                }
            }
            Message::ClearTransientAction { profile, slot } => {
                self.transient_actions.remove(&(profile, slot));
            }
            Message::MoveSlotToProfile { src_profile, src_slot, dst_profile } => {
                self.transient_actions.remove(&(src_profile.clone(), src_slot.clone()));
                // Same slot name on the destination side. Backend
                // refuses if it'd collide with an existing slot there;
                // we surface that via the error banner like the rest of
                // the saves operations.
                match saves::move_slot_between_profiles(
                    &src_profile, &src_slot, &dst_profile, &src_slot,
                ) {
                    Ok(_report) => {
                        // Drop the selection if it pointed at the moved
                        // slot - its (profile, slot) tuple changed.
                        if self.selected_slot.as_ref().is_some_and(
                            |(p, s)| p == &src_profile && s == &src_slot,
                        ) {
                            self.selected_slot = None;
                            self.selected_snapshots.clear();
                        }
                        self.error = None;
                    }
                    Err(e) => {
                        tracing::error!(
                            target = "crabby_ui::saves",
                            src_profile = %src_profile,
                            src_slot = %src_slot,
                            dst_profile = %dst_profile,
                            err = %e,
                            "move_slot_between_profiles",
                        );
                        self.error = Some(format!("{e}"));
                    }
                }
                self.invalidate();
                self.refresh(active_profile);
            }
            Message::SelectSlot(maybe_target) => {
                self.selected_slot = maybe_target;
                self.refresh_selected_snapshots();
            }
            Message::SnapshotLabelChanged(s) => {
                self.snapshot_label = s;
            }
            Message::RestoreSnapshot { profile, slot, path } => {
                match saves::restore_snapshot(&profile, &slot, &path) {
                    Ok(()) => {
                        tracing::info!(target = "crabby_ui::saves", profile = %profile, slot = %slot, path = %path.display(), "restore");
                        self.error = None;
                    }
                    Err(e) => {
                        tracing::error!(target = "crabby_ui::saves", profile = %profile, slot = %slot, err = %e, "restore");
                        self.error = Some(format!("{e}"));
                    }
                }
                self.invalidate();
                self.refresh(active_profile);
            }
            Message::DeleteSnapshot(path) => match saves::delete_snapshot(&path) {
                Ok(()) => {
                    self.error = None;
                    self.refresh_selected_snapshots();
                }
                Err(e) => {
                    tracing::error!(target = "crabby_ui::saves", path = %path.display(), err = %e, "delete_snapshot");
                    self.error = Some(format!("{e}"));
                }
            },
            Message::VanillaImportProfileChanged(p) => {
                self.vanilla_import_profile = Some(p);
            }
            Message::VanillaImportSlotChanged(s) => {
                self.vanilla_import_slot = s;
            }
            // Actual import dispatched in app.rs (it owns the
            // VanillaSaveSet). This variant never reaches here unless
            // app.rs forgot to intercept; tolerate by clearing the
            // form so a stuck UI doesn't trap the caller.
            Message::ImportVanilla { .. } => {
                self.vanilla_import_slot.clear();
                self.vanilla_import_profile = None;
            }
        }
        Task::none()
    }

    /// Form helpers for app.rs to read after dispatching ImportVanilla.
    pub fn vanilla_import_target(&self, active_profile: &str) -> (String, String) {
        let profile = self
            .vanilla_import_profile
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| active_profile.to_string());
        let slot = if self.vanilla_import_slot.trim().is_empty() {
            saves::DEFAULT_NAME.to_string()
        } else {
            self.vanilla_import_slot.trim().to_string()
        };
        (profile, slot)
    }

    /// Surface a freshly-failed import via the existing error banner.
    /// Called from app.rs after import_vanilla_to_slot returns Err.
    pub fn set_error(&mut self, err: impl Into<String>) {
        self.error = Some(err.into());
    }

    /// Clear the vanilla-import form. Called from app.rs after a
    /// successful import (where the next refresh will also drop the
    /// section since the scan returns None).
    pub fn clear_vanilla_import_form(&mut self) {
        self.vanilla_import_slot.clear();
        self.vanilla_import_profile = None;
        self.error = None;
    }

    /// Render the tab body.
    pub fn view<'a>(
        &'a self,
        active_profile: &'a str,
        vanilla_save_set: Option<&'a saves::VanillaSaveSet>,
        profile_options: Vec<String>,
        palette: &Palette,
    ) -> Element<'a, Message> {
        let p = *palette;

        // ---- Header ----
        let toggle_label = if self.show_all_profiles {
            "Show active profile only"
        } else {
            "Show all profiles"
        };
        let toggle_btn = button(text(toggle_label).size(11))
            .padding([4, 10])
            .style(button_style(p, ButtonKind::Ghost))
            .on_press(Message::ToggleShowAllProfiles);
        let refresh_btn = button(text("Refresh").size(11))
            .padding([4, 10])
            .style(button_style(p, ButtonKind::Default))
            .on_press(Message::Refresh);
        let header = row![
            text("Saves").size(20).color(p.fg_0),
            crate::style::hspace(),
            toggle_btn,
            refresh_btn,
        ]
        .align_y(Alignment::Center)
        .spacing(8);

        // ---- Banner ----
        let scope_text = if self.show_all_profiles {
            format!("Showing slots across all profiles. Active mod profile: `{active_profile}`. Switching the active save takes effect on RTV's next launch.")
        } else {
            format!("Showing slots for the active mod profile (`{active_profile}`). Slot switches take effect on RTV's next launch.")
        };
        let banner = container(text(scope_text).size(11).color(p.fg_2))
            .padding([8, 12])
            .style(surface_style(p, SurfaceKind::Bg3))
            .width(Length::Fill);

        // ---- Optional error banner ----
        let error_banner: Element<'a, Message> = if let Some(e) = &self.error {
            container(text(e.clone()).size(12).color(p.err))
                .padding([8, 12])
                .style(surface_style(p, SurfaceKind::Bg3))
                .width(Length::Fill)
                .into()
        } else {
            iced::widget::Space::new()
                .width(Length::Fixed(0.0))
                .height(Length::Fixed(0.0))
                .into()
        };

        // ---- New-slot input (always under the active profile) ----
        let new_slot_input = text_input(
            &format!("New slot name (in `{active_profile}`)"),
            &self.new_slot_input,
        )
        .on_input(Message::NewSlotInputChanged)
        .on_submit(Message::CreateSlot)
        .padding([5, 10])
        .size(12)
        .width(Length::Fill);
        let create_btn = button(text("Create slot").size(11))
            .padding([5, 12])
            .style(button_style(p, ButtonKind::Primary))
            .on_press(Message::CreateSlot);
        let new_slot_row = row![new_slot_input, create_btn].spacing(8);

        // ---- Slot list ----
        let slot_rows: Vec<Element<'a, Message>> = if self.slots.is_empty() {
            let msg = if self.show_all_profiles {
                "No slots anywhere yet. Launch RTV once to seed a default slot, or create one above.".to_string()
            } else {
                format!("No slots in `{active_profile}` yet. Launch RTV once or create one above.")
            };
            vec![text(msg).size(12).color(p.fg_3).into()]
        } else {
            self.slots
                .iter()
                .map(|s| self.slot_row(s, active_profile, &profile_options, p))
                .collect()
        };
        let slots_section = column![
            text("SLOTS").size(11).color(p.fg_2),
            iced::widget::Column::with_children(slot_rows).spacing(6),
        ]
        .spacing(8);

        // ---- Vanilla section (renders only when loose root saves
        // were detected). Shown between the new-slot row and the
        // owned slot list so the import affordance is right next to
        // the slots it would land in.
        let vanilla_section: Element<'a, Message> = match vanilla_save_set {
            None => iced::widget::Space::new()
                .width(Length::Fixed(0.0))
                .height(Length::Fixed(0.0))
                .into(),
            Some(set) => self.vanilla_section_view(set, active_profile, profile_options, p),
        };

        let body = column![
            header,
            banner,
            error_banner,
            new_slot_row,
            vanilla_section,
            slots_section,
        ]
        .spacing(14)
        .padding(18);

        scrollable(body).height(Length::Fill).into()
    }

    /// Render the "Vanilla (unassigned)" import card. Only called when
    /// the App detected loose `*.tres` files at user-data root.
    fn vanilla_section_view<'a>(
        &'a self,
        set: &'a saves::VanillaSaveSet,
        active_profile: &'a str,
        profile_options: Vec<String>,
        p: Palette,
    ) -> Element<'a, Message> {
        use iced::widget::pick_list;

        let header = row![
            text("VANILLA (unassigned)").size(11).color(p.fg_2),
            crate::style::hspace(),
            text(format!("{} file(s)", set.files.len())).size(11).color(p.fg_3),
        ]
        .align_y(Alignment::Center)
        .spacing(8);

        let summary = format!(
            "{} loose save file(s) at the game user dir, total {}{}. Import them into a profile slot to make them visible to crabby's slot picker.",
            set.files.len(),
            fmt_size(Some(set.total_size_bytes)),
            match set.last_modified {
                Some(_) => format!(", last modified {}", fmt_modified(set.last_modified)),
                None => String::new(),
            },
        );
        let summary_line = text(summary).size(11).color(p.fg_2);

        // Profile dropdown - defaults to active profile when nothing
        // explicitly picked.
        let current_profile = self
            .vanilla_import_profile
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| active_profile.to_string());
        let profile_list_for_picker = profile_options.clone();
        let profile_picker = pick_list(
            profile_list_for_picker,
            Some(current_profile),
            Message::VanillaImportProfileChanged,
        )
        .padding([5, 10])
        .text_size(12);

        let slot_input = text_input("Slot name (default: `default`)", &self.vanilla_import_slot)
            .on_input(Message::VanillaImportSlotChanged)
            .padding([5, 10])
            .size(12)
            .width(Length::Fill);

        // The import button needs concrete (profile, slot) values at
        // press time; form defaults are resolved here so the button
        // can carry them in the message payload.
        let (target_profile, target_slot) = self.vanilla_import_target(active_profile);
        let import_btn = button(text("Import to profile").size(11))
            .padding([5, 12])
            .style(button_style(p, ButtonKind::Primary))
            .on_press(Message::ImportVanilla {
                profile: target_profile,
                slot: target_slot,
            });

        let form_row = row![profile_picker, slot_input, import_btn].spacing(8);

        container(
            column![header, summary_line, form_row]
                .spacing(8),
        )
        .padding([10, 12])
        .style(surface_style(p, SurfaceKind::Bg3))
        .width(Length::Fill)
        .into()
    }

    fn slot_row<'a>(
        &'a self,
        s: &'a SlotInfo,
        active_profile: &str,
        profile_options: &[String],
        p: Palette,
    ) -> Element<'a, Message> {
        let active_marker: Element<'a, Message> = if s.active {
            crate::style::pill(p, "active", crate::style::PillTone::Ok).into()
        } else {
            iced::widget::Space::new().width(Length::Fixed(0.0)).into()
        };

        // Profile chip - only visible when showing all profiles, since
        // it's redundant in the scoped view.
        let profile_chip: Element<'a, Message> = if self.show_all_profiles {
            crate::style::pill(p, &s.profile, crate::style::PillTone::Neutral).into()
        } else {
            iced::widget::Space::new().width(Length::Fixed(0.0)).into()
        };

        let title = text(s.name.clone()).size(13).color(p.fg_0);
        let size_text = text(fmt_size(s.size_bytes)).size(11).color(p.fg_3);

        // Switch is disabled when this slot belongs to a non-active
        // profile in "show all" mode - switching across profiles is a
        // bigger gesture than this UI commits to in v1 (it'd require
        // also switching the mod profile to keep state consistent).
        let foreign = s.profile != active_profile;
        let switch_btn: Element<'a, Message> = if s.active {
            button(text("In use").size(11))
                .padding([3, 10])
                .style(button_style(p, ButtonKind::Default))
                .into()
        } else if foreign {
            button(text("Switch profile first").size(11))
                .padding([3, 10])
                .style(button_style(p, ButtonKind::Default))
                .into()
        } else {
            button(text("Switch").size(11))
                .padding([3, 10])
                .style(button_style(p, ButtonKind::Primary))
                .on_press(Message::SwitchSlot {
                    profile: s.profile.clone(),
                    slot: s.name.clone(),
                })
                .into()
        };

        // Snapshot button: in-place state machine driven by
        // `transient_actions`. Idle → "Snapshot" → "Saving…" while the
        // dispatch is in flight → "Snapshotted ✓" for ~2.5s on success
        // (then auto-clears) or "Failed ✗" persisting on failure (banner
        // carries the detail; cleared by any other action). Width is
        // fixed so the row doesn't reflow as state changes.
        let snap_state = self
            .transient_actions
            .get(&(s.profile.clone(), s.name.clone()));
        let snap_btn: Element<'a, Message> = match snap_state {
            Some(TransientAction::SnapshotInFlight) => button(
                text("Saving…").size(11).color(p.fg_2).width(Length::Fixed(100.0)),
            )
            .padding([3, 10])
            .style(button_style(p, ButtonKind::Default))
            .into(),
            Some(TransientAction::SnapshotJustSucceeded) => button(
                text("Snapshotted ✓").size(11).color(p.ok).width(Length::Fixed(100.0)),
            )
            .padding([3, 10])
            .style(button_style(p, ButtonKind::Default))
            .into(),
            Some(TransientAction::SnapshotJustFailed) => button(
                text("Failed ✗").size(11).color(p.err).width(Length::Fixed(100.0)),
            )
            .padding([3, 10])
            .style(button_style(p, ButtonKind::Default))
            .into(),
            _ => button(text("Snapshot").size(11).width(Length::Fixed(100.0)))
                .padding([3, 10])
                .style(button_style(p, ButtonKind::Default))
                .on_press(Message::SnapshotSlot {
                    profile: s.profile.clone(),
                    slot: s.name.clone(),
                })
                .into(),
        };

        let is_selected = self.selected_slot.as_ref().is_some_and(|(p_, s_)| p_ == &s.profile && s_ == &s.name);
        let select_label = if is_selected { "Hide snapshots" } else { "Snapshots" };
        let select_msg = if is_selected {
            Message::SelectSlot(None)
        } else {
            Message::SelectSlot(Some((s.profile.clone(), s.name.clone())))
        };
        let select_btn = button(text(select_label).size(11))
            .padding([3, 10])
            .style(button_style(p, ButtonKind::Ghost))
            .on_press(select_msg);

        let delete_btn: Element<'a, Message> = if s.active {
            button(text("Delete").size(11))
                .padding([3, 10])
                .style(button_style(p, ButtonKind::Default))
                .into()
        } else {
            button(text("Delete").size(11))
                .padding([3, 10])
                .style(button_style(p, ButtonKind::Default))
                .on_press(Message::DeleteSlot {
                    profile: s.profile.clone(),
                    slot: s.name.clone(),
                })
                .into()
        };

        // Move-to-profile cell. Three visual states, all roughly the
        // same width so the row doesn't reflow:
        //   1. Disabled - static "Move" pill when there's no possible
        //      destination (active slot, or single-profile install).
        //   2. Idle pick_list - shows destination profiles; clicking
        //      one transitions to (3) when confirm-destructive is on.
        //   3. Pending confirm - replaced by [✗][Confirm] inline.
        //      Confirm carries a tooltip with the destination name so
        //      the detail shows without the button overflowing.
        let move_state = self
            .transient_actions
            .get(&(s.profile.clone(), s.name.clone()));
        let move_destinations: Vec<String> = profile_options
            .iter()
            .filter(|name| name.as_str() != s.profile.as_str())
            .cloned()
            .collect();
        let move_picker: Element<'a, Message> = match move_state {
            Some(TransientAction::PendingMove { dst_profile }) => {
                let cancel_btn = button(text("✗").size(11).color(p.fg_2))
                    .padding([3, 8])
                    .style(button_style(p, ButtonKind::Default))
                    .on_press(Message::ClearTransientAction {
                        profile: s.profile.clone(),
                        slot: s.name.clone(),
                    });
                let confirm_btn = button(text("Confirm").size(11))
                    .padding([3, 10])
                    .style(button_style(p, ButtonKind::Primary))
                    .on_press(Message::MoveSlotToProfile {
                        src_profile: s.profile.clone(),
                        src_slot: s.name.clone(),
                        dst_profile: dst_profile.clone(),
                    });
                let confirm_with_tip = tooltip(
                    confirm_btn,
                    container(text(format!("Move to '{}'", dst_profile)).size(11).color(p.fg_0))
                        .padding(6)
                        .style(surface_style(p, SurfaceKind::Bg2)),
                    tooltip::Position::Top,
                );
                row![cancel_btn, confirm_with_tip]
                    .spacing(4)
                    .align_y(Alignment::Center)
                    .into()
            }
            _ => {
                if s.active || move_destinations.is_empty() {
                    button(text("Move").size(11))
                        .padding([3, 10])
                        .style(button_style(p, ButtonKind::Default))
                        .into()
                } else {
                    use iced::widget::pick_list;
                    let src_profile = s.profile.clone();
                    let src_slot = s.name.clone();
                    pick_list(
                        move_destinations,
                        None::<String>,
                        move |dst_profile| Message::RequestMoveSlotToProfile {
                            src_profile: src_profile.clone(),
                            src_slot: src_slot.clone(),
                            dst_profile,
                        },
                    )
                    .placeholder("Move →")
                    .padding([3, 10])
                    .text_size(11)
                    .into()
                }
            }
        };

        let header_row = row![
            column![title, size_text].spacing(2).width(Length::Fill),
            profile_chip,
            active_marker,
            switch_btn,
            snap_btn,
            select_btn,
            move_picker,
            delete_btn,
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .padding([8, 12]);

        let body: Element<'a, Message> = if is_selected {
            column![
                header_row,
                container(self.snapshot_section(&s.profile, &s.name, p))
                    .padding(iced::Padding { top: 0.0, right: 12.0, bottom: 12.0, left: 12.0 }),
            ]
            .into()
        } else {
            header_row.into()
        };

        container(body)
            .style(surface_style(p, SurfaceKind::Bg3))
            .width(Length::Fill)
            .into()
    }

    fn snapshot_section<'a>(
        &'a self,
        profile: &str,
        slot: &str,
        p: Palette,
    ) -> Element<'a, Message> {
        let label_input = text_input(
            "Snapshot label (auto-stamp if empty)",
            &self.snapshot_label,
        )
        .on_input(Message::SnapshotLabelChanged)
        .on_submit(Message::SnapshotSlot {
            profile: profile.to_string(),
            slot: slot.to_string(),
        })
        .padding([5, 10])
        .size(12)
        .width(Length::Fill);
        let snap_btn = button(text("Snapshot now").size(11))
            .padding([5, 12])
            .style(button_style(p, ButtonKind::Primary))
            .on_press(Message::SnapshotSlot {
                profile: profile.to_string(),
                slot: slot.to_string(),
            });
        let snap_input_row = row![label_input, snap_btn].spacing(8);

        let header = text(format!("SNAPSHOTS - {profile}/{slot}")).size(11).color(p.fg_2);

        let rows: Vec<Element<'a, Message>> = if self.selected_snapshots.is_empty() {
            vec![
                text("No snapshots yet. Click Snapshot now to make one.")
                    .size(12)
                    .color(p.fg_3)
                    .into(),
            ]
        } else {
            self.selected_snapshots
                .iter()
                .map(|s| snapshot_row(profile, slot, s, p))
                .collect()
        };

        column![header, snap_input_row, iced::widget::Column::with_children(rows).spacing(6)]
            .spacing(8)
            .into()
    }
}

fn snapshot_row<'a>(
    profile: &str,
    slot: &str,
    s: &'a SnapshotInfo,
    p: Palette,
) -> Element<'a, Message> {
    let title = text(s.name.clone()).size(12).color(p.fg_0);
    let meta_bits: Vec<String> = vec![
        fmt_size(Some(s.size_bytes)),
        fmt_modified(s.modified),
    ];
    let meta = text(meta_bits.join(" • ")).size(11).color(p.fg_3);

    let restore_btn = button(text("Restore").size(11))
        .padding([3, 10])
        .style(button_style(p, ButtonKind::Primary))
        .on_press(Message::RestoreSnapshot {
            profile: profile.to_string(),
            slot: slot.to_string(),
            path: s.path.clone(),
        });
    let delete_btn = button(text("Delete").size(11))
        .padding([3, 10])
        .style(button_style(p, ButtonKind::Default))
        .on_press(Message::DeleteSnapshot(s.path.clone()));

    container(
        row![
            column![title, meta].spacing(2).width(Length::Fill),
            restore_btn,
            delete_btn,
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .padding([8, 12]),
    )
    .style(surface_style(p, SurfaceKind::Bg3))
    .width(Length::Fill)
    .into()
}

fn fmt_size(bytes: Option<u64>) -> String {
    let Some(b) = bytes else { return "—".into() };
    if b < 1024 {
        format!("{b} B")
    } else if b < 1024 * 1024 {
        format!("{:.1} KiB", b as f64 / 1024.0)
    } else if b < 1024 * 1024 * 1024 {
        format!("{:.1} MiB", b as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GiB", b as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

fn fmt_modified(t: Option<SystemTime>) -> String {
    let Some(t) = t else { return "—".into() };
    let secs = t.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let age = now.saturating_sub(secs);
    match age {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m ago", age / 60),
        3600..=86_399 => format!("{}h ago", age / 3600),
        _ => format!("{}d ago", age / 86_400),
    }
}

const _: fn(SavesError) = |_| ();
