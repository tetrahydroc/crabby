//! Reader/writer for Mod Configuration Menu (MCM) config files.
//!
//! MCM stores per-mod settings under `user://MCM/<modFolder>/config.ini`
//! (where `user://` is Godot's user-data dir, mapped per-platform; see
//! [`user_data_dir`]). Each file is a Godot ConfigFile with one section
//! per value type (`Bool`, `Int`, `Slider`, `String`, `Dropdown`,
//! `Color`, `Keycode`) and dict values shaped like:
//!
//! ```text
//! [Bool]
//! adsZoom={
//! &"default": true,
//! &"name": "ADS Zoom - Enabled",
//! &"tooltip": "...",
//! &"value": true
//! }
//! ```
//!
//! The `&` prefix on most keys is Godot's StringName syntax. Some keys
//! (`category`, `on_value_changed`) come without it because the mod
//! authors typed them as plain strings. The reader tolerates both.
//!
//! # Why a hand-rolled parser
//!
//! Godot's ConfigFile has loose typing (Vector2, PackedStringArray,
//! StringName, etc.) that doesn't need full modelling here, just enough
//! to render a UI and write back single-key updates without disturbing
//! anything else. The writer round-trips by re-rendering the entire
//! file, so unsupported types in untouched fields will be lost on
//! save. That's a known limitation; v2 would preserve unparsed bytes.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use crabby_error::{CrabbyError, Result};

/// One parsed value from a section. The `kind` determines how the UI
/// renders + edits it; `extras` carries kind-specific data (options
/// for dropdowns, range for ints/sliders).
#[derive(Debug, Clone, PartialEq)]
pub struct McmField {
    /// Section name (`Bool`, `Int`, `Slider`, `String`, `Dropdown`, `Color`, `Keycode`).
    pub section: String,
    /// Key within the section (e.g. `adsZoom`).
    pub key: String,
    /// Human-readable label for the UI (`name` field).
    pub name: String,
    /// Tooltip / description.
    pub tooltip: String,
    /// Optional category for grouping (some mods use `category`, others omit it).
    pub category: Option<String>,
    /// Display order within the section. `0` when missing.
    pub menu_pos: i64,
    /// Current value.
    pub value: McmValue,
    /// Default value, for "reset to default" UI.
    pub default: McmValue,
    /// Kind-specific extras (range, options).
    pub extras: McmExtras,
    /// Raw `key: value` tokens not modelled here, emitted verbatim
    /// during write-back so MCM-only fields like `on_value_changed`
    /// survive a round-trip.
    pub passthrough: Vec<(String, String)>,
}

/// A typed value. Bool/Int/Float/String cover the bulk; dropdowns
/// are modelled as Int (the index) so a separate enum kind isn't needed.
#[derive(Debug, Clone, PartialEq)]
pub enum McmValue {
    /// Boolean.
    Bool(bool),
    /// Integer (covers Int, Slider, Dropdown index, Keycode).
    Int(i64),
    /// Floating-point (some Slider entries store as float, e.g. 40.0).
    Float(f64),
    /// String (covers String, Color hex codes).
    Str(String),
}

/// Kind-specific extra fields that the UI uses to constrain the editor.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct McmExtras {
    /// Inclusive minimum for Int/Slider sections.
    pub min_range: Option<f64>,
    /// Inclusive maximum for Int/Slider sections.
    pub max_range: Option<f64>,
    /// Step size for Slider sections.
    pub step: Option<f64>,
    /// Choice labels for Dropdown sections (index = current `value`).
    pub options: Vec<String>,
}

/// A whole MCM config file, parsed.
#[derive(Debug, Clone, Default)]
pub struct McmConfig {
    /// Path the file was loaded from. Used for write-back.
    pub path: PathBuf,
    /// Fields in the order they appear on disk. Sorted by
    /// `(category, menu_pos, key)` for display, but persisted in source
    /// order so re-saving doesn't reflow hand-edits.
    pub fields: Vec<McmField>,
    /// Stash of raw lines not recognized (inside section bodies).
    /// Re-emitted verbatim so unsupported types survive a round-trip
    /// through the writer.
    raw_unknown: Vec<(String, String, String)>,
}

