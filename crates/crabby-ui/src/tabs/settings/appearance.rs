//! Appearance sub-view: mode toggle, accent picker (OKLCH
//! lightness/chroma/hue sliders + live swatch + saved-colors row),
//! bg-tint hue slider, and per-tone pill color overrides.
//!
//! Emits [`crate::app::ThemeChange`] messages - the App owns the
//! theme + persistence. This module is just the dial.

use iced::widget::{button, column, container, row, scrollable, slider, text};
use iced::{Alignment, Element, Length};

use crate::app::ThemeChange;
use crate::launcher_config::MAX_SAVED_COLORS;
use crate::style::{ButtonKind, button_style};
use crate::theme::{CrabbyTheme, Mode, Oklch, Palette};

/// Render the Appearance sub-view body.
///
/// `saved_colors` and `pill_overrides` are pulled from `LauncherConfig`
/// (they outlive any single `CrabbyTheme`).
#[must_use]
pub fn view<'a>(
    theme: &'a CrabbyTheme,
    saved_colors: &'a [[f32; 3]],
    pill_overrides: &'a std::collections::BTreeMap<String, [f32; 3]>,
    palette: &Palette,
) -> Element<'a, ThemeChange> {
    let p = *palette;

    let header = text("Appearance").size(20).color(p.fg_0);

    let body = column![
        header,
        section_mode(theme, p),
        section_accent(theme, saved_colors, p),
        section_bg_tint(theme, p),
        section_pills(saved_colors, pill_overrides, p),
    ]
    .spacing(20)
    .padding(20);

    scrollable(body).height(Length::Fill).into()
}

fn eyebrow<'a>(label: &'a str, p: Palette) -> Element<'a, ThemeChange> {
    text(label).size(11).color(p.fg_2).into()
}

fn section_mode<'a>(theme: &'a CrabbyTheme, p: Palette) -> Element<'a, ThemeChange> {
    let segmented = row![
        seg_button("Dark", theme.mode == Mode::Dark, p, ThemeChange::Mode(Mode::Dark)),
        seg_button("Light", theme.mode == Mode::Light, p, ThemeChange::Mode(Mode::Light)),
    ]
    .spacing(6);
    column![eyebrow("MODE", p), segmented].spacing(8).into()
}

/// Per-tone pill-color overrides. Each row shows the tone's resolved
/// swatch + a strip of saved colors to apply + a Reset button when the
/// tone has been overridden.
fn section_pills<'a>(
    saved_colors: &'a [[f32; 3]],
    overrides: &'a std::collections::BTreeMap<String, [f32; 3]>,
    p: Palette,
) -> Element<'a, ThemeChange> {
    let rows: Vec<Element<'a, ThemeChange>> = [
        ("ok", "OK / Installed", Oklch(0.74, 0.11, 155.0)),
        ("warn", "Warning", Oklch(0.78, 0.13, 75.0)),
        ("err", "Error / Conflicts", Oklch(0.66, 0.17, 25.0)),
    ]
    .into_iter()
    .map(|(key, label, default_color)| pill_row(key, label, default_color, saved_colors, overrides, p))
    .collect();

    column![
        eyebrow("PILL COLORS", p),
        text("Click a saved color to apply it as the override. Reset returns the tone to its design default.")
            .size(11)
            .color(p.fg_3),
        iced::widget::Column::with_children(rows).spacing(8),
    ]
    .spacing(10)
    .into()
}

