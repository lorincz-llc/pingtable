//! Host-list parsing.
//!
//! Accepted target forms:
//!
//! * `example.com`, `192.168.1.10`, `2001:db8::1`            ICMP
//! * `example.com:443`, `192.168.1.10:22`, `[2001:db8::1]:443` TCP connect
//! * `https://example.com/path`                               TCP 443 (http is 80)
//! * `192.168.1.10-192.168.1.20`                              IPv4 range
//! * `192.168.1.0/24`                                         IPv4 CIDR block, /16 or smaller
//!
//! Hosts from a range or a block are pinged every [`BLOCK_EVERY_SECS`] seconds
//! unless an `every=` says otherwise. A name that ends in a number, such as
//! `10.0.0.010`, is refused: the system resolver would read it as some other
//! address.
//!
//! Lines starting with `#` are comments. A line `Group: name` puts the
//! targets after it into that group.
//!
//! Options are `key=value` words and apply to the target (or group) they
//! follow: `every=5s` (interval), `timeout=500ms`, `tag=name` (repeatable).
//! A host's own options win over its group's.
//!
//! Options that shape the ICMP packet: `size=1400` (payload bytes), `ttl=8`,
//! `df=on` (do not fragment), `dscp=46` or `tos=184` (traffic marking) and
//! `data=hello` or `data=0xDEADBEEF` (payload contents, repeated to fill it).

use crate::model::{Overrides, Probe};
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Hard cap on the number of targets one list may expand to.
pub const MAX_HOSTS: usize = 65_536;

/// Hosts from an address block or range are pinged this often, in seconds,
/// unless the list gives them or their group an interval of its own, or the
/// global interval is longer. A block often holds many addresses nobody
/// answers on, and on a local network each of those costs a broadcast.
pub const BLOCK_EVERY_SECS: u32 = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ParseOptions {
    /// One target per line, followed by a free-text description.
    pub description_format: bool,
    /// Leave out the network address of a CIDR block.
    pub cidr_skip_first: bool,
    /// Leave out the broadcast address of a CIDR block.
    pub cidr_skip_last: bool,
}

