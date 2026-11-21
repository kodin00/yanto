//! Project and Deployment machinery behind the service layer: input rules, `.env` handling,
//! Compose helpers, and (below) the runner, queue worker and event hub.

pub mod compose;
pub mod env;
pub mod validate;
