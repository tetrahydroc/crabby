//!
//! Math reference: <https://bottosson.github.io/posts/oklab/>. The
//! `oklch_to_rgb` function below mirrors the CSS Color 4 spec
//! conversions: oklch → oklab → linear sRGB → gamma-encoded sRGB.

use iced::Color;

/// (Lightness, Chroma, Hue-degrees). Hue is in degrees [0, 360).
#[derive(Debug, Clone, Copy)]
pub struct Oklch(pub f32, pub f32, pub f32);

impl Oklch {
    /// Build a [`Color`] with full alpha.
    #[must_use]
    pub fn color(self) -> Color {
        oklch_to_rgb(self.0, self.1, self.2, 1.0)
    }

    /// Build a [`Color`] with the supplied alpha.
    #[must_use]
    pub fn color_alpha(self, alpha: f32) -> Color {
        oklch_to_rgb(self.0, self.1, self.2, alpha)
    }
}

/// Convert oklch coordinates to gamma-encoded sRGB. `h_deg` is in
/// degrees. Output channels are clamped to `[0, 1]` - out-of-gamut
/// values just clip rather than fail.
#[must_use]
pub fn oklch_to_rgb(l: f32, c: f32, h_deg: f32, alpha: f32) -> Color {
    // oklch → oklab
    let h_rad = h_deg.to_radians();
    let a = c * h_rad.cos();
    let b = c * h_rad.sin();

    // oklab → linear sRGB
    let l_ = l + 0.396_337_78 * a + 0.215_803_76 * b;
    let m_ = l - 0.105_561_346 * a - 0.063_854_17 * b;
    let s_ = l - 0.089_484_18 * a - 1.291_485_5 * b;

    let l_ = l_ * l_ * l_;
    let m_ = m_ * m_ * m_;
    let s_ = s_ * s_ * s_;

    let r_lin = 4.076_741_7 * l_ - 3.307_711_6 * m_ + 0.230_969_94 * s_;
    let g_lin = -1.268_438 * l_ + 2.609_757_4 * m_ - 0.341_319_38 * s_;
    let b_lin = -0.004_196_086 * l_ - 0.703_418_6 * m_ + 1.707_614_7 * s_;

    // linear → gamma sRGB (CSS uses the standard sRGB transfer)
    fn gamma(x: f32) -> f32 {
        if x <= 0.003_130_8 {
            12.92 * x
        } else {
            1.055 * x.powf(1.0 / 2.4) - 0.055
        }
    }
    Color {
        r: gamma(r_lin).clamp(0.0, 1.0),
        g: gamma(g_lin).clamp(0.0, 1.0),
        b: gamma(b_lin).clamp(0.0, 1.0),
        a: alpha,
    }
}

/// Light/dark mode discriminator. Drives which background + foreground
/// ramp is active. Accent + status hues are mode-agnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Dark mode (default - matches the design mockup's primary state).
    #[default]
    Dark,
    /// Light mode.
    Light,
}

/// All resolved colors for a given (mode, accent_hue, bg_tint_hue).
///
/// Built once per palette change and threaded through the view tree.
/// Constructing a `Palette` is cheap (just N oklch→rgb conversions)
/// so it's rebuilt on every theme-state change rather than cached.
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    /// Window backdrop (deepest).
    pub bg_0: Color,
    /// Chrome / sidebar.
    pub bg_1: Color,
    /// Main surface.
    pub bg_2: Color,
    /// Elevated row / inputs.
    pub bg_3: Color,
    /// Hover / active.
    pub bg_4: Color,
    /// Strong divider.
    pub line: Color,
    /// Subtle divider.
    pub line_soft: Color,

    /// Primary text.
    pub fg_0: Color,
    /// Secondary text.
    pub fg_1: Color,
    /// Tertiary / labels.
    pub fg_2: Color,
    /// Placeholder / dim.
    pub fg_3: Color,

    /// Accent (used for selection / focus / CTA).
    pub accent: Color,
    /// Accent at 14% alpha (selected-row tint).
    pub accent_soft: Color,
    /// Accent at 45% alpha (focus ring).
    pub accent_edge: Color,
    /// Foreground used on top of solid accent fill (primary buttons).
    pub accent_ink: Color,

    /// Success / healthy.
    pub ok: Color,
    /// Warning.
    pub warn: Color,
    /// Error.
    pub err: Color,
}

