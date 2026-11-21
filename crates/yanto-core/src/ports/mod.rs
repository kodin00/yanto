//! Port traits: every side effect the service layer needs from the outside
//! world (time, Docker, filesystem writes under `/etc`, ...). Real
//! implementations live in `yanto-host` (or here when trivial); fakes live in
//! [`crate::testing`]. See docs/rewrite/08-conventions.md "Ports and fakes".
//!
//! One file per concern, one `pub mod` line per file below.

mod clock;
mod command;
mod host_probe;
mod registry_auth;

pub use clock::{Clock, SystemClock};
pub use command::{
    CancellationToken, CommandOutput, CommandPort, CommandSpec, EXIT_CANCELLED, EXIT_TIMED_OUT,
    LogSink, LogStream, NullSink,
};
pub use host_probe::{HostProbePort, Listener};
pub use registry_auth::{AuthEntry, RegistryAuthPort};