impl Default for ParseOptions {
    fn default() -> Self {
        ParseOptions { description_format: false, cidr_skip_first: true, cidr_skip_last: true }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ParsedHost {
    /// Name or address as it should be shown.
    pub host: String,
    /// Set when the target was given as an IP address.
    pub addr: Option<IpAddr>,
    pub probe: Probe,
    pub description: String,
    pub group: Option<String>,
    pub tags: Vec<String>,
    /// Host options merged over the group's options.
    pub overrides: Overrides,
    /// The line of the list this host came from, counting from 1.
    pub line: usize,
    /// True when the host is one address of a block or range written as one
    /// entry. See [`BLOCK_EVERY_SECS`].
    pub from_block: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ParseReport {
    pub hosts: Vec<ParsedHost>,
    pub warnings: Vec<String>,
}

pub fn parse_host_list(text: &str, opts: &ParseOptions) -> ParseReport {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut report = ParseReport::default();
    let mut group: Option<String> = None;
    let mut group_opts = Options::default();

    'lines: for (idx, raw) in text.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = strip_prefix_ignore_case(line, "group:") {
            let mut words: Vec<&str> = rest.split_whitespace().collect();
            group_opts = take_trailing_options(&mut words, line_no, &mut report.warnings);
            let name = words.join(" ");
            group = if name.is_empty() { None } else { Some(name) };
            continue;
        }

        // Each item is one target spec, its description and its own options.
        let mut items: Vec<(&str, String, Options)> = Vec::new();
        if opts.description_format {
            let mut words: Vec<&str> = line.split_whitespace().collect();
            let own = take_trailing_options(&mut words, line_no, &mut report.warnings);
            if let Some((spec, rest)) = words.split_first() {
                items.push((spec, rest.join(" "), own));
            }
        } else {
            for word in line.split(|c: char| c == ',' || c == ';' || c.is_whitespace()).filter(|s| !s.is_empty()) {
                if word.contains('=') && !word.contains("://") {
                    match items.last_mut() {
                        Some(item) => item.2.apply(word, line_no, &mut report.warnings),
                        None => report
                            .warnings
                            .push(format!("Line {line_no}: option \"{word}\" has no host before it.")),
                    }
                } else {
                    items.push((word, String::new(), Options::default()));
                }
            }
        }

        for (spec, description, own) in items {
            match expand_spec(spec, opts) {
                Ok(entries) => {
                    let from_block = entries.len() > 1;
                    for (host, addr, probe) in entries {
                        if report.hosts.len() >= MAX_HOSTS {
                            report
                                .warnings
                                .push(format!("Stopped at {MAX_HOSTS} hosts; the rest of the list was ignored."));
                            break 'lines;
                        }
                        let mut tags = group_opts.tags.clone();
                        for tag in &own.tags {
                            if !tags.contains(tag) {
                                tags.push(tag.clone());
                            }
                        }
                        report.hosts.push(ParsedHost {
                            host,
                            addr,
                            probe,
                            description: description.clone(),
                            group: group.clone(),
                            tags,
                            overrides: own.overrides.clone().or(group_opts.overrides.clone()),
                            line: line_no,
                            from_block,
                        });
                    }
                }
                Err(why) => report.warnings.push(format!("Line {line_no}: \"{spec}\" skipped: {why}.")),
            }
        }
    }
    report
}

#[derive(Clone, Debug, Default)]
struct Options {
    overrides: Overrides,
    tags: Vec<String>,
}

impl Options {
    fn apply(&mut self, word: &str, line_no: usize, warnings: &mut Vec<String>) {
        let Some((key, value)) = word.split_once('=') else { return };
        let result = match key.to_ascii_lowercase().as_str() {
            "every" | "interval" => parse_duration_ms(value, 1000).map(|ms| {
                self.overrides.interval_secs = Some(ms.div_ceil(1000).clamp(1, 86_400) as u32);
            }),
            "timeout" => parse_duration_ms(value, 1).map(|ms| {
                self.overrides.timeout_ms = Some(ms.clamp(50, 60_000) as u32);
            }),
            "tag" => {
                if value.is_empty() {
                    Err("empty tag".to_string())
                } else {
                    if !self.tags.iter().any(|t| t == value) {
                        self.tags.push(value.to_string());
                    }
                    Ok(())
                }
            }
            "size" => match value.parse::<u32>() {
                Ok(n) if n <= 65_500 => {
                    self.overrides.payload_size = Some(n as u16);
                    Ok(())
                }
                _ => Err("size must be 0 to 65500 bytes".to_string()),
            },
            "ttl" => match value.parse::<u8>() {
                Ok(n) if n >= 1 => {
                    self.overrides.ttl = Some(n);
                    Ok(())
                }
                _ => Err("ttl must be 1 to 255".to_string()),
            },
            "df" => match value.to_ascii_lowercase().as_str() {
                "on" | "yes" | "true" | "1" => {
                    self.overrides.dont_fragment = Some(true);
                    Ok(())
                }
                "off" | "no" | "false" | "0" => {
                    self.overrides.dont_fragment = Some(false);
                    Ok(())
                }
                _ => Err("df must be on or off".to_string()),
            },
            "tos" => match value.parse::<u8>() {
                Ok(n) => {
                    self.overrides.tos = Some(n);
                    Ok(())
                }
                _ => Err("tos must be 0 to 255".to_string()),
            },
            "dscp" => match value.parse::<u8>() {
                Ok(n) if n <= 63 => {
                    self.overrides.tos = Some(n << 2);
                    Ok(())
                }
                _ => Err("dscp must be 0 to 63".to_string()),
            },
            "data" => parse_payload(value).map(|bytes| {
                self.overrides.payload = Some(bytes);
            }),
            other => Err(format!("unknown option \"{other}\"")),
        };
        if let Err(why) = result {
            warnings.push(format!("Line {line_no}: \"{word}\" ignored: {why}."));
        }
    }
}

/// Remove `key=value` words from the end of `words` and return them parsed.
fn take_trailing_options(words: &mut Vec<&str>, line_no: usize, warnings: &mut Vec<String>) -> Options {
    let mut taken = Vec::new();
    while words.len() > 1 {
        match words.last() {
            Some(word) if is_option_word(word) => taken.push(words.pop().unwrap()),
            _ => break,
        }
    }
    let mut options = Options::default();
    for word in taken.into_iter().rev() {
        options.apply(word, line_no, warnings);
    }
    options
}

pub(crate) fn is_option_word(word: &str) -> bool {
    match word.split_once('=') {
        Some((key, _)) => !key.is_empty() && key.chars().all(|c| c.is_ascii_alphabetic()) && !word.contains("://"),
        None => false,
    }
}

/// Payload contents: `0x` followed by hex digits, or plain text.
fn parse_payload(text: &str) -> Result<Vec<u8>, String> {
    let bytes = match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(hex) => {
            if hex.is_empty() || hex.len() % 2 != 0 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err("hex data needs an even number of hex digits after 0x".to_string());
            }
            (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(0)).collect()
        }
        None => text.as_bytes().to_vec(),
    };
    if bytes.is_empty() || bytes.len() > 1024 {
        return Err("data must be 1 to 1024 bytes".to_string());
    }
    Ok(bytes)
}

