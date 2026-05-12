//! Static-pattern scanner for mod GDScript.
//!
//! Walks each mod's `.gd` files, recognizes:
//!
//! 1. **Hook registrations**, e.g. `<lib>.hook(name, cb [, priority])`
//!    where `<lib>` is any local var (the mod-side convention is
//!    `_lib`, `lib`, etc.).
//! 2. **Registry calls**, e.g. `<lib>.register(...)`, `.override(...)`,
//!    `.patch(...)`, plus the planned VML-PR verbs once they land.
//! 3. **Classic Godot mod patterns**, e.g. `take_over_path`, `set_script`,
//!    `ProjectSettings.load_resource_pack`, `extends "res://..."`,
//!    that fight or bypass crabby's bake-time substrate.
//!
//! For each finding the analyzer captures **callsite (line)** + an
//! inferred **resolvability** rating. AOT-eligible findings can be
//! inlined at bake time; the rest stay on the runtime path. The same
//! data feeds the launcher's conflict detector (cross-mod aggregation).
//!
//! # Scope
//!
//! - Regex-based recognition. No call-graph tracing, no constant
//!   propagation. Deliberately under-counts AOT eligibility rather than
//!   over-counts (false negatives are fine; false positives could break
//!   a mod).
//! - Receiver names are heuristic: any `<ident>.hook(...)` call where
//!   the literal first arg looks like `"<script>-<method>[-pre|-post|-callback]"`
//!   is treated as a hook. Captures `_lib.hook(...)`, `lib.hook(...)`,
//!   `crabby.hook(...)`, etc.
//! - Module-scope vs deferred-scope is deferred. The line number is
//!   recorded; the AOT layer can decide later.

#![deny(missing_docs)]

mod discover;

pub use discover::{
    BootScan, analyze_active_profile, analyze_active_profile_with_schema, analyze_enabled_mods,
    one_line_summary, read_mod_file_bytes, read_mod_scripts, scan_active_profile,
};

use std::sync::LazyLock;

use regex::Regex;

/// One mod's full static intent, the `(per-mod report)` shape that
/// downstream consumers (AOT compiler, conflict detector) feed off.
#[derive(Debug, Clone, Default)]
pub struct ModIntent {
    /// Mod id from its manifest. Set by callers (the analyzer doesn't
    /// open `mod.txt`).
    pub mod_id: String,
    /// Hook registrations in source order.
    pub hooks: Vec<HookIntent>,
    /// Registry calls (register / override / patch / append / etc.).
    pub registry_writes: Vec<RegistryWriteIntent>,
    /// Overlay writes (replace_file / add_file). Mods using these
    /// participate in the bake pipeline by replacing or adding PCK
    /// entries before the rewriter runs.
    pub overlay_writes: Vec<OverlayWriteIntent>,
    /// Classic Godot mod patterns worth flagging.
    pub classic_patterns: Vec<ClassicPattern>,
    /// Per-file file-name -> line-count, for context in the report.
    pub files_scanned: Vec<ScannedFile>,
}

/// One scanned file, for report aggregation only.
#[derive(Debug, Clone)]
pub struct ScannedFile {
    /// File name (basename, e.g. `Main.gd`).
    pub filename: String,
    /// Line count of the file.
    pub line_count: usize,
}

/// One `<lib>.hook(name, cb [, priority])` call site.
#[derive(Debug, Clone)]
pub struct HookIntent {
    /// File where the call is.
    pub filename: String,
    /// 1-based line number of the call.
    pub line: usize,
    /// Hook name as the literal string the mod passed (e.g.
    /// `"interface-_ready-pre"`). `None` when the first arg isn't a
    /// string literal, in which case the call falls back to runtime-only.
    pub hook_name: Option<String>,
    /// Decoded kind from the suffix.
    pub kind: HookKind,
    /// Raw source text of the callable arg (e.g. `_on_open` or
    /// `Callable(self, "_on_open")`). Used only for reports today;
    /// AOT will resolve it later.
    pub callable_text: String,
    /// Resolvability classification, drives AOT vs runtime routing.
    pub resolvability: Resolvability,
}

/// Hook kind decoded from the name suffix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookKind {
    /// Bare name, no suffix, meaning replace hook (single owner today).
    Replace,
    /// `-pre` suffix.
    Pre,
    /// `-post` suffix.
    Post,
    /// `-callback` suffix.
    Callback,
    /// Couldn't decode, usually because the name wasn't a literal.
    Unknown,
}

impl HookKind {
    fn from_name(name: &str) -> Self {
        if name.ends_with("-pre") {
            Self::Pre
        } else if name.ends_with("-post") {
            Self::Post
        } else if name.ends_with("-callback") {
            Self::Callback
        } else {
            Self::Replace
        }
    }
}

/// One `<lib>.register|override|patch|append|prepend|remove_from(...)` call.
#[derive(Debug, Clone)]
pub struct RegistryWriteIntent {
    /// File where the call is.
    pub filename: String,
    /// 1-based line number.
    pub line: usize,
    /// Verb (e.g. `register`, `override`).
    pub verb: RegistryVerb,
    /// Registry name (the first arg's literal, e.g. `"items"`). `None`
    /// when the first arg isn't a literal.
    pub registry: Option<String>,
    /// Key the call targets (the second arg's literal, e.g.
    /// `"my-mod-item-id"`). `None` when not a literal.
    pub key: Option<String>,
    /// Raw text of the payload arg(s). Reported as-is; AOT resolves later.
    pub payload_text: String,
    /// Resolvability classification.
    pub resolvability: Resolvability,
}

/// Verb on a registry call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryVerb {
    /// `register`, add a fresh entry.
    Register,
    /// `override`, replace a vanilla entry.
    Override,
    /// `patch`, partial-update an existing entry.
    Patch,
    /// `append`, push onto an array entry (planned VML PR1).
    Append,
    /// `prepend`, push-front onto an array entry.
    Prepend,
    /// `remove_from`, remove an entry from a collection.
    RemoveFrom,
    /// `revert`, revert a prior override/patch.
    Revert,
    /// `remove`, delete an entry.
    Remove,
}

impl RegistryVerb {
    fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "register" => Self::Register,
            "override" => Self::Override,
            "patch" => Self::Patch,
            "append" => Self::Append,
            "prepend" => Self::Prepend,
            "remove_from" => Self::RemoveFrom,
            "revert" => Self::Revert,
            "remove" => Self::Remove,
            _ => return None,
        })
    }
}

/// One `Lib.setup` overlay verb: a bake-time edit to the PCK that
/// replaces or adds a file at a `res://` path. Distinct from
/// [`RegistryVerb`] because overlays operate on PCK entry paths, not
/// on `(registry, key)` pairs, and are applied by the bake pipeline
/// rather than the runtime registry handlers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OverlayVerb {
    /// `replace_file`, swap a vanilla PCK entry's bytes wholesale.
    /// Single-owner per path: two mods replacing the same path is a
    /// hard conflict.
    ReplaceFile,
    /// `add_file`, introduce a new PCK entry at a path vanilla
    /// doesn't have. Two mods adding the same path is a hard conflict;
    /// adding a path vanilla already has is also a conflict (use
    /// `replace_file` instead).
    AddFile,
    /// `replace_method`, swap a single `func` declaration inside an
    /// existing target script. Bake-time only: the rewriter finds
    /// `func <name>` in the target's source, locates the function's
    /// span (signature + indented body), and substitutes the foreign
    /// source. Multiple mods can each replace a different method on
    /// the same target file; two mods replacing the same `(target,
    /// method)` pair is a hard conflict, and a `replace_file` against
    /// the same target shadows every method on it.
    ReplaceMethod,
}

impl OverlayVerb {
    fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "replace_file" => Self::ReplaceFile,
            "add_file" => Self::AddFile,
            "replace_method" => Self::ReplaceMethod,
            _ => return None,
        })
    }

    /// Snake-case verb spelling, matching the setup-plan literal.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReplaceFile => "replace_file",
            Self::AddFile => "add_file",
            Self::ReplaceMethod => "replace_method",
        }
    }
}

/// One `Lib.setup(...)` overlay-verb call. Carries the target PCK path
/// and the source-of-bytes path inside the mod archive.
///
/// Both paths are the `res://...` literals as the mod author wrote
/// them. The bake pipeline resolves `source_path` against the mod's
/// own root, reads the bytes, and applies them at `target_path`.
#[derive(Debug, Clone)]
pub struct OverlayWriteIntent {
    /// File where the call is (mod-relative, e.g. `main.gd`).
    pub filename: String,
    /// 1-based line number of the call inside `filename`.
    pub line: usize,
    /// Which overlay verb was used.
    pub verb: OverlayVerb,
    /// Target PCK path the edit applies to (e.g.
    /// `"res://Scripts/Player.gd"`). `None` when the literal couldn't
    /// be parsed (computed string, runtime concatenation, etc.) - the
    /// bake skips these and the analyzer flags them.
    pub target_path: Option<String>,
    /// Mod-archive-relative source path supplying the replacement
    /// bytes (e.g. `"res://overlays/Player.gd"`). `None` for the same
    /// reason as `target_path`.
    pub source_path: Option<String>,
    /// For `replace_method` only: the method name being replaced
    /// inside `target_path`. `None` for `replace_file` / `add_file`
    /// (which operate on whole files), or when the verb is
    /// `replace_method` but the method-name literal couldn't be
    /// parsed.
    pub method_name: Option<String>,
    /// Resolvability classification. Always `Static` today since the
    /// scanner only matches literal-string args; non-literal args
    /// produce no intent at all.
    pub resolvability: Resolvability,
}

/// One classic Godot mod pattern that fights crabby's substrate.
#[derive(Debug, Clone)]
pub struct ClassicPattern {
    /// File where the call is.
    pub filename: String,
    /// 1-based line number.
    pub line: usize,
    /// Which pattern matched.
    pub pattern: ClassicPatternKind,
    /// Severity tier.
    pub severity: Severity,
    /// Resolved literal target if extractable (e.g. the
    /// `res://...` path passed to `take_over_path`).
    pub target: Option<String>,
    /// Human-readable explanation of WHY this got the severity it did,
    /// e.g. "additive: 0 vanilla method overlap" or "overrides 3
    /// vanilla methods (Save, Load, _ready)". Empty when no schema-
    /// driven analysis ran. UI surfaces this in the conflict panel.
    pub verdict: String,
}

/// Kinds of classic Godot mod-pattern detections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassicPatternKind {
    /// Method call: `take_over_path(...)`.
    TakeOverPath,
    /// Method call: `set_script(...)` on an object.
    SetScript,
    /// Method call: `ProjectSettings.load_resource_pack(...)`.
    LoadResourcePack,
    /// Top-of-file: `extends "res://Scripts/Foo.gd"` (or `Foo.gd`-quoted form).
    ExtendsVanilla,
    /// `preload("res://Scripts/Foo.gd").new()` or `.something()`.
    PreloadVanillaScript,
}

/// How impactful the classic pattern is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Won't work under crabby's substrate at all.
    Hard,
    /// Will likely work but conflicts with crabby's layer.
    Warn,
    /// Worth surfacing to the user but not necessarily problematic.
    Info,
}

/// AOT-eligibility classification. The scanner is conservative, so
/// when unsure, it picks `RuntimeOnly`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolvability {
    /// Eligible for AOT inlining at bake time.
    Static,
    /// Has dynamic bits (non-literal args, conditional branch);
    /// stays on the runtime path.
    RuntimeOnly,
    /// Found inside a deferred init flow (e.g. inside a callback
    /// connected to `lib.frameworks_ready`). AOT-eligible once we
    /// land call-graph tracing in B4; for now treat as runtime.
    Deferred,
}

/// Vanilla method-name schema, indexed by bare script filename
/// (e.g. `Database.gd` → `{"_ready", "Save", ...}`). Built by
/// [`crabby_bake::bake_pck`] as a side-output of pass 1 and handed to
/// the analyzer via [`analyze_active_profile_with_schema`].
///
/// Cheap to clone, so call sites that need to share across closures /
/// threads can wrap in `Arc`.
#[derive(Debug, Clone, Default)]
pub struct VanillaSchema {
    /// Method names per script. Empty when the analyzer is run
    /// without a schema (treated as "couldn't compare", so every
    /// classic pattern stays at its baseline severity).
    pub methods_by_script: std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
}

impl VanillaSchema {
    /// Build from a flat map. Convenience for tests + crabby-bake.
    #[must_use]
    pub fn new(
        methods_by_script: std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
    ) -> Self {
        Self { methods_by_script }
    }

    /// Number of scripts the schema knows about. Useful for sanity logs.
    #[must_use]
    pub fn script_count(&self) -> usize {
        self.methods_by_script.len()
    }

    /// Look up a vanilla script's method set by bare filename
    /// (`Database.gd` style) or by `res://Scripts/Database.gd` path.
    /// Strips the path/extension prefix so callers can pass the form
    /// they have on hand.
    #[must_use]
    pub fn methods_for(&self, target: &str) -> Option<&std::collections::BTreeSet<String>> {
        let bare = target.rsplit('/').next().unwrap_or(target);
        self.methods_by_script.get(bare)
    }
}

/// Classification of a `res://...` path the scanner extracted from a
/// `take_over_path` / `set_script` / `load` / `preload` arg.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetOrigin {
    /// Vanilla RTV path (matches a known vanilla top-level dir).
    /// Touching one of these via classic Godot patterns is the
    /// problematic case, Hard severity.
    Vanilla,
    /// Mod-internal path (`res://mods/...` or any other non-vanilla
    /// res:// path). Mods doing this on their own files is normal,
    /// Info severity.
    ModInternal,
    /// Couldn't determine (non-literal arg, ambiguous shape), Warn
    /// severity since it MIGHT be touching vanilla.
    Unknown,
}

