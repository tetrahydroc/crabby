//! Subcommand handlers. One module per subcommand; each exposes a `run`
//! function taking its typed args and returning [`crabby_error::Result<()>`].

pub mod doctor;
pub mod install;
pub mod mods;
pub mod uninstall;
