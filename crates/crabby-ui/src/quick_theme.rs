//! Floating quick-theme panel - bottom-right of the window.
//!
//! Designed to overlay on top of the main UI via [`iced::widget::stack`].
//! When closed the overlay is just the bottom-right pill; when open
//! the expanded panel sits above the pill.

use iced::widget::{button, column, container, row, slider, text};
use iced::{Alignment, Element, Length};

use crate::app::{Message, ThemeChange};
use crate::launcher_config::MAX_SAVED_COLORS;
use crate::style::{ButtonKind, button_style};
use crate::theme::{CrabbyTheme, Mode, Oklch, Palette};

/// Build the quick-theme overlay. Returns a `Length::Fill` element
/// suitable for stacking - the actual panel is anchored bottom-right
/// inside via container alignment.
#[must_use]
pub fn overlay<'a>(
    theme: &'a CrabbyTheme,
    saved_colors: &'a [[f32; 3]],
    open: bool,
    palette: &Palette,
) -> Element<'a, Message> {
    let p = *palette;

    let body: Element<'a, Message> = if open {
        column![panel(theme, saved_colors, p), pill(theme, p)]
            .spacing(8)
            .align_x(Alignment::End)
            .into()
    } else {
        pill(theme, p).into()
    };

    container(body)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(iced::alignment::Horizontal::Right)
        .align_y(iced::alignment::Vertical::Bottom)
        .padding(iced::Padding {
            top: 0.0,
            right: 18.0,
            bottom: 42.0,
            left: 0.0,
        })
        .into()
}

fn pill<'a>(theme: &'a CrabbyTheme, p: Palette) -> Element<'a, Message> {
    let accent_swatch = container(text("").size(1))
        .width(Length::Fixed(18.0))
        .height(Length::Fixed(18.0))
        .style(move |_t| iced::widget::container::Style {
            background: Some(iced::Background::Color(theme.palette.accent)),
            text_color: Some(p.fg_0),
            border: iced::Border {
                color: p.line,
                width: 1.0,
                radius: 999.0.into(),
            },
            ..Default::default()
        });
    let glyph = match theme.mode {
        Mode::Dark => "🌙",
        Mode::Light => "☀",
    };
    button(
        row![
            accent_swatch,
            text(glyph).size(11).color(p.fg_2),
            text("Theme").size(11).color(p.fg_2),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .padding(iced::Padding {
            top: 0.0,
            right: 10.0,
            bottom: 0.0,
            left: 8.0,
        }),
    )
    .padding(0)
    .style(move |_t, _s| iced::widget::button::Style {
        background: Some(iced::Background::Color(p.bg_2)),
        text_color: p.fg_1,
        border: iced::Border {
            color: p.line,
            width: 1.0,
            radius: 999.0.into(),
        },
        ..Default::default()
    })
    .on_press(Message::ToggleQuickTheme)
    .into()
}

