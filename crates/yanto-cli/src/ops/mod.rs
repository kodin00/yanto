//! Host-facing operations shared by `install`, `doctor`, `self-update` and
//! `uninstall` (docs/rewrite/05). Everything that touches the machine goes
//! through [`host::Host`] and [`layout::Layout`] so tests can run against a
//! temp directory with fakes.

pub mod checks;
pub mod dbcopy;
pub mod host;
pub mod install;
pub mod layout;
pub mod params;
pub mod prompt;
pub mod release;
pub mod render;
pub mod setup_code;
pub mod uninstall;
pub mod update;
pub mod wizard;