impl McmConfig {
    /// Load and parse the MCM file at `path`. Returns `Ok(None)` when
    /// the file doesn't exist (mod just hasn't booted yet, or doesn't
    /// use MCM).
    pub fn load(path: &Path) -> Result<Option<Self>> {
        if !path.is_file() {
            return Ok(None);
        }
        let text =
            fs::read_to_string(path).map_err(|s| CrabbyError::io_at(path.to_path_buf(), s))?;
        let mut cfg = parse(&text).map_err(|source| CrabbyError::Config {
            context: format!("parsing MCM config at {}", path.display()),
            source,
        })?;
        cfg.path = path.to_path_buf();
        Ok(Some(cfg))
    }

    /// Find a field by section + key. `None` when not present.
    #[must_use]
    pub fn find(&self, section: &str, key: &str) -> Option<&McmField> {
        self.fields
            .iter()
            .find(|f| f.section == section && f.key == key)
    }

    /// Update a field's value in-place and write the entire file.
    /// No-op + error if the field isn't present.
    pub fn set_value(&mut self, section: &str, key: &str, value: McmValue) -> Result<()> {
        let Some(field) = self
            .fields
            .iter_mut()
            .find(|f| f.section == section && f.key == key)
        else {
            return Err(CrabbyError::Config {
                context: format!("MCM: no field {section}/{key} in {}", self.path.display()),
                source: "unknown field".into(),
            });
        };
        field.value = value;
        let rendered = render(self);
        fs::write(&self.path, rendered).map_err(|s| CrabbyError::io_at(self.path.clone(), s))?;
        Ok(())
    }

    /// Group fields by category for display. Fields without a category
    /// land in the synthetic `""` bucket. Ordering inside each bucket
    /// is by `menu_pos` then `key`.
    #[must_use]
    pub fn fields_by_category(&self) -> BTreeMap<String, Vec<&McmField>> {
        let mut out: BTreeMap<String, Vec<&McmField>> = BTreeMap::new();
        for f in &self.fields {
            out.entry(f.category.clone().unwrap_or_default())
                .or_default()
                .push(f);
        }
        for v in out.values_mut() {
            v.sort_by(|a, b| a.menu_pos.cmp(&b.menu_pos).then_with(|| a.key.cmp(&b.key)));
        }
        out
    }
}

/// Resolve Godot's `user://` directory for Road to Vostok on the host
/// platform. Mirror of Godot's logic in
/// `core/io/dir_access_unix.cpp` / `dir_access_windows.cpp`:
///
/// - Windows: `%APPDATA%\Godot\app_userdata\<game>`, but RTV ships
///   with a custom application name in `project.godot`, so the dir is
///   `%APPDATA%\Road to Vostok` (no `Godot\app_userdata` prefix).
/// - Linux: `~/.local/share/Road to Vostok/` (XDG_DATA_HOME or fallback).
/// - macOS: `~/Library/Application Support/Road to Vostok/`.
///
/// Empirically RTV uses the bare-app-name layout on every platform
/// tested; if a future Godot version changes the default, a fallback
/// chain will be needed.
#[must_use]
pub fn user_data_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("APPDATA").map(|p| PathBuf::from(p).join("Road to Vostok"))
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
            return Some(PathBuf::from(xdg).join("Road to Vostok"));
        }
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share/Road to Vostok"))
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join("Library/Application Support/Road to Vostok"))
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

/// Resolve `user://MCM/`. `None` when [`user_data_dir`] couldn't.
#[must_use]
pub fn mcm_root() -> Option<PathBuf> {
    user_data_dir().map(|d| d.join("MCM"))
}

/// One-shot normalization pass over every MCM config on disk. Loads
/// each, lets the parser coerce `[Int]` Float values to Int, and
/// re-saves. Idempotent: a clean file round-trips byte-equivalent
/// (the renderer's `section_wants_int` writes int form for already-
/// int values too). Cheap, typical user has < 10 MCM configs.
///
/// Returns `(rewrote_count, error_count)` for the launcher to log.
/// Errors per-file don't block the rest.
///
/// Why: the in-game MCM slider widget always emits Float for `[Int]`
/// sections. Mods like Faction Warfare have `func _spawn_pool() -> int`
/// math that breaks when their Resource property gets assigned a
/// Float. We rewrite the file once to fix it without making the user
/// click through every MCM panel.
pub fn normalize_all_mcm_configs() -> (usize, usize) {
    let mut rewrote = 0usize;
    let mut errors = 0usize;
    for (_name, path) in list_mcm_configs() {
        match McmConfig::load(&path) {
            Ok(Some(cfg)) => {
                // Re-render unconditionally; the renderer handles
                // section-aware coercion. If on-disk bytes already
                // match, the write is a wasted IO but not harmful.
                let rendered = render(&cfg);
                if let Err(e) = fs::write(&path, rendered) {
                    tracing::warn!(path = %path.display(), error = %e, "mcm: normalize write failed");
                    errors += 1;
                } else {
                    rewrote += 1;
                }
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "mcm: normalize load failed");
                errors += 1;
            }
        }
    }
    (rewrote, errors)
}

