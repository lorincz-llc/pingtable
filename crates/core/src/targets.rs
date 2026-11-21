//! Parse named TCP endpoints for the host list and traceroute.

use crate::model::Probe;
use crate::parse::{parse_host_list, ParseOptions};
use std::net::{IpAddr, Ipv6Addr};

/// Maximum number of named TCP monitors to add per host in one request.
pub const MAX_PORTS: usize = 64;

/// Read a list of ports as a person types it: `22, 80 443`, `8000-8010`.
/// Repeats are dropped and the order is kept.
pub fn parse_ports(text: &str) -> Result<Vec<u16>, String> {
    let mut ports: Vec<u16> = Vec::new();
    let push = |port: u16, ports: &mut Vec<u16>| -> Result<(), String> {
        if !ports.contains(&port) {
            if ports.len() >= MAX_PORTS {
                return Err(format!("That is more than {MAX_PORTS} ports. Check fewer at a time."));
            }
            ports.push(port);
        }
        Ok(())
    };
    let one = |word: &str| -> Result<u16, String> {
        match word.parse::<u16>() {
            Ok(port) if port > 0 => Ok(port),
            _ => Err(format!("\"{word}\" is not a port number (1 to 65535).")),
        }
    };
    for word in text.split(|c: char| c == ',' || c == ';' || c.is_whitespace()).filter(|w| !w.is_empty()) {
        match word.split_once('-') {
            Some((first, last)) => {
                let (first, last) = (one(first)?, one(last)?);
                if first > last {
                    return Err(format!("\"{word}\" runs backwards."));
                }
                if (last - first) as usize >= MAX_PORTS {
                    return Err(format!("\"{word}\" is more than {MAX_PORTS} ports. Check fewer at a time."));
                }
                for port in first..=last {
                    push(port, &mut ports)?;
                }
            }
            None => push(one(word)?, &mut ports)?,
        }
    }
    Ok(ports)
}

/// One host as typed, taken apart: its name, its address when it was typed as
/// one, and a port when the text named one (`example.com:8443`,
/// `https://example.com`, `[2001:db8::1]:22`).
pub fn split_target(text: &str) -> Result<(String, Option<IpAddr>, Option<u16>), String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("Type a host name or an address.".into());
    }
    if text.split_whitespace().count() > 1 {
        return Err("Type one host name or address.".into());
    }
    let options = ParseOptions { cidr_skip_first: false, cidr_skip_last: false, ..ParseOptions::default() };
    let report = parse_host_list(text, &options);
    if let Some(warning) = report.warnings.first() {
        return Err(warning.trim_start_matches("Line 1: ").to_string());
    }
    match report.hosts.as_slice() {
        [host] => Ok((
            host.host.clone(),
            host.addr,
            match host.probe {
                Probe::Tcp { port } => Some(port),
                Probe::Icmp => None,
            },
        )),
        [] => Err(format!("\"{text}\" is not a host name or an address.")),
        _ => Err("This operation takes one host, not a range or a block.".into()),
    }
}

