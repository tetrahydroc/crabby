//! Widget styling helpers that resolve [`Palette`](crate::Palette) into
//! iced widget style closures.
//!
//! iced 0.13 styles widgets per-instance by passing a closure with
//! signature `Fn(&Theme, Status) -> Style`. The helpers here capture
//! the palette by reference and return preconfigured styled widgets,
//! avoiding the need to thread the palette through every closure
//! manually.
//!
//! The pattern is: instead of
//!
//! ```ignore
//! button("hi").style(|t, s| { ... custom logic ... })
//! ```
//!
//! callers write
//!
//! ```ignore
//! style::button(palette, "hi", ButtonKind::Primary)
//! ```

use iced::widget::{button, container, text};
use iced::{Background, Border, Color, Element, Length, Theme};

use crate::theme::Palette;

/// Horizontal flex spacer. iced 0.14 dropped the `horizontal_space()`
/// helper; this is the idiomatic replacement so call sites stay tidy.
pub fn hspace() -> iced::widget::Space {
    iced::widget::Space::new().width(Length::Fill)
}

/// Button size variants matching the design's `.btn`, `.btn.sm`, `.btn.lg`.
/// Heights: 22 / 28 / 34 px. Affects padding + font size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ButtonSize {
    /// 22px tall, font 11.5
    Sm,
    /// 28px tall, font 12.5 - the default
    #[default]
    Md,
    /// 34px tall, font 13 - the launch CTA
    Lg,
}

impl ButtonSize {
    /// Iced padding tuple `[vertical, horizontal]` for this size.
    #[must_use]
    pub fn padding(self) -> [u16; 2] {
        match self {
            Self::Sm => [3, 8],
            Self::Md => [5, 12],
            Self::Lg => [7, 16],
        }
    }

    /// Font size for the button label.
    #[must_use]
    pub fn font_size(self) -> u16 {
        match self {
            Self::Sm => 11,
            Self::Md => 12,
            Self::Lg => 13,
        }
    }

    /// Border radius - `Sm` uses 5, others use 6, matching `tokens.css`.
    #[must_use]
    pub fn radius(self) -> f32 {
        match self {
            Self::Sm => 5.0,
            _ => 6.0,
        }
    }
}

/// Pill tone, matching the design's `.pill`, `.pill.acc`, `.pill.ok`,
/// `.pill.warn`, `.pill.err`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PillTone {
    /// Neutral - bg-3 fill, fg-1 text.
    Neutral,
    /// Accent - accent_soft fill, accent text.
    Accent,
    /// Success.
    Ok,
    /// Warning.
    Warn,
    /// Error.
    Err,
}

/// Button kinds match the design's `.btn` class variants:
/// `default` (neutral), `primary` (accent fill), `ghost` (transparent
/// with hover-tint).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    /// Neutral button - bg-3 fill, line border.
    Default,
    /// Accent fill, used for CTAs (Install, Launch).
    Primary,
    /// Transparent until hovered.
    Ghost,
}

/// Build a styled button that resolves to palette colors.
///
/// `palette` is captured by value so the resulting widget owns no
/// borrow against `App` state - iced widgets must outlive their
/// builder scope.
pub fn button_style(
    palette: Palette,
    kind: ButtonKind,
) -> impl Fn(&Theme, button::Status) -> button::Style {
    button_style_with(palette, kind, ButtonSize::Md)
}

/// Like [`button_style`] but with an explicit size variant for the
/// border radius. Padding/font size are still set on the button
/// builder by the caller (iced styles can't change padding).
pub fn button_style_with(
    palette: Palette,
    kind: ButtonKind,
    size: ButtonSize,
) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let (base_bg, hover_bg, border, text_color) = match kind {
            ButtonKind::Default => (palette.bg_3, palette.bg_4, palette.line, palette.fg_0),
            ButtonKind::Primary => {
                (palette.accent, lighten(palette.accent, 0.06), palette.accent_edge, palette.accent_ink)
            }
            ButtonKind::Ghost => (
                Color::TRANSPARENT,
                palette.bg_3,
                Color::TRANSPARENT,
                palette.fg_1,
            ),
        };
        let bg = match status {
            button::Status::Hovered | button::Status::Pressed => hover_bg,
            _ => base_bg,
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color: match (kind, status) {
                (ButtonKind::Ghost, button::Status::Hovered) => palette.fg_0,
                _ => text_color,
            },
            border: Border {
                color: border,
                width: 1.0,
                radius: size.radius().into(),
            },
            ..Default::default()
        }
    }
}

