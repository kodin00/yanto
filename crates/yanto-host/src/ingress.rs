//! Wiring of the real M7 ports for the composition root.

use std::sync::Arc;

use yanto_core::Config;
use yanto_core::ports::{CommandPort, IngressPorts, NoRegistry, RegistryTarget};

use crate::{
    CloudflareHttp, FrpDashboardHttp, HostFrpFiles, HostIngressProbe, HostSystemd, HostTraefik,
    cloudflare::DEFAULT_BASE,
};

/// Real ports for Host Daemons, Cloudflare, Traefik, frps and probes. `registry` is the Hosted
/// Registry hook (production passes `SettingsRegistryTarget`). `owner` is the uid/gid given to files
/// Yanto writes (root in production).
pub fn ingress_ports(
    config: &Config,
    command: Arc<dyn CommandPort>,
    owner: Option<(u32, u32)>,
    registry: Arc<dyn RegistryTarget>,
) -> IngressPorts {
    IngressPorts {
        systemd: Arc::new(HostSystemd::new(command)),
        cloudflare: Arc::new(CloudflareHttp::new(DEFAULT_BASE)),
        traefik: Arc::new(HostTraefik::new(&config.ingress.traefik.dynamic_dir, owner)),
        frp_files: Arc::new(HostFrpFiles),
        frp_dashboard: Arc::new(FrpDashboardHttp::default()),
        probe: Arc::new(HostIngressProbe::default()),
        registry,
    }
}

/// Same as [`ingress_ports`] with no Hosted Registry (registry Hostnames are refused); for
/// harnesses that never register one.
pub fn ingress_ports_without_registry(
    config: &Config,
    command: Arc<dyn CommandPort>,
    owner: Option<(u32, u32)>,
) -> IngressPorts {
    ingress_ports(config, command, owner, Arc::new(NoRegistry))
}