/// Optional per-tone color overrides applied at palette build time.
/// `None` means "use the design default for this tone." Accent isn't
/// here - it's already controlled by the accent picker.
#[derive(Debug, Clone, Copy, Default)]
pub struct PillOverrides {
    /// Override the success/installed tone.
    pub ok: Option<Oklch>,
    /// Override the warning tone.
    pub warn: Option<Oklch>,
    /// Override the error tone.
    pub err: Option<Oklch>,
}

impl PillOverrides {
    /// Hydrate from the persisted `pill_overrides` map. Unknown keys
    /// are ignored (forward-compatible with future tones).
    #[must_use]
    pub fn from_prefs(map: &std::collections::BTreeMap<String, [f32; 3]>) -> Self {
        let pick = |key: &str| map.get(key).map(|lch| Oklch(lch[0], lch[1], lch[2]));
        Self {
            ok: pick("ok"),
            warn: pick("warn"),
            err: pick("err"),
        }
    }
}

impl Palette {
    /// Build the resolved palette from the dynamic axes, using the
    /// design's canonical accent L=0.72, C=0.12. For full control over
    /// the accent (L/C/H), use [`Palette::with_accent`].
    ///
    /// `accent_hue`: degrees, ~220 for the design's default cyan-teal.
    /// `bg_tint_hue`: degrees, ~240 for the cool neutral default.
    #[must_use]
    pub fn new(mode: Mode, accent_hue: f32, bg_tint_hue: f32) -> Self {
        Self::with_accent(mode, Oklch(0.72, 0.12, accent_hue), bg_tint_hue)
    }

    /// Build the resolved palette with full control over the accent's
    /// OKLCH triple. Equivalent to [`Palette::with_overrides`] called
    /// with no pill overrides.
    #[must_use]
    pub fn with_accent(mode: Mode, accent: Oklch, bg_tint_hue: f32) -> Self {
        Self::with_overrides(mode, accent, bg_tint_hue, &PillOverrides::default())
    }

    /// Build the resolved palette with full control over the accent's
    /// OKLCH triple AND optional per-tone pill overrides. Each Some
    /// override replaces the built-in tone color (Ok=green, Warn=amber,
    /// Err=red, with `fg_1` for Neutral); None falls back to the design
    /// default. Accent isn't here - it's already driven by `accent`.
    #[must_use]
    pub fn with_overrides(
        mode: Mode,
        accent: Oklch,
        bg_tint_hue: f32,
        overrides: &PillOverrides,
    ) -> Self {
        let (bg_0, bg_1, bg_2, bg_3, bg_4, line, line_soft, fg_0, fg_1, fg_2, fg_3) = match mode {
            Mode::Dark => (
                Oklch(0.180, 0.008, bg_tint_hue),
                Oklch(0.210, 0.008, bg_tint_hue),
                Oklch(0.235, 0.009, bg_tint_hue),
                Oklch(0.270, 0.010, bg_tint_hue),
                Oklch(0.310, 0.010, bg_tint_hue),
                // line / line_soft are oklch values with alpha; the
                // resolver multiplies into the alpha channel so the
                // surface underneath shows through.
                Oklch(0.340, 0.010, bg_tint_hue),
                Oklch(0.340, 0.010, bg_tint_hue),
                Oklch(0.970, 0.005, bg_tint_hue),
                Oklch(0.780, 0.008, bg_tint_hue),
                Oklch(0.600, 0.010, bg_tint_hue),
                Oklch(0.460, 0.010, bg_tint_hue),
            ),
            Mode::Light => (
                Oklch(0.965, 0.004, bg_tint_hue),
                Oklch(0.985, 0.003, bg_tint_hue),
                Oklch(1.000, 0.000, bg_tint_hue),
                Oklch(0.975, 0.004, bg_tint_hue),
                Oklch(0.945, 0.006, bg_tint_hue),
                Oklch(0.860, 0.006, bg_tint_hue),
                Oklch(0.920, 0.005, bg_tint_hue),
                Oklch(0.180, 0.010, bg_tint_hue),
                Oklch(0.360, 0.010, bg_tint_hue),
                Oklch(0.520, 0.010, bg_tint_hue),
                Oklch(0.660, 0.008, bg_tint_hue),
            ),
        };

        let (line_a, line_soft_a) = match mode {
            Mode::Dark => (0.7_f32, 0.4_f32),
            Mode::Light => (1.0, 1.0),
        };

        Self {
            bg_0: bg_0.color(),
            bg_1: bg_1.color(),
            bg_2: bg_2.color(),
            bg_3: bg_3.color(),
            bg_4: bg_4.color(),
            line: line.color_alpha(line_a),
            line_soft: line_soft.color_alpha(line_soft_a),
            fg_0: fg_0.color(),
            fg_1: fg_1.color(),
            fg_2: fg_2.color(),
            fg_3: fg_3.color(),
            accent: accent.color(),
            accent_soft: accent.color_alpha(0.14),
            accent_edge: accent.color_alpha(0.45),
            // Ink color - fixed near-black with a slight blue cast,
            // matches the design's `--acc-ink` token.
            accent_ink: Oklch(0.18, 0.02, 240.0).color(),
            ok: overrides.ok.unwrap_or(Oklch(0.74, 0.11, 155.0)).color(),
            warn: overrides.warn.unwrap_or(Oklch(0.78, 0.13, 75.0)).color(),
            err: overrides.err.unwrap_or(Oklch(0.66, 0.17, 25.0)).color(),
        }
    }

