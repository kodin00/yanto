//! The tool surface, one module per feature area. Adding an area (for example M7's
//! Hostnames, Cloudflare and frps) is one `mod` line and one `register` line here.

mod authorized_keys;
mod backups;
mod containers;
mod cron;
mod deployments;
mod hostnames;
mod keys;
mod projects;
mod registry;
mod system;

use super::ToolSet;

/// One line per feature module.
pub fn register(set: &mut ToolSet) {
    system::register(set);
    projects::register(set);
    deployments::register(set);
    containers::register(set);
    backups::register(set);
    keys::register(set);
    registry::register(set);
    cron::register(set);
    authorized_keys::register(set);
    hostnames::register(set);
}

/// `Some(None)` for an explicit JSON `null` (clear), `Some(Some(v))` for a value, `None` when absent.
pub(super) fn double_option<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    <Option<T> as serde::Deserialize>::deserialize(d).map(Some)
}
