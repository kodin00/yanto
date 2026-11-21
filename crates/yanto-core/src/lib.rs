//! Domain types, service layer, config and database for Yanto.

pub mod config;
pub mod error;

pub use config::Config;
pub use error::{CoreError, ErrorKind};
