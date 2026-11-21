//! Domain types, service layer, config and database for Yanto.

pub mod config;
pub mod db;
pub mod error;

pub use config::Config;
pub use db::Db;
pub use error::{CoreError, ErrorKind};