/// Enumerate every `<mcm_root>/<modFolder>/config.ini` on disk. Returns
/// `(folder_name, full_path)` pairs.
pub fn list_mcm_configs() -> Vec<(String, PathBuf)> {
    let Some(root) = mcm_root() else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let cfg = path.join("config.ini");
        if cfg.is_file() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                out.push((name.to_string(), cfg));
            }
        }
    }
    out
}

/// Try to associate a mod's id+display-name with an MCM folder. Returns
/// the path of `<mcm_root>/<best-match>/config.ini` or `None`.
///
/// Strategy: case-insensitive match on the slugified mod folder name
/// against the slugified mod id and display name. We try id first, then
/// name, then fall back to substring containment in either direction.
#[must_use]
pub fn find_config_for_mod(mod_id: &str, mod_name: &str) -> Option<PathBuf> {
    let candidates = list_mcm_configs();
    if candidates.is_empty() {
        return None;
    }

    let id_slug = slug(mod_id);
    let name_slug = slug(mod_name);

    // Exact slug match on id.
    for (folder, path) in &candidates {
        if slug(folder) == id_slug {
            return Some(path.clone());
        }
    }
    // Exact slug match on display name.
    for (folder, path) in &candidates {
        if slug(folder) == name_slug {
            return Some(path.clone());
        }
    }
    // Substring fallback, last resort, only if unique.
    let mut substring_hits: Vec<&PathBuf> = Vec::new();
    for (folder, path) in &candidates {
        let f = slug(folder);
        if f.contains(&id_slug)
            || id_slug.contains(&f)
            || f.contains(&name_slug)
            || name_slug.contains(&f)
        {
            substring_hits.push(path);
        }
    }
    if substring_hits.len() == 1 {
        return Some(substring_hits[0].clone());
    }
    None
}

/// Lowercase + alphanum-only. Strips dashes, underscores, spaces, etc.
fn slug(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

// ---- parser / writer ----

fn parse(text: &str) -> std::result::Result<McmConfig, Box<dyn std::error::Error + Send + Sync>> {
    let mut out = McmConfig::default();
    let mut current_section: Option<String> = None;
    let mut iter = text.lines().enumerate().peekable();

    while let Some((lineno, raw)) = iter.next() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if let Some(header) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            current_section = Some(header.trim().to_string());
            continue;
        }
        let Some(section) = &current_section else {
            return Err(format!("line {}: key/value before any section header", lineno + 1).into());
        };
        // `key={` opens a multi-line dict; collect until the matching `}`.
        if let Some((key, after_eq)) = line.split_once('=') {
            let key = key.trim().to_string();
            let mut body = after_eq.trim().to_string();
            if !body.starts_with('{') {
                // Bare scalar value (some MCM entries may use this, not common).
                out.raw_unknown
                    .push((section.clone(), key, format!("{body}")));
                continue;
            }
            // Accumulate until `}` appears at the start of a stripped line.
            while !body.trim_end().ends_with('}') {
                let Some((_, more)) = iter.next() else {
                    return Err(format!("line {}: unterminated dict", lineno + 1).into());
                };
                body.push('\n');
                body.push_str(more);
            }
            match parse_dict(&body, lineno) {
                Ok(map) => {
                    if let Some(field) = field_from_dict(section, &key, &map) {
                        out.fields.push(field);
                    } else {
                        out.raw_unknown.push((section.clone(), key, body));
                    }
                }
                Err(e) => {
                    return Err(format!("line {}: dict parse failed: {e}", lineno + 1).into());
                }
            }
        }
    }
    Ok(out)
}

