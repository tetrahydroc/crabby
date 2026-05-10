//! Read + write the Godot `ConfigFile` flavour used for `mod_config.cfg`.
//!
//! Godot `ConfigFile` is a fully-typed format with arbitrary value
//! syntax (`Vector2(...)`, `PackedStringArray(...)`, etc). Only the
//! narrow subset the schema actually uses is emitted and accepted:
//!
//! - Section headers `[section.subsection]`
//! - Strings: `key = "value"`
//! - Integers: `key = 42`
//! - Inline dicts of string-keyed string/bool/int values:
//!   `key = { "enabled": true, "version": "1.0.3" }`
//! - Comments: lines starting with `;` or `#`
//! - Blank lines ignored
//!
//! Anything richer than that (multi-line dicts, nested arrays, typed
//! constructors) errors with a clear message; better to fail loudly
//! than silently misinterpret a hand-edit.

use std::collections::BTreeMap;
use std::error::Error as StdError;
use std::fmt::Write as _;

use crate::{ModConfig, ModEntry, Profile, RootEntry};

/// Source-error type the parser hands back. Boxed at the call site into
/// [`crabby_error::CrabbyError::Config`].
pub type CfgError = Box<dyn StdError + Send + Sync + 'static>;

/// Render a [`ModConfig`] to its canonical on-disk text form.
pub fn render(cfg: &ModConfig) -> String {
    let mut out = String::new();
    out.push_str("; mod_config.cfg, managed by `crabby mods` and the UI.\n");
    out.push_str("; Hand-edits are fine, but malformed lines reject loudly.\n\n");

    out.push_str("[crabby]\n");
    let _ = writeln!(out, "schema_version = {}", cfg.schema_version);
    let _ = writeln!(
        out,
        "active_profile = \"{}\"",
        escape_string(&cfg.active_profile),
    );

    for (name, profile) in &cfg.profiles {
        let _ = writeln!(out, "\n[profile.{name}]");
        for (id, entry) in &profile.mods {
            // Omit priority_override when None so older mod_config.cfg
            // files round-trip unchanged. Reads tolerate either form.
            match entry.priority_override {
                Some(p) => {
                    let _ = writeln!(
                        out,
                        "{id} = {{ \"enabled\": {}, \"version\": \"{}\", \"priority\": {} }}",
                        entry.enabled,
                        escape_string(&entry.version),
                        p,
                    );
                }
                None => {
                    let _ = writeln!(
                        out,
                        "{id} = {{ \"enabled\": {}, \"version\": \"{}\" }}",
                        entry.enabled,
                        escape_string(&entry.version),
                    );
                }
            }
        }
    }

    // Extra mod-source roots, emitted as numbered keys so order is
    // preserved on round-trip without needing list literal support in
    // the parser. The runtime shim and the launcher both read this
    // section.
    if !cfg.extra_roots.is_empty() {
        out.push_str("\n[crabby.roots]\n");
        for (i, root) in cfg.extra_roots.iter().enumerate() {
            // Normalize backslashes to forward slashes so the file is
            // portable and to avoid accumulating `\\` escapes through
            // round-trips. Godot's path APIs accept both.
            let path_str = root.path.display().to_string().replace('\\', "/");
            let _ = writeln!(
                out,
                "root.{i} = {{ \"path\": \"{}\", \"dev\": {} }}",
                escape_string(&path_str),
                root.dev,
            );
        }
    }

    out
}