/// Parse `500ms`, `5s`, `2m`, `1h` or a bare number (in `bare_unit_ms`).
fn parse_duration_ms(text: &str, bare_unit_ms: u64) -> Result<u64, String> {
    let lower = text.to_ascii_lowercase();
    let (number, unit_ms) = if let Some(n) = lower.strip_suffix("ms") {
        (n, 1)
    } else if let Some(n) = lower.strip_suffix('s') {
        (n, 1000)
    } else if let Some(n) = lower.strip_suffix('m') {
        (n, 60_000)
    } else if let Some(n) = lower.strip_suffix('h') {
        (n, 3_600_000)
    } else {
        (lower.as_str(), bare_unit_ms)
    };
    match number.parse::<f64>() {
        Ok(n) if n > 0.0 && n.is_finite() => Ok((n * unit_ms as f64).round() as u64),
        _ => Err(format!("\"{text}\" is not a duration")),
    }
}

type Entry = (String, Option<IpAddr>, Probe);

fn expand_spec(spec: &str, opts: &ParseOptions) -> Result<Vec<Entry>, String> {
    // URL: keep the host, take the port from the scheme unless one is given.
    if let Some((scheme, rest)) = spec.split_once("://") {
        let default_port = match scheme.to_ascii_lowercase().as_str() {
            "http" | "ws" => Some(80),
            "https" | "wss" => Some(443),
            "ssh" | "sftp" => Some(22),
            "tcp" => None,
            other => return Err(format!("unknown scheme \"{other}\"")),
        };
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        let authority = authority.rsplit('@').next().unwrap_or(authority);
        let mut entries = expand_spec(authority, opts)?;
        for entry in &mut entries {
            if entry.2 == Probe::Icmp {
                match default_port {
                    Some(port) => entry.2 = Probe::Tcp { port },
                    None => return Err("a tcp:// target needs a port".into()),
                }
            }
        }
        return Ok(entries);
    }

    // [IPv6] or [IPv6]:port
    if let Some(rest) = spec.strip_prefix('[') {
        let end = rest.find(']').ok_or("missing closing ]")?;
        let ip: Ipv6Addr = rest[..end].parse().map_err(|_| "not an IPv6 address")?;
        let tail = &rest[end + 1..];
        let probe = if tail.is_empty() {
            Probe::Icmp
        } else if let Some(port) = tail.strip_prefix(':') {
            Probe::Tcp { port: parse_port(port)? }
        } else {
            return Err("unexpected text after ]".into());
        };
        return Ok(vec![(ip.to_string(), Some(IpAddr::V6(ip)), probe)]);
    }

    // Bare IPv6 address.
    if let Ok(ip) = spec.parse::<Ipv6Addr>() {
        return Ok(vec![(ip.to_string(), Some(IpAddr::V6(ip)), Probe::Icmp)]);
    }

    let (host_part, probe) = match spec.rsplit_once(':') {
        Some((host, port)) => {
            if host.contains(':') {
                return Err("not a valid address; write IPv6 with a port as [address]:port".into());
            }
            (host, Probe::Tcp { port: parse_port(port)? })
        }
        None => (spec, Probe::Icmp),
    };
    if host_part.is_empty() {
        return Err("no host".into());
    }

    if let Some((base, bits)) = host_part.split_once('/') {
        let base: Ipv4Addr = base.parse().map_err(|_| "CIDR blocks must start with an IPv4 address")?;
        let bits: u32 = bits.parse().map_err(|_| "prefix length is not a number")?;
        if bits > 32 {
            return Err("prefix length must be 32 or less".into());
        }
        // Refused before it is expanded: a /8 alone is 16.7 million entries.
        let size = 1u64 << (32 - bits);
        if size > MAX_HOSTS as u64 {
            return Err(format!(
                "a /{bits} block has {} addresses, more than the {} a list can hold; split it into blocks of /16 or smaller",
                grouped(size),
                grouped(MAX_HOSTS as u64)
            ));
        }
        let mask: u32 = if bits == 32 { u32::MAX } else { !(u32::MAX >> bits) };
        let mut first = u32::from(base) & mask;
        let mut last = first | !mask;
        if bits <= 30 {
            if opts.cidr_skip_first {
                first += 1;
            }
            if opts.cidr_skip_last {
                last -= 1;
            }
        }
        return Ok(ipv4_span(first, last, probe));
    }

    if let Some((a, b)) = host_part.split_once('-') {
        if let Ok(start) = a.parse::<Ipv4Addr>() {
            let Ok(end) = b.parse::<Ipv4Addr>() else {
                // `10.0.0.1-20` would otherwise pass as a name that never resolves.
                let [x, y, z, _] = start.octets();
                return Err(match b.parse::<u8>() {
                    Ok(n) => format!("write both ends of a range in full, as {start}-{x}.{y}.{z}.{n}"),
                    Err(_) => "write both ends of a range as IPv4 addresses".into(),
                });
            };
            let (a, b) = (u32::from(start), u32::from(end));
            if a > b {
                return Err("range end is before its start".into());
            }
            if b - a >= 65_535 {
                return Err("range is larger than 65,535 addresses".into());
            }
            return Ok(ipv4_span(a, b, probe));
        }
    }

    if let Ok(ip) = host_part.parse::<Ipv4Addr>() {
        return Ok(vec![(ip.to_string(), Some(IpAddr::V4(ip)), probe)]);
    }

    if let Some(why) = numeric_name_problem(host_part) {
        return Err(why);
    }
    if !is_host_name(host_part) {
        return Err("not a host name or address".into());
    }
    Ok(vec![(host_part.to_string(), None, probe)])
}

