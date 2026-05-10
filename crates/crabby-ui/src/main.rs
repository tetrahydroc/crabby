//! `crabby` - unified entry point.
//!
//! Dispatch:
//! - `crabby` (no args) or `crabby --game-dir <path>` → launch the iced UI.
//! - `crabby <subcommand> ...` (install / uninstall / doctor / mods /
//!   help / --help / --version) → forward to the CLI handler.
//! - `crabby ui [--game-dir <path>]` → explicit UI mode.
//!
//! On Windows the binary is built as the `windows` subsystem so
//! double-clicking from explorer doesn't allocate a console window.
//! When CLI mode is selected we attach to the parent console (if any)
//! so output lands in the calling shell. UI mode never touches the
//! console.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::process::ExitCode;

use crabby_ui::launcher_config;
use crabby_ui::App;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

fn main() -> ExitCode {
    // Decide UI vs CLI based on the first positional arg. This happens
    // *before* any tracing setup because the CLI installs its own
    // subscriber and a duplicate registration would panic.
    let raw_args: Vec<String> = std::env::args().skip(1).collect();
    let first = raw_args.iter().find(|a| !a.starts_with('-')).cloned();
    let first_flag = raw_args.iter().find(|a| a.starts_with('-')).cloned();

    // Explicit `crabby ui` always opens the UI (and skips the arg from
    // reaching iced). Treat `--game-dir` and friends as UI flags.
    if first.as_deref() == Some("ui") {
        return run_ui(strip_first_match(&raw_args, "ui"));
    }

    // Any known CLI subcommand → forward to clap.
    if let Some(arg) = first.as_deref() {
        if crabby_cli::CLI_SUBCOMMANDS.iter().any(|c| *c == arg) {
            attach_parent_console_on_windows();
            return crabby_cli::run();
        }
    }
    // Or a CLI-style flag with no subcommand (e.g. `--help`, `--version`).
    if first.is_none() {
        if let Some(flag) = first_flag.as_deref() {
            if crabby_cli::CLI_SUBCOMMANDS.iter().any(|c| *c == flag) {
                attach_parent_console_on_windows();
                return crabby_cli::run();
            }
        }
    }

    // Default: launch the UI.
    run_ui(raw_args)
}

/// Remove the first occurrence of `needle` from `args` (used to strip
/// the `ui` subcommand before forwarding the rest to UI flag parsing).
fn strip_first_match(args: &[String], needle: &str) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    let mut stripped = false;
    for a in args {
        if !stripped && a == needle {
            stripped = true;
            continue;
        }
        out.push(a.clone());
    }
    out
}

/// Boot the iced UI. Returns the process exit code.
fn run_ui(args: Vec<String>) -> ExitCode {
    let cli_override = parse_game_dir_arg(&args);

    // Tracing has two layers stacked under one filter:
    //   1. stderr (human-readable) - useful when launched from a terminal.
    //   2. file (JSON-lines, daily-rotated) - feeds the in-app Logs tab.
    //
    // The `_guard` keeps tracing-appender's background flush thread
    // alive for the process lifetime; dropping it would silently
    // discard buffered log lines on shutdown.
    let env_filter = EnvFilter::try_from_env("CRABBY_LOG")
        .unwrap_or_else(|_| EnvFilter::new("info"));
    let stderr_layer = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);

    let _file_guard = if let Some(log_dir) = launcher_config::log_dir() {
        if let Err(e) = std::fs::create_dir_all(&log_dir) {
            eprintln!("crabby: log dir {} unavailable: {e}", log_dir.display());
            None
        } else {
            let appender = tracing_appender::rolling::daily(
                &log_dir,
                launcher_config::LOG_FILE_PREFIX,
            );
            let (nb_writer, guard) = tracing_appender::non_blocking(appender);
            let file_layer = tracing_subscriber::fmt::layer()
                .json()
                .with_writer(nb_writer)
                .with_ansi(false);
            tracing_subscriber::registry()
                .with(env_filter)
                .with(stderr_layer)
                .with(file_layer)
                .init();
            Some(guard)
        }
    } else {
        // No user-config dir on this platform - stderr only.
        tracing_subscriber::registry()
            .with(env_filter)
            .with(stderr_layer)
            .init();
        None
    };

    // Bundled fonts (see comment block on the previous main.rs revision).
    const INTER_REGULAR: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.ttf");
    const INTER_MEDIUM: &[u8] = include_bytes!("../assets/fonts/Inter-Medium.ttf");
    const INTER_SEMIBOLD: &[u8] = include_bytes!("../assets/fonts/Inter-SemiBold.ttf");
    const INTER_BOLD: &[u8] = include_bytes!("../assets/fonts/Inter-Bold.ttf");
    const JBM_REGULAR: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf");
    const JBM_MEDIUM: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Medium.ttf");
    const JBM_BOLD: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Bold.ttf");
    const TWEMOJI: &[u8] = include_bytes!("../assets/fonts/TwemojiMozilla.ttf");

    let result = iced::application(
        move || -> (App, iced::Task<crabby_ui::Message>) {
            (
                App::new_with_override(cli_override.clone()),
                iced::Task::done(crabby_ui::Message::Refresh),
            )
        },
        App::update,
        App::view,
    )
        .title(App::title)
        .theme(App::theme)
        .subscription(App::subscription)
        .decorations(false)
        .font(INTER_REGULAR)
        .font(INTER_MEDIUM)
        .font(INTER_SEMIBOLD)
        .font(INTER_BOLD)
        .font(JBM_REGULAR)
        .font(JBM_MEDIUM)
        .font(JBM_BOLD)
        .font(TWEMOJI)
        .default_font(iced::Font::with_name("Inter"))
        .run();

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("crabby ui error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Parse `--game-dir <path>` (or `--game-dir=path`) from the supplied
/// arg slice. Used by the UI; scanning `std::env::args` directly is
/// unsafe because some args may have been stripped (e.g. the `ui`
/// subcommand).
fn parse_game_dir_arg(args: &[String]) -> Option<PathBuf> {
    let mut iter = args.iter();
    while let Some(a) = iter.next() {
        if a == "--game-dir" {
            return iter.next().map(PathBuf::from);
        }
        if let Some(rest) = a.strip_prefix("--game-dir=") {
            return Some(PathBuf::from(rest));
        }
    }
    None
}

/// On Windows release builds the binary is the `windows` subsystem so
/// the UI doesn't flash a console. CLI mode then has no console to
/// write to. `AttachConsole(ATTACH_PARENT_PROCESS)` connects stdio to
/// the calling shell when one exists; if `crabby` was launched from
/// explorer with a CLI subcommand (rare), this no-ops and output goes
/// to the file log only.
#[cfg(all(windows, not(debug_assertions)))]
fn attach_parent_console_on_windows() {
    // SAFETY: `AttachConsole` is the documented win32 way to inherit
    // the parent process's console. No invariants beyond "called once
    // before any console IO" - this is at the top of CLI dispatch.
    #[allow(unsafe_code)]
    unsafe {
        windows_sys::Win32::System::Console::AttachConsole(
            windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS,
        );
    }
}

/// Debug builds aren't `windows_subsystem = "windows"` so they already
/// have a console. Non-Windows targets always have stdio attached.
#[cfg(not(all(windows, not(debug_assertions))))]
fn attach_parent_console_on_windows() {}