fn panel<'a>(
    theme: &'a CrabbyTheme,
    saved_colors: &'a [[f32; 3]],
    p: Palette,
) -> Element<'a, Message> {
    let header = row![
        text("Quick theme").size(12).color(p.fg_0),
        crate::style::hspace(),
        button(text("×").size(13).color(p.fg_2))
            .padding(iced::Padding {
                top: 0.0,
                right: 6.0,
                bottom: 0.0,
                left: 6.0
            })
            .style(button_style(p, ButtonKind::Ghost))
            .on_press(Message::ToggleQuickTheme),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .padding([10, 14]);

    let mode_row = row![
        seg(
            "Dark",
            theme.mode == Mode::Dark,
            p,
            ThemeChange::Mode(Mode::Dark)
        ),
        seg(
            "Light",
            theme.mode == Mode::Light,
            p,
            ThemeChange::Mode(Mode::Light)
        ),
    ]
    .spacing(6);

    let preview = container(text("").size(1))
        .width(Length::Fixed(48.0))
        .height(Length::Fixed(48.0))
        .style(move |_t| iced::widget::container::Style {
            background: Some(iced::Background::Color(theme.palette.accent)),
            text_color: Some(p.fg_0),
            border: iced::Border {
                color: p.line,
                width: 1.0,
                radius: 8.0.into(),
            },
            ..Default::default()
        });

    let hue = slider(0.0..=360.0, theme.accent_hue, |v| {
        Message::ThemeChanged(ThemeChange::AccentHue(v))
    })
    .step(1.0)
    .width(Length::Fill);
    let chroma = slider(0.0..=0.30, theme.accent_c, |v| {
        Message::ThemeChanged(ThemeChange::AccentC(v))
    })
    .step(0.005)
    .width(Length::Fill);
    let lightness = slider(0.40..=0.85, theme.accent_l, |v| {
        Message::ThemeChanged(ThemeChange::AccentL(v))
    })
    .step(0.005)
    .width(Length::Fill);

    let accent_block = column![
        row![
            preview,
            column![
                mini_slider("Hue", &format!("{:.0}°", theme.accent_hue), hue, p),
                mini_slider("Chr", &format!("{:.3}", theme.accent_c), chroma, p),
                mini_slider("L", &format!("{:.2}", theme.accent_l), lightness, p),
            ]
            .spacing(8)
            .width(Length::Fill),
        ]
        .spacing(12)
        .align_y(Alignment::Center),
    ];

    let bg_tint = slider(0.0..=360.0, theme.bg_tint_hue, |v| {
        Message::ThemeChanged(ThemeChange::BgTintHue(v))
    })
    .step(1.0)
    .width(Length::Fill);

    let saved = saved_strip(saved_colors, p);
    let save_btn = button(text("Save current").size(10))
        .padding(iced::Padding {
            top: 3.0,
            right: 8.0,
            bottom: 3.0,
            left: 8.0,
        })
        .style(button_style(p, ButtonKind::Default))
        .on_press(Message::ThemeChanged(ThemeChange::SaveCurrent));

    let body = column![
        eyebrow("MODE", p),
        mode_row,
        eyebrow("ACCENT", p),
        accent_block,
        row![
            eyebrow("SAVED", p),
            crate::style::hspace(),
            text(format!("{}/{}", saved_colors.len(), MAX_SAVED_COLORS))
                .size(10)
                .color(p.fg_3),
            save_btn,
        ]
        .spacing(6)
        .align_y(Alignment::Center),
        saved,
        eyebrow("BACKGROUND", p),
        mini_slider("Hue", &format!("{:.0}°", theme.bg_tint_hue), bg_tint, p),
        text("More options in Settings → Appearance.")
            .size(10)
            .color(p.fg_3),
    ]
    .spacing(8)
    .padding(14);

    container(column![header, body].spacing(0))
        .width(Length::Fixed(280.0))
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
        .into()
}

fn eyebrow<'a>(label: &'a str, p: Palette) -> Element<'a, Message> {
    text(label).size(10).color(p.fg_2).into()
}

fn seg<'a>(label: &'a str, active: bool, p: Palette, change: ThemeChange) -> Element<'a, Message> {
    let kind = if active {
        ButtonKind::Primary
    } else {
        ButtonKind::Ghost
    };
    button(text(label).size(10))
        .padding([4, 10])
        .style(button_style(p, kind))
        .on_press(Message::ThemeChanged(change))
        .into()
}

fn mini_slider<'a>(
    label: &'a str,
    value_str: &str,
    s: iced::widget::Slider<'a, f32, Message>,
    p: Palette,
) -> Element<'a, Message> {
    column![
        row![
            text(label)
                .size(10)
                .color(p.fg_2)
                .width(Length::Fixed(36.0)),
            crate::style::hspace(),
            text(value_str.to_string()).size(10).color(p.fg_1),
        ]
        .align_y(Alignment::Center),
        s,
    ]
    .spacing(2)
    .into()
}

fn saved_strip<'a>(saved: &'a [[f32; 3]], p: Palette) -> Element<'a, Message> {
    if saved.is_empty() {
        return text("No saved colors yet.").size(10).color(p.fg_3).into();
    }
    let cells: Vec<Element<'a, Message>> = saved.iter().map(|lch| saved_cell(*lch, p)).collect();
    iced::widget::Row::with_children(cells)
        .spacing(6)
        .align_y(Alignment::Center)
        .wrap()
        .into()
}

fn saved_cell<'a>(lch: [f32; 3], p: Palette) -> Element<'a, Message> {
    let oklch = Oklch(lch[0], lch[1], lch[2]);
    button(text("").size(1))
        .width(Length::Fixed(20.0))
        .height(Length::Fixed(20.0))
        .padding(0)
        .style(move |_t, _s| iced::widget::button::Style {
            background: Some(iced::Background::Color(oklch.color())),
            text_color: p.fg_0,
            border: iced::Border {
                color: p.line,
                width: 1.0,
                radius: 999.0.into(),
            },
            ..Default::default()
        })
        .on_press(Message::ThemeChanged(ThemeChange::ApplySaved(lch)))
        .into()
}