/// Classify a literal path string. The vanilla-root list mirrors
/// [`EXTENDS_VANILLA`]'s allowlist.
fn classify_target_path(path: &str) -> TargetOrigin {
    let p = path.trim_matches(|c: char| c == '"' || c == '\'').trim();
    if !p.starts_with("res://") {
        return TargetOrigin::Unknown;
    }
    let rest = &p["res://".len()..];
    // Mod-internal: anything under res://mods/ (or vmz mount roots).
    if rest.starts_with("mods/") {
        return TargetOrigin::ModInternal;
    }
    // Vanilla: top-level dir matches a known RTV root.
    let first_seg = rest.split('/').next().unwrap_or("");
    const VANILLA_ROOTS: &[&str] = &[
        "Scripts",
        "Resources",
        "UI",
        "AI",
        "Audio",
        "Effects",
        "Modular",
        "Nature",
        "Prefabs",
        "Scenes",
        "Items",
        "Loot",
        "Crafting",
        "Editor",
        "Environment",
        "Events",
        "Fonts",
        "Shaders",
        "Terrains",
        "Traders",
        "Assets",
    ];
    if VANILLA_ROOTS.iter().any(|r| *r == first_seg) {
        TargetOrigin::Vanilla
    } else {
        // res:// but not vanilla and not mods/ - could be a mod-side
        // bundled path (some packers put assets at res://<mod_id>/...
        // directly). Treat as mod-internal; conflict detector can
        // refine.
        TargetOrigin::ModInternal
    }
}

/// Map a `TargetOrigin` to a severity for the classic-pattern flags.
/// This is the **baseline** severity used when no schema-driven
/// override is available. With a schema + paired swap-script, the
/// caller refines the verdict using [`grade_swap_severity`].
fn severity_for(origin: TargetOrigin) -> Severity {
    match origin {
        TargetOrigin::Vanilla => Severity::Hard,
        TargetOrigin::ModInternal => Severity::Info,
        TargetOrigin::Unknown => Severity::Warn,
    }
}

/// Refine a classic-pattern severity for a `take_over_path` /
/// `set_script` site that targets a vanilla script, given:
/// - the **vanilla method names** for the script being swapped, AND
/// - the **swap-script's parsed function set** (already loaded by the
///   caller).
///
/// Verdict rules:
/// - 0 vanilla overlap → additive swap → `Info` ("safe, adds methods,
///   doesn't override")
/// - any vanilla overlap → likely overrides hooks other mods rely on
///   → `Warn`. No escalation to Hard yet, since until a real-world case
///   pins overlap-count to severity, "any overlap is risky" is the
///   right floor.
///
/// Returns `(severity, verdict_text)`. The verdict explains *why*,
/// surfaced in the conflict panel so the user sees "overrides 3
/// vanilla methods" not just "Warn."
fn grade_swap_severity(
    vanilla_methods: &std::collections::BTreeSet<String>,
    swap_methods: &std::collections::BTreeSet<String>,
) -> (Severity, String) {
    let overlap: Vec<&String> = swap_methods.intersection(vanilla_methods).collect();
    if overlap.is_empty() {
        (
            Severity::Info,
            format!(
                "additive: defines {} method(s), 0 vanilla overlap",
                swap_methods.len()
            ),
        )
    } else {
        let preview = if overlap.len() <= 5 {
            overlap
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        } else {
            let first = overlap
                .iter()
                .take(4)
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            format!("{first}, +{} more", overlap.len() - 4)
        };
        (
            Severity::Warn,
            format!("overrides {} vanilla method(s): {preview}", overlap.len()),
        )
    }
}

/// Find every `var <name> = preload("res://...")` / `load("res://...")`
/// binding at module or function scope and return a `name → path` map.
/// Used to pair `take_over_path("res://Scripts/X.gd")` followed later
/// by `<obj>.set_script(<that_var>)` (or vice-versa) with the swap-
/// script's source.
///
/// Limitations: matches on textual `var <name> = (pre)load("...")`
/// only. Doesn't track reassignment, doesn't follow non-literal
/// expressions. Misses produce `TargetOrigin::Unknown`, the
/// conservative default.
fn collect_var_to_path_bindings(source: &str) -> std::collections::BTreeMap<String, String> {
    static VAR_BINDING: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r#"(?m)\bvar\s+([A-Za-z_][A-Za-z0-9_]*)\s*(?::\s*[A-Za-z_][A-Za-z0-9_]*\s*)?=\s*(?:pre)?load\s*\(\s*"(res://[^"]+\.gd)"\s*\)"#,
        )
        .expect("VAR_BINDING regex")
    });
    let mut out = std::collections::BTreeMap::new();
    for cap in VAR_BINDING.captures_iter(source) {
        if let (Some(name), Some(path)) = (cap.get(1), cap.get(2)) {
            out.insert(name.as_str().to_string(), path.as_str().to_string());
        }
    }
    out
}

/// Strip GDScript line comments. Returns a copy of `source` where
/// every `# …` tail (after a `#` that isn't inside a string literal)
/// is replaced by spaces, preserving line numbering since callers use
/// byte offsets to compute line numbers.
fn strip_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    for line in source.split_inclusive('\n') {
        let mut in_str = false;
        let mut esc = false;
        let mut comment_start: Option<usize> = None;
        for (i, c) in line.char_indices() {
            if in_str {
                if esc {
                    esc = false;
                } else if c == '\\' {
                    esc = true;
                } else if c == '"' {
                    in_str = false;
                }
                continue;
            }
            if c == '"' {
                in_str = true;
            } else if c == '#' {
                comment_start = Some(i);
                break;
            }
        }
        match comment_start {
            None => out.push_str(line),
            Some(idx) => {
                out.push_str(&line[..idx]);
                // Preserve trailing whitespace (incl. newline) from
                // the original line so byte offsets line up.
                for c in line[idx..].chars() {
                    if c == '\n' {
                        out.push('\n');
                    } else {
                        out.push(' ');
                    }
                }
            }
        }
    }
    out
}

/// Extract a literal `res://...gd` path from a `set_script(...)` arg
/// when it has the shape `load("res://...")` / `preload("res://...")`.
/// Returns `None` for anything else (variable refs, computed paths),
/// and the caller treats those as `TargetOrigin::Unknown`.
fn extract_load_path(arg: &str) -> Option<String> {
    static LOAD_LITERAL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"\b(?:load|preload)\s*\(\s*"(res://[^"]+\.gd)"\s*\)"#)
            .expect("LOAD_LITERAL regex")
    });
    LOAD_LITERAL
        .captures(arg)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

// --- Recognizers (regex-based, intentionally conservative) -------------------

/// `<ident>.hook("string-literal", <callable>[, <priority>])`. Captures the
/// literal name, the callable text, and the priority slot if present.
/// The receiver-ident is intentionally not constrained, since
/// TraderImprovements uses `_lib.hook(...)`, the design docs say
/// `lib.hook(...)`, and crabby's shim API is `crabby.hook(...)`. Any
/// is accepted.
static HOOK_CALL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?m)^\s*[A-Za-z_][A-Za-z0-9_]*\s*\.\s*hook\s*\(\s*"([^"]+)"\s*,\s*([^,)]+?)\s*(?:,\s*([^)]+?)\s*)?\)"#,
    )
    .expect("HOOK_CALL regex")
});

/// `<ident>.<verb>("registry-name", ...)` for a known registry verb.
///
/// Critically, the first arg MUST be a string literal. This is how the
/// analyzer disambiguates vostok-style registry verbs (`lib.append(
/// "items", id, val)`) from GDScript built-ins like `Array.append(value)`
/// and `Dictionary.remove(key)`. Mods that pass a non-literal first arg
/// (e.g. `lib.register(get_registry_name(), ...)`) fall off the AOT
/// path entirely and only show up in the runtime trace; the scanner
/// is intentionally conservative here.
static REGISTRY_CALL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?m)^\s*[A-Za-z_][A-Za-z0-9_]*\s*\.\s*(register|override|patch|append|prepend|remove_from|revert|remove)\s*\(\s*"([^"]+)"\s*(?:,\s*([^)]*))?\)"#,
    )
    .expect("REGISTRY_CALL regex")
});

/// `<ident>.<verb>_many("registry-name", {id: data, ...}[, ...])`, the
/// batched form of every primitive verb. Same first-arg-must-be-literal
/// rule as `REGISTRY_CALL`. Captures verb (sans `_many` suffix) and
/// registry name.
///
/// Note: array-op _many verbs (append_many / prepend_many / remove_from_many)
/// take `(registry, field, entries)` where `field` is the second
/// positional arg, not part of the dict, but for the analyzer's purposes
/// (counting writes per registry) the field name isn't needed.
static REGISTRY_MANY_CALL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?m)^\s*[A-Za-z_][A-Za-z0-9_]*\s*\.\s*(register|override|patch|append|prepend|remove_from|revert|remove)_many\s*\(\s*"([^"]+)"\s*,"#,
    )
    .expect("REGISTRY_MANY_CALL regex")
});

/// `<ident>.<aggregator>({id: data, ...})`, the one-shot bundle
/// helpers. Each aggregator implies a fixed registry: `register_item`
/// → items, `register_weapon` → weapons (which fans out internally to
/// items + scenes + loot), `register_ai_loadout` → ai_loadouts. The
/// regex captures the helper name; the dispatcher maps it to the
/// implied registry.
static AGGREGATOR_CALL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?m)^\s*[A-Za-z_][A-Za-z0-9_]*\s*\.\s*(register_item|register_weapon|register_magazine|register_attachment|register_furniture|register_ai_loadout)\s*\("#,
    )
    .expect("AGGREGATOR_CALL regex")
});

/// `<ident>.hook_many({"hook-name": cb, ...})`, batched hook
/// registration. Each dict key counts as one hook intent. The regex
/// captures the start of the dict literal; the loop walks for matching
/// `}` then re-scans the inner text for `"<name>":` keys.
static HOOK_MANY_CALL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\b[A-Za-z_][A-Za-z0-9_]*\s*\.\s*hook_many\s*\(\s*\{"#)
        .expect("HOOK_MANY_CALL regex")
});

