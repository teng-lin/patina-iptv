//! The native hosts: the desktops, the iPad and Android. Storage is a
//! private directory, the database key comes from the OS key store, the
//! network is `iptv-net`, work runs on threads, and recordings register
//! with the OS scheduler.
#[cfg(target_os = "ios")]
mod ios_device;

#[cfg(target_os = "android")]
mod android_transport;
#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple_airplay;
#[cfg(any(
    target_os = "android",
    target_os = "linux",
    target_os = "macos",
    windows
))]
mod casting;
mod clock;
mod external_player;
pub(crate) use external_player::*;
pub(crate) mod credential_key;
mod keep_awake;
pub(crate) mod platform_data;
mod process;
pub(crate) mod recording_host;
#[cfg(all(target_os = "ios", target_abi = "sim"))]
mod simulator_entitlements;
pub(crate) mod transport;

use std::path::PathBuf;

use iptv_core::capabilities::Platform;

/// The platform this build runs on.
pub(crate) const PLATFORM: Platform = if cfg!(target_os = "android") {
    Platform::Android
} else if cfg!(target_os = "linux") {
    Platform::Linux
} else if cfg!(target_os = "macos") {
    Platform::Macos
} else if cfg!(windows) {
    Platform::Windows
} else if cfg!(target_os = "ios") {
    Platform::Ipad
} else {
    Platform::Unknown
};

pub(crate) use clock::{offset_minutes, reload_time_zone};
#[cfg(feature = "test-support")]
pub(crate) use platform_data::host_handle;
pub(crate) use platform_data::{flush_on_host_stop, initialize, resolve_data_dir};
pub(crate) use process::{log_library_base, process_age};

/// Whether a capture runs while the desktop could not be kept awake
/// (`keep_awake.rs`).
pub(crate) fn keep_awake_failed() -> bool {
    keep_awake::failed()
}

/// The execution this host supplies has blocking workers
/// (`Runtime::has_workers`).
pub(crate) const HAS_WORKERS: bool = true;

/// Native hosts with a remote say so by platform (`host::ANDROID`).
pub(crate) const REMOTE_ONLY: bool = false;

/// Imports and guide downloads run on the refresh pool's workers.
pub(crate) const IMPORTS: bool = true;

#[cfg(feature = "test-support")]
static TEST_CAST: std::sync::OnceLock<std::sync::Arc<dyn iptv_runtime::cast::Port>> =
    std::sync::OnceLock::new();

#[cfg(feature = "test-support")]
static TEST_NATIVE_CAST: std::sync::OnceLock<std::sync::Arc<dyn iptv_runtime::cast::NativePort>> =
    std::sync::OnceLock::new();

/// The runtime's ports on this host.
pub(crate) fn ports(directory: PathBuf, _pairing_label: String) -> iptv_runtime::Ports {
    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    let sender = casting::Sender::new(&directory, _pairing_label);
    #[cfg(target_os = "android")]
    let sender =
        casting::android::registered().then(|| casting::Sender::new(&directory, _pairing_label));
    let mut ports = iptv_runtime::Ports::new(directory, credential_key::database_key);
    #[cfg(any(target_os = "linux", target_os = "macos", windows))]
    {
        ports.cast = Some(std::sync::Arc::new(sender));
    }
    #[cfg(target_os = "android")]
    if let Some(sender) = sender {
        ports.cast = Some(std::sync::Arc::new(sender));
    }
    #[cfg(feature = "test-support")]
    if let Some(port) = TEST_CAST.get() {
        ports.cast = Some(port.clone());
    }
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        ports.native_cast = Some(std::sync::Arc::new(apple_airplay::Sender::new()));
    }
    #[cfg(feature = "test-support")]
    if let Some(port) = TEST_NATIVE_CAST.get() {
        ports.native_cast = Some(port.clone());
    }
    ports.background_recordings = crate::host::BUILT.background_recordings();
    ports.recording_schedule = recording_host::schedule;
    #[cfg(target_os = "android")]
    {
        ports.recording_failed = recording_host::notify_failed;
        ports.recording_standby = Some(recording_host::standby_blocks_recording);
    }
    ports.exclude_from_backup = platform_data::exclude_from_backup;
    ports
}

/// Time since the Unix epoch on the system clock; `None` before it.
pub(crate) fn since_epoch() -> Option<std::time::Duration> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
}

