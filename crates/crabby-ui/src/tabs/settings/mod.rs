//! Settings tab - left-rail shell hosting Appearance / General /
//! Diagnostics sub-views. The shell owns the active sub-view + the
//! Appearance state; General reuses the existing diagnostics State
//! (which holds mod-sources mutations) for now, and Diagnostics is
//! the read-only install snapshot that lives in [`crate::tabs::diagnostics`].

use std::path::Path;

use iced::widget::{button, column, container, row, text};
use iced::{Alignment, Element, Length};

use crate::style::{ButtonKind, SurfaceKind, button_style, surface_style};
use crate::theme::Palette;

pub mod appearance;

/// Per-tab message - wraps sub-view messages plus rail-navigation.
#[derive(Debug, Clone)]
pub enum Message {
    /// Rail entry clicked.
    SelectSection(Section),
    /// Forwarded from the Appearance sub-view. Only emitted by view
    /// helpers in `appearance.rs`; the App routes the inner change
    /// straight to its theme-handling code.
    Theme(crate::app::ThemeChange),
    /// Forwarded from the General sub-view's mod-sources controls.
    /// Reuses the existing diagnostics::Message variants since the
    /// underlying CRUD on `mod_config.cfg` is the same.
    General(crate::tabs::diagnostics::Message),
}

/// Which sub-view is rendering in the body pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Section {
    /// Theme controls - mode, density, accent + bg-tint pickers, saved colors.
    #[default]
    Appearance,
    /// Game dir, cache dir, mod sources.
    General,
    /// Read-only install/PCK/manifest snapshot.
    Diagnostics,
}

impl Section {
    fn label(self) -> &'static str {
        match self {
            Self::Appearance => "Appearance",
            Self::General => "General",
            Self::Diagnostics => "Diagnostics",
        }
    }
}

/// Per-tab state.
#[derive(Debug, Default)]
pub struct State {
    /// Currently-rendered sub-view.
    pub section: Section,
}

impl State {
    /// Apply a settings-tab message. The App routes
    /// `Message::Theme(...)` and `Message::General(...)` directly to
    /// the theme/diagnostics handlers - this only handles rail nav.
    pub fn update(&mut self, msg: Message) {
        if let Message::SelectSection(s) = msg {
            self.section = s;
        }
    }

    /// Render the rail + the active sub-view.
    pub fn view<'a>(
        &'a self,
        game_dir: Option<&'a Path>,
        theme: &'a crate::theme::CrabbyTheme,
        saved_colors: &'a [[f32; 3]],
        pill_overrides: &'a std::collections::BTreeMap<String, [f32; 3]>,
        diagnostics: &'a crate::tabs::diagnostics::State,
        confirm_destructive_actions: bool,
        palette: &Palette,
    ) -> Element<'a, Message> {
        let p = *palette;

        let rail = self.rail(p);

        let body: Element<'a, Message> = match self.section {
            Section::Appearance => {
                appearance::view(theme, saved_colors, pill_overrides, &p).map(Message::Theme)
            }
            Section::General => diagnostics
                .general_view(game_dir, confirm_destructive_actions, &p)
                .map(Message::General),
            Section::Diagnostics => diagnostics.diagnostics_view(&p).map(Message::General),
        };

        let body_container = container(body)
            .style(surface_style(p, SurfaceKind::Bg2))
            .width(Length::Fill)
            .height(Length::Fill);

        row![rail, body_container]
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn rail<'a>(&'a self, p: Palette) -> Element<'a, Message> {
        let entries = [Section::Appearance, Section::General, Section::Diagnostics];
        let rows: Vec<Element<'a, Message>> = entries
            .into_iter()
            .map(|s| {
                let active = self.section == s;
                let label_color = if active { p.fg_0 } else { p.fg_2 };
                button(
                    text(s.label())
                        .size(12)
                        .color(label_color)
                        .width(Length::Fill),
                )
                .padding([8, 14])
                .style(button_style(
                    p,
                    if active {
                        ButtonKind::Primary
                    } else {
                        ButtonKind::Ghost
                    },
                ))
                .width(Length::Fill)
                .on_press(Message::SelectSection(s))
                .into()
            })
            .collect();

        container(
            column![
                text("SETTINGS").size(11).color(p.fg_2),
                iced::widget::Space::new()
                    .width(Length::Fill)
                    .height(Length::Fixed(4.0)),
                iced::widget::Column::with_children(rows).spacing(2),
            ]
            .spacing(8)
            .padding(14),
        )
        .style(surface_style(p, SurfaceKind::Bg1))
        .width(Length::Fixed(180.0))
        .height(Length::Fill)
        .align_y(Alignment::Start)
        .into()
    }
}
