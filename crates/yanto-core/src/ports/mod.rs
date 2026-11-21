//! Port traits: every side effect the service layer needs from the outside
//! world (time, Docker, filesystem writes under `/etc`, ...). Real
//! implementations live in `yanto-host` (or here when trivial); fakes live in
//! [`crate::testing`]. See docs/rewrite/08-conventions.md "Ports and fakes".
//!
//! One file per concern, one `pub mod` line per file below.

mod clock;

pub use clock::{Clock, SystemClock};