/// An image's size from its header (NSH's decoders' own probe).
pub(crate) fn image_extent(bytes: &[u8]) -> super::Extent {
    use nsh_core::image::Natural;
    match Natural::of(bytes) {
        Some(Natural::Pixels(width, height)) => super::Extent::Pixels(width, height),
        Some(Natural::Vector(_)) => super::Extent::Vector,
        None => super::Extent::Unknown,
    }
}

/// Runs `work` off the UI thread, on a thread of its own named `name`.
pub(crate) fn background(name: &str, work: impl FnOnce() + Send + 'static) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name(name.into())
        .spawn(work)
        .map(drop)
}

/// The host's recording notifications follow the interface language
/// ([`language_changed`]).
pub(crate) const NOTIFICATIONS_FOLLOW_LANGUAGE: bool = cfg!(target_os = "android");

/// The interface language is now `tag`.
pub(crate) fn language_changed(tag: &str) {
    #[cfg(target_os = "android")]
    recording_host::set_language(tag);
    #[cfg(not(target_os = "android"))]
    let _ = tag;
}

/// Detects the active LAN IPv4 subnet (e.g. `192.168.1.0/24`) on Linux desktops.
pub(crate) fn local_ipv4_subnet() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        let default_ip = default_route_ipv4();
        let candidates = lan_ipv4_interfaces();
        if let Some(default_ip) = default_ip
            && let Some((_, cidr)) = candidates.iter().find(|(ip, _)| *ip == default_ip)
        {
            return Some(cidr.clone());
        }
        if let Some((_, cidr)) = candidates.iter().find(|(ip, _)| ip.is_private()) {
            return Some(cidr.clone());
        }
        default_ip.map(|ip| {
            let [a, b, c, _] = ip.octets();
            format!("{a}.{b}.{c}.0/24")
        })
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// Detects whether the Linux host has UFW or firewalld installed.
pub(crate) fn detect_linux_firewall() -> super::LinuxFirewall {
    #[cfg(target_os = "linux")]
    {
        if std::path::Path::new("/etc/ufw/ufw.conf").exists() || command_in_path("ufw") {
            return super::LinuxFirewall::Ufw;
        }
        if command_in_path("firewall-cmd") {
            return super::LinuxFirewall::Firewalld;
        }
    }
    super::LinuxFirewall::Ufw
}

#[cfg(target_os = "linux")]
fn command_in_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
}

#[cfg(target_os = "linux")]
fn default_route_ipv4() -> Option<std::net::Ipv4Addr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.0.2.1:80").ok()?;
    let std::net::SocketAddr::V4(addr) = socket.local_addr().ok()? else {
        return None;
    };
    Some(*addr.ip())
}

#[cfg(target_os = "linux")]
fn lan_ipv4_interfaces() -> Vec<(std::net::Ipv4Addr, String)> {
    use std::net::Ipv4Addr;

    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: `getifaddrs` writes a linked list pointer into `head` on success
    // (return value 0), which we always release with `freeifaddrs` before returning.
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return Vec::new();
    }
    struct Guard(*mut libc::ifaddrs);
    impl Drop for Guard {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: `self.0` was allocated by `getifaddrs`.
                unsafe { libc::freeifaddrs(self.0) };
            }
        }
    }
    let _guard = Guard(head);
    let mut results = Vec::new();
    let mut cursor = head;
    while !cursor.is_null() {
        // SAFETY: `cursor` is a valid node in the list returned by `getifaddrs`.
        let iface = unsafe { &*cursor };
        cursor = iface.ifa_next;
        let flags = iface.ifa_flags as i32;
        if flags & libc::IFF_UP == 0
            || flags & libc::IFF_LOOPBACK != 0
            || flags & libc::IFF_POINTOPOINT != 0
            || iface.ifa_addr.is_null()
            || iface.ifa_netmask.is_null()
        {
            continue;
        }
        // SAFETY: `ifa_addr` and `ifa_netmask` are non-null `sockaddr` pointers.
        let (addr_family, mask_family) = unsafe {
            (
                i32::from((*iface.ifa_addr).sa_family),
                i32::from((*iface.ifa_netmask).sa_family),
            )
        };
        if addr_family != libc::AF_INET || mask_family != libc::AF_INET {
            continue;
        }
        // SAFETY: Both `sa_family` values were verified to be `AF_INET`.
        let (ip, mask) = unsafe {
            let addr = &*(iface.ifa_addr as *const libc::sockaddr_in);
            let netmask = &*(iface.ifa_netmask as *const libc::sockaddr_in);
            (
                Ipv4Addr::from(u32::from_be(addr.sin_addr.s_addr)),
                Ipv4Addr::from(u32::from_be(netmask.sin_addr.s_addr)),
            )
        };
        if let Some(cidr) = cidr_from_ipv4_mask(ip, mask) {
            results.push((ip, cidr));
        }
    }
    results
}