    /// Convenience - the design's default palette (dark, accent=220, tint=240).
    #[must_use]
    pub fn default_dark() -> Self {
        Self::new(Mode::Dark, 220.0, 240.0)
    }
}

/// Bundled theme state - palette + axes. Threaded through the view
/// tree so widgets can resolve any token without re-deriving.
#[derive(Debug, Clone, Copy)]
pub struct CrabbyTheme {
    /// Resolved color palette for the current axes.
    pub palette: Palette,
    /// Light/dark discriminator. Some places switch behavior beyond
    /// just colors (e.g. the "traffic light" dim style on light mode).
    pub mode: Mode,
    /// Accent OKLCH lightness [0.40, 0.85] practical range.
    pub accent_l: f32,
    /// Accent OKLCH chroma [0.0, 0.30] practical range.
    pub accent_c: f32,
    /// Current accent hue in degrees [0, 360).
    pub accent_hue: f32,
    /// Current background-tint hue in degrees [0, 360).
    pub bg_tint_hue: f32,
}

impl Default for CrabbyTheme {
    fn default() -> Self {
        let mode = Mode::default();
        let accent_l = 0.72_f32;
        let accent_c = 0.12_f32;
        let accent_hue = 220.0_f32;
        let bg_tint_hue = 240.0_f32;
        Self {
            palette: Palette::with_accent(mode, Oklch(accent_l, accent_c, accent_hue), bg_tint_hue),
            mode,
            accent_l,
            accent_c,
            accent_hue,
            bg_tint_hue,
        }
    }
}

impl CrabbyTheme {
    /// Rebuild the palette after one of the dynamic axes changed.
    /// `overrides` carries per-tone pill overrides; pass an empty
    /// `PillOverrides::default()` when there are none.
    pub fn refresh(&mut self, overrides: &PillOverrides) {
        self.palette = Palette::with_overrides(
            self.mode,
            Oklch(self.accent_l, self.accent_c, self.accent_hue),
            self.bg_tint_hue,
            overrides,
        );
    }

    /// Toggle between dark and light. Caller must follow up with
    /// [`refresh`](Self::refresh) to rebuild the palette.
    pub fn toggle_mode(&mut self) {
        self.mode = match self.mode {
            Mode::Dark => Mode::Light,
            Mode::Light => Mode::Dark,
        };
    }

    /// Hydrate from persisted prefs. Tolerates missing or
    /// out-of-range fields by clamping to safe values.
    #[must_use]
    pub fn from_prefs(prefs: &crate::launcher_config::ThemePrefs) -> Self {
        let mode = match prefs.mode.as_str() {
            "light" => Mode::Light,
            _ => Mode::Dark,
        };
        let accent_l = prefs.accent_l.clamp(0.0, 1.0);
        let accent_c = prefs.accent_c.clamp(0.0, 0.4);
        let accent_hue = prefs.accent_h.rem_euclid(360.0);
        let bg_tint_hue = prefs.bg_tint_h.rem_euclid(360.0);
        let overrides = PillOverrides::from_prefs(&prefs.pill_overrides);
        Self {
            palette: Palette::with_overrides(
                mode,
                Oklch(accent_l, accent_c, accent_hue),
                bg_tint_hue,
                &overrides,
            ),
            mode,
            accent_l,
            accent_c,
            accent_hue,
            bg_tint_hue,
        }
    }