/// Parse the on-disk text form back into a [`ModConfig`].
pub fn parse(text: &str) -> Result<ModConfig, CfgError> {
    let mut current: Option<String> = None;
    let mut crabby = CrabbySection::default();
    let mut profiles: BTreeMap<String, Profile> = BTreeMap::new();
    // Roots collected as (index, RootEntry) for sort by index
    // before returning, preserves the on-disk order regardless of
    // line ordering.
    let mut roots: Vec<(u32, RootEntry)> = Vec::new();

    for (lineno, raw) in text.lines().enumerate() {
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }

        if let Some(header) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            current = Some(header.trim().to_owned());
            // Materialize empty profiles so [profile.foo] with no entries
            // still produces a Profile in the map.
            if let Some(name) = header.trim().strip_prefix("profile.") {
                profiles.entry(name.trim().to_owned()).or_default();
            }
            continue;
        }

        let (key, value) = split_kv(line, lineno)?;
        match current.as_deref() {
            Some("crabby") => crabby.set(&key, value, lineno)?,
            Some("crabby.roots") => {
                let idx = key.strip_prefix("root.").and_then(|s| s.parse::<u32>().ok())
                    .ok_or_else(|| {
                        format!(
                            "line {}: [crabby.roots] keys must be `root.<n>`, got {key:?}",
                            lineno + 1,
                        )
                    })?;
                roots.push((idx, parse_root_entry(value, lineno)?));
            }
            Some(section) if section.starts_with("profile.") => {
                let profile_name = section.trim_start_matches("profile.").trim();
                if profile_name.is_empty() {
                    return Err(
                        format!("line {}: section [profile.] has empty name", lineno + 1,).into(),
                    );
                }
                let entry = parse_mod_entry(value, lineno)?;
                profiles
                    .entry(profile_name.to_owned())
                    .or_default()
                    .mods
                    .insert(key, entry);
            }
            Some(other) => {
                return Err(format!(
                    "line {}: unknown section [{}] (expected [crabby], [crabby.roots], or [profile.<name>])",
                    lineno + 1,
                    other,
                )
                .into());
            }
            None => {
                return Err(
                    format!("line {}: key/value before any section header", lineno + 1,).into(),
                );
            }
        }
    }

    let schema_version = crabby
        .schema_version
        .ok_or("[crabby] missing required `schema_version`")?;
    let active_profile = crabby
        .active_profile
        .ok_or("[crabby] missing required `active_profile`")?;

    roots.sort_by_key(|(i, _)| *i);
    let extra_roots = roots.into_iter().map(|(_, r)| r).collect();

    Ok(ModConfig {
        schema_version,
        active_profile,
        profiles,
        extra_roots,
    })
}

#[derive(Default)]
struct CrabbySection {
    schema_version: Option<u32>,
    active_profile: Option<String>,
}

impl CrabbySection {
    fn set(&mut self, key: &str, value: &str, lineno: usize) -> Result<(), CfgError> {
        match key {
            "schema_version" => {
                let n: u32 = value.parse().map_err(|_| {
                    format!(
                        "line {}: schema_version must be an integer, got {value:?}",
                        lineno + 1,
                    )
                })?;
                self.schema_version = Some(n);
            }
            "active_profile" => {
                self.active_profile = Some(parse_string_literal(value, lineno)?);
            }
            other => {
                return Err(
                    format!("line {}: unknown key {other:?} in [crabby]", lineno + 1,).into(),
                );
            }
        }
        Ok(())
    }
}

/// Parse a `{ "enabled": true, "version": "..." }` literal.
///
/// Tolerates either `:` or `=` between key and value (Godot `ConfigFile`
/// emits `:`, hand-edits sometimes type `=`). Any unknown key is an error;
/// silently dropping a typo'd field would mask user mistakes.
fn parse_mod_entry(value: &str, lineno: usize) -> Result<ModEntry, CfgError> {
    let inner = value
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .ok_or_else(|| {
            format!(
                "line {}: expected `{{ \"enabled\": <bool>, \"version\": \"...\" }}` value, got {value:?}",
                lineno + 1,
            )
        })?;

    let mut enabled: Option<bool> = None;
    let mut version: Option<String> = None;
    let mut priority_override: Option<i64> = None;

    for raw_pair in split_top_level_commas(inner) {
        let pair = raw_pair.trim();
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair
            .split_once(':')
            .or_else(|| pair.split_once('='))
            .ok_or_else(|| {
                format!(
                    "line {}: dict entry must be `key: value`, got {pair:?}",
                    lineno + 1,
                )
            })?;
        let key = parse_string_literal(k.trim(), lineno)?;
        let v = v.trim();
        match key.as_str() {
            "enabled" => {
                let b = match v {
                    "true" => true,
                    "false" => false,
                    other => {
                        return Err(format!(
                            "line {}: `enabled` must be true or false, got {other:?}",
                            lineno + 1,
                        )
                        .into());
                    }
                };
                enabled = Some(b);
            }
            "version" => {
                version = Some(parse_string_literal(v, lineno)?);
            }
            "priority" => {
                let n: i64 = v.parse().map_err(|_| {
                    format!(
                        "line {}: `priority` must be an integer, got {v:?}",
                        lineno + 1,
                    )
                })?;
                priority_override = Some(n);
            }
            other => {
                return Err(format!(
                    "line {}: unknown key {other:?} in mod entry (expected `enabled`, `version`, `priority`)",
                    lineno + 1,
                )
                .into());
            }
        }
    }

    Ok(ModEntry {
        enabled: enabled
            .ok_or_else(|| format!("line {}: mod entry missing `enabled`", lineno + 1))?,
        version: version
            .ok_or_else(|| format!("line {}: mod entry missing `version`", lineno + 1))?,
        priority_override,
    })
}