#[cfg(target_os = "linux")]
fn cidr_from_ipv4_mask(ip: std::net::Ipv4Addr, mask: std::net::Ipv4Addr) -> Option<String> {
    let mask_bits = u32::from(mask);
    let prefix = mask_bits.leading_ones();
    if prefix == 0 || prefix == 32 || (mask_bits << prefix) != 0 {
        return None;
    }
    let network = std::net::Ipv4Addr::from(u32::from(ip) & mask_bits);
    Some(format!("{network}/{prefix}"))
}

#[cfg(all(test, target_os = "linux"))]
mod lan_subnet_tests {
    use super::cidr_from_ipv4_mask;

    #[test]
    fn cidr_uses_exact_contiguous_netmask_prefix() {
        assert_eq!(
            cidr_from_ipv4_mask(
                "10.20.30.40".parse().unwrap(),
                "255.255.0.0".parse().unwrap()
            ),
            Some("10.20.0.0/16".into())
        );
        assert_eq!(
            cidr_from_ipv4_mask(
                "192.0.2.55".parse().unwrap(),
                "255.255.254.0".parse().unwrap()
            ),
            Some("192.0.2.0/23".into())
        );
        assert_eq!(
            cidr_from_ipv4_mask(
                "192.0.2.55".parse().unwrap(),
                "255.0.255.0".parse().unwrap()
            ),
            None
        );
    }
}

/// Public entry points of the native hosts.
pub mod exports {
    /// Installs a generated receiver for an isolated headless app journey.
    #[cfg(feature = "test-support")]
    pub fn install_test_cast_port(port: std::sync::Arc<dyn iptv_runtime::cast::Port>) {
        assert!(super::TEST_CAST.set(port).is_ok());
    }

    /// Installs a loopback receiver and uses the native network casting port.
    #[cfg(all(
        feature = "test-support",
        any(target_os = "linux", target_os = "macos", windows)
    ))]
    pub fn install_test_loopback_cast(address: std::net::SocketAddr) {
        assert!(address.ip().is_loopback());
        install_test_cast_port(std::sync::Arc::new(super::casting::Sender::for_loopback(
            address,
        )));
    }

    /// Supplies an owner-local native route adapter in the existing app harness.
    #[cfg(feature = "test-support")]
    pub fn install_test_native_cast_port(port: std::sync::Arc<dyn iptv_runtime::cast::NativePort>) {
        assert!(super::TEST_NATIVE_CAST.set(port).is_ok());
    }

    /// Explicit synthetic-key injection for the headless test binary only.
    #[cfg(feature = "test-support")]
    pub use super::credential_key::install_test_key;
    pub use super::platform_data::exit_background_recorder_launch;
    /// Use a fresh explicit synthetic profile without changing the process home.
    #[cfg(feature = "test-support")]
    pub use super::platform_data::install_test_data_dir;
    // Android's host registers its components in Kotlin.
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "ios", windows))]
    use iptv_player_mpv as mpv;
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "ios", windows))]
    use nsh_core::prelude::*;

    #[cfg(windows)]
    #[native_components(windows)]
    pub fn components(components: &nsh_platform_windows::Components) {
        mpv::register(components, super::transport::policy);
    }

    /// The Linux desktop's native components: `tv.player` on libmpv (the
    /// Android host registers its own in Kotlin).
    #[cfg(target_os = "linux")]
    #[native_components(linux)]
    pub fn components(components: &nsh_platform_linux::Components) {
        mpv::register(components, super::transport::policy);
    }

    /// The macOS desktop's native components: `tv.player` on libmpv, as on
    /// Linux.
    #[cfg(target_os = "macos")]
    #[native_components(macos)]
    pub fn components(components: &nsh_platform_macos::Components) {
        super::apple_airplay::register(components);
        mpv::register(components, super::transport::policy);
    }

    /// iOS and iPadOS's native components: `tv.player` on libmpv (MPVKit's,
    /// OpenGL ES), as on the desktops.
    #[cfg(target_os = "ios")]
    #[native_components(ios)]
    pub fn components(components: &nsh_platform_ios::Components) {
        super::apple_airplay::register(components);
        super::ios_device::register(components);
        mpv::register(components, super::transport::policy);
    }
}