/// Turn one typed entry (`nas.lan  NAS in the closet`) into one entry for each
/// chosen port (`nas.lan:443  NAS in the closet`), ready for the host list.
/// `keep_ping` keeps the plain entry as well. An entry that already names a
/// port or a scheme, or that is not a host at all, comes back unchanged.
pub fn entries_for_ports(entry: &str, ports: &[u16], keep_ping: bool) -> Vec<String> {
    let entry = entry.trim();
    let (spec, rest) = match entry.split_once(char::is_whitespace) {
        Some((spec, rest)) => (spec, rest.trim()),
        None => (entry, ""),
    };
    let options = ParseOptions { cidr_skip_first: false, cidr_skip_last: false, ..ParseOptions::default() };
    let report = parse_host_list(spec, &options);
    let plain_host = report.warnings.is_empty() && !report.hosts.is_empty() && report.hosts.iter().all(|h| h.probe == Probe::Icmp);
    if ports.is_empty() || !plain_host {
        return vec![entry.to_string()];
    }
    let base = if spec.parse::<Ipv6Addr>().is_ok() { format!("[{spec}]") } else { spec.to_string() };
    let tail = if rest.is_empty() { String::new() } else { format!(" {rest}") };
    let mut entries = Vec::with_capacity(ports.len() + 1);
    if keep_ping {
        entries.push(entry.to_string());
    }
    entries.extend(ports.iter().map(|port| format!("{base}:{port}{tail}")));
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_are_read_the_way_people_type_them() {
        assert_eq!(parse_ports("22, 80 443").unwrap(), [22, 80, 443]);
        assert_eq!(parse_ports("8000-8003;22").unwrap(), [8000, 8001, 8002, 8003, 22]);
        assert_eq!(parse_ports(" 443 443,443 ").unwrap(), [443]);
        assert_eq!(parse_ports("").unwrap(), [] as [u16; 0]);
    }

    #[test]
    fn bad_ports_are_explained() {
        assert!(parse_ports("0").unwrap_err().contains("not a port number"));
        assert!(parse_ports("70000").unwrap_err().contains("not a port number"));
        assert!(parse_ports("https").unwrap_err().contains("not a port number"));
        assert!(parse_ports("90-80").unwrap_err().contains("backwards"));
        assert!(parse_ports("1-65535").unwrap_err().contains("more than 64"));
        let many: Vec<String> = (1..=65).map(|p| p.to_string()).collect();
        assert!(parse_ports(&many.join(" ")).unwrap_err().contains("more than 64"));
        assert_eq!(parse_ports("1-64").unwrap().len(), 64);
    }

    #[test]
    fn a_typed_host_is_taken_apart() {
        assert_eq!(split_target("example.com").unwrap(), ("example.com".into(), None, None));
        assert_eq!(split_target(" example.com:8443 ").unwrap(), ("example.com".into(), None, Some(8443)));
        assert_eq!(split_target("https://example.com/path").unwrap(), ("example.com".into(), None, Some(443)));
        let (host, addr, port) = split_target("[2001:db8::1]:22").unwrap();
        assert_eq!((host.as_str(), addr, port), ("2001:db8::1", "2001:db8::1".parse().ok(), Some(22)));
        let (host, addr, port) = split_target("192.0.2.7").unwrap();
        assert_eq!((host.as_str(), addr, port), ("192.0.2.7", "192.0.2.7".parse().ok(), None));
    }

    #[test]
    fn the_target_is_exactly_one_host() {
        assert!(split_target("").unwrap_err().contains("Type a host"));
        assert!(split_target("a.example b.example").unwrap_err().contains("one host"));
        assert!(split_target("192.0.2.0/30").unwrap_err().contains("not a range"));
        assert!(split_target("192.0.2.1-192.0.2.4").unwrap_err().contains("not a range"));
        assert!(split_target("exa mple").is_err());
        assert!(split_target("example.com:notaport").is_err());
    }

    #[test]
    fn an_entry_becomes_one_entry_for_each_port() {
        assert_eq!(entries_for_ports("nas.lan", &[443, 22], false), ["nas.lan:443", "nas.lan:22"]);
        assert_eq!(entries_for_ports("nas.lan  NAS in the closet", &[443], true), ["nas.lan  NAS in the closet", "nas.lan:443 NAS in the closet"]);
        assert_eq!(entries_for_ports("2001:db8::1 every=5s", &[22], false), ["[2001:db8::1]:22 every=5s"]);
        assert_eq!(entries_for_ports("[2001:db8::1]", &[22], false), ["[2001:db8::1]:22"]);
        assert_eq!(entries_for_ports("192.0.2.0/30", &[80], false), ["192.0.2.0/30:80"]);
        assert_eq!(entries_for_ports("192.0.2.1-192.0.2.4", &[80], false), ["192.0.2.1-192.0.2.4:80"]);
        // No ports chosen, a port or scheme already there, or not a host: unchanged.
        assert_eq!(entries_for_ports("nas.lan", &[], true), ["nas.lan"]);
        assert_eq!(entries_for_ports("nas.lan:8443 web", &[443], true), ["nas.lan:8443 web"]);
        assert_eq!(entries_for_ports("https://example.com", &[22], false), ["https://example.com"]);
        assert_eq!(entries_for_ports("# a note", &[22], false), ["# a note"]);
        // Every result is something the host list accepts as a TCP check.
        for entry in entries_for_ports("192.0.2.9", &[22, 80, 443], false) {
            let report = parse_host_list(&entry, &ParseOptions::default());
            assert!(report.warnings.is_empty() && matches!(report.hosts[0].probe, Probe::Tcp { .. }), "{entry}");
        }
    }

}