/// Inside a `hook_many({...})` dict literal: `"name": <callable>`.
/// Used to extract individual hook names from the batched call.
static HOOK_MANY_ENTRY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""([^"]+)"\s*:"#).expect("HOOK_MANY_ENTRY regex"));

/// Open paren for `setup([...])` calls. The loop walks for the
/// matching `)` then scans the inner Array literal for plan entries.
static SETUP_CALL_OPEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\b[A-Za-z_][A-Za-z0-9_]*\s*\.\s*setup\s*\(\s*\["#).expect("SETUP_CALL_OPEN regex")
});

/// Locates `<call>(` openings, where the caller scans for the matching
/// `)` to extract the full arg text, since the args may contain nested
/// `load(...)` / `preload(...)` calls that a plain `[^)]` match would
/// truncate.
static TAKE_OVER_PATH_OPEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\btake_over_path\s*\("#).expect("TAKE_OVER_PATH_OPEN"));
static SET_SCRIPT_OPEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\bset_script\s*\("#).expect("SET_SCRIPT_OPEN"));

/// Walk forward from `start` (which must point just past an opening
/// `(`) and return the byte offset of the matching `)`, or `None` if
/// unbalanced. Skips parens inside string literals.
fn find_matching_paren(s: &str, start: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut depth = 1i32;
    let mut in_str = false;
    let mut esc = false;
    let mut i = start;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
        } else {
            match c {
                '"' => in_str = true,
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

/// Generic matching-delimiter scanner. Pass `(open, close)` such as
/// `('[', ']')` or `('{', '}')`. `start` is the byte index of the
/// character JUST AFTER the opening delimiter (depth starts at 1).
/// Honors string-literal escaping the same way `find_matching_paren`
/// does.
fn find_matching(s: &str, start: usize, open: char, close: char) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut depth = 1i32;
    let mut in_str = false;
    let mut esc = false;
    let mut i = start;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
        } else if c == '"' {
            in_str = true;
        } else if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// Walk a `setup([...])` plan body and emit synthetic RegistryWrite /
/// Hook intents for every recognized entry. Plan body is the byte
/// range `[start..end]` of the source, i.e. the contents of the outer
/// Array literal handed to `lib.setup(...)`.
///
/// Recognized entry shapes (vostok's setup.gd schema):
///   ["register"|"override"|"patch"|"revert"|"remove", "kind", ...]
///   ["append"|"prepend"|"remove_from", "kind", "field", ...]
///   ["hooks", { "name": cb, ... }]
///   ["register_item"|"register_weapon"|... aggregators, { id: data }]
///   ["when", predicate, sub_plan]   <-- recurses into sub_plan
///
/// Limits: only literal verb strings count. A plan entry whose verb
/// slot is computed (`[get_verb(), ...]`) is invisible to the scanner
/// (same conservative posture as the rest of the analyzer).
fn scan_setup_plan(filename: &str, source: &str, start: usize, end: usize, out: &mut ModIntent) {
    // The first slot of every entry is `"verb"`. Match the leading-
    // bracket-then-string pattern: `[` (possibly with whitespace)
    // followed by a `"<word>"`. Capture group 1 = the verb.
    static SETUP_ENTRY: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"\[\s*"([A-Za-z_][A-Za-z0-9_]*)"\s*"#).expect("SETUP_ENTRY regex")
    });
    // Inside an entry, after the verb slot: pull the optional registry
    // string slot for primitive verbs.
    static SETUP_SECOND_STR: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"^,\s*"([^"]+)""#).expect("SETUP_SECOND_STR regex"));
    // Two-arg literal-string capture for verbs like
    // `["replace_file", "res://target", "res://source"]`. Captures
    // both string slots in order.
    static SETUP_TWO_STRS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"^,\s*"([^"]+)"\s*,\s*"([^"]+)""#).expect("SETUP_TWO_STRS regex")
    });
    // Three-arg literal-string capture for `replace_method`:
    // `["replace_method", "res://target", "method", "res://source"]`.
    // Captures target_path, method_name, source_path in order.
    static SETUP_THREE_STRS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"^,\s*"([^"]+)"\s*,\s*"([^"]+)"\s*,\s*"([^"]+)""#)
            .expect("SETUP_THREE_STRS regex")
    });

    let body = &source[start..end];
    for cap in SETUP_ENTRY.captures_iter(body) {
        let verb = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let entry_start_in_body = cap.get(0).expect("entry").start();
        let abs_start = start + entry_start_in_body;
        let line = byte_to_line(source, abs_start);

        // Parse the verb and translate to the analyzer's intent types.
        match verb {
            // Primitive registry verbs (require a literal "kind" in
            // slot 1).
            "register" | "override" | "patch" | "revert" | "remove" | "append" | "prepend"
            | "remove_from" => {
                let after_verb = &body[cap.get(0).unwrap().end()..];
                let registry = SETUP_SECOND_STR
                    .captures(after_verb)
                    .and_then(|c| c.get(1).map(|m| m.as_str().to_string()));
                let Some(rverb) = RegistryVerb::from_str(verb) else {
                    continue;
                };
                out.registry_writes.push(RegistryWriteIntent {
                    filename: filename.to_string(),
                    line,
                    verb: rverb,
                    registry,
                    key: None,
                    payload_text: String::from("<setup>"),
                    resolvability: Resolvability::Static,
                });
            }
            // Aggregator verbs (no kind slot, since aggregator name
            // implies the registry).
            "register_item"
            | "register_weapon"
            | "register_magazine"
            | "register_attachment"
            | "register_furniture"
            | "register_ai_loadout" => {
                let synthetic_registry = match verb {
                    "register_item" => "items",
                    "register_weapon" => "weapons",
                    "register_magazine" => "magazines",
                    "register_attachment" => "attachments",
                    "register_furniture" => "furniture",
                    "register_ai_loadout" => "ai_loadouts",
                    _ => unreachable!(),
                };
                out.registry_writes.push(RegistryWriteIntent {
                    filename: filename.to_string(),
                    line,
                    verb: RegistryVerb::Register,
                    registry: Some(synthetic_registry.to_string()),
                    key: None,
                    payload_text: format!("<setup-aggregator:{verb}>"),
                    resolvability: Resolvability::Static,
                });
            }
            // ["hooks", { "name": cb, ... }] -- find the `{`,
            // walk to its `}`, pull each "name": key as a hook intent.
            "hooks" => {
                let after_verb_byte = cap.get(0).unwrap().end();
                let rest = &body[after_verb_byte..];
                if let Some(brace_off) = rest.find('{') {
                    let abs_brace_open = start + after_verb_byte + brace_off;
                    let dict_start = abs_brace_open + 1;
                    if let Some(close) = find_matching(source, dict_start, '{', '}') {
                        let inner = &source[dict_start..close];
                        for entry in HOOK_MANY_ENTRY.captures_iter(inner) {
                            let name = entry.get(1).map(|c| c.as_str().to_string());
                            let kind = name
                                .as_deref()
                                .map(HookKind::from_name)
                                .unwrap_or(HookKind::Unknown);
                            out.hooks.push(HookIntent {
                                filename: filename.to_string(),
                                line,
                                hook_name: name,
                                kind,
                                callable_text: String::from("<setup-hooks>"),
                                resolvability: Resolvability::Static,
                            });
                        }
                    }
                }
            }
            // ["replace_file", "res://target", "res://source"] or
            // ["add_file", "res://target", "res://source"]. Both have
            // the same shape: two literal string args, applied at bake
            // time before the rewriter runs.
            "replace_file" | "add_file" => {
                let after_verb = &body[cap.get(0).unwrap().end()..];
                let (target_path, source_path) = SETUP_TWO_STRS
                    .captures(after_verb)
                    .map(|c| {
                        (
                            c.get(1).map(|m| m.as_str().to_string()),
                            c.get(2).map(|m| m.as_str().to_string()),
                        )
                    })
                    .unwrap_or((None, None));
                let Some(overlay_verb) = OverlayVerb::from_str(verb) else {
                    continue;
                };
                out.overlay_writes.push(OverlayWriteIntent {
                    filename: filename.to_string(),
                    line,
                    verb: overlay_verb,
                    target_path,
                    source_path,
                    method_name: None,
                    resolvability: Resolvability::Static,
                });
            }
            // ["replace_method", "res://target", "method_name", "res://source"]
            // Three literal string args; the rewriter swaps just one
            // `func <name>` declaration inside an otherwise-vanilla
            // target script.
            "replace_method" => {
                let after_verb = &body[cap.get(0).unwrap().end()..];
                let (target_path, method_name, source_path) = SETUP_THREE_STRS
                    .captures(after_verb)
                    .map(|c| {
                        (
                            c.get(1).map(|m| m.as_str().to_string()),
                            c.get(2).map(|m| m.as_str().to_string()),
                            c.get(3).map(|m| m.as_str().to_string()),
                        )
                    })
                    .unwrap_or((None, None, None));
                out.overlay_writes.push(OverlayWriteIntent {
                    filename: filename.to_string(),
                    line,
                    verb: OverlayVerb::ReplaceMethod,
                    target_path,
                    source_path,
                    method_name,
                    resolvability: Resolvability::Static,
                });
            }
            // ["when", predicate, sub_plan] -- no-op for the
            // analyzer. The SETUP_ENTRY regex is depth-agnostic, so
            // entries nested inside a when-block's sub_plan are
            // already picked up by this same loop. Recursing here
            // would double-count them. Mods that gate registrations
            // behind a when-predicate the analyzer can't evaluate
            // (Callable lambda, runtime state) get a "may register"
            // signal (same conservative posture as treating
            // unhookable code paths as hooked for collision purposes).
            "when" => {
                // Intentionally empty.
            }
            _ => {
                // Unknown verb in plan slot 0, so silently skip.
                // Either a typo (mod author will see it via setup()'s
                // runtime result dict) or a verb the analyzer doesn't
                // yet recognize.
            }
        }
    }
}

/// `ProjectSettings.load_resource_pack("...")`.
static LOAD_RESOURCE_PACK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\bProjectSettings\s*\.\s*load_resource_pack\s*\(\s*([^)]+?)\s*\)"#)
        .expect("LOAD_RESOURCE_PACK regex")
});

/// `extends "res://Scripts/Foo.gd"` (or `'res://...'`), which only
/// matches extends pointing at **vanilla** script paths (`res://Scripts/...`
/// or `res://Resources/...`), not mod-internal extends like
/// `res://mods/<id>/HookKit/BaseHook.gd` which are perfectly fine.
///
/// The discriminator is: vanilla scripts live under a small set of
/// well-known top-level dirs; mod-internal scripts live under
/// `res://mods/<id>/...`. The vanilla roots are allowlisted rather than
/// blocklisting `res://mods/` so a mod that decides to extend e.g.
/// `res://addons/...` still gets flagged for review.
static EXTENDS_VANILLA: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?m)^\s*extends\s+["']res://(Scripts|Resources|UI|AI|Audio|Effects|Modular|Nature|Prefabs|Scenes|Items|Loot|Crafting|Editor|Environment|Events|Fonts|Shaders|Terrains|Traders|Assets)/[^"']+\.gd["']"#,
    )
    .expect("EXTENDS_VANILLA regex")
});

/// `preload("res://Scripts/Foo.gd")` followed by `.new()` or any chained
/// call. Same vanilla-root allowlist as [`EXTENDS_VANILLA`] to avoid
/// flagging mod-internal preloads.
static PRELOAD_SCRIPT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\bpreload\s*\(\s*"res://Scripts/[^"]+\.gd"\s*\)\s*\.\s*[A-Za-z_]"#)
        .expect("PRELOAD_SCRIPT regex")
});

// --- Scan a single file ------------------------------------------------------

/// Optional context passed to [`scan_source_with_ctx`] to enable
/// schema-driven severity refinement on `take_over_path`/`set_script`.
///
/// `mod_files` is a map of `(in-mod-relative-path → source)` for the
/// current mod's other `.gd` files. Used to resolve the swap-script
/// source when comparing its method set against vanilla's. Pass an
/// empty map to disable cross-file resolution.
#[derive(Debug, Default)]
pub struct ScanContext<'a> {
    /// Vanilla method-name schema. `None` to use baseline severities.
    pub schema: Option<&'a VanillaSchema>,
    /// Other `.gd` files in the same mod, by mod-relative path. Used
    /// to load the swap-script's source when grading severity.
    pub mod_files: std::collections::BTreeMap<String, &'a str>,
}

/// Backward-compatible scan with no schema/context. Equivalent to
/// `scan_source_with_ctx(filename, source, &ScanContext::default(), out)`.
pub fn scan_source(filename: &str, raw_source: &str, out: &mut ModIntent) {
    scan_source_with_ctx(filename, raw_source, &ScanContext::default(), out);
}