fn pill_row<'a>(
    key: &'static str,
    label: &'a str,
    default_color: Oklch,
    saved_colors: &'a [[f32; 3]],
    overrides: &'a std::collections::BTreeMap<String, [f32; 3]>,
    p: Palette,
) -> Element<'a, ThemeChange> {
    let current = overrides
        .get(key)
        .map(|lch| Oklch(lch[0], lch[1], lch[2]))
        .unwrap_or(default_color);
    let is_overridden = overrides.contains_key(key);

    let swatch = swatch_box(current, 28.0, true, p);
    let label_w = text(label.to_string()).size(12).color(p.fg_0).width(Length::Fixed(160.0));
    let reset_btn: Element<'a, ThemeChange> = if is_overridden {
        button(text("Reset").size(11))
            .padding([3, 10])
            .style(button_style(p, ButtonKind::Ghost))
            .on_press(ThemeChange::ClearPillOverride { tone_key: key.into() })
            .into()
    } else {
        text("Default").size(11).color(p.fg_3).into()
    };

    let saved_strip: Element<'a, ThemeChange> = if saved_colors.is_empty() {
        text("(no saved colors yet)").size(11).color(p.fg_3).into()
    } else {
        let cells: Vec<Element<'a, ThemeChange>> = saved_colors
            .iter()
            .map(|lch| pick_swatch(key, *lch, p))
            .collect();
        iced::widget::Row::with_children(cells)
            .spacing(6)
            .align_y(Alignment::Center)
            .wrap()
            .into()
    };

    container(
        column![
            row![swatch, label_w, crate::style::hspace(), reset_btn]
                .spacing(8)
                .align_y(Alignment::Center),
            saved_strip,
        ]
        .spacing(8)
        .padding([8, 12]),
    )
    .style(move |_t| iced::widget::container::Style {
        background: Some(iced::Background::Color(p.bg_3)),
        text_color: Some(p.fg_0),
        border: iced::Border { color: p.line_soft, width: 1.0, radius: 6.0.into() },
        ..Default::default()
    })
    .width(Length::Fill)
    .into()
}

fn pick_swatch<'a>(key: &'static str, lch: [f32; 3], p: Palette) -> Element<'a, ThemeChange> {
    let oklch = Oklch(lch[0], lch[1], lch[2]);
    button(text("").size(1))
        .width(Length::Fixed(20.0))
        .height(Length::Fixed(20.0))
        .padding(0)
        .style(move |_t, _s| iced::widget::button::Style {
            background: Some(iced::Background::Color(oklch.color())),
            text_color: p.fg_0,
            border: iced::Border { color: p.line, width: 1.0, radius: 999.0.into() },
            ..Default::default()
        })
        .on_press(ThemeChange::SetPillOverride { tone_key: key.into(), lch })
        .into()
}

fn section_accent<'a>(
    theme: &'a CrabbyTheme,
    saved_colors: &'a [[f32; 3]],
    p: Palette,
) -> Element<'a, ThemeChange> {
    let preview = swatch_box(
        Oklch(theme.accent_l, theme.accent_c, theme.accent_hue),
        72.0,
        true,
        p,
    );

    let hue_slider = slider(0.0..=360.0, theme.accent_hue, ThemeChange::AccentHue)
        .step(1.0)
        .width(Length::Fill);
    let chroma_slider = slider(0.0..=0.30, theme.accent_c, ThemeChange::AccentC)
        .step(0.005)
        .width(Length::Fill);
    let lightness_slider = slider(0.40..=0.85, theme.accent_l, ThemeChange::AccentL)
        .step(0.005)
        .width(Length::Fill);

    let sliders = column![
        labeled_slider("Hue", &format!("{:.0}°", theme.accent_hue), hue_slider, p),
        labeled_slider("Chroma", &format!("{:.3}", theme.accent_c), chroma_slider, p),
        labeled_slider("Lightness", &format!("{:.2}", theme.accent_l), lightness_slider, p),
    ]
    .spacing(10)
    .width(Length::Fill);

    let picker_row = row![preview, sliders]
        .spacing(16)
        .align_y(Alignment::Center);

    let saved_header = row![
        eyebrow("SAVED COLORS", p),
        crate::style::hspace(),
        text(format!("{}/{}", saved_colors.len(), MAX_SAVED_COLORS))
            .size(11)
            .color(p.fg_3),
        button(text("Save current").size(11))
            .padding([3, 10])
            .style(button_style(p, ButtonKind::Default))
            .on_press(ThemeChange::SaveCurrent),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let saved_row = saved_colors_row(saved_colors, p);

    column![
        eyebrow("ACCENT", p),
        picker_row,
        saved_header,
        saved_row,
    ]
    .spacing(10)
    .into()
}

