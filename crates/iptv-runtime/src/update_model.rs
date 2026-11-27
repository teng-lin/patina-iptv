//! Portable release and update status values. Package authorization stays native.

/// An explicitly selected package link, not a verified installation asset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DownloadLink {
    pub name: String,
    pub url: String,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseInfo {
    /// Semantic version without the optional tag's `v` prefix.
    pub version: String,
    /// Plain release body; views must not execute embedded HTML or remote media.
    pub notes: String,
    pub published_at: Option<String>,
    pub url: String,
    pub download: Option<DownloadLink>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReleaseStatus {
    Current,
    /// A development or newer installation must never be offered a downgrade.
    Ahead,
    UpdateAvailable,
    /// A newer release exists, but this installation has no matching package.
    NoCompatiblePackage,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseCheck {
    pub installed_version: String,
    pub release: ReleaseInfo,
    pub status: ReleaseStatus,
}

/// Deliberately contains no server body, URL, or provider credentials.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckError {
    InvalidInstalledVersion,
    Network,
    Timeout,
    RateLimited,
    Http(u16),
    InvalidResponse,
    NoStableRelease,
    TooManyReleases,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Frequency {
    Never,
    OnLaunch,
    Daily,
    #[default]
    Weekly,
    Monthly,
}

impl Frequency {
    pub fn from_setting(value: &str) -> Self {
        match value {
            "never" => Self::Never,
            "on_launch" => Self::OnLaunch,
            "daily" => Self::Daily,
            "monthly" => Self::Monthly,
            _ => Self::Weekly,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    Manual,
    Startup,
    /// A foreground timer or resume observation, including a long-running app.
    Automatic,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub checking: bool,
    pub cached: Option<ReleaseCheck>,
    pub error: Option<CheckError>,
    pub notice: Option<ReleaseCheck>,
    pub last_success: Option<i64>,
    pub last_attempt: Option<i64>,
    pub retry_after: Option<i64>,
    pub skipped_version: Option<String>,
}
