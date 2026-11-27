//! Public release discovery, independent of authorization to install a package.
//!
//! GitHub metadata is suitable for release notes and manual download links. It
//! never creates a [`super::ReleaseAsset`]: unattended replacement still requires
//! the separately authenticated update repository and platform installer checks.

use std::{cmp::Ordering, time::Duration};

use iptv_import::FetchErrorKind;
use iptv_net::{NetClient, NetConfig};
use iptv_providers::HttpRequest;
use semver::Version;
use serde_json::Value;

use super::InstalledPackage;

const API: &str = "https://api.github.com/repos/teng-lin/patina-iptv/releases";
const RELEASES: &str = "https://github.com/teng-lin/patina-iptv/releases";
const PAGE_SIZE: usize = 100;
const MAX_PAGES: usize = 10;
const MAX_BODY: usize = 2 << 20;
const DEADLINE: Duration = Duration::from_secs(45);

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

impl crate::Runtime {
    /// Runs a bounded public GitHub check on this owner's worker. Provider
    /// cookies, headers, credentials and network configuration are never reused.
    pub async fn check_github_release(
        &self,
        installed: InstalledPackage,
    ) -> Result<ReleaseCheck, CheckError> {
        self.services()
            .spawn(async move { discover(installed).await })
            .await
            .map_err(|_| CheckError::Network)?
    }
}

/// Checks stable releases using semantic version precedence, not publication
/// order. Re-publishing an older maintenance release cannot offer a downgrade.
pub async fn discover(installed: InstalledPackage) -> Result<ReleaseCheck, CheckError> {
    let (endpoint, public_only) = endpoint()?;
    let client = NetClient::new(config()).map_err(|_| CheckError::Network)?;
    check(&client, &endpoint, public_only, installed, DEADLINE).await
}

fn config() -> NetConfig {
    NetConfig {
        user_agent: "Patina".into(),
        allow_files: false,
        max_redirects: 0,
        max_buffered_bytes: MAX_BODY,
        max_streamed_bytes: MAX_BODY as u64,
        connect_timeout: Duration::from_secs(10),
        read_timeout: Duration::from_secs(15),
        request_timeout: Duration::from_secs(15),
        ..Default::default()
    }
}

fn endpoint() -> Result<(String, bool), CheckError> {
    // This escape hatch is absent from production packages and accepts literal
    // loopback addresses only, so fixture tests exercise the actual HTTP path.
    #[cfg(all(feature = "test-support", debug_assertions))]
    if let Some(value) = std::env::var_os("IPTV_TEST_UPDATE_API") {
        let value = value
            .into_string()
            .map_err(|_| CheckError::InvalidResponse)?;
        if !fixture_endpoint(&value) {
            return Err(CheckError::InvalidResponse);
        }
        return Ok((value, false));
    }
    Ok((API.into(), true))
}

#[cfg(all(feature = "test-support", debug_assertions))]
pub(super) fn fixture_endpoint(value: &str) -> bool {
    let Ok(url) = url::Url::parse(value) else {
        return false;
    };
    let loopback = url
        .host_str()
        .and_then(|host| {
            host.trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .ok()
        })
        .is_some_and(|address| address.is_loopback());
    url.scheme() == "http"
        && loopback
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
}

async fn check(
    client: &NetClient,
    endpoint: &str,
    public_only: bool,
    installed: InstalledPackage,
    deadline: Duration,
) -> Result<ReleaseCheck, CheckError> {
    let current = parse_version(installed.version).ok_or(CheckError::InvalidInstalledVersion)?;
    let installed = installed.packaged();
    tokio::time::timeout(deadline, async {
        let mut latest: Option<(Version, Value)> = None;
        for page in 1..=MAX_PAGES {
            let mut request =
                HttpRequest::get(format!("{endpoint}?per_page={PAGE_SIZE}&page={page}"))
                    .with_header("Accept", "application/vnd.github+json")
                    .with_header("X-GitHub-Api-Version", "2022-11-28");
            request.public_only = public_only;
            let response = client
                .fetch(request)
                .await
                .map_err(|error| match error.kind {
                    FetchErrorKind::Timeout | FetchErrorKind::Deadline => CheckError::Timeout,
                    FetchErrorKind::TooLarge => CheckError::InvalidResponse,
                    _ => CheckError::Network,
                })?;
            match response.status {
                200 => (),
                403 | 429 => return Err(CheckError::RateLimited),
                status => return Err(CheckError::Http(status)),
            }
            let values: Value =
                serde_json::from_slice(&response.body).map_err(|_| CheckError::InvalidResponse)?;
            let releases = values.as_array().ok_or(CheckError::InvalidResponse)?;
            if releases.len() > PAGE_SIZE {
                return Err(CheckError::InvalidResponse);
            }
            for release in releases {
                let draft = release["draft"]
                    .as_bool()
                    .ok_or(CheckError::InvalidResponse)?;
                let prerelease = release["prerelease"]
                    .as_bool()
                    .ok_or(CheckError::InvalidResponse)?;
                if draft || prerelease {
                    continue;
                }
                let tag = release["tag_name"]
                    .as_str()
                    .ok_or(CheckError::InvalidResponse)?;
                let Some(version) = parse_version(tag).filter(|version| version.pre.is_empty())
                else {
                    continue;
                };
                if latest.as_ref().is_none_or(|(previous, _)| {
                    version.cmp_precedence(previous) == Ordering::Greater
                }) {
                    latest = Some((version, release.clone()));
                }
            }
            if releases.len() < PAGE_SIZE {
                let (version, release) = latest.ok_or(CheckError::NoStableRelease)?;
                return describe(current, version, release, &installed);
            }
        }
        // Do not report "up to date" after truncating a repository's history.
        Err(CheckError::TooManyReleases)
    })
    .await
    .map_err(|_| CheckError::Timeout)?
}