fn section_bg_tint<'a>(theme: &'a CrabbyTheme, p: Palette) -> Element<'a, ThemeChange> {
    let tint_slider = slider(0.0..=360.0, theme.bg_tint_hue, ThemeChange::BgTintHue)
        .step(1.0)
        .width(Length::Fill);
    let preset_buttons = row![
        seg_button("Cool 240°", (theme.bg_tint_hue - 240.0).abs() < 1.0, p, ThemeChange::BgTintHue(240.0)),
        seg_button("Slate 270°", (theme.bg_tint_hue - 270.0).abs() < 1.0, p, ThemeChange::BgTintHue(270.0)),
        seg_button("Neutral 0°", theme.bg_tint_hue.abs() < 1.0, p, ThemeChange::BgTintHue(0.0)),
        seg_button("Warm 60°", (theme.bg_tint_hue - 60.0).abs() < 1.0, p, ThemeChange::BgTintHue(60.0)),
    ]
    .spacing(6);

    column![
        eyebrow("BACKGROUND TINT", p),
        labeled_slider(
            "Hue",
            &format!("{:.0}°", theme.bg_tint_hue),
            tint_slider,
            p,
        ),
        preset_buttons,
        text("Subtle hue mixed into the surface ramp. Effect is most visible on dark mode.")
            .size(11)
            .color(p.fg_3),
    ]
    .spacing(8)
    .into()
}

fn labeled_slider<'a>(
    label: &'a str,
    value_str: &str,
    s: iced::widget::Slider<'a, f32, ThemeChange>,
    p: Palette,
) -> Element<'a, ThemeChange> {
    column![
        row![
            text(label).size(11).color(p.fg_2).width(Length::Fixed(80.0)),
            crate::style::hspace(),
            text(value_str.to_string()).size(11).color(p.fg_1),
        ]
        .align_y(Alignment::Center),
        s,
    ]
    .spacing(4)
    .into()
}

fn seg_button<'a>(label: &'a str, active: bool, p: Palette, msg: ThemeChange) -> Element<'a, ThemeChange> {
    let kind = if active { ButtonKind::Primary } else { ButtonKind::Default };
    button(text(label).size(11))
        .padding([5, 12])
        .style(button_style(p, kind))
        .on_press(msg)
        .into()
}

/// Saved-colors row. Each swatch is a button - click to apply that
/// color into the live accent. A small "×" button next to each removes
/// it. Iced doesn't support overlay/positional badges cleanly, so the
/// [swatch, ×] pair stacks vertically per slot.
fn saved_colors_row<'a>(saved: &'a [[f32; 3]], p: Palette) -> Element<'a, ThemeChange> {
    if saved.is_empty() {
        return text("No saved colors. Pick an accent above and click \"Save current\".")
            .size(11)
            .color(p.fg_3)
            .into();
    }
    let cells: Vec<Element<'a, ThemeChange>> = saved
        .iter()
        .enumerate()
        .map(|(idx, lch)| saved_color_cell(idx, *lch, p))
        .collect();
    iced::widget::Row::with_children(cells)
        .spacing(8)
        .align_y(Alignment::Start)
        .wrap()
        .into()
}

fn saved_color_cell<'a>(idx: usize, lch: [f32; 3], p: Palette) -> Element<'a, ThemeChange> {
    let oklch = Oklch(lch[0], lch[1], lch[2]);
    let swatch = button(text("").size(1))
        .width(Length::Fixed(28.0))
        .height(Length::Fixed(28.0))
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
        .on_press(ThemeChange::ApplySaved(lch));
    let remove = button(text("×").size(11).color(p.fg_3))
        .padding([0, 4])
        .style(button_style(p, ButtonKind::Ghost))
        .on_press(ThemeChange::RemoveSaved(idx));
    column![swatch, remove]
        .spacing(2)
        .align_x(Alignment::Center)
        .into()
}

/// A square swatch box used in the picker preview.
fn swatch_box<'a>(
    color: Oklch,
    size: f32,
    rounded: bool,
    p: Palette,
) -> Element<'a, ThemeChange> {
    let radius: f32 = if rounded { 8.0 } else { 0.0 };
    container(text("").size(1))
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .style(move |_t| iced::widget::container::Style {
            background: Some(iced::Background::Color(color.color())),
            text_color: Some(p.fg_0),
            border: iced::Border {
                color: p.line,
                width: 1.0,
                radius: radius.into(),
            },
            ..Default::default()
        })
        .into()
}
