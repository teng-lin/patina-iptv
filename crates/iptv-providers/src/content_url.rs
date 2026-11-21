//! URLs supplied by playlist/guide content never grant access to local resources.
//! DNS answers must additionally be checked and pinned by the transport adapter.
use std::{fmt, net::IpAddr};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContentUrlError;
impl fmt::Display for ContentUrlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("content URL must name a public HTTP(S) host")
    }
}
impl std::error::Error for ContentUrlError {}

pub fn validate(value: &str) -> Result<(), ContentUrlError> {
    let url = url::Url::parse(value.trim()).map_err(|_| ContentUrlError)?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(ContentUrlError);
    }
    let valid = match url.host().ok_or(ContentUrlError)? {
        url::Host::Ipv4(ip) => public_ip(ip.into()),
        url::Host::Ipv6(ip) => public_ip(ip.into()),
        url::Host::Domain(host) => {
            let host = host.trim_end_matches('.').to_ascii_lowercase();
            host.contains('.')
                && !["localhost", "local", "internal", "home.arpa"]
                    .iter()
                    .any(|suffix| host == *suffix || host.ends_with(&format!(".{suffix}")))
        }
    };
    if valid { Ok(()) } else { Err(ContentUrlError) }
}

/// A newly supplied guide location has no authority to forward credentials.
/// Address/DNS policy remains the transport's responsibility (including its
/// explicit debug-only synthetic-fixture policy).
pub fn validate_uncredentialed_http(value: &str) -> Result<(), ContentUrlError> {
    let url = url::Url::parse(value.trim()).map_err(|_| ContentUrlError)?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(ContentUrlError);
    }
    Ok(())
}

/// Explicitly selected sources may name LAN servers, but still need HTTP(S).
pub fn validate_picked(value: &str) -> Result<(), ContentUrlError> {
    let url = url::Url::parse(value.trim()).map_err(|_| ContentUrlError)?;
    if matches!(url.scheme(), "http" | "https") && url.host().is_some() {
        Ok(())
    } else {
        Err(ContentUrlError)
    }
}

/// Conservative global-unicast classification; disallows link-local, multicast,
/// loopback, shared/private space and IP transition mechanisms that embed LAN IPs.
pub fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(a == 0
                || a == 10
                || a == 127
                || a >= 224
                || (a == 100 && (64..=127).contains(&b))
                || (a == 169 && b == 254)
                || (a == 172 && (16..=31).contains(&b))
                // 192.88.99.0/24: the retired 6to4 relay anycast (RFC 7526).
                || (a == 192
                    && (b == 168 || (b == 0 && (c == 0 || c == 2)) || (b == 88 && c == 99)))
                || (a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
                || (a == 203 && b == 0 && c == 113))
        }
        IpAddr::V6(ip) => {
            if let Some(v4) = ip.to_ipv4_mapped() {
                return public_ip(v4.into());
            }
            let s = ip.segments();
            // RFC 6052 well-known NAT64 prefix: the destination is the
            // embedded IPv4 address, which must itself be public.
            if s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
                let octets = ip.octets();
                return public_ip(
                    std::net::Ipv4Addr::new(octets[12], octets[13], octets[14], octets[15]).into(),
                );
            }
            // Inside 2000::/3, by the IANA special-purpose registry: Teredo
            // (2001::/32), benchmarking (2001:2::/48), ORCHID and ORCHIDv2
            // (2001:10::/28, 2001:20::/28), documentation (2001:db8::/32,
            // 3fff::/20) and 6to4 (2002::/16), which embeds an IPv4 address.
            let special_2001 = s[0] == 0x2001
                && (s[1] == 0
                    || s[1] == 0xdb8
                    || (s[1] == 2 && s[2] == 0)
                    || matches!(s[1] & 0xfff0, 0x10 | 0x20));
            let documentation = s[0] == 0x3fff && (s[1] & 0xf000) == 0;
            (s[0] & 0xe000) == 0x2000 && s[0] != 0x2002 && !special_2001 && !documentation
        }
    }
}

