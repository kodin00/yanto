//! Real implementations of the core ports (Docker, cron, git, ...). One module per concern;
//! the composition root (`yanto-cli`) wires them in. Host-integration tests live next to
//! the code or in `tests/`; anything needing a real Docker daemon is `#[ignore]`.

pub mod commands;
pub mod probe;
pub mod registry_auth;

pub use commands::TokioCommands;
pub use probe::ProcProbe;
pub use registry_auth::RunDirRegistryAuth;