/// Parse a `{ "path": "...", "dev": true }` literal for a [`crate::RootEntry`].
fn parse_root_entry(value: &str, lineno: usize) -> Result<RootEntry, CfgError> {
    let inner = value
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .ok_or_else(|| {
            format!(
                "line {}: expected `{{ \"path\": \"...\", \"dev\": <bool> }}` value, got {value:?}",
                lineno + 1,
            )
        })?;

    let mut path: Option<String> = None;
    let mut dev: Option<bool> = None;

    for raw_pair in split_top_level_commas(inner) {
        let pair = raw_pair.trim();
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair
            .split_once(':')
            .or_else(|| pair.split_once('='))
            .ok_or_else(|| {
                format!(
                    "line {}: dict entry must be `key: value`, got {pair:?}",
                    lineno + 1,
                )
            })?;
        let key = parse_string_literal(k.trim(), lineno)?;
        let v = v.trim();
        match key.as_str() {
            "path" => path = Some(parse_string_literal(v, lineno)?),
            "dev" => {
                dev = Some(match v {
                    "true" => true,
                    "false" => false,
                    other => {
                        return Err(format!(
                            "line {}: `dev` must be true or false, got {other:?}",
                            lineno + 1,
                        )
                        .into());
                    }
                });
            }
            other => {
                return Err(format!(
                    "line {}: unknown key {other:?} in root entry (expected `path`, `dev`)",
                    lineno + 1,
                )
                .into());
            }
        }
    }

    Ok(RootEntry {
        path: path
            .ok_or_else(|| format!("line {}: root entry missing `path`", lineno + 1))?
            .into(),
        dev: dev.unwrap_or(false),
    })
}

/// Split `a, b, c` while ignoring commas inside `"..."` strings.
fn split_top_level_commas(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut in_string = false;
    let bytes = s.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'"' {
            in_string = !in_string;
            continue;
        }
        if b == b',' && !in_string {
            out.push(&s[start..i]);
            start = i + 1;
        }
    }
    out.push(&s[start..]);
    out
}

/// Strip surrounding `"..."` from a string literal and unescape `\\`
/// and `\"`. Errors if the value isn't quoted.
///
/// Must mirror [`escape_string`] exactly; without unescaping, every
/// save->load round-trip doubles backslashes in Windows paths.
fn parse_string_literal(value: &str, lineno: usize) -> Result<String, CfgError> {
    let trimmed = value.trim();
    let inner = trimmed
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .ok_or_else(|| {
            format!(
                "line {}: expected a quoted string, got {trimmed:?}",
                lineno + 1,
            )
        })?;
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('\\') => out.push('\\'),
                Some('"') => out.push('"'),
                Some(other) => {
                    return Err(format!(
                        "line {}: unknown escape \\\\{other} in string literal",
                        lineno + 1,
                    )
                    .into());
                }
                None => {
                    return Err(format!(
                        "line {}: trailing backslash in string literal",
                        lineno + 1,
                    )
                    .into());
                }
            }
        } else {
            out.push(c);
        }
    }
    Ok(out)
}

/// `key = value` split on the first `=`. Errors if no `=` is present.
fn split_kv(line: &str, lineno: usize) -> Result<(String, &str), CfgError> {
    let (k, v) = line
        .split_once('=')
        .ok_or_else(|| format!("line {}: expected `key = value`, got {line:?}", lineno + 1,))?;
    Ok((k.trim().to_owned(), v.trim()))
}

fn strip_comment(line: &str) -> &str {
    let trimmed = line.trim_start();
    if trimmed.starts_with(';') || trimmed.starts_with('#') {
        return "";
    }
    line
}

