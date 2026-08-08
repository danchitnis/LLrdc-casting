/*
 * 100% Safe Rust Network Module: Discovers active IPv4 network interfaces
 */

#![forbid(unsafe_code)]

/// Query usable private IPv4 addresses on physical LAN interfaces.
///
/// Interface names are intentionally restricted to Linux Ethernet and Wi-Fi
/// naming families. This prevents loopback, Docker bridges, Tailscale, and
/// other virtual addresses from becoming casting endpoints.
pub fn get_active_ipv4_addresses() -> Vec<(String, String)> {
    let mut results = Vec::new();

    if let Ok(addrs) = nix::ifaddrs::getifaddrs() {
        for ifaddr in addrs {
            if interface_priority(&ifaddr.interface_name).is_none() {
                continue;
            }
            if let Some(address) = ifaddr.address {
                if let Some(sock_in) = address.as_sockaddr_in() {
                    let ip = std::net::Ipv4Addr::from(sock_in.ip());
                    if is_usable_private_ipv4(ip) {
                        let ip_str = ip.to_string();
                        if !results.iter().any(|(iface, item_ip)| {
                            iface == &ifaddr.interface_name && item_ip == &ip_str
                        }) {
                            results.push((ifaddr.interface_name.clone(), ip_str));
                        }
                    }
                }
            }
        }
    }

    sort_addresses(&mut results);
    results
}

/// Return the first usable RFC1918 address on a physical interface.
pub fn get_preferred_private_ipv4() -> Option<String> {
    get_active_ipv4_addresses()
        .into_iter()
        .map(|(_, ip)| ip)
        .next()
}

fn interface_priority(name: &str) -> Option<u8> {
    if name.starts_with("eth")
        || name.starts_with("end")
        || name.starts_with("enp")
        || name.starts_with("eno")
        || name.starts_with("ens")
        || name.starts_with("enx")
    {
        Some(0)
    } else if name.starts_with("wlan") || name.starts_with("wlp") || name.starts_with("wl") {
        Some(1)
    } else {
        None
    }
}

fn is_usable_private_ipv4(ip: std::net::Ipv4Addr) -> bool {
    !ip.is_loopback() && !ip.is_link_local() && ip.is_private()
}

fn sort_addresses(addresses: &mut [(String, String)]) {
    addresses.sort_by(|(left_iface, left_ip), (right_iface, right_ip)| {
        interface_priority(left_iface)
            .cmp(&interface_priority(right_iface))
            .then_with(|| left_iface.cmp(right_iface))
            .then_with(|| left_ip.cmp(right_ip))
    });
}

#[cfg(test)]
mod tests {
    use super::{interface_priority, is_usable_private_ipv4, sort_addresses};

    #[test]
    fn ignores_non_lan_interface_families() {
        assert_eq!(interface_priority("lo"), None);
        assert_eq!(interface_priority("docker0"), None);
        assert_eq!(interface_priority("tailscale0"), None);
        assert_eq!(interface_priority("end0"), Some(0));
        assert_eq!(interface_priority("wlan0"), Some(1));
    }

    #[test]
    fn accepts_only_usable_private_addresses() {
        assert!(is_usable_private_ipv4("192.168.1.72".parse().unwrap()));
        assert!(is_usable_private_ipv4("10.0.0.2".parse().unwrap()));
        assert!(is_usable_private_ipv4("172.17.0.1".parse().unwrap()));
        assert!(!is_usable_private_ipv4("127.0.0.1".parse().unwrap()));
        assert!(!is_usable_private_ipv4("169.254.1.2".parse().unwrap()));
    }

    #[test]
    fn sorts_ethernet_before_wifi() {
        let mut addresses = vec![
            ("wlan0".to_string(), "192.168.1.20".to_string()),
            ("end0".to_string(), "192.168.1.72".to_string()),
        ];
        sort_addresses(&mut addresses);
        assert_eq!(addresses[0].0, "end0");
        assert_eq!(addresses[1].0, "wlan0");
    }
}