/// Container backgrounds - used for the tab bar, profile bar, status
/// bar, panel surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceKind {
    /// Window backdrop (deepest).
    Bg0,
    /// Chrome / sidebar (titlebar, tab bar, status bar).
    Bg1,
    /// Main surface (body, panels).
    Bg2,
    /// Elevated row / inputs / pills.
    Bg3,
}

/// Container background style for a given surface tier.
pub fn surface_style(palette: Palette, kind: SurfaceKind) -> impl Fn(&Theme) -> container::Style {
    move |_theme| {
        let bg = match kind {
            SurfaceKind::Bg0 => palette.bg_0,
            SurfaceKind::Bg1 => palette.bg_1,
            SurfaceKind::Bg2 => palette.bg_2,
            SurfaceKind::Bg3 => palette.bg_3,
        };
        container::Style {
            background: Some(Background::Color(bg)),
            text_color: Some(palette.fg_0),
            ..Default::default()
        }
    }
}

/// Bottom-bordered container - for the tab bar / profile bar / status bar
/// where there's a 1px divider underneath the surface.
pub fn band_style(palette: Palette, kind: SurfaceKind) -> impl Fn(&Theme) -> container::Style {
    move |_theme| {
        let bg = match kind {
            SurfaceKind::Bg0 => palette.bg_0,
            SurfaceKind::Bg1 => palette.bg_1,
            SurfaceKind::Bg2 => palette.bg_2,
            SurfaceKind::Bg3 => palette.bg_3,
        };
        container::Style {
            background: Some(Background::Color(bg)),
            text_color: Some(palette.fg_0),
            border: Border {
                color: palette.line_soft,
                width: 0.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        }
    }
}

/// Text color presets - fg-0 (primary), fg-1 (secondary), fg-2
/// (tertiary), fg-3 (placeholder), accent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextTone {
    /// Primary text.
    Fg0,
    /// Secondary text.
    Fg1,
    /// Tertiary / labels.
    Fg2,
    /// Placeholder / dim.
    Fg3,
    /// Accent (links, active tab labels).
    Accent,
    /// Status colors.
    Ok,
    /// Status colors.
    Warn,
    /// Status colors.
    Err,
}

/// Resolve a text-tone token into the palette's `Color`.
#[must_use]
pub fn text_color(palette: Palette, tone: TextTone) -> Color {
    match tone {
        TextTone::Fg0 => palette.fg_0,
        TextTone::Fg1 => palette.fg_1,
        TextTone::Fg2 => palette.fg_2,
        TextTone::Fg3 => palette.fg_3,
        TextTone::Accent => palette.accent,
        TextTone::Ok => palette.ok,
        TextTone::Warn => palette.warn,
        TextTone::Err => palette.err,
    }
}

/// `text(...)` style closure for a given tone.
pub fn text_style(palette: Palette, tone: TextTone) -> impl Fn(&Theme) -> text::Style {
    move |_theme| text::Style {
        color: Some(text_color(palette, tone)),
    }
}

/// Lighten a color by `amount` in [0, 1]. Used by the primary-button
/// hover state to mirror the CSS `filter: brightness(1.06)` from the
/// design.
fn lighten(c: Color, amount: f32) -> Color {
    Color {
        r: (c.r + amount).min(1.0),
        g: (c.g + amount).min(1.0),
        b: (c.b + amount).min(1.0),
        a: c.a,
    }
}

// ---- design-system primitives ----

