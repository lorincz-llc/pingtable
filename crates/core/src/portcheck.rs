//! Port check: is anything listening on these TCP ports of one host?
//!
//! Every port gets one TCP connection attempt, all at the same time, so the
//! whole check takes no longer than one timeout. A connection that opens is
//! closed again at once; no data is sent.
//!
//! * the connection opens: something is listening (`open`)
//! * the host refuses it: the host is there, nothing listens (`closed`)
//! * nothing comes back: a firewall dropped it, or the host is off (`no_answer`)

use crate::model::{Probe, Status};
use crate::parse::{parse_host_list, ParseOptions};
use crate::probe::{self, ProbeParams};
use serde::Serialize;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::thread;

/// Most ports one check may try. This is a check of a few chosen ports, not a
/// scan of a whole range.
pub const MAX_PORTS: usize = 64;

/// The shortest time a port check should wait for each port. Windows tries a
/// refused connection again before it reports the refusal, which takes about
/// two seconds; with a shorter wait a closed port would be reported as one
/// that gave no answer.
pub const MIN_TIMEOUT_MS: u32 = 3000;

/// The ports offered as one-click choices, in the order they are shown.
pub const COMMON_PORTS: [u16; 12] = [22, 80, 443, 3389, 445, 53, 25, 3306, 5432, 1433, 8080, 8443];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PortState {
    /// The connection opened: something is listening.
    Open,
    /// The host answered and refused: nothing is listening.
    Closed,
    /// No reply before the timeout: filtered by a firewall, or the host is off.
    NoAnswer,
    /// The attempt could not be made or failed some other way.
    Failed,
}

impl PortState {
    /// Short text for a printed result.
    pub fn label(self) -> &'static str {
        match self {
            PortState::Open => "open",
            PortState::Closed => "closed",
            PortState::NoAnswer => "no answer",
            PortState::Failed => "failed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PortResult {
    pub port: u16,
    /// What usually listens on this port, when it is a well-known one.
    pub service: Option<&'static str>,
    pub state: PortState,
    pub status: Status,
    /// Time the connection took to open, or to be refused.
    pub rtt_us: Option<u32>,
}

/// What usually listens on a TCP port. Only a label: the check does not look
/// at what answers.
pub fn service_name(port: u16) -> Option<&'static str> {
    Some(match port {
        21 => "FTP",
        22 => "SSH",
        23 => "Telnet",
        25 => "SMTP",
        53 => "DNS",
        80 => "HTTP",
        88 => "Kerberos",
        110 => "POP3",
        135 => "RPC",
        139 => "NetBIOS",
        143 => "IMAP",
        179 => "BGP",
        389 => "LDAP",
        443 => "HTTPS",
        445 => "SMB",
        465 => "SMTPS",
        587 => "Mail submission",
        631 => "IPP printing",
        636 => "LDAPS",
        993 => "IMAPS",
        995 => "POP3S",
        1433 => "SQL Server",
        1521 => "Oracle",
        1883 => "MQTT",
        2049 => "NFS",
        3306 => "MySQL",
        3389 => "Remote Desktop",
        5432 => "PostgreSQL",
        5900 => "VNC",
        5985 => "WinRM",
        5986 => "WinRM over TLS",
        6379 => "Redis",
        8080 => "HTTP alternate",
        8443 => "HTTPS alternate",
        9100 => "Printer",
        27017 => "MongoDB",
        _ => return None,
    })
}

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
        _ => Err("A port check takes one host, not a range or a block.".into()),
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

/// Try every port once, all at the same time. Results come back in the order
/// the ports were given. Only `timeout` and the source addresses of `params`
/// matter here.
pub fn check(target: IpAddr, ports: &[u16], params: &ProbeParams) -> Vec<PortResult> {
    let mut wanted: Vec<u16> = Vec::new();
    for &port in ports {
        if port > 0 && !wanted.contains(&port) && wanted.len() < MAX_PORTS {
            wanted.push(port);
        }
    }
    let try_port = |port: u16| -> PortResult {
        let outcome = probe::tcp(SocketAddr::new(target, port), params);
        let state = match outcome.status {
            Status::Ok => PortState::Open,
            Status::Refused => PortState::Closed,
            Status::Timeout => PortState::NoAnswer,
            _ => PortState::Failed,
        };
        PortResult { port, service: service_name(port), state, status: outcome.status, rtt_us: outcome.rtt_us }
    };
    thread::scope(|scope| {
        let handles: Vec<_> = wanted.iter().map(|&port| (port, scope.spawn(move || try_port(port)))).collect();
        handles
            .into_iter()
            .map(|(port, handle)| {
                // A probe that panicked is reported as a failed attempt, not lost.
                handle.join().unwrap_or(PortResult { port, service: service_name(port), state: PortState::Failed, status: Status::Error, rtt_us: None })
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::time::Duration;

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
    fn a_port_check_takes_exactly_one_host() {
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
        for entry in entries_for_ports("192.0.2.9", &COMMON_PORTS, false) {
            let report = parse_host_list(&entry, &ParseOptions::default());
            assert!(report.warnings.is_empty() && matches!(report.hosts[0].probe, Probe::Tcp { .. }), "{entry}");
        }
    }

    #[test]
    fn open_and_closed_ports_are_told_apart() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let open = listener.local_addr().unwrap().port();
        // A port that was just free: bind it, note it, let it go.
        let closed = {
            let gone = TcpListener::bind("127.0.0.1:0").unwrap();
            gone.local_addr().unwrap().port()
        };
        let params = ProbeParams { timeout: Duration::from_millis(1500), ..ProbeParams::default() };
        let results = check("127.0.0.1".parse().unwrap(), &[open, closed, open], &params);
        assert_eq!(results.len(), 2, "a repeated port is tried once: {results:?}");
        assert_eq!((results[0].port, results[0].state, results[0].status), (open, PortState::Open, Status::Ok));
        assert!(results[0].rtt_us.is_some());
        assert_eq!((results[1].port, results[1].state, results[1].status), (closed, PortState::Closed, Status::Refused));
    }

    #[test]
    fn well_known_ports_have_a_label() {
        assert_eq!(service_name(443), Some("HTTPS"));
        assert_eq!(service_name(3389), Some("Remote Desktop"));
        assert_eq!(service_name(49152), None);
        assert!(COMMON_PORTS.iter().all(|&p| service_name(p).is_some()));
        assert!(check("127.0.0.1".parse().unwrap(), &[], &ProbeParams::default()).is_empty());
    }
}