/// Run all recognizers across one source file, appending findings to
/// `out`. With a populated [`ScanContext`], `take_over_path`/`set_script`
/// findings get refined severities via schema-driven function-set
/// comparison.
pub fn scan_source_with_ctx(
    filename: &str,
    raw_source: &str,
    ctx: &ScanContext<'_>,
    out: &mut ModIntent,
) {
    let line_count = raw_source.lines().count();
    out.files_scanned.push(ScannedFile {
        filename: filename.to_string(),
        line_count,
    });

    // Strip GDScript line comments before regex passes, otherwise
    // commented-out `set_script` / `take_over_path` lines false-flag
    // (real-world example: ImmersiveXP/Main.gd has commented-out
    // examples in its top-of-file design notes). `strip_comments`
    // preserves byte offsets so line numbers stay accurate.
    let source_owned = strip_comments(raw_source);
    let source = source_owned.as_str();

    // Pre-scan: pair `var x = preload("res://...")` bindings with
    // names so `take_over_path(x.resource_path)` / the companion
    // `set_script(x)` resolve to the swap-script's source path.
    let var_bindings = collect_var_to_path_bindings(source);

    // Hook calls.
    for cap in HOOK_CALL.captures_iter(source) {
        let name = cap.get(1).map(|m| m.as_str().to_string());
        let callable = cap
            .get(2)
            .map(|m| m.as_str().trim().to_string())
            .unwrap_or_default();
        let line = byte_to_line(source, cap.get(0).expect("hook match").start());
        let kind = name
            .as_deref()
            .map(HookKind::from_name)
            .unwrap_or(HookKind::Unknown);
        // Any literal-named hook is `Static` for now. The deferred-vs-true-static
        // distinction lands when call-graph tracing arrives.
        let resolvability = if name.is_some() {
            Resolvability::Static
        } else {
            Resolvability::RuntimeOnly
        };
        out.hooks.push(HookIntent {
            filename: filename.to_string(),
            line,
            hook_name: name,
            kind,
            callable_text: callable,
            resolvability,
        });
    }

    // Registry calls. The regex requires the first arg be a string
    // literal, the disambiguator vs GDScript built-ins like
    // `Array.append(value)`. Anything AOT-eligible must supply a
    // literal registry name anyway, so this isn't a real loss.
    for cap in REGISTRY_CALL.captures_iter(source) {
        let verb_str = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let Some(verb) = RegistryVerb::from_str(verb_str) else {
            continue;
        };
        let registry = cap.get(2).map(|m| m.as_str().to_string());
        let rest = cap.get(3).map(|m| m.as_str()).unwrap_or("");
        let (key, payload_text, resolvability) = parse_registry_rest(rest, registry.is_some());
        let line = byte_to_line(source, cap.get(0).expect("reg match").start());
        out.registry_writes.push(RegistryWriteIntent {
            filename: filename.to_string(),
            line,
            verb,
            registry,
            key,
            payload_text,
            resolvability,
        });
    }

    // _many batched form: `lib.register_many("kind", {...})` and
    // friends. Synthesizes one RegistryWriteIntent per call (not per
    // dict entry); the analyzer's downstream consumers (stats,
    // conflict detector) treat a `_many` call as a single "this mod
    // touches that registry" signal. Per-id detail would need to walk
    // the dict literal which is overkill.
    for cap in REGISTRY_MANY_CALL.captures_iter(source) {
        let verb_str = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let Some(verb) = RegistryVerb::from_str(verb_str) else {
            continue;
        };
        let registry = cap.get(2).map(|m| m.as_str().to_string());
        let line = byte_to_line(source, cap.get(0).expect("many match").start());
        out.registry_writes.push(RegistryWriteIntent {
            filename: filename.to_string(),
            line,
            verb,
            registry,
            key: None,
            payload_text: String::from("<many>"),
            resolvability: Resolvability::Static,
        });
    }

    // Aggregator helpers: `lib.register_weapon({...})` etc. Each
    // helper implies a fixed "registry" (synthetic name matching the
    // primitive consts: "weapons", "items", "ai_loadouts" etc.). We
    // emit one Register intent per call.
    for m in AGGREGATOR_CALL.captures_iter(source) {
        let helper = m.get(1).map(|c| c.as_str()).unwrap_or("");
        let synthetic_registry = match helper {
            "register_item" => "items",
            "register_weapon" => "weapons",
            "register_magazine" => "magazines",
            "register_attachment" => "attachments",
            "register_furniture" => "furniture",
            "register_ai_loadout" => "ai_loadouts",
            _ => continue,
        };
        let line = byte_to_line(source, m.get(0).expect("agg match").start());
        out.registry_writes.push(RegistryWriteIntent {
            filename: filename.to_string(),
            line,
            verb: RegistryVerb::Register,
            registry: Some(synthetic_registry.to_string()),
            key: None,
            payload_text: format!("<aggregator:{helper}>"),
            resolvability: Resolvability::Static,
        });
    }

    // hook_many({"name": cb, ...}): one HookIntent per dict key.
    // Walks the dict literal and pulls out every literal name. Non-
    // literal keys (computed names) are rare in practice and would
    // require lexical analysis the analyzer doesn't do.
    for m in HOOK_MANY_CALL.find_iter(source) {
        let brace_start = m.end(); // byte after the `{`
        let Some(close) = find_matching(source, brace_start, '{', '}') else {
            continue;
        };
        let inner = &source[brace_start..close];
        for entry in HOOK_MANY_ENTRY.captures_iter(inner) {
            let name = entry.get(1).map(|c| c.as_str().to_string());
            let line = byte_to_line(source, m.start());
            let kind = name
                .as_deref()
                .map(HookKind::from_name)
                .unwrap_or(HookKind::Unknown);
            out.hooks.push(HookIntent {
                filename: filename.to_string(),
                line,
                hook_name: name,
                kind,
                callable_text: String::from("<hook_many>"),
                resolvability: Resolvability::Static,
            });
        }
    }

    // setup([...]) declarative plan. Each top-level entry is an Array
    // whose first slot is a verb string. The plan body is scanned for
    // `["<verb>", ...]` openings and the equivalent
    // RegistryWriteIntent / HookIntent is synthesized. Nested
    // `when`-blocks are recursively visited via the same loop because
    // the inner Array appears at top-of-plan position the same way.
    for m in SETUP_CALL_OPEN.find_iter(source) {
        // m.end() points just past the outer `[`. Find its matching
        // `]` for the plan body, then scan inside for entries.
        let plan_start = m.end();
        let Some(plan_end) = find_matching(source, plan_start, '[', ']') else {
            continue;
        };
        scan_setup_plan(filename, source, plan_start, plan_end, out);
    }

    // Classic patterns. take_over_path / set_script are origin-aware:
    // mod calling these on its OWN res:// paths = normal (Info);
    // on vanilla paths = the actual concern (Hard); non-literal args
    // = can't tell (Warn).
    //
    // Uses an "open paren" regex + manual paren-balance walk for
    // these because args may contain nested calls like
    // `set_script(load("res://..."))` that would defeat a `[^)]`
    // match.
    for m in TAKE_OVER_PATH_OPEN.find_iter(source) {
        let Some(close) = find_matching_paren(source, m.end()) else {
            continue;
        };
        let arg = source[m.end()..close].trim().to_string();
        let origin = if arg.starts_with('"') {
            classify_target_path(&arg)
        } else {
            TargetOrigin::Unknown
        };
        let line = byte_to_line(source, m.start());
        let baseline = severity_for(origin);
        let verdict = match origin {
            TargetOrigin::Vanilla => {
                "swaps a vanilla script (companion `set_script(load(\"...\"))` decides what runs)"
                    .into()
            }
            TargetOrigin::ModInternal => "mod-internal swap; doesn't touch vanilla".into(),
            TargetOrigin::Unknown => "non-literal arg; can't determine target".into(),
        };
        out.classic_patterns.push(ClassicPattern {
            filename: filename.to_string(),
            line,
            pattern: ClassicPatternKind::TakeOverPath,
            severity: baseline,
            target: Some(strip_quotes(arg)),
            verdict,
        });
    }
    for m in SET_SCRIPT_OPEN.find_iter(source) {
        let Some(close) = find_matching_paren(source, m.end()) else {
            continue;
        };
        let arg = source[m.end()..close].trim().to_string();
        // The arg might be `load("res://...")`, `preload("res://...")`,
        // OR a bare variable name like `script` that was bound earlier
        // via `var script = preload("res://...")`. Resolve both shapes.
        let load_path = extract_load_path(&arg).or_else(|| {
            let trimmed = arg.trim();
            // Identifier-shaped arg → look up in var_bindings.
            if trimmed
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !trimmed.is_empty()
            {
                var_bindings.get(trimmed).cloned()
            } else {
                None
            }
        });
        let origin = match load_path.as_deref() {
            Some(p) => classify_target_path(p),
            None => TargetOrigin::Unknown,
        };
        let line = byte_to_line(source, m.start());
        // Schema-driven refinement for `set_script(load("res://mods/...")):
        // if the *attached* script is loadable from mod_files AND a
        // vanilla script that this set_script is replacing is known
        // (via a paired take_over_path earlier in the same source),
        // run the function-set comparison.
        let mut severity = severity_for(origin);
        let mut verdict = match origin {
            TargetOrigin::Vanilla => "attaches a vanilla script to a node (rare; usually paired with take_over_path elsewhere)".into(),
            TargetOrigin::ModInternal => "attaches a mod-internal script".into(),
            TargetOrigin::Unknown => "non-literal script arg; can't determine".into(),
        };
        if let (Some(swap_path), Some(schema)) = (load_path.as_deref(), ctx.schema) {
            // The swap-script is mod-internal. Find a paired
            // take_over_path that says which vanilla script is
            // being swapped out. Scans the same source for
            // `take_over_path("res://Scripts/X.gd")`; if there's
            // exactly one vanilla target before this set_script,
            // it's used. Otherwise leave verdict at baseline.
            if let Some(vanilla_target) = nearest_vanilla_take_over_path(source, m.start()) {
                if let Some(vanilla_set) = schema.methods_for(&vanilla_target) {
                    // Resolve the swap-script source from mod_files.
                    if let Some(swap_source) = lookup_mod_file(&ctx.mod_files, swap_path) {
                        if let Ok(parsed) = crabby_parser::parse_script(
                            &swap_path
                                .rsplit('/')
                                .next()
                                .unwrap_or("anon.gd")
                                .to_string(),
                            swap_source,
                        ) {
                            let swap_set: std::collections::BTreeSet<String> =
                                parsed.functions.iter().map(|f| f.name.clone()).collect();
                            let (refined_sev, refined_verdict) =
                                grade_swap_severity(vanilla_set, &swap_set);
                            severity = refined_sev;
                            verdict = format!(
                                "swaps vanilla `{vanilla_target}` with mod script: {refined_verdict}",
                            );
                        }
                    }
                }
            }
        }
        out.classic_patterns.push(ClassicPattern {
            filename: filename.to_string(),
            line,
            pattern: ClassicPatternKind::SetScript,
            severity,
            target: load_path.or_else(|| Some(strip_quotes(arg))),
            verdict,
        });
    }
    record_classic(
        source,
        filename,
        &LOAD_RESOURCE_PACK,
        ClassicPatternKind::LoadResourcePack,
        Severity::Hard,
        out,
    );
    record_classic(
        source,
        filename,
        &EXTENDS_VANILLA,
        ClassicPatternKind::ExtendsVanilla,
        Severity::Warn,
        out,
    );
    record_classic(
        source,
        filename,
        &PRELOAD_SCRIPT,
        ClassicPatternKind::PreloadVanillaScript,
        Severity::Info,
        out,
    );
}

fn record_classic(
    source: &str,
    filename: &str,
    re: &Regex,
    pattern: ClassicPatternKind,
    severity: Severity,
    out: &mut ModIntent,
) {
    for cap in re.captures_iter(source) {
        let target = cap.get(1).map(|m| m.as_str().trim().to_string());
        let target = target.map(strip_quotes);
        let line = byte_to_line(source, cap.get(0).expect("cls match").start());
        let verdict = match pattern {
            ClassicPatternKind::LoadResourcePack => {
                "side-pack mounting bypasses crabby's PCK rewrite".into()
            }
            ClassicPatternKind::ExtendsVanilla => {
                "extends a vanilla script (usually fine but can fight other mods)".into()
            }
            ClassicPatternKind::PreloadVanillaScript => {
                "constructs a vanilla script directly (usually fine)".into()
            }
            _ => String::new(),
        };
        out.classic_patterns.push(ClassicPattern {
            filename: filename.to_string(),
            line,
            pattern,
            severity,
            target,
            verdict,
        });
    }
}

/// Find the nearest `take_over_path("res://Scripts/X.gd")` BEFORE
/// `before_byte` in the source. Returns the literal path string.
/// Heuristic for pairing a `set_script(load(...))` site with the
/// vanilla target it's replacing; works when both sit in the same
/// function (the common case in IXP / similar mods).
fn nearest_vanilla_take_over_path(source: &str, before_byte: usize) -> Option<String> {
    let prefix = &source[..before_byte.min(source.len())];
    let mut found: Option<String> = None;
    for m in TAKE_OVER_PATH_OPEN.find_iter(prefix) {
        let close = find_matching_paren(prefix, m.end())?;
        let arg = prefix[m.end()..close].trim();
        if !arg.starts_with('"') {
            continue;
        }
        let path = strip_quotes(arg.to_string());
        if matches!(classify_target_path(&path), TargetOrigin::Vanilla) {
            found = Some(path);
        }
    }
    found
}

/// Look up a mod-internal `res://...` path in `mod_files`. Strips
/// the `res://` prefix and tries a few suffix-match shapes (the mod
/// author may have packed the file under their mod's internal layout
/// which doesn't match the runtime `res://` path 1:1). Returns the
/// source text on first hit.
fn lookup_mod_file<'a>(
    mod_files: &std::collections::BTreeMap<String, &'a str>,
    res_path: &str,
) -> Option<&'a str> {
    let stripped = res_path.trim_start_matches("res://");
    // Direct match first.
    if let Some(&src) = mod_files.get(stripped) {
        return Some(src);
    }
    // Suffix match, since the in-archive name might omit the leading
    // mod-id segment.
    for (name, &src) in mod_files {
        if name.ends_with(stripped) {
            return Some(src);
        }
        // Or stripped ends with name (mod packed its files at a
        // shallower level than the res:// path implies).
        if stripped.ends_with(name.as_str()) {
            return Some(src);
        }
    }
    None
}

/// Convert a byte offset into the source to a 1-based line number.
fn byte_to_line(source: &str, byte_off: usize) -> usize {
    1 + source[..byte_off.min(source.len())]
        .bytes()
        .filter(|&b| b == b'\n')
        .count()
}

/// Parse the *rest* of a registry call (the args after the literal
/// registry name) into key + payload. Splits at the first top-level
/// comma to peel off the key, treats everything after as payload text.
///
/// `registry_present` reflects whether the regex captured the registry
/// arg, and if it did and key is also a literal, the call is fully
/// AOT-eligible (`Static`). Otherwise `RuntimeOnly`.
fn parse_registry_rest(
    rest: &str,
    registry_present: bool,
) -> (Option<String>, String, Resolvability) {
    if rest.is_empty() {
        return (
            None,
            String::new(),
            if registry_present {
                Resolvability::Static
            } else {
                Resolvability::RuntimeOnly
            },
        );
    }
    let parts = split_top_level_commas(rest, 1);
    let key = parts.first().and_then(|s| string_literal(s));
    let payload_text = if parts.len() > 1 {
        parts[1..].join(", ").trim().to_string()
    } else {
        String::new()
    };
    let resolvability = if registry_present && key.is_some() {
        Resolvability::Static
    } else {
        Resolvability::RuntimeOnly
    };
    (key, payload_text, resolvability)
}

/// Split `args` on commas that aren't nested inside `()`, `[]`, `{}`,
/// or string literals. Caps at `max_splits` splits; remainder lands as
/// the last element.
fn split_top_level_commas(args: &str, max_splits: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut buf = String::new();
    let mut depth_paren = 0i32;
    let mut depth_brack = 0i32;
    let mut depth_brace = 0i32;
    let mut in_str = false;
    let mut esc = false;
    let mut splits_done = 0usize;
    for c in args.chars() {
        if in_str {
            buf.push(c);
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_str = true;
                buf.push(c);
            }
            '(' => {
                depth_paren += 1;
                buf.push(c);
            }
            ')' => {
                depth_paren -= 1;
                buf.push(c);
            }
            '[' => {
                depth_brack += 1;
                buf.push(c);
            }
            ']' => {
                depth_brack -= 1;
                buf.push(c);
            }
            '{' => {
                depth_brace += 1;
                buf.push(c);
            }
            '}' => {
                depth_brace -= 1;
                buf.push(c);
            }
            ',' if depth_paren == 0
                && depth_brack == 0
                && depth_brace == 0
                && splits_done < max_splits =>
            {
                out.push(buf.trim().to_string());
                buf.clear();
                splits_done += 1;
            }
            _ => buf.push(c),
        }
    }
    out.push(buf.trim().to_string());
    out
}

/// If `s` is a `"..."` literal, return its body.
fn string_literal(s: &str) -> Option<String> {
    let s = s.trim();
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        Some(s[1..s.len() - 1].to_string())
    } else {
        None
    }
}