fn ipv4_span(first: u32, last: u32, probe: Probe) -> Vec<Entry> {
    (first..=last)
        .map(|n| {
            let ip = Ipv4Addr::from(n);
            (ip.to_string(), Some(IpAddr::V4(ip)), probe)
        })
        .collect()
}

fn parse_port(text: &str) -> Result<u16, String> {
    match text.parse::<u16>() {
        Ok(port) if port > 0 => Ok(port),
        _ => Err(format!("\"{text}\" is not a port number")),
    }
}

fn is_host_name(text: &str) -> bool {
    text.len() <= 253
        && !text.starts_with(['.', '-'])
        && text.chars().all(|c| c.is_alphanumeric() || matches!(c, '-' | '.' | '_'))
}

/// Why `name` cannot be looked up as a name, when it ends in a number.
///
/// The system resolver reads such a name as an address in an old shorthand:
/// `192.168.001.010` is 192.168.1.8 (a leading zero means octal), `192.168.1`
/// is 192.168.0.1 and `0x7f.1` is 127.0.0.1. No real name ends in a number, so
/// it is refused rather than pinging an address nobody wrote. None for an
/// ordinary name. Check for a plain address first: `10.0.0.1` ends in a number too.
pub fn numeric_name_problem(name: &str) -> Option<String> {
    let name = name.strip_suffix('.').unwrap_or(name);
    let labels: Vec<&str> = name.split('.').collect();
    let decimal = |label: &str| !label.is_empty() && label.bytes().all(|b| b.is_ascii_digit());
    let hex = |label: &str| {
        label.strip_prefix("0x").or_else(|| label.strip_prefix("0X")).is_some_and(|digits| digits.bytes().all(|b| b.is_ascii_hexdigit()))
    };
    let last = labels.last().copied().unwrap_or("");
    if !decimal(last) && !hex(last) {
        return None;
    }
    if labels.len() == 4 && labels.iter().all(|l| decimal(l)) {
        let plain: Vec<&str> = labels.iter().map(|l| if l.trim_start_matches('0').is_empty() { "0" } else { l.trim_start_matches('0') }).collect();
        let plain = plain.join(".");
        if plain.parse::<Ipv4Addr>().is_ok() {
            return Some(if labels.iter().any(|l| l.len() > 1 && l.starts_with('0')) {
                format!("an address cannot have leading zeros, which would be read as octal; write {plain}")
            } else {
                format!("write the address as {plain}")
            });
        }
    }
    Some("a name cannot end in a number, and an IPv4 address is four numbers from 0 to 255 without leading zeros".into())
}