fn escape_string(s: &str) -> String {
    // Escape only what would corrupt parsing. Mod ids / version strings
    // / profile names should never contain these in practice; guard
    // anyway so a stray `"` doesn't silently break the file.
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with_one_mod(enabled: bool) -> ModConfig {
        let mut cfg = ModConfig::default_fresh();
        cfg.active_profile_mut().mods.insert(
            "hold-breath".into(),
            ModEntry {
                enabled,
                version: "1.0.3".into(),
                priority_override: None,
            },
        );
        cfg
    }

    #[test]
    fn render_then_parse_roundtrips() {
        let original = config_with_one_mod(true);
        let text = render(&original);
        let parsed = parse(&text).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn priority_override_roundtrips() {
        let mut original = ModConfig::default_fresh();
        original.active_profile_mut().mods.insert(
            "mcm".into(),
            ModEntry {
                enabled: true,
                version: "2.6.3".into(),
                priority_override: Some(-100),
            },
        );
        original.active_profile_mut().mods.insert(
            "default-prio".into(),
            ModEntry {
                enabled: true,
                version: "1.0".into(),
                priority_override: None,
            },
        );
        let text = render(&original);
        // Override emits the priority field; None entries omit it.
        assert!(text.contains(r#""priority": -100"#), "render: {text}");
        assert!(!text.contains("default-prio = { \"enabled\": true, \"version\": \"1.0\", \"priority\""), "default-prio shouldn't carry priority: {text}");
        let parsed = parse(&text).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn parse_accepts_disabled_entry() {
        let text = r#"
[crabby]
schema_version = 1
active_profile = "default"

[profile.default]
hold-breath = { "enabled": false, "version": "1.0.3" }
"#;
        let cfg = parse(text).unwrap();
        let m = cfg
            .active_profile()
            .unwrap()
            .mods
            .get("hold-breath")
            .unwrap();
        assert!(!m.enabled);
        assert_eq!(m.version, "1.0.3");
    }

    #[test]
    fn parse_tolerates_equals_in_dict() {
        // hand-edits sometimes use `=` rather than `:` inside the dict
        let text = r#"
[crabby]
schema_version = 1
active_profile = "default"

[profile.default]
hold-breath = { "enabled" = true, "version" = "1.0.3" }
"#;
        let cfg = parse(text).unwrap();
        assert!(
            cfg.active_profile()
                .unwrap()
                .mods
                .get("hold-breath")
                .unwrap()
                .enabled
        );
    }

    #[test]
    fn parse_rejects_unknown_dict_key() {
        let text = r#"
[crabby]
schema_version = 1
active_profile = "default"

[profile.default]
hold-breath = { "enabled": true, "version": "1.0.3", "typo": "x" }
"#;
        assert!(parse(text).is_err());
    }

    #[test]
    fn parse_rejects_unknown_top_section() {
        let text = r#"
[crabby]
schema_version = 1
active_profile = "default"

[junk]
foo = "bar"
"#;
        assert!(parse(text).is_err());
    }

    #[test]
    fn parse_materializes_empty_profile() {
        let text = r#"
[crabby]
schema_version = 1
active_profile = "default"

[profile.default]

[profile.empty]
"#;
        let cfg = parse(text).unwrap();
        assert!(cfg.profiles.contains_key("empty"));
        assert!(cfg.profiles["empty"].mods.is_empty());
    }

    #[test]
    fn render_is_deterministic() {
        let cfg = config_with_one_mod(true);
        assert_eq!(render(&cfg), render(&cfg));
    }

    #[test]
    fn roots_roundtrip_preserves_order_and_dev_flag() {
        let mut cfg = ModConfig::default_fresh();
        cfg.extra_roots = vec![
            RootEntry { path: "/home/me/dev-mod".into(), dev: true },
            RootEntry { path: "D:/shared".into(), dev: false },
        ];
        let text = render(&cfg);
        let parsed = parse(&text).unwrap();
        assert_eq!(parsed.extra_roots.len(), 2);
        assert_eq!(parsed.extra_roots[0].path, std::path::PathBuf::from("/home/me/dev-mod"));
        assert!(parsed.extra_roots[0].dev);
        assert_eq!(parsed.extra_roots[1].path, std::path::PathBuf::from("D:/shared"));
        assert!(!parsed.extra_roots[1].dev);
    }

    #[test]
    fn root_path_with_backslashes_survives_repeated_roundtrips() {
        // Regression: parser used to skip unescaping, so each save+load
        // doubled backslashes in Windows paths.
        let mut cfg = ModConfig::default_fresh();
        cfg.extra_roots = vec![RootEntry {
            path: r"C:\Users\me\code\my-mod".into(),
            dev: true,
        }];
        let mut current = cfg;
        for _ in 0..5 {
            let text = render(&current);
            current = parse(&text).unwrap();
        }
        // Forward slashes are fine, normalized on write. The point
        // is that the path is still a single-segment Windows path, not
        // `C:\\\\\\Users\\\\\\me...`.
        let p = current.extra_roots[0].path.display().to_string();
        assert!(!p.contains(r"\\"), "path accumulated escapes: {p}");
    }

    #[test]
    fn roots_section_omitted_when_empty() {
        let cfg = ModConfig::default_fresh();
        let text = render(&cfg);
        assert!(!text.contains("[crabby.roots]"));
    }
}
