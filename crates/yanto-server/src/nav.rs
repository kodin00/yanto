//! Navigation registry. Each feature module adds ONE line to [`NAV`]; an entry is shown
//! when the signed-in actor has the entry's [`AppView`] (derived from permissions), or to
//! every signed-in user when `view` is `None`. Order here is display order.

use yanto_core::actor::{Actor, AppView};

#[derive(Debug, Clone, Copy)]
pub struct NavEntry {
    pub label: &'static str,
    pub href: &'static str,
    /// `None` = visible to every signed-in user (account pages).
    pub view: Option<AppView>,
}

pub const NAV: &[NavEntry] = &[
    NavEntry {
        label: "Dashboard",
        href: "/",
        view: Some(AppView::Dashboard),
    },
    NavEntry {
        label: "Projects",
        href: "/projects",
        view: Some(AppView::Projects),
    },
    NavEntry {
        label: "Deployments",
        href: "/deployments",
        view: Some(AppView::Deployments),
    },
    NavEntry {
        label: "Deploy keys",
        href: "/deploy-keys",
        view: Some(AppView::Ssh),
    },
    NavEntry {
        label: "Registry",
        href: "/registry",
        view: Some(AppView::Registry),
    },
    NavEntry {
        label: "Registry credentials",
        href: "/registry-credentials",
        view: Some(AppView::Registry),
    },
    NavEntry {
        label: "Containers",
        href: "/containers",
        view: Some(AppView::Containers),
    },
    NavEntry {
        label: "System",
        href: "/system",
        view: Some(AppView::System),
    },
    NavEntry {
        label: "Users",
        href: "/users",
        view: Some(AppView::Users),
    },
    NavEntry {
        label: "Cron",
        href: "/cron",
        view: Some(AppView::Cron),
    },
    NavEntry {
        label: "SSH keys",
        href: "/ssh",
        view: Some(AppView::Ssh),
    },
    NavEntry {
        label: "Hostnames",
        href: "/hostnames",
        view: Some(AppView::Hostnames),
    },
    NavEntry {
        label: "Cloudflare",
        href: "/cloudflare",
        view: Some(AppView::Dns),
    },
    NavEntry {
        label: "DNS",
        href: "/dns",
        view: Some(AppView::Dns),
    },
    NavEntry {
        label: "FRP",
        href: "/frp",
        view: Some(AppView::Frp),
    },
    NavEntry {
        label: "Host Daemons",
        href: "/daemons",
        view: Some(AppView::System),
    },
    NavEntry {
        label: "Backups",
        href: "/backups",
        view: Some(AppView::Backups),
    },
    NavEntry {
        label: "Audit",
        href: "/audit",
        view: Some(AppView::Audit),
    },
    NavEntry {
        label: "API tokens",
        href: "/settings/api-tokens",
        view: None,
    },
    NavEntry {
        label: "Account",
        href: "/settings/sessions",
        view: None,
    },
];

/// Entries the actor may see, in order.
pub fn visible(actor: &Actor) -> Vec<&'static NavEntry> {
    let views = actor.app_views();
    NAV.iter()
        .filter(|e| e.view.is_none_or(|v| views.contains(&v)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn hrefs_are_unique() {
        let set: BTreeSet<_> = NAV.iter().map(|e| e.href).collect();
        assert_eq!(set.len(), NAV.len());
    }

    #[test]
    fn owner_sees_everything_members_do_not_see_users() {
        let owner = visible(&Actor::cli());
        assert_eq!(owner.len(), NAV.len());
        let mut member = Actor::cli();
        member.role = yanto_core::actor::Role::Member;
        let labels: Vec<_> = visible(&member).iter().map(|e| e.label).collect();
        assert!(!labels.contains(&"Users"));
        assert!(labels.contains(&"Audit") && labels.contains(&"API tokens"));
    }
}
