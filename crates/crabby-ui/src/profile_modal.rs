//! Profile create/edit modal overlay.
//!
//! Replaces the inline editor strip that used to drop down under the
//! profile bar. Two flavors:
//!
//! - **Create:** name input + Create / Cancel
//! - **Edit:** name input pre-filled with the active profile's name +
//!   Save / Delete (when allowed) / Cancel
//!
//! Driven by `profiles::ProfileModal` on the App's `profiles` state.
//! Stacked on top of the main UI via the same pattern as the modpack
//! overlay - when modal is `None` an empty space is rendered so clicks
//! pass through.

use iced::widget::{button, column, container, row, text, text_input};
use iced::{Alignment, Element, Length};

use crate::style::{ButtonKind, button_style};
use crate::tabs::profiles::{self, ProfileModal};
use crate::theme::Palette;

/// Render the profile-modal overlay. Returns an empty element when no
/// modal is open.
#[must_use]
pub fn overlay<'a>(
    state: &'a profiles::State,
    palette: &Palette,
) -> Element<'a, crate::app::Message> {
    let p = *palette;
    match state.modal {
        ProfileModal::None => empty(),
        ProfileModal::Create => modal(p, create_view(state, p)),
        ProfileModal::Edit => modal(p, edit_view(state, p)),
    }
}

fn empty<'a>() -> Element<'a, crate::app::Message> {
    iced::widget::Space::new()
        .width(Length::Fixed(0.0))
        .height(Length::Fixed(0.0))
        .into()
}

fn modal<'a>(
    p: Palette,
    body: Element<'a, crate::app::Message>,
) -> Element<'a, crate::app::Message> {
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
        .max_width(420.0);
    let backdrop = iced::Color { a: 0.55, ..p.bg_0 };
    container(panel)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .style(move |_t| iced::widget::container::Style {
            background: Some(iced::Background::Color(backdrop)),
            ..Default::default()
        })
        .into()
}

fn header<'a>(title: String, p: Palette) -> Element<'a, crate::app::Message> {
    row![
        text(title).size(14).color(p.fg_0),
        crate::style::hspace(),
        button(text("×").size(14).color(p.fg_2))
            .padding(iced::Padding {
                top: 0.0,
                right: 8.0,
                bottom: 0.0,
                left: 8.0,
            })
            .style(button_style(p, ButtonKind::Ghost))
            .on_press(crate::app::Message::Profiles(profiles::Message::DismissModal)),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .padding([12, 16])
    .into()
}

fn create_view<'a>(
    state: &'a profiles::State,
    p: Palette,
) -> Element<'a, crate::app::Message> {
    let h = header("Create profile".into(), p);
    let input = text_input("Profile name", &state.editor_input)
        .on_input(|s| {
            crate::app::Message::Profiles(profiles::Message::EditorInputChanged(s))
        })
        .on_submit(crate::app::Message::Profiles(profiles::Message::CreateProfile))
        .padding([6, 10])
        .size(12)
        .width(Length::Fill);

    let err: Element<'a, crate::app::Message> = if let Some(e) = &state.editor_error {
        text(e.clone()).size(11).color(p.err).into()
    } else {
        empty()
    };

    let actions = row![
        crate::style::hspace(),
        button(text("Cancel").size(11))
            .padding([4, 12])
            .style(button_style(p, ButtonKind::Default))
            .on_press(crate::app::Message::Profiles(profiles::Message::DismissModal)),
        button(text("Create").size(11))
            .padding([4, 14])
            .style(button_style(p, ButtonKind::Primary))
            .on_press(crate::app::Message::Profiles(profiles::Message::CreateProfile)),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let body = column![input, err, actions]
        .spacing(10)
        .padding(iced::Padding {
            top: 0.0,
            right: 16.0,
            bottom: 16.0,
            left: 16.0,
        });

    column![h, Element::<crate::app::Message>::from(body)]
        .spacing(0)
        .into()
}

fn edit_view<'a>(
    state: &'a profiles::State,
    p: Palette,
) -> Element<'a, crate::app::Message> {
    let h = header(format!("Edit profile: {}", state.active), p);

    let input = text_input("Profile name", &state.editor_input)
        .on_input(|s| {
            crate::app::Message::Profiles(profiles::Message::EditorInputChanged(s))
        })
        .on_submit(crate::app::Message::Profiles(profiles::Message::RenameActive))
        .padding([6, 10])
        .size(12)
        .width(Length::Fill);

    let err: Element<'a, crate::app::Message> = if let Some(e) = &state.editor_error {
        text(e.clone()).size(11).color(p.err).into()
    } else {
        empty()
    };

    // Delete is only allowed when there's >1 profile AND the active
    // profile isn't the only one. The *active* profile can't be
    // deleted through this modal because the input is the active
    // profile's name - instead, delete buttons are surfaced for the
    // *other* profiles, like the old strip did.
    let mut delete_chips: Vec<Element<'a, crate::app::Message>> = vec![];
    let can_delete_any = state.all.len() > 1;
    for name in &state.all {
        if name == &state.active {
            continue;
        }
        let chip: Element<'a, crate::app::Message> = if can_delete_any {
            button(text(format!("Delete {name}")).size(11))
                .padding([3, 10])
                .style(button_style(p, ButtonKind::Ghost))
                .on_press(crate::app::Message::Profiles(
                    profiles::Message::DeleteProfile(name.clone()),
                ))
                .into()
        } else {
            text(name.clone()).size(11).color(p.fg_3).into()
        };
        delete_chips.push(chip);
    }
    let delete_section: Element<'a, crate::app::Message> = if delete_chips.is_empty() {
        text("Only profile, can't delete the last one.")
            .size(11)
            .color(p.fg_3)
            .into()
    } else {
        column![
            text("OTHER PROFILES").size(10).color(p.fg_2),
            iced::widget::Row::with_children(delete_chips)
                .spacing(6)
                .wrap(),
        ]
        .spacing(4)
        .into()
    };

    let actions = row![
        crate::style::hspace(),
        button(text("Cancel").size(11))
            .padding([4, 12])
            .style(button_style(p, ButtonKind::Default))
            .on_press(crate::app::Message::Profiles(profiles::Message::DismissModal)),
        button(text("Save").size(11))
            .padding([4, 14])
            .style(button_style(p, ButtonKind::Primary))
            .on_press(crate::app::Message::Profiles(profiles::Message::RenameActive)),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let body = column![input, err, delete_section, actions]
        .spacing(12)
        .padding(iced::Padding {
            top: 0.0,
            right: 16.0,
            bottom: 16.0,
            left: 16.0,
        });

    column![h, Element::<crate::app::Message>::from(body)]
        .spacing(0)
        .into()
}