fn parse_version(value: &str) -> Option<Version> {
    Version::parse(value.strip_prefix('v').unwrap_or(value)).ok()
}

fn describe(
    current: Version,
    latest: Version,
    release: Value,
    installed: &InstalledPackage,
) -> Result<ReleaseCheck, CheckError> {
    let tag = release["tag_name"]
        .as_str()
        .ok_or(CheckError::InvalidResponse)?;
    let url = format!("{RELEASES}/tag/{tag}");
    if release["html_url"].as_str() != Some(&url) {
        return Err(CheckError::InvalidResponse);
    }
    let notes = match &release["body"] {
        Value::Null => String::new(),
        Value::String(body) => body.clone(),
        _ => return Err(CheckError::InvalidResponse),
    };
    let published_at = match &release["published_at"] {
        Value::Null => None,
        Value::String(date) => Some(date.clone()),
        _ => return Err(CheckError::InvalidResponse),
    };
    let assets = release["assets"]
        .as_array()
        .ok_or(CheckError::InvalidResponse)?;
    let name = suffix(installed).map(|suffix| format!("Patina-{latest}-{suffix}"));
    let mut matching = assets.iter().filter(|asset| {
        name.as_deref()
            .is_some_and(|name| asset["name"].as_str() == Some(name))
    });
    let download = if let Some(asset) = matching.next() {
        if matching.next().is_some() {
            return Err(CheckError::InvalidResponse);
        }
        let name = name.ok_or(CheckError::InvalidResponse)?;
        let url = format!("{RELEASES}/download/{tag}/{name}");
        let size = asset["size"].as_u64().ok_or(CheckError::InvalidResponse)?;
        if asset["browser_download_url"].as_str() != Some(&url)
            || !(1..=2_147_483_648).contains(&size)
        {
            return Err(CheckError::InvalidResponse);
        }
        Some(DownloadLink { name, url, size })
    } else {
        None
    };
    let status = match current.cmp_precedence(&latest) {
        Ordering::Equal => ReleaseStatus::Current,
        Ordering::Greater => ReleaseStatus::Ahead,
        Ordering::Less if download.is_some() => ReleaseStatus::UpdateAvailable,
        Ordering::Less => ReleaseStatus::NoCompatiblePackage,
    };
    Ok(ReleaseCheck {
        installed_version: installed.version.into(),
        status,
        release: ReleaseInfo {
            version: latest.to_string(),
            notes,
            published_at,
            url,
            download,
        },
    })
}

fn suffix(installed: &InstalledPackage) -> Option<&'static str> {
    let format = installed.format.as_deref();
    match (installed.platform, installed.architecture, format) {
        ("android", "arm64-v8a" | "armeabi-v7a", None | Some("apk")) => {
            Some("android-tablet-tv.apk")
        }
        ("macos", "arm64", None | Some("dmg")) => Some("macos-arm64.dmg"),
        ("windows", "x86_64", None | Some("exe")) => Some("windows-x86_64-setup.exe"),
        ("linux", "x86_64", Some("appimage")) => Some("linux-x86_64.AppImage"),
        ("linux", "x86_64", Some("deb")) => Some("linux-x86_64.deb"),
        ("linux", "x86_64", Some("rpm")) => Some("linux-x86_64.rpm"),
        ("linux", "x86_64", Some("pacman")) => Some("linux-x86_64.pkg.tar.zst"),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
