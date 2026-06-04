//! Shared network helpers: public-IP classification used by webfetch
//! (blocks non-public) and credential transport (permits loopback/private).

use std::net::IpAddr;

/// Returns `true` when `ip` is a globally routable unicast address suitable
/// for public webfetch targets.
///
/// Normalises IPv4-mapped-IPv6 addresses to their embedded IPv4 form, then
/// delegates to [`ip_rfc::global`] with explicit denials for multicast and
/// the deprecated IPv6 site-local range (`fec0::/10`).
///
/// This is the **single** public-IP classifier in the crate. Keep policy
/// differences (webfetch deny vs credential‑transport permit) at the call
/// sites.
pub(crate) fn is_public_ip(ip: IpAddr) -> bool {
    let normalized_ip = match ip {
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4() {
                IpAddr::V4(v4)
            } else {
                IpAddr::V6(v6)
            }
        }
        IpAddr::V4(v4) => IpAddr::V4(v4),
    };
    ip_rfc::global(&normalized_ip)
        && !normalized_ip.is_multicast()
        && !matches!(normalized_ip, IpAddr::V6(ip) if (ip.segments()[0] & 0xffc0) == 0xfec0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_private_and_special_addresses() {
        let blocked = [
            "0.0.0.0",
            "10.0.0.1",
            "127.0.0.1",
            "169.254.1.1",
            "172.16.0.1",
            "192.0.0.1",
            "192.0.2.1",
            "192.168.0.1",
            "198.18.0.1",
            "198.51.100.1",
            "203.0.113.1",
            "224.0.0.1",
            "240.0.0.1",
            "::",
            "::1",
            "2001:db8::1",
            "fc00::1",
            "fe80::1",
            "fec0::1",
            "ff0e::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "::ffff:169.254.169.254",
            "::ffff:192.168.1.1",
        ];
        for ip_str in blocked {
            let ip: IpAddr = ip_str.parse().unwrap();
            assert!(!is_public_ip(ip), "{ip} should be blocked");
        }
    }

    #[test]
    fn allows_public_addresses() {
        let allowed = [
            "93.184.216.34",
            "192.0.0.9",
            "192.0.0.10",
            "2606:2800:220:1:248:1893:25c8:1946",
        ];
        for ip_str in allowed {
            let ip: IpAddr = ip_str.parse().unwrap();
            assert!(is_public_ip(ip), "{ip} should be allowed");
        }
    }

    #[test]
    fn ipv4_mapped_ipv6_and_unique_local_alignment() {
        // IPv4-mapped-IPv6 should be treated the same as their IPv4 equivalent.
        assert_eq!(
            is_public_ip("::ffff:192.168.1.1".parse::<IpAddr>().unwrap()),
            is_public_ip("192.168.1.1".parse::<IpAddr>().unwrap())
        );
        // Unique-local IPv6 (fc00::/7) should be non-public.
        assert!(!is_public_ip("fc00::1".parse::<IpAddr>().unwrap()));
        assert!(!is_public_ip("fdff::1".parse::<IpAddr>().unwrap()));
        // Site-local (deprecated fec0::/10) should be non-public.
        assert!(!is_public_ip("fec0::1".parse::<IpAddr>().unwrap()));
    }
}