/// Parse the body of a Godot inline dict (`{ &"k": v, "k": v, ... }`)
/// into a `BTreeMap<String, &str>` of raw value tokens. Strips the
/// outer braces and tolerates the StringName `&"..."` prefix on keys.
fn parse_dict(
    body: &str,
    lineno: usize,
) -> std::result::Result<BTreeMap<String, String>, Box<dyn std::error::Error + Send + Sync>> {
    let trimmed = body.trim();
    let inner = trimmed
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .ok_or_else(|| format!("line {}: dict not braced", lineno + 1))?;
    let mut out = BTreeMap::new();
    for raw_pair in split_top_level_commas(inner) {
        let pair = raw_pair.trim();
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair
            .split_once(':')
            .ok_or_else(|| format!("line {}: dict entry missing `:` ({pair:?})", lineno + 1))?;
        let mut k = k.trim();
        if let Some(rest) = k.strip_prefix('&') {
            k = rest;
        }
        let key = k.trim().trim_matches('"').to_string();
        out.insert(key, v.trim().to_string());
    }
    Ok(out)
}

fn split_top_level_commas(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut in_string = false;
    let mut depth: i32 = 0;
    let bytes = s.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'"' if !escaped_at(bytes, i) => in_string = !in_string,
            b'[' | b'(' | b'{' if !in_string => depth += 1,
            b']' | b')' | b'}' if !in_string => depth -= 1,
            b',' if !in_string && depth == 0 => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

fn escaped_at(bytes: &[u8], i: usize) -> bool {
    let mut count = 0;
    let mut j = i;
    while j > 0 && bytes[j - 1] == b'\\' {
        count += 1;
        j -= 1;
    }
    count % 2 == 1
}

fn field_from_dict(section: &str, key: &str, map: &BTreeMap<String, String>) -> Option<McmField> {
    let value_tok = map.get("value")?;
    let default_tok = map.get("default")?;
    let value = coerce_for_section(parse_value(value_tok)?, section);
    let default = coerce_for_section(parse_value(default_tok)?, section);
    let name = strip_str(map.get("name").map_or("", String::as_str)).unwrap_or_default();
    let tooltip = strip_str(map.get("tooltip").map_or("", String::as_str)).unwrap_or_default();
    let category =
        strip_str(map.get("category").map_or("", String::as_str)).filter(|s| !s.is_empty());
    let menu_pos = map
        .get("menu_pos")
        .and_then(|t| t.parse::<i64>().ok())
        .unwrap_or(0);
    let mut extras = McmExtras::default();
    if let Some(t) = map.get("minRange") {
        extras.min_range = t.parse().ok();
    }
    if let Some(t) = map.get("maxRange") {
        extras.max_range = t.parse().ok();
    }
    if let Some(t) = map.get("step") {
        extras.step = t.parse().ok();
    }
    if let Some(t) = map.get("options") {
        extras.options = parse_str_array(t);
    }

    // Stash any unclaimed keys so they survive write-back.
    const KNOWN: &[&str] = &[
        "value", "default", "name", "tooltip", "category", "menu_pos", "minRange", "maxRange",
        "step", "options",
    ];
    let passthrough: Vec<(String, String)> = map
        .iter()
        .filter(|(k, _)| !KNOWN.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    Some(McmField {
        section: section.to_string(),
        key: key.to_string(),
        name,
        tooltip,
        category,
        menu_pos,
        value,
        default,
        extras,
        passthrough,
    })
}

fn parse_value(tok: &str) -> Option<McmValue> {
    let t = tok.trim();
    if t == "true" {
        return Some(McmValue::Bool(true));
    }
    if t == "false" {
        return Some(McmValue::Bool(false));
    }
    if let Some(s) = strip_str(t) {
        return Some(McmValue::Str(s));
    }
    if let Ok(n) = t.parse::<i64>() {
        return Some(McmValue::Int(n));
    }
    if let Ok(f) = t.parse::<f64>() {
        return Some(McmValue::Float(f));
    }
    None
}

/// Force Float values to Int when the section semantically expects
/// integers. The in-game MCM's slider widget always emits Float for
/// `[Int]` sections (its SpinBox.value getter returns float regardless
/// of `rounded = true`), so a config file that's been touched in-game
/// has `value=59.0` instead of `value=59`. Consuming mods declare
/// these as `@export var foo: int = 0`, and assigning a Float into
/// that Variant slot leaves it Float-typed, which then breaks any
/// `func _spawn_pool() -> int` math the mod runs against it.
fn coerce_for_section(v: McmValue, section: &str) -> McmValue {
    if section_wants_int(section) {
        if let McmValue::Float(f) = v {
            return McmValue::Int(f as i64);
        }
    }
    v
}

fn strip_str(t: &str) -> Option<String> {
    let t = t.trim();
    let inner = t.strip_prefix('"')?.strip_suffix('"')?;
    Some(inner.replace("\\\"", "\"").replace("\\\\", "\\"))
}

fn parse_str_array(t: &str) -> Vec<String> {
    let t = t.trim();
    let Some(inner) = t.strip_prefix('[').and_then(|s| s.strip_suffix(']')) else {
        return Vec::new();
    };
    split_top_level_commas(inner)
        .iter()
        .filter_map(|p| strip_str(p.trim()))
        .collect()
}

fn render(cfg: &McmConfig) -> String {
    let mut out = String::new();
    // Group by section, preserving on-disk order within section.
    let mut sections: BTreeMap<String, Vec<&McmField>> = BTreeMap::new();
    for f in &cfg.fields {
        sections.entry(f.section.clone()).or_default().push(f);
    }
    // Append unknown raws bucketed by section so they land in the right place.
    let mut unknown_by_section: BTreeMap<String, Vec<&(String, String, String)>> = BTreeMap::new();
    for u in &cfg.raw_unknown {
        unknown_by_section.entry(u.0.clone()).or_default().push(u);
    }

    let mut all_sections: Vec<String> = sections
        .keys()
        .chain(unknown_by_section.keys())
        .cloned()
        .collect();
    all_sections.sort();
    all_sections.dedup();

    for section in all_sections {
        let _ = writeln!(out, "[{section}]");
        out.push('\n');
        if let Some(fields) = sections.get(&section) {
            for f in fields {
                let _ = writeln!(out, "{}={}", f.key, render_dict(f));
            }
        }
        if let Some(unknowns) = unknown_by_section.get(&section) {
            for (_s, k, body) in unknowns {
                let _ = writeln!(out, "{k}={body}");
            }
        }
        out.push('\n');
    }
    out
}

fn render_dict(f: &McmField) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(cat) = &f.category {
        parts.push(format!("\"category\": \"{}\"", escape(cat)));
    }
    parts.push(format!(
        "&\"default\": {}",
        render_value(&f.default, &f.section)
    ));
    parts.push(format!("&\"menu_pos\": {}", f.menu_pos));
    parts.push(format!("&\"name\": \"{}\"", escape(&f.name)));
    if !f.extras.options.is_empty() {
        parts.push(format!(
            "&\"options\": {}",
            render_str_array(&f.extras.options)
        ));
    }
    if let Some(min) = f.extras.min_range {
        parts.push(format!("&\"minRange\": {}", trim_float(min)));
    }
    if let Some(max) = f.extras.max_range {
        parts.push(format!("&\"maxRange\": {}", trim_float(max)));
    }
    if let Some(step) = f.extras.step {
        parts.push(format!("&\"step\": {}", trim_float(step)));
    }
    parts.push(format!("&\"tooltip\": \"{}\"", escape(&f.tooltip)));
    parts.push(format!(
        "&\"value\": {}",
        render_value(&f.value, &f.section)
    ));
    for (k, v) in &f.passthrough {
        // Passthrough keys without `&` prefix, they were stored as
        // plain strings in the source, and MCM accepts both forms.
        parts.push(format!("\"{k}\": {v}"));
    }
    format!("{{\n{}\n}}", parts.join(",\n"))
}

/// True for sections whose values are semantically integers, even
/// when the in-game MCM's SpinBox widget hands back a Float (which
/// it always does, regardless of `rounded = true`). Renderer + parser
/// coerce Float → Int for these sections so the on-disk file matches
/// what consuming mods declare in their `@export var foo: int` fields.
///
/// Without this, FW's `_spawn_pool() -> int` and similar routines
/// silently fail when their math involves a float-typed Resource
/// property, leaving spawnPool/spawnLimit/etc. at vanilla defaults.
fn section_wants_int(section: &str) -> bool {
    matches!(section, "Int" | "Dropdown" | "Keycode")
}

fn render_value(v: &McmValue, section: &str) -> String {
    if section_wants_int(section) {
        // Coerce Float-shaped values to int form so `@export var
        // foo: int = 0` consumers get an int when they read.
        return match v {
            McmValue::Bool(b) => b.to_string(),
            McmValue::Int(n) => n.to_string(),
            McmValue::Float(f) => (*f as i64).to_string(),
            McmValue::Str(s) => format!("\"{}\"", escape(s)),
        };
    }
    match v {
        McmValue::Bool(b) => b.to_string(),
        McmValue::Int(n) => n.to_string(),
        McmValue::Float(f) => trim_float(*f),
        McmValue::Str(s) => format!("\"{}\"", escape(s)),
    }
}

fn render_str_array(items: &[String]) -> String {
    let inner: Vec<String> = items.iter().map(|s| format!("\"{}\"", escape(s))).collect();
    format!("[{}]", inner.join(", "))
}

fn trim_float(f: f64) -> String {
    // Match MCM's emit style: integers render with `.0`, fractions trim.
    if f.fract() == 0.0 {
        format!("{f:.1}")
    } else {
        format!("{f}")
    }
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[Bool]

adsZoom={
&"default": true,
&"name": "ADS Zoom - Enabled",
&"tooltip": "",
&"value": false
}

[Int]

globalWeightPct={
"category": "Global Sync",
&"default": 40,
&"maxRange": 100,
&"menu_pos": 1,
&"minRange": 0,
&"name": "Global influence weight (%)",
&"tooltip": "How much, in percent.",
&"value": 75
}
"#;

    #[test]
    fn parses_bool_and_int_with_category_and_range() {
        let cfg = parse(SAMPLE).unwrap();
        assert_eq!(cfg.fields.len(), 2);
        let b = &cfg.fields[0];
        assert_eq!(b.section, "Bool");
        assert_eq!(b.key, "adsZoom");
        assert_eq!(b.value, McmValue::Bool(false));
        assert_eq!(b.default, McmValue::Bool(true));
        assert!(b.category.is_none());

        let i = &cfg.fields[1];
        assert_eq!(i.value, McmValue::Int(75));
        assert_eq!(i.extras.min_range, Some(0.0));
        assert_eq!(i.extras.max_range, Some(100.0));
        assert_eq!(i.category.as_deref(), Some("Global Sync"));
    }

    #[test]
    fn round_trip_preserves_value_change() {
        let mut cfg = parse(SAMPLE).unwrap();
        // Find and bump.
        let f = cfg
            .fields
            .iter_mut()
            .find(|f| f.key == "globalWeightPct")
            .unwrap();
        f.value = McmValue::Int(50);
        let text = render(&cfg);
        let cfg2 = parse(&text).unwrap();
        assert_eq!(cfg2.fields.len(), 2);
        let f2 = cfg2
            .fields
            .iter()
            .find(|f| f.key == "globalWeightPct")
            .unwrap();
        assert_eq!(f2.value, McmValue::Int(50));
    }

    #[test]
    fn parses_dropdown_options() {
        let text = r#"
[Dropdown]
shopperCadenceIdx={
"category": "Cadence",
&"default": 1,
&"menu_pos": 1,
&"name": "Shopper simulation interval",
&"options": ["1 hour", "3 hours (default)", "6 hours", "12 hours"],
&"tooltip": "How often.",
&"value": 2
}
"#;
        let cfg = parse(text).unwrap();
        let f = &cfg.fields[0];
        assert_eq!(f.section, "Dropdown");
        assert_eq!(f.value, McmValue::Int(2));
        assert_eq!(f.extras.options.len(), 4);
        assert_eq!(f.extras.options[1], "3 hours (default)");
    }

    #[test]
    fn on_value_changed_survives_round_trip() {
        let text = r#"
[Bool]
wipePressed={
"category": "Data",
&"default": false,
&"menu_pos": 0,
&"name": "Wipe",
"on_value_changed": "on_wipe_pressed",
&"tooltip": "wipe",
&"value": false
}
"#;
        let mut cfg = parse(text).unwrap();
        let f = cfg
            .fields
            .iter_mut()
            .find(|f| f.key == "wipePressed")
            .unwrap();
        f.value = McmValue::Bool(true);
        let rendered = render(&cfg);
        assert!(
            rendered.contains("\"on_value_changed\": \"on_wipe_pressed\""),
            "lost callback: {rendered}"
        );
        let cfg2 = parse(&rendered).unwrap();
        let f2 = cfg2.fields.iter().find(|f| f.key == "wipePressed").unwrap();
        assert_eq!(f2.value, McmValue::Bool(true));
        assert!(f2.passthrough.iter().any(|(k, _)| k == "on_value_changed"));
    }

    #[test]
    fn slug_collapses_punctuation() {
        assert_eq!(slug("Hold-Breath"), "holdbreath");
        assert_eq!(
            slug("Real-Gun-&-Attachment-Names"),
            "realgunattachmentnames"
        );
        assert_eq!(slug("global-economy"), "globaleconomy");
    }

    #[test]
    fn parses_real_global_economy_config_if_present() {
        // Skipped on machines without an RTV install. Uses the sample
        // shape from the user's actual config to make sure the parser
        // doesn't choke on real-world input (mixed `&"k"` / `"k"`,
        // floats, percent-escapes in tooltips, etc.).
        let Some(home) = std::env::var_os("APPDATA")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share"))
            })
        else {
            return;
        };
        let path = home
            .join("Road to Vostok")
            .join("MCM/global-economy/config.ini");
        if !path.is_file() {
            eprintln!("skipping: no MCM config at {}", path.display());
            return;
        }
        let cfg = McmConfig::load(&path).unwrap().unwrap();
        assert!(!cfg.fields.is_empty(), "real config has at least one field");
        // Round-trip should be a no-op for value-equality.
        let rendered = render(&cfg);
        let cfg2 = parse(&rendered).unwrap();
        assert_eq!(cfg.fields.len(), cfg2.fields.len());
    }

    #[test]
    fn int_section_float_value_coerced_to_int_on_parse() {
        // In-game MCM's slider widget always emits Float for [Int]
        // sections (SpinBox.value returns float regardless of
        // `rounded = true`). Crabby's parser must coerce so consuming
        // mods reading via `@export var foo: int` see int math.
        let text = r#"
[Int]
spawn_pool_bonus={
&"default": 0,
&"menu_pos": 11,
&"name": "Extra Reserve Enemies",
&"tooltip": "...",
&"minRange": 0,
&"maxRange": 60,
&"value": 59.0
}
"#;
        let cfg = parse(text).unwrap();
        let f = &cfg.fields[0];
        assert_eq!(f.section, "Int");
        assert_eq!(
            f.value,
            McmValue::Int(59),
            "Float 59.0 in [Int] must coerce to Int(59)"
        );
        assert_eq!(f.default, McmValue::Int(0));
    }

    #[test]
    fn int_section_renders_int_even_when_in_memory_is_float() {
        // Same coercion at write time as a belt-and-suspenders fallback,
        // if anything mutates the in-memory field to Float (e.g. an
        // older crabby version's Float-typed buffer), rendering still
        // emits int form for [Int] sections.
        let mut cfg = McmConfig::default();
        cfg.fields.push(McmField {
            section: "Int".into(),
            key: "spawn_pool_bonus".into(),
            name: "X".into(),
            tooltip: String::new(),
            category: None,
            menu_pos: 0,
            value: McmValue::Float(59.0),
            default: McmValue::Float(0.0),
            extras: McmExtras::default(),
            passthrough: Vec::new(),
        });
        let rendered = render(&cfg);
        // Should contain `value=59` (no `.0`), not `value=59.0`.
        assert!(
            rendered.contains("&\"value\": 59\n") || rendered.contains("&\"value\": 59,"),
            "expected `&\"value\": 59` in render, got:\n{rendered}",
        );
        assert!(
            !rendered.contains("&\"value\": 59.0"),
            "Float-form leaked through Int-section render:\n{rendered}",
        );
    }

    #[test]
    fn float_section_keeps_float() {
        // Negative case: [Float] sections preserve Float values
        // (those mods declare `@export var x: float = 1.0` and need
        // float in / float out).
        let text = r#"
[Float]
boss_health_multiplier={
&"default": 1.0,
&"menu_pos": 32,
&"name": "Boss Health",
&"tooltip": "...",
&"minRange": 0.25,
&"maxRange": 5.0,
&"value": 0.997
}
"#;
        let cfg = parse(text).unwrap();
        let f = &cfg.fields[0];
        assert_eq!(f.section, "Float");
        assert_eq!(f.value, McmValue::Float(0.997));
    }
}
