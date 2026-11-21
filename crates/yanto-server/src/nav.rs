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
        label: "Users",
        href: "/users",
        view: Some(AppView::Users),
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