/// Strip outer quotes from a target string, if present.
fn strip_quotes(s: String) -> String {
    let s = s.trim();
    if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

// --- Cross-mod conflict detection --------------------------------------------

/// One detected conflict across the active profile's mods.
///
/// Conflicts are computed from the per-mod `ModIntent`s by
/// [`detect_conflicts`], the cross-mod aggregation that the per-mod
/// scanner can't see on its own.
#[derive(Debug, Clone)]
pub struct Conflict {
    /// What kind of conflict this is.
    pub kind: ConflictKind,
    /// Mods participating in the conflict, in the order they
    /// declared the conflicting thing. Always >= 2 unless
    /// [`ConflictKind::SelfPattern`], a single-mod finding surfaced
    /// through the same channel.
    pub participants: Vec<ConflictParticipant>,
    /// Short human-readable summary for the conflict-list row.
    /// Doesn't repeat per-participant detail (those have their own
    /// `verdict` text on the participant struct).
    pub headline: String,
}

/// One mod's involvement in a conflict.
#[derive(Debug, Clone)]
pub struct ConflictParticipant {
    /// Mod id.
    pub mod_id: String,
    /// File:line where this mod's contribution to the conflict lives.
    pub callsite: String,
    /// Per-participant detail (the verdict text from the underlying
    /// finding when present; empty otherwise).
    pub detail: String,
}

/// Discriminator for [`Conflict`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConflictKind {
    /// Two or more mods write the same `(registry, key)` pair, where
    /// register / override / patch all collide regardless of verb.
    RegistryCollision {
        /// Registry name (e.g. `"items"`).
        registry: String,
        /// Key inside the registry.
        key: String,
    },
    /// Two or more mods register a `Replace` hook on the same name.
    /// The runtime "first-wins" arbitration still applies; this just
    /// surfaces the contention earlier.
    ReplaceHookCollision {
        /// Bare hook name (no `-pre`/`-post`/`-callback`).
        hook_name: String,
    },
    /// Two or more mods swap the same vanilla `res://Scripts/X.gd`
    /// via `take_over_path`. The mods fight each other AND bypass
    /// crabby's substrate.
    DuplicateVanillaSwap {
        /// Vanilla path being swapped (e.g. `res://Scripts/Database.gd`).
        target: String,
    },
    /// Two or more mods declared `replace_file` on the same target
    /// PCK path. File-level replacement is single-owner per path.
    FileReplaceCollision {
        /// Target PCK path (e.g. `"res://Scripts/Player.gd"`).
        target: String,
    },
    /// Two or more mods declared `add_file` for the same new PCK
    /// path. New-file paths are single-owner per path.
    AddFileCollision {
        /// New path the mods are fighting over.
        target: String,
    },
    /// Two or more mods declared `replace_method` for the same
    /// `(target, method)` pair. Method-level replacement is
    /// single-owner per `(target, method)`; one mod replacing the
    /// signature/body cannot coexist with another doing the same.
    MethodReplaceCollision {
        /// Target script (e.g. `"res://Scripts/Player.gd"`).
        target: String,
        /// Method name being replaced.
        method: String,
    },
    /// One mod declared `replace_file` against a target while another
    /// mod declared `replace_method` against the same target. Whole-
    /// file replacement evicts every method on the target, so the
    /// method-level edit silently never runs. Surfaced as Hard so the
    /// author has to disambiguate.
    FileReplaceShadowsMethod {
        /// Target script being fought over.
        target: String,
        /// Method name the method-level edit was trying to replace.
        method: String,
    },
    /// Single-mod finding worth surfacing, a Hard or Warn classic
    /// pattern. Reuses the conflict surface so the UI has one channel
    /// to show "things wrong with this mod."
    SelfPattern {
        /// Underlying classic-pattern kind (TakeOverPath, SetScript,
        /// etc.) so the UI can render an appropriate icon/copy.
        pattern: ClassicPatternKind,
        /// Severity from the underlying finding.
        severity: Severity,
    },
}

/// Aggregate cross-mod intents into a list of conflicts.
///
/// Detects:
/// - **Registry collisions**: same `(registry, key)` from 2+ mods
/// - **Replace-hook collisions**: same bare hook name with `Replace`
///   kind from 2+ mods
/// - **Duplicate vanilla swaps**: 2+ mods `take_over_path` the same
///   vanilla `res://Scripts/X.gd`
/// - **Self-patterns**: per-mod Hard/Warn classic findings, surfaced
///   so the UI's "conflicts" channel covers everything weird about
///   any mod (one place to look)
///
/// Output ordering: registry collisions first (most common + most
/// game-breaking), then replace-hook, then duplicate-swap, then
/// self-patterns. Within each kind, sorted by key for stability.
#[must_use]
pub fn detect_conflicts(intents: &[ModIntent]) -> Vec<Conflict> {
    use std::collections::BTreeMap;

    let mut out: Vec<Conflict> = Vec::new();

    // Registry collisions: bucket by (registry, key).
    let mut reg_buckets: BTreeMap<(String, String), Vec<(String, String, String)>> =
        BTreeMap::new();
    for intent in intents {
        for w in &intent.registry_writes {
            let (Some(r), Some(k)) = (&w.registry, &w.key) else {
                continue;
            };
            reg_buckets
                .entry((r.clone(), k.clone()))
                .or_default()
                .push((
                    intent.mod_id.clone(),
                    format!("{}:{}", w.filename, w.line),
                    format!("{:?}({:?}, {:?})", w.verb, r, k),
                ));
        }
    }
    for ((registry, key), mods) in reg_buckets {
        if mods.len() < 2 {
            continue;
        }
        out.push(Conflict {
            kind: ConflictKind::RegistryCollision {
                registry: registry.clone(),
                key: key.clone(),
            },
            headline: format!("{} mods touch `{}/{}`", mods.len(), registry, key,),
            participants: mods
                .into_iter()
                .map(|(mid, cs, detail)| ConflictParticipant {
                    mod_id: mid,
                    callsite: cs,
                    detail,
                })
                .collect(),
        });
    }

    // Replace-hook collisions: bucket Replace hooks by name.
    let mut replace_buckets: BTreeMap<String, Vec<(String, String, String)>> = BTreeMap::new();
    for intent in intents {
        for h in &intent.hooks {
            if h.kind != HookKind::Replace {
                continue;
            }
            let Some(name) = h.hook_name.as_ref() else {
                continue;
            };
            replace_buckets.entry(name.clone()).or_default().push((
                intent.mod_id.clone(),
                format!("{}:{}", h.filename, h.line),
                h.callable_text.clone(),
            ));
        }
    }
    for (name, mods) in replace_buckets {
        if mods.len() < 2 {
            continue;
        }
        out.push(Conflict {
            kind: ConflictKind::ReplaceHookCollision {
                hook_name: name.clone(),
            },
            headline: format!(
                "{} mods replace `{}` (first-wins arbitration applies at runtime)",
                mods.len(),
                name,
            ),
            participants: mods
                .into_iter()
                .map(|(mid, cs, callable)| ConflictParticipant {
                    mod_id: mid,
                    callsite: cs,
                    detail: format!("replace handler: {}", callable),
                })
                .collect(),
        });
    }

    // Duplicate vanilla swaps: bucket TakeOverPath findings by
    // resolved vanilla target.
    let mut swap_buckets: BTreeMap<String, Vec<(String, String, String)>> = BTreeMap::new();
    for intent in intents {
        for c in &intent.classic_patterns {
            if c.pattern != ClassicPatternKind::TakeOverPath {
                continue;
            }
            let Some(target) = &c.target else { continue };
            // Only track vanilla targets; mod-internal swaps don't
            // collide across mods (each mod owns its own files).
            if !matches!(classify_target_path(target), TargetOrigin::Vanilla) {
                continue;
            }
            swap_buckets.entry(target.clone()).or_default().push((
                intent.mod_id.clone(),
                format!("{}:{}", c.filename, c.line),
                c.verdict.clone(),
            ));
        }
    }
    for (target, mods) in swap_buckets {
        if mods.len() < 2 {
            continue;
        }
        out.push(Conflict {
            kind: ConflictKind::DuplicateVanillaSwap {
                target: target.clone(),
            },
            headline: format!("{} mods swap `{}` via take_over_path", mods.len(), target,),
            participants: mods
                .into_iter()
                .map(|(mid, cs, detail)| ConflictParticipant {
                    mod_id: mid,
                    callsite: cs,
                    detail,
                })
                .collect(),
        });
    }

    // Self-patterns: surface single-mod Hard / Warn classic findings
    // through the conflict channel, but ONLY when a literal vanilla
    // target was resolved. Unresolved-target findings (e.g.
    // `stub.set_script(script)` where `script` is built dynamically
    // via `GDScript.new()`) keep their per-mod severity in the
    // analyzer report but don't trigger the conflict pill, since
    // there's nothing actionable to surface; the attached script is
    // unknown.
    //
    // Also skip Info, since a mod-internal swap on its own files is
    // just the mod being normal.
    for intent in intents {
        for c in &intent.classic_patterns {
            if matches!(c.severity, Severity::Info) {
                continue;
            }
            // Skip findings without a resolved vanilla target.
            // `take_over_path`/`set_script` only surface as conflicts
            // when there's a known vanilla `res://Scripts/X.gd`.
            // Other patterns (LoadResourcePack, ExtendsVanilla,
            // PreloadVanillaScript) still surface, since those don't
            // have the same dynamic-script ambiguity.
            let needs_resolved_target = matches!(
                c.pattern,
                ClassicPatternKind::TakeOverPath | ClassicPatternKind::SetScript,
            );
            if needs_resolved_target {
                let target_is_vanilla = c
                    .target
                    .as_deref()
                    .map(|t| matches!(classify_target_path(t), TargetOrigin::Vanilla))
                    .unwrap_or(false);
                if !target_is_vanilla {
                    continue;
                }
            }
            let target_str = c.target.as_deref().unwrap_or("?");
            let kind_label = match c.pattern {
                ClassicPatternKind::TakeOverPath => "take_over_path",
                ClassicPatternKind::SetScript => "set_script",
                ClassicPatternKind::LoadResourcePack => "load_resource_pack",
                ClassicPatternKind::ExtendsVanilla => "extends vanilla",
                ClassicPatternKind::PreloadVanillaScript => "preload vanilla",
            };
            out.push(Conflict {
                kind: ConflictKind::SelfPattern {
                    pattern: c.pattern,
                    severity: c.severity,
                },
                headline: format!("{}: {kind_label} on `{target_str}`", intent.mod_id,),
                participants: vec![ConflictParticipant {
                    mod_id: intent.mod_id.clone(),
                    callsite: format!("{}:{}", c.filename, c.line),
                    detail: c.verdict.clone(),
                }],
            });
        }
    }

    // Overlay collisions: bucket replace_file and add_file by target
    // path. Each verb is single-owner per path - two mods touching the
    // same path is a hard conflict, since the bake can only apply one.
    //
    // replace_method is bucketed separately by (target, method) since
    // multiple mods can each replace a *different* method on the same
    // target without colliding; collision is per-method-pair.
    let mut replace_buckets: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    let mut add_buckets: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    let mut method_buckets: BTreeMap<(String, String), Vec<(String, String)>> = BTreeMap::new();
    for intent in intents {
        for w in &intent.overlay_writes {
            let Some(target) = w.target_path.as_deref() else {
                continue;
            };
            match w.verb {
                OverlayVerb::ReplaceFile => {
                    replace_buckets
                        .entry(target.to_string())
                        .or_default()
                        .push((intent.mod_id.clone(), format!("{}:{}", w.filename, w.line)));
                }
                OverlayVerb::AddFile => {
                    add_buckets
                        .entry(target.to_string())
                        .or_default()
                        .push((intent.mod_id.clone(), format!("{}:{}", w.filename, w.line)));
                }
                OverlayVerb::ReplaceMethod => {
                    let Some(method) = w.method_name.as_deref() else {
                        continue;
                    };
                    method_buckets
                        .entry((target.to_string(), method.to_string()))
                        .or_default()
                        .push((intent.mod_id.clone(), format!("{}:{}", w.filename, w.line)));
                }
            }
        }
    }
    for (target, mods) in &replace_buckets {
        if mods.len() < 2 {
            continue;
        }
        out.push(Conflict {
            kind: ConflictKind::FileReplaceCollision {
                target: target.clone(),
            },
            headline: format!("{} mods declare `replace_file` on `{}`", mods.len(), target),
            participants: mods
                .iter()
                .map(|(mid, cs)| ConflictParticipant {
                    mod_id: mid.clone(),
                    callsite: cs.clone(),
                    detail: format!("replace_file({target})"),
                })
                .collect(),
        });
    }
    for (target, mods) in add_buckets {
        if mods.len() < 2 {
            continue;
        }
        out.push(Conflict {
            kind: ConflictKind::AddFileCollision {
                target: target.clone(),
            },
            headline: format!("{} mods declare `add_file` on `{}`", mods.len(), target),
            participants: mods
                .into_iter()
                .map(|(mid, cs)| ConflictParticipant {
                    mod_id: mid,
                    callsite: cs,
                    detail: format!("add_file({target})"),
                })
                .collect(),
        });
    }
    for ((target, method), mods) in &method_buckets {
        if mods.len() < 2 {
            continue;
        }
        out.push(Conflict {
            kind: ConflictKind::MethodReplaceCollision {
                target: target.clone(),
                method: method.clone(),
            },
            headline: format!(
                "{} mods declare `replace_method` on `{}#{}`",
                mods.len(),
                target,
                method,
            ),
            participants: mods
                .iter()
                .map(|(mid, cs)| ConflictParticipant {
                    mod_id: mid.clone(),
                    callsite: cs.clone(),
                    detail: format!("replace_method({target}, {method})"),
                })
                .collect(),
        });
    }
    // A replace_file against a target evicts every method on it, so any
    // replace_method against the same target silently no-ops. Surface
    // as a hard conflict so the mod authors disambiguate. One conflict
    // per (target, method) pair, listing both the replace_file owner(s)
    // and the replace_method owner.
    for ((target, method), method_mods) in &method_buckets {
        let Some(file_mods) = replace_buckets.get(target) else {
            continue;
        };
        let mut participants: Vec<ConflictParticipant> = Vec::new();
        for (mid, cs) in file_mods {
            participants.push(ConflictParticipant {
                mod_id: mid.clone(),
                callsite: cs.clone(),
                detail: format!("replace_file({target})"),
            });
        }
        for (mid, cs) in method_mods {
            participants.push(ConflictParticipant {
                mod_id: mid.clone(),
                callsite: cs.clone(),
                detail: format!("replace_method({target}, {method})"),
            });
        }
        out.push(Conflict {
            kind: ConflictKind::FileReplaceShadowsMethod {
                target: target.clone(),
                method: method.clone(),
            },
            headline: format!(
                "`replace_file` on `{}` shadows `replace_method` on `{}`",
                target, method,
            ),
            participants,
        });
    }

    out
}