/// A bare local path or decoded file URL. Remote URLs return None; malformed
/// file URLs are rejected rather than escaping restore confinement checks.
pub fn local_path(value: &str) -> Result<Option<std::path::PathBuf>, ContentUrlError> {
    let value = value.trim();
    if value
        .get(..5)
        .is_some_and(|p| p.eq_ignore_ascii_case("file:"))
    {
        let url = url::Url::parse(value).map_err(|_| ContentUrlError)?;
        if url
            .host_str()
            .is_some_and(|host| !host.is_empty() && host != "localhost")
        {
            return Err(ContentUrlError);
        }
        let path = percent_encoding::percent_decode_str(url.path())
            .decode_utf8()
            .map_err(|_| ContentUrlError)?;
        if !path.starts_with('/') || path.contains('\0') {
            return Err(ContentUrlError);
        }
        // A drive-qualified file URL is /C:/...; the native Windows path is C:/....
        #[cfg(windows)]
        if path.as_bytes().get(1).is_some_and(u8::is_ascii_alphabetic)
            && path.as_bytes().get(2) == Some(&b':')
            && path.as_bytes().get(3) == Some(&b'/')
        {
            return Ok(Some(std::path::PathBuf::from(&path[1..])));
        }
        return Ok(Some(std::path::PathBuf::from(path.as_ref())));
    }
    Ok((!value.contains("://")).then(|| std::path::PathBuf::from(value)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn windows_file_urls_decode_into_native_drive_paths() {
        assert_eq!(
            local_path("file:///C:/Synthetic%20Data/list.m3u")
                .unwrap()
                .unwrap(),
            std::path::Path::new("C:/Synthetic Data/list.m3u")
        );
        assert!(local_path("file://remote.example.test/share/list.m3u").is_err());
        assert!(local_path("file:///C:/bad%00.m3u").is_err());
    }

    #[test]
    fn content_cannot_name_local_paths_or_non_public_hosts() {
        for url in [
            "/etc/hosts",
            "file:///etc/hosts",
            "FILE:///etc/hosts",
            "http://127.1/a",
            "http://2130706433/a",
            "http://0x7f000001/a",
            "http://10.0.0.1/a",
            "http://169.254.169.254/a",
            "http://[::1]/a",
            "http://[::ffff:127.0.0.1]/a",
            "http://[fc00::1]/a",
            "http://localhost./a",
            "http://box.local/a",
            "http://box/a",
            "aaaaaaé",
        ] {
            assert!(validate(url).is_err(), "{url}");
        }
        assert!(validate("https://feed.example.test/list").is_ok());
        assert!(public_ip("8.8.8.8".parse().unwrap()));
        assert!(public_ip("2606:4700:4700::1111".parse().unwrap()));
        assert!(public_ip("64:ff9b::808:808".parse().unwrap()));
        assert!(!public_ip("64:ff9b::a00:1".parse().unwrap()));
        assert!(!public_ip("64:ff9b::7f00:1".parse().unwrap()));
    }

    #[test]
    fn special_purpose_ranges_are_not_public_and_their_neighbors_are() {
        for ip in [
            // IPv4: protocol assignments, documentation, 6to4 relay anycast,
            // benchmarking.
            "192.0.0.8",
            "192.0.2.1",
            "192.88.99.0",
            "192.88.99.1",
            "192.88.99.255",
            "198.18.0.1",
            "198.51.100.7",
            "203.0.113.9",
            // IPv6: Teredo, benchmarking, ORCHID, ORCHIDv2, documentation,
            // 6to4.
            "2001::1",
            "2001:0:ffff::1",
            "2001:2::1",
            "2001:2:0:ffff::1",
            "2001:10::1",
            "2001:1f:ffff::1",
            "2001:20::1",
            "2001:2f:ffff::1",
            "2001:db8::1",
            "3fff::1",
            "3fff:fff:ffff::1",
            "2002:c000:201::1",
        ] {
            assert!(!public_ip(ip.parse().unwrap()), "{ip}");
            let url = match ip.contains(':') {
                true => format!("http://[{ip}]/a"),
                false => format!("http://{ip}/a"),
            };
            assert!(validate(&url).is_err(), "{url}");
        }
        for ip in [
            "192.88.98.255",
            "192.88.100.0",
            "192.89.99.1",
            "193.88.99.1",
            "192.0.1.1",
            // Just outside each range, and still global unicast.
            "2001:1::1",
            "2001:2:1::1",
            "2001:3::1",
            "2001:f::1",
            "2001:30::1",
            "2001:200::1",
            "2001:4860:4860::8888",
            "3ffe::1",
            "3fff:1000::1",
            "2003::1",
        ] {
            assert!(public_ip(ip.parse().unwrap()), "{ip}");
        }
    }
}