/// `16777216` as `16,777,216`.
fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn strip_prefix_ignore_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix).then(|| &text[prefix.len()..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Overrides;

    fn simple(text: &str) -> ParseReport {
        parse_host_list(text, &ParseOptions::default())
    }

    fn described(text: &str) -> ParseReport {
        parse_host_list(text, &ParseOptions { description_format: true, ..Default::default() })
    }

    fn hosts(report: &ParseReport) -> Vec<&str> {
        report.hosts.iter().map(|h| h.host.as_str()).collect()
    }

    #[test]
    fn simple_list_splits_on_separators() {
        let r = simple("example.com, 10.0.0.1;10.0.0.2\tfoo-bar.lan\n8.8.8.8");
        assert_eq!(hosts(&r), ["example.com", "10.0.0.1", "10.0.0.2", "foo-bar.lan", "8.8.8.8"]);
        assert!(r.warnings.is_empty());
        assert_eq!(r.hosts[0].addr, None);
        assert_eq!(r.hosts[1].addr, Some("10.0.0.1".parse().unwrap()));
        assert!(r.hosts.iter().all(|h| h.probe == Probe::Icmp));
    }

    #[test]
    fn port_means_tcp() {
        let r = simple("192.168.0.100:80 www.example.net:443 [2001:db8::1]:8443");
        assert_eq!(r.hosts[0].probe, Probe::Tcp { port: 80 });
        assert_eq!(r.hosts[1].probe, Probe::Tcp { port: 443 });
        assert_eq!(r.hosts[2].probe, Probe::Tcp { port: 8443 });
        assert_eq!(r.hosts[2].host, "2001:db8::1");
    }

    #[test]
    fn ipv6_forms() {
        let r = simple("2001:db8::1 [::1] fe80::1");
        assert_eq!(hosts(&r), ["2001:db8::1", "::1", "fe80::1"]);
        assert!(r.hosts.iter().all(|h| h.probe == Probe::Icmp && h.addr.is_some()));
    }

    #[test]
    fn range_expands_inclusive() {
        let r = simple("192.168.0.10-192.168.0.13");
        assert_eq!(hosts(&r), ["192.168.0.10", "192.168.0.11", "192.168.0.12", "192.168.0.13"]);
        let r = simple("10.0.0.5-10.0.0.1");
        assert!(r.hosts.is_empty());
        assert_eq!(r.warnings.len(), 1);
    }

    #[test]
    fn cidr_skips_network_and_broadcast_by_default() {
        let r = simple("192.168.1.0/30");
        assert_eq!(hosts(&r), ["192.168.1.1", "192.168.1.2"]);
        let r = simple("192.168.1.77/24");
        assert_eq!(r.hosts.len(), 254);
        assert_eq!(r.hosts[0].host, "192.168.1.1");
        assert_eq!(r.hosts[253].host, "192.168.1.254");
    }

    #[test]
    fn cidr_can_keep_every_address() {
        let opts = ParseOptions { cidr_skip_first: false, cidr_skip_last: false, ..Default::default() };
        let r = parse_host_list("10.1.2.0/29:22", &opts);
        assert_eq!(r.hosts.len(), 8);
        assert!(r.hosts.iter().all(|h| h.probe == Probe::Tcp { port: 22 }));
        let r = parse_host_list("10.1.2.4/31 10.1.2.9/32", &ParseOptions::default());
        assert_eq!(hosts(&r), ["10.1.2.4", "10.1.2.5", "10.1.2.9"]);
    }

    #[test]
    fn description_format_with_groups_and_comments() {
        let text = "\u{feff}Group: Servers\n192.168.1.10 Main Server\n192.168.1.30:443 HTTPS Web Site\n#192.168.1.14 Host 03\n\ngroup:  Edge \nrouter.lan\n";
        let r = described(text);
        assert_eq!(hosts(&r), ["192.168.1.10", "192.168.1.30", "router.lan"]);
        assert_eq!(r.hosts[0].description, "Main Server");
        assert_eq!(r.hosts[0].group.as_deref(), Some("Servers"));
        assert_eq!(r.hosts[1].probe, Probe::Tcp { port: 443 });
        assert_eq!(r.hosts[1].description, "HTTPS Web Site");
        assert_eq!(r.hosts[2].group.as_deref(), Some("Edge"));
        assert_eq!(r.hosts[2].description, "");
    }

    #[test]
    fn urls_become_tcp_checks() {
        let r = simple("https://example.com/status?x=1 http://10.0.0.9 ssh://user@box.lan tcp://db.lan:5432");
        assert_eq!(hosts(&r), ["example.com", "10.0.0.9", "box.lan", "db.lan"]);
        let ports: Vec<Probe> = r.hosts.iter().map(|h| h.probe).collect();
        assert_eq!(
            ports,
            [Probe::Tcp { port: 443 }, Probe::Tcp { port: 80 }, Probe::Tcp { port: 22 }, Probe::Tcp { port: 5432 }]
        );
    }

    #[test]
    fn bad_entries_are_reported_not_fatal() {
        let r = simple("good.lan bad host! 10.0.0.1:99999 10.0.0.0/4 [::1 ftp://x");
        assert_eq!(hosts(&r), ["good.lan", "bad"]);
        assert_eq!(r.warnings.len(), 5);
        assert!(r.warnings[0].starts_with("Line 1:"));
    }

    #[test]
    fn expansion_is_capped() {
        let r = simple("10.0.0.0/16 10.1.0.0/16");
        assert_eq!(r.hosts.len(), MAX_HOSTS);
        assert!(r.warnings.iter().any(|w| w.contains("Stopped at")));
    }

    #[test]
    fn a_block_too_big_for_a_list_is_refused_before_it_is_expanded() {
        // A /8 is 16.7 million addresses. Expanded first and cut down after, it
        // took over a gigabyte, and a failed allocation ends the process.
        let r = simple("10.0.0.0/8 192.168.1.1");
        assert_eq!(hosts(&r), ["192.168.1.1"]);
        assert_eq!(r.warnings, ["Line 1: \"10.0.0.0/8\" skipped: a /8 block has 16,777,216 addresses, more than the 65,536 a list can hold; split it into blocks of /16 or smaller."]);
        let r = simple("10.0.0.0/15:22 172.16.0.0/0 10.0.0.0/33");
        assert!(r.hosts.is_empty());
        assert_eq!(r.warnings.len(), 3, "{:?}", r.warnings);
        assert!(r.warnings[1].contains("4,294,967,296"), "{:?}", r.warnings);
        // The largest block that fits is still taken whole.
        let opts = ParseOptions { cidr_skip_first: false, cidr_skip_last: false, ..Default::default() };
        assert_eq!(parse_host_list("10.0.0.0/16", &opts).hosts.len(), MAX_HOSTS);
    }

    #[test]
    fn names_that_end_in_a_number_are_refused() {
        // The system resolver reads these as addresses in an old shorthand and
        // would ping something else: 192.168.1.8 for the first.
        let r = simple("192.168.001.010 10.0.0.010 10.0.0.1.");
        assert!(r.hosts.is_empty());
        assert_eq!(
            r.warnings,
            [
                "Line 1: \"192.168.001.010\" skipped: an address cannot have leading zeros, which would be read as octal; write 192.168.1.10.",
                "Line 1: \"10.0.0.010\" skipped: an address cannot have leading zeros, which would be read as octal; write 10.0.0.10.",
                "Line 1: \"10.0.0.1.\" skipped: write the address as 10.0.0.1.",
            ]
        );
        for odd in ["192.168.1", "10.0.0.256", "8080", "0x7f.1", "0x7f000001", "host.123", "db.123:5432"] {
            let r = simple(odd);
            assert!(r.hosts.is_empty(), "{odd} was accepted");
            assert!(r.warnings[0].contains("without leading zeros"), "{odd}: {:?}", r.warnings);
        }
        // Names with numbers in them, but not at the end, are names.
        let r = simple("host1.lan 10-0-0-1.example 1.example.com 0xcafe.lan server2");
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        assert_eq!(r.hosts.len(), 5);
    }

    #[test]
    fn a_short_range_asks_for_both_ends() {
        let r = simple("10.0.0.1-20 10.0.0.1-x");
        assert!(r.hosts.is_empty());
        assert_eq!(r.warnings[0], "Line 1: \"10.0.0.1-20\" skipped: write both ends of a range in full, as 10.0.0.1-10.0.0.20.");
        assert!(r.warnings[1].contains("as IPv4 addresses"), "{:?}", r.warnings);
    }

    #[test]
    fn hosts_from_a_block_are_marked() {
        let r = simple("10.0.0.0/30 10.0.0.9-10.0.0.10 10.0.0.20/32 10.0.0.30-10.0.0.30 one.lan");
        let marks: Vec<bool> = r.hosts.iter().map(|h| h.from_block).collect();
        assert_eq!(marks, [true, true, true, true, false, false, false]);
    }

    #[test]
    fn options_follow_their_host() {
        let r = simple("fast.lan every=5s timeout=300ms tag=lan slow.example.com every=2m\nplain.lan");
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        assert_eq!(r.hosts[0].overrides, Overrides { interval_secs: Some(5), timeout_ms: Some(300), ..Overrides::default() });
        assert_eq!(r.hosts[0].tags, ["lan"]);
        assert_eq!(r.hosts[1].overrides, Overrides { interval_secs: Some(120), ..Overrides::default() });
        assert!(r.hosts[2].overrides.is_empty());
    }

    #[test]
    fn group_options_are_inherited_and_host_wins() {
        let text = "Group: Home Lab every=5s tag=lan\n10.0.0.1 Router\n10.0.0.2 NAS box every=1s tag=storage\nGroup: Cloud\nvps.example.com Web";
        let r = described(text);
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        assert_eq!(r.hosts[0].group.as_deref(), Some("Home Lab"));
        assert_eq!(r.hosts[0].overrides.interval_secs, Some(5));
        assert_eq!(r.hosts[0].description, "Router");
        assert_eq!(r.hosts[1].overrides.interval_secs, Some(1));
        assert_eq!(r.hosts[1].description, "NAS box");
        assert_eq!(r.hosts[1].tags, ["lan", "storage"]);
        assert_eq!(r.hosts[2].group.as_deref(), Some("Cloud"));
        assert!(r.hosts[2].overrides.is_empty() && r.hosts[2].tags.is_empty());
    }

    #[test]
    fn range_options_apply_to_every_address() {
        let r = simple("10.0.0.1-10.0.0.3 every=10s");
        assert_eq!(r.hosts.len(), 3);
        assert!(r.hosts.iter().all(|h| h.overrides.interval_secs == Some(10)));
    }

    #[test]
    fn bad_options_warn() {
        let r = simple("every=5s a.lan colour=red timeout=soon");
        assert_eq!(hosts(&r), ["a.lan"]);
        assert_eq!(r.warnings.len(), 3);
    }

    #[test]
    fn packet_options() {
        let r = simple("big.lan size=1400 df=on ttl=8 dscp=46 data=0xDEADBEEF\ntext.lan data=hello tos=16 df=off");
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        let o = &r.hosts[0].overrides;
        assert_eq!((o.payload_size, o.dont_fragment, o.ttl, o.tos), (Some(1400), Some(true), Some(8), Some(184)));
        assert_eq!(o.payload.as_deref(), Some(&[0xDE, 0xAD, 0xBE, 0xEF][..]));
        assert!(o.shapes_packet());
        let o = &r.hosts[1].overrides;
        assert_eq!((o.payload.as_deref(), o.tos, o.dont_fragment), (Some(&b"hello"[..]), Some(16), Some(false)));

        let bad = simple("x.lan size=70000 ttl=0 df=maybe dscp=64 data=0xABC");
        assert_eq!(bad.warnings.len(), 5, "{:?}", bad.warnings);
        assert!(!bad.hosts[0].overrides.shapes_packet());
    }
}