/// Returns the conflicts a particular mod participates in. Useful for
/// the per-mod detail panel.
#[must_use]
pub fn conflicts_involving<'a>(conflicts: &'a [Conflict], mod_id: &str) -> Vec<&'a Conflict> {
    conflicts
        .iter()
        .filter(|c| c.participants.iter().any(|p| p.mod_id == mod_id))
        .collect()
}

/// Returns true when `mod_id` participates in at least one conflict.
/// Used by the mod-list to decide whether to swap the row's status
/// pill for the conflict pill.
#[must_use]
pub fn mod_has_conflicts(conflicts: &[Conflict], mod_id: &str) -> bool {
    conflicts
        .iter()
        .any(|c| c.participants.iter().any(|p| p.mod_id == mod_id))
}

/// Highest severity among the conflicts a particular mod participates
/// in. Returns `None` when the mod has no conflicts. Used to color
/// the row's conflict pill (red for Hard, amber for Warn) and to
/// decide the default open/closed state of the detail-page panel.
#[must_use]
pub fn mod_max_conflict_severity(conflicts: &[Conflict], mod_id: &str) -> Option<Severity> {
    let mut worst: Option<Severity> = None;
    for c in conflicts {
        if !c.participants.iter().any(|p| p.mod_id == mod_id) {
            continue;
        }
        let sev = severity_of(c);
        worst = Some(match worst {
            None => sev,
            Some(prev) => severity_max(prev, sev),
        });
    }
    worst
}

/// True when `mod_id` participates in at least one Hard conflict.
/// Convenience wrapper over [`mod_max_conflict_severity`].
#[must_use]
pub fn mod_has_hard_conflict(conflicts: &[Conflict], mod_id: &str) -> bool {
    matches!(
        mod_max_conflict_severity(conflicts, mod_id),
        Some(Severity::Hard)
    )
}

/// Distill a slice of `ModIntent`s into the set of hook BASE names
/// referenced by any literal-named `hook(...)` call across all mods.
///
/// "Base" here means the `<script>-<method>` form with the kind
/// suffix stripped, e.g. `interface-_ready-post` → `interface-_ready`.
/// That base is what the rewriter compares against to decide whether
/// a vanilla method needs its dispatcher wrapper at all.
///
/// Dynamic-named hooks (`hook(get_name(), cb)`) can't contribute,
/// since their target is unknown at bake time. The wrapper-skip
/// path treats unknowns conservatively: if no static intent claims
/// a method, it gets skipped. Mods needing dynamic hooks must opt
/// out of the AOT wrapper-skip somehow.
#[must_use]
pub fn collect_hooked_method_bases(intents: &[ModIntent]) -> std::collections::HashSet<String> {
    let mut out: std::collections::HashSet<String> = std::collections::HashSet::new();
    for intent in intents {
        for h in &intent.hooks {
            let Some(name) = h.hook_name.as_deref() else {
                continue;
            };
            // Strip the kind suffix to get the base. Replace hooks
            // (no suffix) are already the base.
            let base: &str = name
                .strip_suffix("-pre")
                .or_else(|| name.strip_suffix("-post"))
                .or_else(|| name.strip_suffix("-callback"))
                .unwrap_or(name);
            out.insert(base.to_string());
        }
    }
    out
}

/// Per-base flags recording which kinds of hook are actually
/// registered against a hook BASE name across all enabled mods.
///
/// Used to drop dispatch-site emission for kinds that nobody hooks.
/// A method whose base is in the map but where (say) only `pre = true`
/// will emit a wrapper that performs the `-pre` dispatch only, with no
/// replace probe, no `-post`, no `-callback`.
///
/// All four false would mean the base shouldn't be in the map at all,
/// since `collect_hooked_method_kinds` skips entries that decode no kind.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HookKindsPresent {
    /// At least one `-pre` hook is registered for this base.
    pub pre: bool,
    /// At least one `-post` hook is registered.
    pub post: bool,
    /// At least one `-callback` hook is registered.
    pub callback: bool,
    /// At least one bare-name (replace) hook is registered.
    pub replace: bool,
}

impl HookKindsPresent {
    /// Returns `true` when no kind is set. A kindless base means the
    /// wrapper should be skipped entirely.
    #[must_use]
    pub fn is_empty(self) -> bool {
        !(self.pre || self.post || self.callback || self.replace)
    }

    /// Returns `true` when every kind is set. Equivalent to legacy
    /// "wrap everything" behavior, where partial-emit has nothing to elide.
    #[must_use]
    pub fn is_full(self) -> bool {
        self.pre && self.post && self.callback && self.replace
    }
}

/// Distill a slice of `ModIntent`s into a per-base map of which hook
/// kinds are actually present.
///
/// Same input convention as [`collect_hooked_method_bases`]: only
/// literal hook names contribute (dynamic-named hooks can't be
/// classified). Returned map is keyed by base name (`<script>-<method>`,
/// kind suffix stripped).
///
/// Bases that decode at least one kind appear in the map; bases with
/// no decodable kind are absent (matching `collect_hooked_method_bases`
/// semantics). Callers that want the "skip the whole wrapper" check
/// can use `map.contains_key(base)`, equivalent to the old HashSet-based
/// check.
#[must_use]
pub fn collect_hooked_method_kinds(
    intents: &[ModIntent],
) -> std::collections::HashMap<String, HookKindsPresent> {
    collect_hooked_method_kinds_from_refs(intents.iter())
}

/// Same shape as [`collect_hooked_method_kinds`] but accepts any
/// iterator of intent references, including views borrowed from a
/// shared scan (e.g. [`crate::BootScan::enabled_intents`]). Lets
/// callers avoid materializing a `Vec<ModIntent>` just to take its
/// slice.
pub fn collect_hooked_method_kinds_from_refs<'a, I>(
    intents: I,
) -> std::collections::HashMap<String, HookKindsPresent>
where
    I: IntoIterator<Item = &'a ModIntent>,
{
    let mut out: std::collections::HashMap<String, HookKindsPresent> =
        std::collections::HashMap::new();
    for intent in intents {
        for h in &intent.hooks {
            let Some(name) = h.hook_name.as_deref() else {
                continue;
            };
            let (base, kind) = if let Some(b) = name.strip_suffix("-pre") {
                (b, HookKind::Pre)
            } else if let Some(b) = name.strip_suffix("-post") {
                (b, HookKind::Post)
            } else if let Some(b) = name.strip_suffix("-callback") {
                (b, HookKind::Callback)
            } else {
                (name, HookKind::Replace)
            };
            let entry = out.entry(base.to_string()).or_default();
            match kind {
                HookKind::Pre => entry.pre = true,
                HookKind::Post => entry.post = true,
                HookKind::Callback => entry.callback = true,
                HookKind::Replace => entry.replace = true,
                HookKind::Unknown => {}
            }
        }
    }
    out
}

/// Map a [`Conflict`] to its representative severity. Cross-mod
/// collisions (registry / replace / dup-swap) are Warn, since they're
/// real but the runtime still functions; the user just gets a chosen
/// arbitration. SelfPattern reports its own severity.
fn severity_of(c: &Conflict) -> Severity {
    match c.kind {
        ConflictKind::DuplicateVanillaSwap { .. }
        | ConflictKind::FileReplaceCollision { .. }
        | ConflictKind::AddFileCollision { .. }
        | ConflictKind::MethodReplaceCollision { .. }
        | ConflictKind::FileReplaceShadowsMethod { .. } => Severity::Hard,
        ConflictKind::RegistryCollision { .. } | ConflictKind::ReplaceHookCollision { .. } => {
            Severity::Warn
        }
        ConflictKind::SelfPattern { severity, .. } => severity,
    }
}

fn severity_max(a: Severity, b: Severity) -> Severity {
    use Severity::*;
    match (a, b) {
        (Hard, _) | (_, Hard) => Hard,
        (Warn, _) | (_, Warn) => Warn,
        _ => Info,
    }
}

// --- Top-level entry ---------------------------------------------------------

/// Scan every file in `files` and aggregate into one `ModIntent`. The
/// caller is responsible for tagging `mod_id`. No schema → baseline
/// classic-pattern severities.
pub fn analyze_mod<'a>(
    mod_id: &str,
    files: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> ModIntent {
    let collected: Vec<(&str, &str)> = files.into_iter().collect();
    analyze_mod_with_schema(mod_id, collected.iter().copied(), None)
}