/// Render a status pill - `[ • Healthy ]`-style chip with a colored dot.
/// Mirrors the design's `.pill` class with tone variants.
pub fn pill<'a, Msg: 'a>(palette: Palette, label: &'a str, tone: PillTone) -> Element<'a, Msg> {
    use iced::widget::{container, row, text};
    use iced::Alignment;
    let (bg, fg, border) = match tone {
        PillTone::Neutral => (palette.bg_3, palette.fg_1, palette.line_soft),
        PillTone::Accent => (palette.accent_soft, palette.accent, palette.accent_edge),
        PillTone::Ok => (with_alpha(palette.ok, 0.14), palette.ok, with_alpha(palette.ok, 0.4)),
        PillTone::Warn => (with_alpha(palette.warn, 0.14), palette.warn, with_alpha(palette.warn, 0.4)),
        PillTone::Err => (with_alpha(palette.err, 0.14), palette.err, with_alpha(palette.err, 0.4)),
    };
    let dot = container(text(""))
        .width(Length::Fixed(6.0))
        .height(Length::Fixed(6.0))
        .style(move |_t| container::Style {
            background: Some(Background::Color(fg)),
            border: Border { color: fg, width: 0.0, radius: 999.0.into() },
            ..Default::default()
        });
    container(
        row![
            dot,
            text(label.to_string()).size(11).color(fg),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .padding([2, 8])
    .style(move |_t| container::Style {
        background: Some(Background::Color(bg)),
        text_color: Some(fg),
        border: Border { color: border, width: 1.0, radius: 999.0.into() },
        ..Default::default()
    })
    .into()
}

/// Eyebrow text - small uppercase tracker label used above sections.
/// Matches the design's `.h-eyebrow` class.
pub fn eyebrow<'a, Msg: 'a>(palette: Palette, label: &'a str) -> Element<'a, Msg> {
    use iced::widget::text;
    text(label.to_uppercase())
        .size(11)
        .color(palette.fg_2)
        .into()
}

/// Thin divider line - 1px high, line_soft color.
pub fn divider<'a, Msg: 'a>(palette: Palette) -> Element<'a, Msg> {
    use iced::widget::container;
    container(iced::widget::Space::new().height(Length::Fixed(1.0)))
        .width(Length::Fill)
        .style(move |_t| container::Style {
            background: Some(Background::Color(palette.line_soft)),
            ..Default::default()
        })
        .into()
}

/// Patterned placeholder thumbnail - 45° hatched fill with a label
/// inside. Used in mod-detail hero where there's no real screenshot.
/// Iced doesn't have an SVG-pattern fill so this approximates with a
/// solid bg-3 + a centered mono caption.
pub fn thumb<'a, Msg: 'a>(palette: Palette, label: &'a str, w: f32, h: f32) -> Element<'a, Msg> {
    use iced::widget::{container, text};
    // The label sits inside a fixed-size box. center_x/center_y on a
    // container expand it to its parent's available space, which is
    // *not* wanted for a fixed-dimension thumbnail.
    container(text(label).size(10).color(palette.fg_3))
        .width(Length::Fixed(w))
        .height(Length::Fixed(h))
        .padding(8)
        .style(move |_t| container::Style {
            background: Some(Background::Color(palette.bg_3)),
            border: Border { color: palette.line_soft, width: 1.0, radius: 5.0.into() },
            ..Default::default()
        })
        .into()
}

/// Filter-chip button. Renders as a small pill with label + count.
/// `tone` colors the active count + active border. The widget owns
/// no message handler - the caller chains `.on_press(...)`.
pub fn filter_chip_style(
    palette: Palette,
    active: bool,
    tone: PillTone,
) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, _status| {
        let (bg, border, text_color) = if active {
            (palette.accent_soft, palette.accent_edge, palette.accent)
        } else {
            let tone_color = match tone {
                PillTone::Accent => palette.accent,
                PillTone::Ok => palette.ok,
                PillTone::Warn => palette.warn,
                PillTone::Err => palette.err,
                PillTone::Neutral => palette.fg_1,
            };
            (palette.bg_3, palette.line_soft, tone_color)
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color,
            border: Border { color: border, width: 1.0, radius: 999.0.into() },
            ..Default::default()
        }
    }
}

/// Inline-link button style - accent-colored text on a transparent
/// background, no border. Hovering tints with `accent_soft` so the
/// hit target reads as interactive without being heavy.
pub fn link_button_style(palette: Palette) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let bg = match status {
            button::Status::Hovered | button::Status::Pressed => palette.accent_soft,
            _ => Color::TRANSPARENT,
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color: palette.accent,
            border: Border {
                color: Color::TRANSPARENT,
                width: 0.0,
                radius: 4.0.into(),
            },
            ..Default::default()
        }
    }
}

/// Toggle-switch container background style - the bg track with a
/// rounded pill shape. Inner thumb is rendered separately on top.
/// Returns the appropriate background for `on`/`off` state.
#[must_use]
pub fn toggle_track_color(palette: Palette, on: bool) -> Color {
    if on { palette.accent } else { palette.bg_4 }
}

/// Foreground (thumb) color for the toggle switch.
#[must_use]
pub fn toggle_thumb_color(palette: Palette, on: bool) -> Color {
    if on { palette.accent_ink } else { palette.fg_1 }
}

fn with_alpha(c: Color, a: f32) -> Color {
    Color { a, ..c }
}
