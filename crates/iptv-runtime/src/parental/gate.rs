//! Leaf parental gate data and policy helpers shared by the store layer and
//! parental orchestration.
use iptv_core::parental::{Access, Policy, Target, UnlockWindow};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Locked,
    Incorrect,
    RetryAfter(u64),
    NoPin,
    InvalidPin,
    Storage,
    OwnerClosed,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Locked => "Parental access is locked",
            Self::Incorrect => "Incorrect PIN or recovery code",
            Self::RetryAfter(_) => "Too many attempts; try again later",
            Self::NoPin => "Set a parental PIN first",
            Self::InvalidPin => "Use four to eight digits",
            Self::Storage => "Parental settings are unavailable",
            Self::OwnerClosed => "The application has closed",
        })
    }
}

impl std::error::Error for Error {}

/// A group unlock authorizes the handoff subject without unlocking its siblings.
#[derive(Clone)]
pub(crate) struct TransferredGroups {
    pub(crate) channel: Target,
    pub(crate) groups: Vec<Target>,
}

impl TransferredGroups {
    pub(crate) fn allows(transfer: Option<&Self>, channel: &Target, group: &Target) -> bool {
        transfer
            .is_some_and(|transfer| transfer.channel == *channel && transfer.groups.contains(group))
    }
}

pub(crate) fn now_ms() -> u64 {
    crate::time::since_epoch()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

pub(crate) fn policy_settings(settings: &iptv_settings::Settings) -> Policy {
    use iptv_settings::Target as Scope;
    let scope = Scope::Global;
    let window = match settings.choice(iptv_settings::keys::PARENTAL_UNLOCK_WINDOW, &scope) {
        "min_10" => UnlockWindow::Minutes(10),
        "min_30" => UnlockWindow::Minutes(30),
        "hour_1" => UnlockWindow::Minutes(60),
        "hour_2" => UnlockWindow::Minutes(120),
        "until_restart" => UnlockWindow::UntilRestart,
        "until_unblocked_channel" => UnlockWindow::UntilUnblockedChannel,
        _ => UnlockWindow::EveryTime,
    };
    Policy {
        live: settings.bool(iptv_settings::keys::PARENTAL_LIVE_ENABLED, &scope),
        vod: settings.bool(iptv_settings::keys::PARENTAL_VOD_ENABLED, &scope),
        settings: settings.choice(iptv_settings::keys::PARENTAL_REQUIRE_PIN_FOR, &scope)
            == "blocked_content_and_settings",
        window,
    }
}

/// Native command gate. Resolve membership from storage for every direct-ID
/// command; callers cannot omit or spoof the item's category.
#[derive(Clone)]
pub(crate) struct Snapshot {
    pub access: Access,
    pub(crate) transferred_groups: Option<TransferredGroups>,
    pub(crate) epoch: Arc<AtomicU64>,
    pub(crate) at: u64,
    pub(crate) policy_revision: Arc<AtomicU64>,
    pub(crate) policy_at: u64,
}

impl Snapshot {
    pub(crate) fn inspection_current(&self, grants: &[Target]) -> bool {
        self.grants_current(grants, |grant, now| self.access.granted(grant, now))
    }

    pub(crate) fn playback_current(&self, subject: &Target, grants: &[Target]) -> bool {
        self.grants_current(grants, |grant, now| {
            self.access.granted(grant, now)
                || TransferredGroups::allows(self.transferred_groups.as_ref(), subject, grant)
        })
    }

    fn grants_current(&self, grants: &[Target], allows: impl Fn(&Target, u64) -> bool) -> bool {
        let now = now_ms();
        self.policy_current()
            && (grants.is_empty()
                || (self.epoch.load(Ordering::SeqCst) == self.at
                    && grants.iter().all(|grant| allows(grant, now))))
    }

    pub(crate) fn policy_current(&self) -> bool {
        self.policy_revision.load(Ordering::SeqCst) == self.policy_at
    }
}

/// Parental policy verified against committed storage for external playback.
#[derive(Clone)]
pub struct ExternalPlaybackAuthorization {
    access: Snapshot,
    target: Target,
    grants: Vec<Target>,
}

impl ExternalPlaybackAuthorization {
    pub(crate) fn new(access: Snapshot, target: Target, grants: Vec<Target>) -> Self {
        Self {
            access,
            target,
            grants,
        }
    }

    /// Check immediately before granting the relay admission, without yielding.
    pub fn is_current(&self) -> bool {
        self.access.playback_current(&self.target, &self.grants)
    }
}

pub(crate) use crate::error::SETTINGS_LOCKED_REASON;
