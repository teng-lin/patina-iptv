use serde_json::Value;

/// Identity of a packaged build. An unknown format only receives release information.
#[derive(Debug, Clone)]
pub struct InstalledPackage {
    pub platform: &'static str,
    pub architecture: &'static str,
    pub format: Option<String>,
    pub version: &'static str,
    pub android_code: Option<u64>,
    /// Known minimum capabilities of this installation.
    pub minimum: Value,
}

impl InstalledPackage {
    /// A release build may stamp its direct package format. Linux identifies
    /// the package beside its executable because its four formats share a build.
    pub fn native(version: &'static str) -> Self {
        let platform = std::env::consts::OS;
        let architecture = match (platform, std::env::consts::ARCH) {
            ("android", "aarch64") => "arm64-v8a",
            ("android", "arm") => "armeabi-v7a",
            ("macos", "aarch64") => "arm64",
            (_, architecture) => architecture,
        };
        let minimum = match platform {
            "android" => serde_json::json!({"androidApi": 26}),
            "windows" => serde_json::json!({"windows": "10.0"}),
            "macos" => serde_json::json!({"macos": "14.0"}),
            _ => serde_json::json!({"glibc": "2.39", "gtk": "4.14"}),
        };
        Self {
            platform,
            architecture,
            format: match platform {
                "macos" => option_env!("PATINA_UPDATE_PACKAGE").map(str::to_owned),
                _ => None,
            },
            version,
            android_code: None,
            minimum,
        }
    }

    pub(super) fn packaged(mut self) -> Self {
        if matches!(self.platform, "linux" | "windows") && self.format.is_none() {
            self.format = std::env::current_exe()
                .ok()
                .and_then(|exe| std::fs::read_to_string(exe.parent()?.join("patina-package")).ok())
                .map(|s| s.trim().to_owned());
        }
        self
    }
}

/// Immutable package named by a verified release. Only the selector creates one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseAsset {
    pub(super) url: String,
    pub(super) sha256: String,
    pub(super) length: u64,
    android_code: Option<u64>,
}
impl ReleaseAsset {
    pub fn url(&self) -> &str {
        &self.url
    }
    pub fn android_code(&self) -> Option<u64> {
        self.android_code
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum UpdateResult {
    Current,
    Available {
        version: String,
        notes: String,
        preview: bool,
        asset: ReleaseAsset,
    },
    RequirementsUnconfirmed,
    NoCompatiblePackage,
    Unavailable,
}

fn version(text: &str) -> Option<[u32; 3]> {
    let mut parts = text.split('.');
    let mut value = [0; 3];
    for part in &mut value {
        let text = parts.next()?;
        *part = text.parse().ok()?;
        if part.to_string() != text {
            return None;
        }
    }
    parts.next().is_none().then_some(value)
}

fn meets(have: &Value, need: &Value) -> Option<bool> {
    for (name, required) in need.as_object()? {
        let actual = &have[name];
        if let Some(minimum) = required.as_u64() {
            if actual.as_u64().is_none_or(|v| v < minimum) {
                return Some(false);
            }
        } else {
            let parts = |s: &str| {
                s.split('.')
                    .map(str::parse::<u32>)
                    .collect::<Result<Vec<_>, _>>()
                    .ok()
            };
            let required = parts(required.as_str()?)?;
            let Some(mut actual) = actual.as_str().and_then(parts) else {
                return Some(false);
            };
            actual.resize(actual.len().max(required.len()), 0);
            if actual < required {
                return Some(false);
            }
        }
    }
    Some(true)
}

pub(super) fn select(bytes: &[u8], installed: InstalledPackage) -> Option<UpdateResult> {
    let index: Value = serde_json::from_slice(bytes).ok()?;
    if index["schema"].as_u64()? != 1 {
        return None;
    }
    let current = version(installed.version)?;
    let Some(format) = installed.format else {
        return Some(UpdateResult::NoCompatiblePackage);
    };
    let mut matches = index["packages"].as_array()?.iter().filter(|p| {
        p["source"] == "github-release"
            && p["platform"] == installed.platform
            && p["format"] == format
    });
    let Some(package) = matches.next() else {
        return Some(UpdateResult::NoCompatiblePackage);
    };
    if matches.next().is_some() {
        return None;
    }
    if !package["architectures"]
        .as_array()?
        .iter()
        .any(|abi| abi == installed.architecture)
    {
        return Some(UpdateResult::NoCompatiblePackage);
    }
    let text = package["version"].as_str()?;
    let offered = version(text)?;
    if offered <= current {
        return Some(UpdateResult::Current);
    }
    if !meets(&installed.minimum, &package["minimum"])? {
        // Android reports the device API. Desktop stamps describe the oldest
        // supported host, so a higher floor needs a manual compatibility check.
        return Some(if installed.platform == "android" {
            UpdateResult::NoCompatiblePackage
        } else {
            UpdateResult::RequirementsUnconfirmed
        });
    }
    if installed.platform == "android"
        && installed
            .android_code
            .is_none_or(|n| package["versionCode"].as_u64().is_none_or(|code| code <= n))
    {
        return Some(UpdateResult::NoCompatiblePackage);
    }
    let notes = format!("https://github.com/teng-lin/patina-iptv/releases/tag/v{text}");
    if package["notes"] != notes {
        return None;
    }
    let preview = match installed.platform {
        "windows" | "macos" => match package["signing"].as_str()? {
            "signed" => false,
            "preview" => true,
            _ => return None,
        },
        _ => false,
    };
    let suffix = match (installed.platform, format.as_str()) {
        ("android", "apk") if package["signing"] == "apk" => "android-tablet-tv.apk",
        ("windows", "exe") => "windows-x86_64-setup.exe",
        ("macos", "dmg") => "macos-arm64.dmg",
        ("linux", "appimage") => "linux-x86_64.AppImage",
        ("linux", "deb") => "linux-x86_64.deb",
        ("linux", "rpm") => "linux-x86_64.rpm",
        ("linux", "pacman") => "linux-x86_64.pkg.tar.zst",
        _ => return None,
    };
    let url = format!(
        "https://github.com/teng-lin/patina-iptv/releases/download/v{text}/Patina-{text}-{suffix}"
    );
    let sha256 = package["sha256"].as_str()?;
    let length = package["length"].as_u64()?;
    if package["url"] != url
        || !(1..=2_147_483_648).contains(&length)
        || sha256.len() != 64
        || !sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return None;
    }
    let asset = ReleaseAsset {
        url,
        length,
        sha256: sha256.into(),
        android_code: package["versionCode"].as_u64(),
    };
    Some(UpdateResult::Available {
        version: text.to_owned(),
        notes,
        preview,
        asset,
    })
}
