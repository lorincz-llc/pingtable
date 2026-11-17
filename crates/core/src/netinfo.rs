//! Name lookups and the system neighbour (ARP) table.

use std::collections::HashMap;
use std::net::{IpAddr, ToSocketAddrs};

/// Resolve a host name to one address. IPv4 is chosen unless `prefer_ipv6`
/// is set or the name only has IPv6 addresses.
pub fn resolve(host: &str, prefer_ipv6: bool) -> Option<IpAddr> {
    let addrs: Vec<IpAddr> = (host, 0u16).to_socket_addrs().ok()?.map(|a| a.ip()).collect();
    let wanted = addrs.iter().find(|a| a.is_ipv6() == prefer_ipv6);
    wanted.or(addrs.first()).copied()
}

/// Reverse lookup. Returns None when there is no name.
pub fn reverse(ip: IpAddr) -> Option<String> {
    let name = dns_lookup::lookup_addr(&ip).ok()?;
    let name = name.trim_end_matches('.').to_string();
    if name.is_empty() || name.parse::<IpAddr>().is_ok() {
        None
    } else {
        Some(name)
    }
}

/// This computer's name.
pub fn local_name() -> String {
    std::env::var("COMPUTERNAME")
        .ok()
        .or_else(|| std::fs::read_to_string("/proc/sys/kernel/hostname").ok())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "this computer".to_string())
}

/// One address on one network interface of this computer.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct Interface {
    pub name: String,
    pub addr: IpAddr,
    pub loopback: bool,
}

/// Every address this computer has, loopback last. IPv6 link-local addresses
/// are left out; they cannot be used as a source without naming the interface.
pub fn interfaces() -> Vec<Interface> {
    let mut list: Vec<Interface> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .map(|i| Interface { loopback: i.is_loopback(), addr: i.ip(), name: i.name })
        .filter(|i| match i.addr {
            IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) != 0xfe80,
            IpAddr::V4(_) => true,
        })
        .collect();
    list.sort_by(|a, b| (a.loopback, a.addr.is_ipv6(), &a.name, a.addr).cmp(&(b.loopback, b.addr.is_ipv6(), &b.name, b.addr)));
    list
}

/// The address this computer would send from to reach `target`. Nothing is
/// sent: connecting a UDP socket only asks the system to pick a route.
pub fn source_for(target: IpAddr) -> Option<IpAddr> {
    let bind: std::net::SocketAddr = if target.is_ipv4() { "0.0.0.0:0".parse().ok()? } else { "[::]:0".parse().ok()? };
    let socket = std::net::UdpSocket::bind(bind).ok()?;
    socket.connect((target, 9)).ok()?;
    socket.local_addr().ok().map(|a| a.ip())
}

pub fn format_mac(mac: &[u8; 6]) -> String {
    format!("{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}", mac[0], mac[1], mac[2], mac[3], mac[4], mac[5])
}

/// MAC addresses the system already knows, keyed by IP address. This only
/// reads the neighbour table; it never sends ARP itself, so an address shows
/// up after the host has been pinged on the local network.
pub fn neighbours() -> HashMap<IpAddr, [u8; 6]> {
    imp::neighbours()
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};
    use std::ptr;
    use std::slice;
    use windows_sys::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIpNetTable2, MIB_IPNET_ROW2, MIB_IPNET_TABLE2};
    use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6, AF_UNSPEC};

    pub fn neighbours() -> HashMap<IpAddr, [u8; 6]> {
        let mut map = HashMap::new();
        let mut table: *mut MIB_IPNET_TABLE2 = ptr::null_mut();
        if unsafe { GetIpNetTable2(AF_UNSPEC, &mut table) } != 0 || table.is_null() {
            return map;
        }
        unsafe {
            let count = (*table).NumEntries as usize;
            let rows: &[MIB_IPNET_ROW2] = slice::from_raw_parts((*table).Table.as_ptr(), count);
            for row in rows {
                if row.PhysicalAddressLength != 6 {
                    continue;
                }
                let mut mac = [0u8; 6];
                mac.copy_from_slice(&row.PhysicalAddress[..6]);
                if mac == [0u8; 6] || mac == [0xff; 6] {
                    continue;
                }
                let family = row.Address.si_family;
                if family == AF_INET {
                    let raw = row.Address.Ipv4.sin_addr.S_un.S_addr;
                    map.insert(IpAddr::V4(Ipv4Addr::from(raw.to_ne_bytes())), mac);
                } else if family == AF_INET6 {
                    let raw = row.Address.Ipv6.sin6_addr.u.Byte;
                    map.insert(IpAddr::V6(Ipv6Addr::from(raw)), mac);
                }
            }
            FreeMibTable(table as *const _);
        }
        map
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    pub fn neighbours() -> HashMap<IpAddr, [u8; 6]> {
        let text = std::fs::read_to_string("/proc/net/arp").unwrap_or_default();
        parse_proc_arp(&text)
    }

    pub(super) fn parse_proc_arp(text: &str) -> HashMap<IpAddr, [u8; 6]> {
        let mut map = HashMap::new();
        for line in text.lines().skip(1) {
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() < 4 {
                continue;
            }
            let (Ok(ip), Some(mac)) = (cols[0].parse::<IpAddr>(), parse_mac(cols[3])) else { continue };
            if mac != [0u8; 6] {
                map.insert(ip, mac);
            }
        }
        map
    }

    fn parse_mac(text: &str) -> Option<[u8; 6]> {
        let mut mac = [0u8; 6];
        let mut parts = text.split(':');
        for byte in &mut mac {
            *byte = u8::from_str_radix(parts.next()?, 16).ok()?;
        }
        parts.next().is_none().then_some(mac)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn reads_proc_arp() {
            let text = "IP address       HW type     Flags       HW address            Mask     Device\n\
                        192.168.1.1      0x1         0x2         a4:91:b1:00:12:ff     *        eth0\n\
                        192.168.1.9      0x1         0x0         00:00:00:00:00:00     *        eth0\n";
            let map = parse_proc_arp(text);
            assert_eq!(map.len(), 1);
            let mac = map[&"192.168.1.1".parse::<IpAddr>().unwrap()];
            assert_eq!(format_mac(&mac), "A4:91:B1:00:12:FF");
        }
    }
}

#[cfg(test)]
mod general_tests {
    use super::*;

    #[test]
    fn this_computer_has_a_name_and_a_loopback_address() {
        assert!(!local_name().is_empty());
        let all = interfaces();
        assert!(all.iter().any(|i| i.loopback), "{all:?}");
        assert!(all.iter().position(|i| i.loopback).unwrap() >= all.iter().filter(|i| !i.loopback).count());
        assert_eq!(source_for("127.0.0.1".parse().unwrap()), Some("127.0.0.1".parse().unwrap()));
    }
}
