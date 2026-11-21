//! Port traits: every side effect the service layer needs from the outside
//! world (time, Docker, filesystem writes under `/etc`, ...). Real
//! implementations live in `yanto-host` (or here when trivial); fakes live in
//! [`crate::testing`]. See docs/rewrite/08-conventions.md "Ports and fakes".
//!
//! One file per concern, one `pub mod` line per file below.

mod clock;
mod docker;
mod host_stats;

pub use clock::{Clock, SystemClock};
pub use docker::*;
pub use host_stats::*;