/// Like [`analyze_mod`] but uses a vanilla schema to grade
/// `take_over_path` / `set_script` severity by comparing the swap-
/// script's function set against vanilla's. Pass `None` for `schema`
/// to get baseline severities (equivalent to [`analyze_mod`]).
pub fn analyze_mod_with_schema<'a>(
    mod_id: &str,
    files: impl IntoIterator<Item = (&'a str, &'a str)>,
    schema: Option<&VanillaSchema>,
) -> ModIntent {
    // Build the in-mod files map first so each scan can resolve
    // swap-script sources from sibling files.
    let collected: Vec<(&str, &str)> = files.into_iter().collect();
    let mod_files: std::collections::BTreeMap<String, &str> = collected
        .iter()
        .map(|(n, s)| ((*n).to_string(), *s))
        .collect();
    let ctx = ScanContext { schema, mod_files };
    let mut out = ModIntent {
        mod_id: mod_id.to_string(),
        ..Default::default()
    };
    for (filename, source) in &collected {
        scan_source_with_ctx(filename, source, &ctx, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_basic_hook() {
        let src = r#"func _ready():
    _lib.hook("interface-_ready-post", _on_ready)
"#;
        let mut out = ModIntent::default();
        scan_source("M.gd", src, &mut out);
        assert_eq!(out.hooks.len(), 1, "{:#?}", out);
        let h = &out.hooks[0];
        assert_eq!(h.hook_name.as_deref(), Some("interface-_ready-post"));
        assert_eq!(h.kind, HookKind::Post);
        assert_eq!(h.callable_text, "_on_ready");
        assert_eq!(h.resolvability, Resolvability::Static);
    }

    #[test]
    fn picks_up_replace_hook_default_kind() {
        let src = r#"_lib.hook("interface-_ready", cb)"#;
        let mut out = ModIntent::default();
        scan_source("X.gd", src, &mut out);
        assert_eq!(out.hooks[0].kind, HookKind::Replace);
    }

    #[test]
    fn parses_register_with_string_args() {
        let src = r#"_lib.register("items", "my_mod_id", payload)"#;
        let mut out = ModIntent::default();
        scan_source("X.gd", src, &mut out);
        assert_eq!(out.registry_writes.len(), 1);
        let w = &out.registry_writes[0];
        assert_eq!(w.verb, RegistryVerb::Register);
        assert_eq!(w.registry.as_deref(), Some("items"));
        assert_eq!(w.key.as_deref(), Some("my_mod_id"));
        assert_eq!(w.payload_text, "payload");
        assert_eq!(w.resolvability, Resolvability::Static);
    }

    #[test]
    fn flags_take_over_path_vanilla_as_hard() {
        let src = r#"some_resource.take_over_path("res://Scripts/Foo.gd")"#;
        let mut out = ModIntent::default();
        scan_source("X.gd", src, &mut out);
        let p = &out.classic_patterns[0];
        assert_eq!(p.pattern, ClassicPatternKind::TakeOverPath);
        assert_eq!(p.severity, Severity::Hard);
        assert_eq!(p.target.as_deref(), Some("res://Scripts/Foo.gd"));
    }

    #[test]
    fn flags_take_over_path_mod_internal_as_info() {
        let src = r#"thing.take_over_path("res://mods/MyMod/Foo.gd")"#;
        let mut out = ModIntent::default();
        scan_source("X.gd", src, &mut out);
        assert_eq!(out.classic_patterns[0].severity, Severity::Info);
    }

    #[test]
    fn flags_take_over_path_unknown_arg_as_warn() {
        let src = r#"thing.take_over_path(some_var.resource_path)"#;
        let mut out = ModIntent::default();
        scan_source("X.gd", src, &mut out);
        assert_eq!(out.classic_patterns[0].severity, Severity::Warn);
    }

    #[test]
    fn set_script_with_vanilla_load_is_hard() {
        let src = r#"node.set_script(load("res://Scripts/Loader.gd"))"#;
        let mut out = ModIntent::default();
        scan_source("X.gd", src, &mut out);
        let p = &out.classic_patterns[0];
        assert_eq!(p.pattern, ClassicPatternKind::SetScript);
        assert_eq!(p.severity, Severity::Hard);
    }

    #[test]
    fn set_script_with_mod_internal_load_is_info() {
        let src = r#"gameData.set_script(load("res://ImmersiveXP/GameData.gd"))"#;
        let mut out = ModIntent::default();
        scan_source("X.gd", src, &mut out);
        assert_eq!(out.classic_patterns[0].severity, Severity::Info);
    }

    #[test]
    fn set_script_with_unknown_arg_is_warn() {
        let src = r#"node.set_script(my_local_script_var)"#;
        let mut out = ModIntent::default();
        scan_source("X.gd", src, &mut out);
        assert_eq!(out.classic_patterns[0].severity, Severity::Warn);
    }

    #[test]
    fn comments_dont_match() {
        let src = "# Loader.set_script(load(\"res://Scripts/Loader.gd\"))\nfunc f(): pass\n";
        let mut out = ModIntent::default();
        scan_source("X.gd", src, &mut out);
        assert!(
            out.classic_patterns.is_empty(),
            "{:#?}",
            out.classic_patterns
        );
    }

    #[test]
    fn strip_comments_preserves_string_literal_hashes() {
        // A `#` inside a quoted string isn't a comment.
        let stripped = strip_comments(r#"var x = "color #ff00ff" # real comment"#);
        assert!(stripped.contains("\"color #ff00ff\""));
        assert!(!stripped.contains("real comment"));
    }

    #[test]
    fn flags_extends_vanilla() {
        let src = r#"extends "res://Scripts/Inventory.gd""#;
        let mut out = ModIntent::default();
        scan_source("X.gd", src, &mut out);
        assert_eq!(out.classic_patterns.len(), 1);
        assert_eq!(
            out.classic_patterns[0].pattern,
            ClassicPatternKind::ExtendsVanilla
        );
    }

    #[test]
    fn flags_load_resource_pack() {
        let src = r#"ProjectSettings.load_resource_pack("user://my.zip")"#;
        let mut out = ModIntent::default();
        scan_source("X.gd", src, &mut out);
        assert_eq!(
            out.classic_patterns[0].pattern,
            ClassicPatternKind::LoadResourcePack
        );
        assert_eq!(out.classic_patterns[0].severity, Severity::Hard);
    }

    #[test]
    fn line_numbers_are_1_based() {
        let src = "line one\nline two\n_lib.hook(\"a-b\", c)\n";
        let mut out = ModIntent::default();
        scan_source("X.gd", src, &mut out);
        assert_eq!(out.hooks[0].line, 3);
    }

    #[test]
    fn split_handles_nested_parens() {
        let parts = split_top_level_commas(r#""items", "id", Callable(self, "x")"#, 2);
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], "\"items\"");
        assert_eq!(parts[1], "\"id\"");
        assert_eq!(parts[2], "Callable(self, \"x\")");
    }

    fn schema_with(pairs: &[(&str, &[&str])]) -> VanillaSchema {
        let mut m = std::collections::BTreeMap::new();
        for (file, methods) in pairs {
            m.insert(
                (*file).to_string(),
                methods.iter().map(|s| s.to_string()).collect(),
            );
        }
        VanillaSchema::new(m)
    }

    #[test]
    fn schema_grades_additive_swap_as_info() {
        // Mod swaps Database.gd with a script that defines NEW methods.
        // 0 vanilla overlap → additive → Info.
        let main = r#"func _ready():
    var script = preload("res://MyMod/ExtraDB.gd")
    script.take_over_path("res://Scripts/Database.gd")
    Database.set_script(script)
"#;
        let extra_db =
            "extends Resource\n\nfunc my_extra_method():\n\tpass\n\nfunc another():\n\tpass\n";
        let schema = schema_with(&[("Database.gd", &["_ready", "Save", "Load"])]);
        let intent = analyze_mod_with_schema(
            "mymod",
            [("Main.gd", main), ("ExtraDB.gd", extra_db)],
            Some(&schema),
        );
        // The set_script finding should be Info with the additive verdict.
        let ss = intent
            .classic_patterns
            .iter()
            .find(|c| c.pattern == ClassicPatternKind::SetScript)
            .expect("set_script finding present");
        assert_eq!(ss.severity, Severity::Info, "{ss:#?}");
        assert!(ss.verdict.contains("additive"), "{}", ss.verdict);
    }

    #[test]
    fn schema_grades_overriding_swap_as_warn() {
        // Mod's swap script defines a method that vanilla also has →
        // overrides → Warn.
        let main = r#"func _ready():
    var script = preload("res://MyMod/Replacement.gd")
    script.take_over_path("res://Scripts/Database.gd")
    Database.set_script(script)
"#;
        let replacement = "extends Resource\n\nfunc Save():\n\tpass\n\nfunc _ready():\n\tpass\n";
        let schema = schema_with(&[("Database.gd", &["_ready", "Save", "Load"])]);
        let intent = analyze_mod_with_schema(
            "mymod",
            [("Main.gd", main), ("Replacement.gd", replacement)],
            Some(&schema),
        );
        let ss = intent
            .classic_patterns
            .iter()
            .find(|c| c.pattern == ClassicPatternKind::SetScript)
            .expect("set_script finding");
        assert_eq!(ss.severity, Severity::Warn, "{ss:#?}");
        assert!(ss.verdict.contains("overrides"), "{}", ss.verdict);
        // Verdict should mention the overlapping method names.
        assert!(ss.verdict.contains("Save"), "{}", ss.verdict);
    }

    #[test]
    fn schema_absent_keeps_baseline_severity() {
        // Without a schema, the existing Hard/Info/Warn baselines apply.
        let main = r#"node.set_script(load("res://Scripts/Loader.gd"))"#;
        let intent = analyze_mod_with_schema("m", [("Main.gd", main)], None);
        assert_eq!(intent.classic_patterns[0].severity, Severity::Hard);
    }

    #[test]
    fn detects_registry_collision() {
        let a = analyze_mod(
            "mod-a",
            [("a.gd", r#"_lib.register("items", "shared", payload_a)"#)],
        );
        let b = analyze_mod(
            "mod-b",
            [("b.gd", r#"_lib.register("items", "shared", payload_b)"#)],
        );
        let conflicts = detect_conflicts(&[a, b]);
        let reg = conflicts
            .iter()
            .find(|c| matches!(c.kind, ConflictKind::RegistryCollision { .. }));
        assert!(reg.is_some(), "{conflicts:#?}");
        assert_eq!(reg.unwrap().participants.len(), 2);
    }

    #[test]
    fn no_collision_for_different_keys() {
        let a = analyze_mod("a", [("a.gd", r#"_lib.register("items", "key_a", x)"#)]);
        let b = analyze_mod("b", [("b.gd", r#"_lib.register("items", "key_b", y)"#)]);
        let conflicts = detect_conflicts(&[a, b]);
        assert!(
            !conflicts
                .iter()
                .any(|c| matches!(c.kind, ConflictKind::RegistryCollision { .. })),
            "{conflicts:#?}",
        );
    }

    #[test]
    fn detects_replace_hook_collision() {
        let a = analyze_mod(
            "a",
            [("a.gd", r#"_lib.hook("interface-_ready", _on_ready_a)"#)],
        );
        let b = analyze_mod(
            "b",
            [("b.gd", r#"_lib.hook("interface-_ready", _on_ready_b)"#)],
        );
        let conflicts = detect_conflicts(&[a, b]);
        assert!(
            conflicts
                .iter()
                .any(|c| matches!(c.kind, ConflictKind::ReplaceHookCollision { .. })),
            "{conflicts:#?}",
        );
    }

    #[test]
    fn pre_post_dont_collide() {
        // -pre / -post chain freely; no collision.
        let a = analyze_mod("a", [("a.gd", r#"_lib.hook("interface-_ready-pre", a)"#)]);
        let b = analyze_mod("b", [("b.gd", r#"_lib.hook("interface-_ready-pre", b)"#)]);
        let conflicts = detect_conflicts(&[a, b]);
        assert!(
            !conflicts
                .iter()
                .any(|c| matches!(c.kind, ConflictKind::ReplaceHookCollision { .. })),
            "pre hooks shouldn't collide as Replace: {conflicts:#?}",
        );
    }

    #[test]
    fn detects_duplicate_vanilla_swap() {
        let a = analyze_mod(
            "a",
            [("a.gd", r#"x.take_over_path("res://Scripts/Database.gd")"#)],
        );
        let b = analyze_mod(
            "b",
            [("b.gd", r#"y.take_over_path("res://Scripts/Database.gd")"#)],
        );
        let conflicts = detect_conflicts(&[a, b]);
        assert!(
            conflicts
                .iter()
                .any(|c| matches!(c.kind, ConflictKind::DuplicateVanillaSwap { .. })),
            "{conflicts:#?}",
        );
    }

    #[test]
    fn self_pattern_surfaces_for_hard_severity() {
        let a = analyze_mod(
            "a",
            [("a.gd", r#"x.take_over_path("res://Scripts/Database.gd")"#)],
        );
        let conflicts = detect_conflicts(&[a]);
        assert!(
            conflicts
                .iter()
                .any(|c| matches!(c.kind, ConflictKind::SelfPattern { .. })),
            "{conflicts:#?}",
        );
    }

    #[test]
    fn self_pattern_skipped_for_info() {
        // A mod-internal `take_over_path` is Info; shouldn't show up
        // as a self-pattern conflict (too noisy, not actionable).
        let a = analyze_mod(
            "a",
            [("a.gd", r#"x.take_over_path("res://mods/a/foo.gd")"#)],
        );
        let conflicts = detect_conflicts(&[a]);
        assert!(
            !conflicts
                .iter()
                .any(|c| matches!(c.kind, ConflictKind::SelfPattern { .. })),
            "info-severity shouldn't surface as conflict: {conflicts:#?}",
        );
    }

    #[test]
    fn self_pattern_skipped_for_unknown_target() {
        // Real-world: TI builds a stub script via `GDScript.new()` +
        // `script.source_code = "..."` then `stub.set_script(script)`.
        // The analyzer can't resolve `script` to a literal `res://`
        // path, so the user shouldn't be alarmed with a conflict
        // (nothing actionable to say).
        let src = r#"func make_stub():
    var script := GDScript.new()
    script.source_code = "extends Control"
    script.reload()
    var stub = Control.new()
    stub.set_script(script)
"#;
        let intent = analyze_mod("dynamic-stub-mod", [("Main.gd", src)]);
        // Per-mod finding still exists at Warn (Unknown target).
        assert!(!intent.classic_patterns.is_empty());
        // But it shouldn't surface as a SelfPattern conflict.
        let conflicts = detect_conflicts(&[intent]);
        assert!(
            !conflicts
                .iter()
                .any(|c| matches!(c.kind, ConflictKind::SelfPattern { .. })),
            "unknown-target finding shouldn't trigger conflict pill: {conflicts:#?}",
        );
    }

    #[test]
    fn collect_hooked_bases_strips_kind_suffix() {
        let a = analyze_mod(
            "a",
            [(
                "a.gd",
                r#"_lib.hook("interface-_ready-post", cb)
_lib.hook("interface-close-pre", cb)
_lib.hook("ai-death", cb)
"#,
            )],
        );
        let bases = collect_hooked_method_bases(&[a]);
        assert!(bases.contains("interface-_ready"));
        assert!(bases.contains("interface-close"));
        assert!(bases.contains("ai-death"));
        // No suffixed entries leaked through.
        assert!(!bases.contains("interface-_ready-post"));
        assert!(!bases.contains("interface-close-pre"));
        assert_eq!(bases.len(), 3);
    }

    #[test]
    fn collect_hooked_bases_dedupes_across_mods_and_kinds() {
        let a = analyze_mod("a", [("a.gd", r#"_lib.hook("interface-_ready-pre", cb)"#)]);
        let b = analyze_mod("b", [("b.gd", r#"_lib.hook("interface-_ready-post", cb)"#)]);
        let bases = collect_hooked_method_bases(&[a, b]);
        assert_eq!(bases.len(), 1);
        assert!(bases.contains("interface-_ready"));
    }

    #[test]
    fn collect_hooked_kinds_separates_each_kind() {
        let a = analyze_mod(
            "a",
            [(
                "a.gd",
                r#"_lib.hook("interface-_ready-pre", cb)
_lib.hook("interface-_ready-post", cb)
_lib.hook("ai-death", cb)
_lib.hook("compiler-spawn-callback", cb)
"#,
            )],
        );
        let kinds = collect_hooked_method_kinds(&[a]);
        let intf = kinds.get("interface-_ready").expect("interface key");
        assert!(intf.pre);
        assert!(intf.post);
        assert!(!intf.callback);
        assert!(!intf.replace);
        let ai = kinds.get("ai-death").expect("ai key");
        assert!(!ai.pre);
        assert!(!ai.post);
        assert!(!ai.callback);
        assert!(ai.replace);
        let comp = kinds.get("compiler-spawn").expect("compiler key");
        assert!(comp.callback);
        assert!(!comp.replace);
    }

    #[test]
    fn collect_hooked_kinds_unions_across_mods() {
        let a = analyze_mod("a", [("a.gd", r#"_lib.hook("interface-_ready-pre", cb)"#)]);
        let b = analyze_mod("b", [("b.gd", r#"_lib.hook("interface-_ready-post", cb)"#)]);
        let kinds = collect_hooked_method_kinds(&[a, b]);
        let intf = kinds.get("interface-_ready").expect("interface key");
        assert!(intf.pre, "mod a contributed pre");
        assert!(intf.post, "mod b contributed post");
        assert!(!intf.callback);
        assert!(!intf.replace);
    }

    #[test]
    fn hook_kinds_present_helpers() {
        let empty = HookKindsPresent::default();
        assert!(empty.is_empty());
        assert!(!empty.is_full());
        let full = HookKindsPresent {
            pre: true,
            post: true,
            callback: true,
            replace: true,
        };
        assert!(!full.is_empty());
        assert!(full.is_full());
        let partial = HookKindsPresent {
            pre: true,
            ..Default::default()
        };
        assert!(!partial.is_empty());
        assert!(!partial.is_full());
    }

    // ---- New-verb scanner tests ----

    #[test]
    fn many_verb_register_many_counts_as_registry_intent() {
        let m = analyze_mod(
            "m",
            [(
                "m.gd",
                r#"_lib.register_many("items", {"a": data_a, "b": data_b})"#,
            )],
        );
        assert_eq!(m.registry_writes.len(), 1);
        let w = &m.registry_writes[0];
        assert_eq!(w.verb, RegistryVerb::Register);
        assert_eq!(w.registry.as_deref(), Some("items"));
        assert_eq!(w.payload_text, "<many>");
    }

    #[test]
    fn many_verb_array_ops_carry_registry_name() {
        let m = analyze_mod(
            "m",
            [(
                "m.gd",
                r#"_lib.append_many("items", "compatible", {"AKM": [m1, m2]})
_lib.remove_from_many("recipes", "ingredients", {"r1": [i1]})"#,
            )],
        );
        assert_eq!(m.registry_writes.len(), 2);
        let verbs: Vec<_> = m.registry_writes.iter().map(|w| w.verb).collect();
        assert!(verbs.contains(&RegistryVerb::Append));
        assert!(verbs.contains(&RegistryVerb::RemoveFrom));
    }

    #[test]
    fn aggregator_calls_recognized() {
        let m = analyze_mod(
            "m",
            [(
                "m.gd",
                r#"_lib.register_weapon({"AKM": {item_path = "..."}})
_lib.register_ai_loadout({"AKM_Bandit": {weapon_scene = "AKM"}})
_lib.register_furniture({"Table": {item_path = "..."}})"#,
            )],
        );
        assert_eq!(m.registry_writes.len(), 3);
        let kinds: Vec<&str> = m
            .registry_writes
            .iter()
            .filter_map(|w| w.registry.as_deref())
            .collect();
        assert!(kinds.contains(&"weapons"));
        assert!(kinds.contains(&"ai_loadouts"));
        assert!(kinds.contains(&"furniture"));
    }

    #[test]
    fn hook_many_emits_one_intent_per_dict_key() {
        let m = analyze_mod(
            "m",
            [(
                "m.gd",
                r#"_lib.hook_many({
    "interface-_ready-pre": _on_ready,
    "ai-changestate-post": _on_state,
    "controller-_physics_process-pre": _tick,
})"#,
            )],
        );
        assert_eq!(m.hooks.len(), 3);
        let names: Vec<&str> = m
            .hooks
            .iter()
            .filter_map(|h| h.hook_name.as_deref())
            .collect();
        assert!(names.contains(&"interface-_ready-pre"));
        assert!(names.contains(&"ai-changestate-post"));
        assert!(names.contains(&"controller-_physics_process-pre"));
    }

    #[test]
    fn setup_plan_primitive_verbs_become_intents() {
        let m = analyze_mod(
            "m",
            [(
                "m.gd",
                r#"_lib.setup([
    ["register", "items", {"my_item": data}],
    ["patch", "items", {"AKM": {damage = 200}}],
    ["override", "scenes", {"Potato": new_potato}],
])"#,
            )],
        );
        assert_eq!(m.registry_writes.len(), 3);
        // Make sure the verbs + registries came through correctly.
        let pairs: Vec<(RegistryVerb, &str)> = m
            .registry_writes
            .iter()
            .filter_map(|w| w.registry.as_deref().map(|r| (w.verb, r)))
            .collect();
        assert!(pairs.contains(&(RegistryVerb::Register, "items")));
        assert!(pairs.contains(&(RegistryVerb::Patch, "items")));
        assert!(pairs.contains(&(RegistryVerb::Override, "scenes")));
    }

    #[test]
    fn setup_plan_aggregator_entries_recognized() {
        let m = analyze_mod(
            "m",
            [(
                "m.gd",
                r#"_lib.setup([
    ["register_weapon", {"AKM": {item_path = "..."}}],
    ["register_ai_loadout", {"AKM_Bandit": {weapon_scene = "AKM"}}],
])"#,
            )],
        );
        assert_eq!(m.registry_writes.len(), 2);
        let kinds: Vec<&str> = m
            .registry_writes
            .iter()
            .filter_map(|w| w.registry.as_deref())
            .collect();
        assert!(kinds.contains(&"weapons"));
        assert!(kinds.contains(&"ai_loadouts"));
    }

    #[test]
    fn setup_plan_hooks_block_emits_hook_intents() {
        let m = analyze_mod(
            "m",
            [(
                "m.gd",
                r#"_lib.setup([
    ["hooks", {
        "interface-_ready-post": _setup_ui,
        "ai-changestate-pre": _on_state,
    }],
])"#,
            )],
        );
        assert_eq!(m.hooks.len(), 2);
        let names: Vec<&str> = m
            .hooks
            .iter()
            .filter_map(|h| h.hook_name.as_deref())
            .collect();
        assert!(names.contains(&"interface-_ready-post"));
        assert!(names.contains(&"ai-changestate-pre"));
    }

    #[test]
    fn setup_plan_when_block_recurses() {
        let m = analyze_mod(
            "m",
            [(
                "m.gd",
                r#"_lib.setup([
    ["when", true, [
        ["register", "items", {"conditional_item": data}],
    ]],
])"#,
            )],
        );
        // The inner register entry should be picked up despite being
        // wrapped in a when-block.
        assert_eq!(m.registry_writes.len(), 1);
        let w = &m.registry_writes[0];
        assert_eq!(w.verb, RegistryVerb::Register);
        assert_eq!(w.registry.as_deref(), Some("items"));
    }

    #[test]
    fn setup_plan_overlay_verbs_dont_lose_replace_when_other_files_present() {
        // Regression: in a real bake the analyzer reported add_file=1
        // but replace_file=0 for the overlay-test mod. The mod ships
        // three .gd files (main.gd, overlays/Item.gd,
        // overlays/OverlayTestHelper.gd). analyze_mod sees ALL of
        // them. Confirm that the overlay verbs in main.gd survive
        // when the analyzer also scans the overlay scripts.
        let main_gd = r#"extends Node

func _ready() -> void:
    Lib.setup([
        ["replace_file", "res://Scripts/Item.gd", "res://overlays/Item.gd"],
        ["add_file", "res://overlays/OverlayTestHelper.gd", "res://overlays/OverlayTestHelper.gd"],
    ])
"#;
        // A vanilla-shaped Item.gd, big and full of unrelated code.
        let vanilla_item_like = r#"extends Control
class_name Item

@export var slotData: SlotData
var interface = null
var dragging = false
var size = 0
var name = ""

func Initialize(source, data):
    interface = source
    slotData.Update(data)
    name = slotData.itemData.file
    size = slotData.itemData.size * 64

func Value() -> int:
    var value = slotData.itemData.value
    return int(roundf(value))
"#;
        let helper_gd = r#"extends RefCounted
func status() -> String:
    return "alive"
"#;
        let m = analyze_mod(
            "crabby-overlay-test",
            [
                ("main.gd", main_gd),
                ("overlays/Item.gd", vanilla_item_like),
                ("overlays/OverlayTestHelper.gd", helper_gd),
            ],
        );
        let by_verb: std::collections::HashMap<OverlayVerb, &OverlayWriteIntent> =
            m.overlay_writes.iter().map(|w| (w.verb, w)).collect();
        assert!(
            by_verb.contains_key(&OverlayVerb::ReplaceFile),
            "replace_file dropped when overlay scripts also scanned. overlay_writes = {:?}",
            m.overlay_writes,
        );
        assert!(
            by_verb.contains_key(&OverlayVerb::AddFile),
            "add_file dropped. overlay_writes = {:?}",
            m.overlay_writes,
        );
    }

    #[test]
    fn setup_plan_overlay_verbs_match_real_test_mod_layout() {
        // Regression: replace_file was reported as "0 replaced" in
        // a real bake even though add_file came through as 1. Test
        // against the exact source layout the test mod ships with
        // (preceding comments, indentation, multi-line plan).
        let m = analyze_mod(
            "crabby-overlay-test",
            [(
                "main.gd",
                r#"# crabby-overlay-test: exercises both overlay verbs end-to-end.
#
# Comment block above setup() sometimes throws regex matchers off.
extends Node

func _ready() -> void:
    Lib.setup([
        ["replace_file", "res://Scripts/Item.gd", "res://overlays/Item.gd"],
        ["add_file", "res://overlays/OverlayTestHelper.gd", "res://overlays/OverlayTestHelper.gd"],
    ])
"#,
            )],
        );
        let by_verb: std::collections::HashMap<OverlayVerb, &OverlayWriteIntent> =
            m.overlay_writes.iter().map(|w| (w.verb, w)).collect();
        assert!(
            by_verb.contains_key(&OverlayVerb::ReplaceFile),
            "replace_file not detected in test-mod-shaped source. overlay_writes = {:?}",
            m.overlay_writes,
        );
        assert!(
            by_verb.contains_key(&OverlayVerb::AddFile),
            "add_file not detected. overlay_writes = {:?}",
            m.overlay_writes,
        );
    }

    #[test]
    fn setup_plan_overlay_verbs_become_intents() {
        let m = analyze_mod(
            "m",
            [(
                "m.gd",
                r#"_lib.setup([
    ["replace_file", "res://Scripts/Player.gd", "res://overlays/Player.gd"],
    ["add_file", "res://Scripts/CoopSync.gd", "res://overlays/CoopSync.gd"],
])"#,
            )],
        );
        assert_eq!(m.overlay_writes.len(), 2);

        let by_verb: std::collections::HashMap<OverlayVerb, &OverlayWriteIntent> =
            m.overlay_writes.iter().map(|w| (w.verb, w)).collect();

        let replace = by_verb
            .get(&OverlayVerb::ReplaceFile)
            .expect("replace_file intent");
        assert_eq!(
            replace.target_path.as_deref(),
            Some("res://Scripts/Player.gd")
        );
        assert_eq!(
            replace.source_path.as_deref(),
            Some("res://overlays/Player.gd")
        );

        let add = by_verb.get(&OverlayVerb::AddFile).expect("add_file intent");
        assert_eq!(
            add.target_path.as_deref(),
            Some("res://Scripts/CoopSync.gd")
        );
        assert_eq!(
            add.source_path.as_deref(),
            Some("res://overlays/CoopSync.gd")
        );
    }

    #[test]
    fn setup_plan_overlay_verb_with_non_literal_args_skips_silently() {
        // The scanner only matches literal-string args. A computed
        // target path produces no intent, mirroring how registry verbs
        // with non-literal keys behave.
        let m = analyze_mod(
            "m",
            [(
                "m.gd",
                r#"_lib.setup([
    ["replace_file", target_var, "res://overlays/Player.gd"],
])"#,
            )],
        );
        // Verb still recognized, but with both paths as None since the
        // two-string capture failed.
        assert_eq!(m.overlay_writes.len(), 1);
        assert_eq!(m.overlay_writes[0].verb, OverlayVerb::ReplaceFile);
        assert_eq!(m.overlay_writes[0].target_path, None);
        assert_eq!(m.overlay_writes[0].source_path, None);
    }

    #[test]
    fn detect_conflicts_replace_file_collision() {
        let a = analyze_mod(
            "a",
            [(
                "a.gd",
                r#"_lib.setup([
    ["replace_file", "res://Scripts/Player.gd", "res://a/Player.gd"],
])"#,
            )],
        );
        let b = analyze_mod(
            "b",
            [(
                "b.gd",
                r#"_lib.setup([
    ["replace_file", "res://Scripts/Player.gd", "res://b/Player.gd"],
])"#,
            )],
        );
        let c = analyze_mod(
            "c",
            [(
                "c.gd",
                r#"_lib.setup([
    ["replace_file", "res://Scripts/Other.gd", "res://c/Other.gd"],
])"#,
            )],
        );
        let conflicts = detect_conflicts(&[a, b, c]);
        let collision = conflicts
            .iter()
            .find(|c| matches!(c.kind, ConflictKind::FileReplaceCollision { .. }))
            .expect("replace_file collision detected");
        assert_eq!(collision.participants.len(), 2);
        let mod_ids: Vec<&str> = collision
            .participants
            .iter()
            .map(|p| p.mod_id.as_str())
            .collect();
        assert!(mod_ids.contains(&"a"));
        assert!(mod_ids.contains(&"b"));
        // Third mod targets a different path, so it doesn't participate.
        assert!(!mod_ids.contains(&"c"));
    }

    #[test]
    fn detect_conflicts_add_file_collision() {
        let a = analyze_mod(
            "a",
            [(
                "a.gd",
                r#"_lib.setup([
    ["add_file", "res://Scripts/Coop.gd", "res://a/Coop.gd"],
])"#,
            )],
        );
        let b = analyze_mod(
            "b",
            [(
                "b.gd",
                r#"_lib.setup([
    ["add_file", "res://Scripts/Coop.gd", "res://b/Coop.gd"],
])"#,
            )],
        );
        let conflicts = detect_conflicts(&[a, b]);
        let collision = conflicts
            .iter()
            .find(|c| matches!(c.kind, ConflictKind::AddFileCollision { .. }))
            .expect("add_file collision detected");
        assert_eq!(collision.participants.len(), 2);
    }

    #[test]
    fn detect_conflicts_overlay_collisions_are_hard_severity() {
        let a = analyze_mod(
            "a",
            [(
                "a.gd",
                r#"_lib.setup([
    ["replace_file", "res://Scripts/Player.gd", "res://a/Player.gd"],
])"#,
            )],
        );
        let b = analyze_mod(
            "b",
            [(
                "b.gd",
                r#"_lib.setup([
    ["replace_file", "res://Scripts/Player.gd", "res://b/Player.gd"],
])"#,
            )],
        );
        let conflicts = detect_conflicts(&[a, b]);
        // Hard severity gates installation in the launcher.
        assert!(mod_has_hard_conflict(&conflicts, "a"));
        assert!(mod_has_hard_conflict(&conflicts, "b"));
    }

    #[test]
    fn mod_has_conflicts_helper() {
        let a = analyze_mod("a", [("a.gd", r#"_lib.register("items", "shared", x)"#)]);
        let b = analyze_mod("b", [("b.gd", r#"_lib.register("items", "shared", y)"#)]);
        let c = analyze_mod("c", [("c.gd", r#"_lib.register("items", "unique", z)"#)]);
        let conflicts = detect_conflicts(&[a, b, c]);
        assert!(mod_has_conflicts(&conflicts, "a"));
        assert!(mod_has_conflicts(&conflicts, "b"));
        assert!(!mod_has_conflicts(&conflicts, "c"));
    }
}