    /// Snapshot back to the persisted shape. Saved colors and pill
    /// overrides are owned by the launcher config (they outlive any
    /// single CrabbyTheme), so the caller copies them in after this
    /// returns.
    #[must_use]
    pub fn to_prefs(
        &self,
        saved_colors: Vec<[f32; 3]>,
        pill_overrides: std::collections::BTreeMap<String, [f32; 3]>,
    ) -> crate::launcher_config::ThemePrefs {
        crate::launcher_config::ThemePrefs {
            mode: match self.mode {
                Mode::Dark => "dark".into(),
                Mode::Light => "light".into(),
            },
            accent_l: self.accent_l,
            accent_c: self.accent_c,
            accent_h: self.accent_hue,
            bg_tint_h: self.bg_tint_hue,
            saved_colors,
            pill_overrides,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// White (oklch L=1, C=0) round-trips to white in sRGB.
    #[test]
    fn oklch_white() {
        let c = oklch_to_rgb(1.0, 0.0, 0.0, 1.0);
        assert!((c.r - 1.0).abs() < 0.01, "{c:?}");
        assert!((c.g - 1.0).abs() < 0.01);
        assert!((c.b - 1.0).abs() < 0.01);
    }

    /// Black (oklch L=0) round-trips to black.
    #[test]
    fn oklch_black() {
        let c = oklch_to_rgb(0.0, 0.0, 0.0, 1.0);
        assert!(c.r.abs() < 0.01);
        assert!(c.g.abs() < 0.01);
        assert!(c.b.abs() < 0.01);
    }

    /// Sanity: the design's accent (oklch(0.72 0.12 220)) is a
    /// perceptibly cyan-teal. Doesn't have to match a specific RGB but
    /// should pass the smell test (G > R, B > R, neither saturated).
    #[test]
    fn accent_is_teal_ish() {
        let c = Oklch(0.72, 0.12, 220.0).color();
        assert!(c.g > c.r, "expected greener than red, got {c:?}");
        assert!(c.b > c.r, "expected bluer than red");
        assert!(c.g < 1.0 && c.b < 1.0, "shouldn't be saturated");
    }

    /// Dark default palette has bg_0 dark and fg_0 light (i.e. they
    /// don't accidentally invert).
    #[test]
    fn dark_palette_has_correct_polarity() {
        let p = Palette::default_dark();
        let bg_avg = (p.bg_0.r + p.bg_0.g + p.bg_0.b) / 3.0;
        let fg_avg = (p.fg_0.r + p.fg_0.g + p.fg_0.b) / 3.0;
        assert!(bg_avg < 0.3, "dark bg should be dim, got {bg_avg}");
        assert!(fg_avg > 0.85, "dark fg should be bright, got {fg_avg}");
    }

    /// Light palette inverts the polarity.
    #[test]
    fn light_palette_has_correct_polarity() {
        let p = Palette::new(Mode::Light, 220.0, 240.0);
        let bg_avg = (p.bg_0.r + p.bg_0.g + p.bg_0.b) / 3.0;
        let fg_avg = (p.fg_0.r + p.fg_0.g + p.fg_0.b) / 3.0;
        assert!(bg_avg > 0.85, "light bg should be near-white, got {bg_avg}");
        assert!(fg_avg < 0.3, "light fg should be near-black, got {fg_avg}");
    }

    #[test]
    fn theme_toggle_inverts_polarity() {
        let mut t = CrabbyTheme::default();
        let dark_bg = t.palette.bg_0;
        t.toggle_mode();
        t.refresh(&PillOverrides::default());
        let light_bg = t.palette.bg_0;
        assert_ne!(dark_bg.r, light_bg.r);
    }

    #[test]
    fn pill_override_applies_to_palette() {
        let mut prefs = crate::launcher_config::ThemePrefs::default();
        // Replace the green Ok tone with a fully red one.
        prefs.pill_overrides.insert("ok".into(), [0.66, 0.17, 25.0]);
        let theme = CrabbyTheme::from_prefs(&prefs);
        // Resolved `ok` should be red-leaning, not green.
        assert!(
            theme.palette.ok.r > theme.palette.ok.g,
            "{:?}",
            theme.palette.ok
        );
    }
}
