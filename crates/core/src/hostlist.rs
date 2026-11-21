//! Editing the host list as text, and importing hosts from other files.
//!
//! The host list is plain text that people edit by hand, so changes made from
//! the table (add a host, remove one) are small edits to that text: comments,
//! blank lines and ordering stay exactly as they were.

use crate::parse::{is_option_word, parse_host_list, ParseOptions};
use serde::Serialize;
use std::net::Ipv6Addr;

/// The group a `Group:` line starts, without its options. `Some("")` for a
/// bare `Group:` line, which ends grouping. None for any other line.
fn group_of_line(line: &str) -> Option<String> {
    let line = line.trim();
    let head = line.get(..6)?;
    if !head.eq_ignore_ascii_case("group:") {
        return None;
    }
    let mut words: Vec<&str> = line[6..].split_whitespace().collect();
    while words.len() > 1 && words.last().is_some_and(|w| is_option_word(w)) {
        words.pop();
    }
    Some(words.join(" "))
}

/// Group names in the order they first appear.
pub fn group_names(text: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for name in text.lines().filter_map(group_of_line).filter(|n| !n.is_empty()) {
        if !names.iter().any(|n| n.eq_ignore_ascii_case(&name)) {
            names.push(name);
        }
    }
    names
}

/// Add lines to the list: into `group` (created at the end if it does not
/// exist yet), or among the ungrouped hosts at the top when `group` is None.
pub fn add_entries(text: &str, group: Option<&str>, entries: &[String]) -> String {
    let entries: Vec<&str> = entries.iter().map(|e| e.trim()).filter(|e| !e.is_empty()).collect();
    if entries.is_empty() {
        return text.to_string();
    }
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let group = group.map(str::trim).filter(|g| !g.is_empty());

    // Where each line's section starts: (line index of the header or None for the top, group name).
    let mut section_of_line: Vec<String> = Vec::with_capacity(lines.len());
    let mut current = String::new();
    for line in &lines {
        if let Some(name) = group_of_line(line) {
            current = name;
        }
        section_of_line.push(current.clone());
    }

    let wanted = group.unwrap_or("");
    let last_in_section = (0..lines.len()).rev().find(|&i| section_of_line[i].eq_ignore_ascii_case(wanted) && !lines[i].trim().is_empty());

    match (group, last_in_section) {
        // The section exists: put the new lines after its last non-blank line.
        (_, Some(at)) => {
            for (n, entry) in entries.iter().enumerate() {
                lines.insert(at + 1 + n, entry.to_string());
            }
        }
        // A new group goes at the end.
        (Some(name), None) => {
            if lines.last().is_some_and(|l| !l.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(format!("Group: {name}"));
            lines.extend(entries.iter().map(|e| e.to_string()));
        }
        // Ungrouped, and nothing ungrouped yet: the top of the list, before the first group.
        (None, None) => {
            let has_groups = lines.iter().any(|l| group_of_line(l).is_some());
            let mut block: Vec<String> = entries.iter().map(|e| e.to_string()).collect();
            if has_groups {
                block.push(String::new());
                lines.splice(0..0, block);
            } else {
                lines.extend(block);
            }
        }
    }
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// Delete lines by number (1-based, as the parser counts them).
pub fn remove_lines(text: &str, line_numbers: &[usize]) -> String {
    let kept: Vec<&str> = text
        .lines()
        .enumerate()
        .filter(|(i, _)| !line_numbers.contains(&(i + 1)))
        .map(|(_, line)| line)
        .collect();
    let mut out = kept.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

/// Rewrite a list typed in the loose form (several hosts on a line, split by
/// spaces, commas or semicolons) as one host per line, so descriptions can
/// follow. Options stay with the host they followed.
pub fn to_described(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || group_of_line(trimmed).is_some() {
            out.push(line.to_string());
            continue;
        }
        let mut current: Option<String> = None;
        for word in trimmed.split(|c: char| c == ',' || c == ';' || c.is_whitespace()).filter(|w| !w.is_empty()) {
            match (&mut current, is_option_word(word)) {
                (Some(host), true) => {
                    host.push(' ');
                    host.push_str(word);
                }
                _ => {
                    out.extend(current.take());
                    current = Some(word.to_string());
                }
            }
        }
        out.extend(current);
    }
    let mut joined = out.join("\n");
    joined.push('\n');
    joined
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

/// Hosts read from another file, as lines ready for the host list (one host
/// per line followed by its description, with `Group:` headers).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Import {
    /// What the file was taken to be, for showing to the user.
    pub format: String,
    pub lines: Vec<String>,
    pub hosts: usize,
    /// Rows that were left out, and why.
    pub skipped: Vec<String>,
}

fn split_csv_row(line: &str, delimiter: char) -> Vec<String> {
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cell.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            c if c == delimiter && !quoted => cells.push(std::mem::take(&mut cell)),
            c => cell.push(c),
        }
    }
    cells.push(cell);
    cells.into_iter().map(|c| c.trim().to_string()).collect()
}

fn normalise_header(cell: &str) -> String {
    cell.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_ascii_lowercase()
}

const HOST_COLUMNS: [&str; 12] =
    ["host", "hostname", "address", "target", "destination", "fqdn", "dns", "dnsname", "url", "server", "node", "device"];
const IP_COLUMNS: [&str; 4] = ["ip", "ipaddress", "ipv4", "ipv4address"];
const DESCRIPTION_COLUMNS: [&str; 10] =
    ["description", "desc", "label", "alias", "comment", "comments", "notes", "note", "displayname", "friendlyname"];
const GROUP_COLUMNS: [&str; 6] = ["group", "folder", "category", "site", "location", "groupname"];
const PORT_COLUMNS: [&str; 2] = ["port", "tcpport"];
const TAG_COLUMNS: [&str; 2] = ["tags", "tag"];
const EVERY_COLUMNS: [&str; 4] = ["every", "interval", "intervalsecs", "intervalseconds"];
/// pingtable's own export: `icmp` or `tcp:443`, the timeout in milliseconds,
/// and the packet options as host-list words (`size=1400 ttl=8`).
const PROBE_COLUMNS: [&str; 1] = ["probe"];
const TIMEOUT_COLUMNS: [&str; 1] = ["timeoutms"];
const OPTIONS_COLUMNS: [&str; 1] = ["options"];
/// The options an `options` column may give. Interval, timeout and tags have columns of their own.
const PACKET_OPTIONS: [&str; 6] = ["size", "ttl", "df", "tos", "dscp", "data"];

/// A cell a spreadsheet would have run as a formula, as `export::to_csv`
/// writes it (`'=x`), back to its text.
fn undo_defuse(cell: &str) -> &str {
    match cell.strip_prefix('\'') {
        Some(rest) if rest.starts_with(['=', '+', '-', '@']) => rest,
        _ => cell,
    }
}

/// True when the word is one packet option with a value the host list accepts.
fn is_packet_option(word: &str) -> bool {
    let known = word.split_once('=').is_some_and(|(key, _)| PACKET_OPTIONS.contains(&key.to_ascii_lowercase().as_str()));
    if !known || !is_option_word(word) {
        return false;
    }
    let report = parse_host_list(&format!("x.example {word}"), &ParseOptions { description_format: true, ..ParseOptions::default() });
    report.warnings.is_empty() && report.hosts.first().is_some_and(|h| h.overrides.shapes_packet())
}

/// True when the text is one target the host list would accept.
fn is_target(text: &str) -> bool {
    if text.is_empty() || text.contains(char::is_whitespace) {
        return false;
    }
    let report = parse_host_list(text, &ParseOptions { description_format: true, ..ParseOptions::default() });
    report.warnings.is_empty() && !report.hosts.is_empty()
}

/// Read hosts from a file's text. Understands CSV with a header row (from
/// pingtable itself, PingInfoView and most other tools: columns are matched by
/// name), CSV without one (host, description, group), and plain host lists.
pub fn import_text(text: &str) -> Import {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let rows: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let Some(first) = rows.first() else { return Import { format: "empty file".into(), ..Import::default() } };

    let delimiter = [',', ';', '\t'].into_iter().max_by_key(|d| first.matches(*d).count()).unwrap_or(',');
    let header: Vec<String> = split_csv_row(first, delimiter).iter().map(|c| normalise_header(c)).collect();
    let find = |names: &[&str]| header.iter().position(|h| names.contains(&h.as_str()));
    let name_column = header.iter().position(|h| h == "name");
    let host_column = find(&HOST_COLUMNS);
    let ip_column = find(&IP_COLUMNS);
    let has_header = host_column.is_some() || ip_column.is_some() || (name_column.is_some() && header.len() > 1);
    let looks_like_csv = first.matches(delimiter).count() > 0;

    if !has_header && !looks_like_csv {
        // A plain list: keep it as it is.
        let lines: Vec<String> = text.lines().map(|l| l.trim_end().to_string()).collect();
        let hosts = parse_host_list(text, &ParseOptions { description_format: true, ..ParseOptions::default() }).hosts.len();
        return Import { format: "plain host list".into(), lines, hosts, skipped: Vec::new() };
    }

    // Which column is which. "name" is the host when nothing better names one, else a description.
    let (host_at, ip_at, description_at) = if has_header {
        let host_at = host_column.or(if ip_column.is_none() { name_column } else { None });
        let description_at = find(&DESCRIPTION_COLUMNS).or(if host_at != name_column { name_column } else { None });
        (host_at, ip_column, description_at)
    } else {
        (Some(0), None, Some(1))
    };
    let group_at = if has_header { find(&GROUP_COLUMNS) } else { Some(2) };
    let (port_at, tags_at, every_at) = if has_header { (find(&PORT_COLUMNS), find(&TAG_COLUMNS), find(&EVERY_COLUMNS)) } else { (None, None, None) };
    let (probe_at, timeout_at, options_at) =
        if has_header { (find(&PROBE_COLUMNS), find(&TIMEOUT_COLUMNS), find(&OPTIONS_COLUMNS)) } else { (None, None, None) };

    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    let mut import = Import {
        format: if has_header { "CSV with a header row".into() } else { "CSV without a header row".into() },
        ..Import::default()
    };
    for (index, row) in rows.iter().enumerate().skip(usize::from(has_header)) {
        let cells = split_csv_row(row, delimiter);
        let cell = |at: Option<usize>| at.and_then(|i| cells.get(i)).map(|c| c.as_str()).unwrap_or("");
        let mut target = cell(host_at).to_string();
        if !is_target(&target) {
            target = cell(ip_at).to_string();
        }
        if !is_target(&target) {
            import.skipped.push(format!("Row {}: no usable host in \"{}\".", index + 1, row.chars().take(60).collect::<String>()));
            continue;
        }
        // The port, from a port column or from a `probe` column such as `tcp:443`.
        let probe = cell(probe_at);
        let port = match cell(port_at) {
            "" => probe.get(..4).filter(|kind| kind.eq_ignore_ascii_case("tcp:")).map(|_| &probe[4..]).unwrap_or(""),
            given => given,
        };
        if let Some(port) = port.parse::<u16>().ok().filter(|&p| p > 0) {
            if !target.contains("://") && !target.contains(':') {
                target = format!("{target}:{port}");
            } else if let Ok(v6) = target.parse::<Ipv6Addr>() {
                target = format!("[{v6}]:{port}");
            }
        }
        let mut line = target;
        let description = undo_defuse(cell(description_at)).split_whitespace().collect::<Vec<_>>().join(" ");
        // A description word with "=" would read as an option; keep such words out.
        let description: Vec<&str> = description.split(' ').filter(|w| !w.is_empty() && !is_option_word(w)).collect();
        if !description.is_empty() {
            line.push(' ');
            line.push_str(&description.join(" "));
        }
        for tag in undo_defuse(cell(tags_at)).split([' ', '|', ',']).filter(|t| !t.is_empty() && !t.contains('=')) {
            line.push_str(&format!(" tag={tag}"));
        }
        if let Ok(secs) = cell(every_at).trim_end_matches('s').parse::<u32>() {
            if secs > 0 {
                line.push_str(&format!(" every={secs}s"));
            }
        }
        if let Ok(ms) = cell(timeout_at).trim_end_matches("ms").parse::<u32>() {
            if ms > 0 {
                line.push_str(&format!(" timeout={ms}ms"));
            }
        }
        // Packet options only, and only those the host list takes as they are.
        for word in cell(options_at).split_whitespace().filter(|w| is_packet_option(w)) {
            line.push(' ');
            line.push_str(word);
        }
        // A group word with "=" would read as an option of the group; keep such words out.
        let group = undo_defuse(cell(group_at)).split_whitespace().filter(|w| !is_option_word(w)).collect::<Vec<_>>().join(" ");
        match groups.iter_mut().find(|(name, _)| name.eq_ignore_ascii_case(&group)) {
            Some((_, lines)) => lines.push(line),
            None => groups.push((group, vec![line])),
        }
        import.hosts += 1;
    }

    // Ungrouped hosts first, then each group under its header.
    groups.sort_by_key(|(name, _)| !name.is_empty());
    for (name, lines) in groups {
        if !name.is_empty() {
            if !import.lines.is_empty() {
                import.lines.push(String::new());
            }
            import.lines.push(format!("Group: {name}"));
        }
        import.lines.extend(lines);
    }
    import
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = "# my network\nrouter.lan Router\n\nGroup: Servers every=5s\nnas.lan NAS\nweb.lan Web\n\nGroup: Cloud\nvps.example.com VPS\n";

    #[test]
    fn groups_are_listed_without_their_options() {
        assert_eq!(group_names(LIST), ["Servers", "Cloud"]);
        assert!(group_names("a.lan\nb.lan").is_empty());
    }

    #[test]
    fn adding_to_an_existing_group_goes_after_its_last_host() {
        let out = add_entries(LIST, Some("servers"), &["db.lan Database".into(), "  ".into()]);
        assert_eq!(out, "# my network\nrouter.lan Router\n\nGroup: Servers every=5s\nnas.lan NAS\nweb.lan Web\ndb.lan Database\n\nGroup: Cloud\nvps.example.com VPS\n");
        let last = add_entries(LIST, Some("Cloud"), &["vps2.example.com".into()]);
        assert!(last.ends_with("vps.example.com VPS\nvps2.example.com\n"));
    }

    #[test]
    fn a_new_group_is_created_at_the_end() {
        let out = add_entries(LIST, Some("Printers"), &["hp.lan Laser".into()]);
        assert!(out.ends_with("vps.example.com VPS\n\nGroup: Printers\nhp.lan Laser\n"), "{out}");
        assert_eq!(group_names(&out), ["Servers", "Cloud", "Printers"]);
    }

    #[test]
    fn ungrouped_hosts_stay_out_of_every_group() {
        let out = add_entries(LIST, None, &["ap.lan Access point".into()]);
        assert!(out.starts_with("# my network\nrouter.lan Router\nap.lan Access point\n\nGroup: Servers"), "{out}");

        let only_groups = "Group: A\na.lan\n";
        assert_eq!(add_entries(only_groups, None, &["x.lan".into()]), "x.lan\n\nGroup: A\na.lan\n");
        assert_eq!(add_entries("", None, &["x.lan".into()]), "x.lan\n");
        assert_eq!(add_entries("a.lan\n", None, &["x.lan".into()]), "a.lan\nx.lan\n");

        // Whatever the edit, the parser must agree about who is in which group.
        let report = parse_host_list(&out, &ParseOptions { description_format: true, ..ParseOptions::default() });
        let ap = report.hosts.iter().find(|h| h.host == "ap.lan").unwrap();
        assert_eq!((ap.group.as_deref(), ap.description.as_str()), (None, "Access point"));
    }

    #[test]
    fn lines_are_removed_by_number() {
        let out = remove_lines(LIST, &[2, 6]);
        assert_eq!(out, "# my network\n\nGroup: Servers every=5s\nnas.lan NAS\n\nGroup: Cloud\nvps.example.com VPS\n");
        assert_eq!(remove_lines("a.lan\n", &[1]), "");
    }

    #[test]
    fn loose_lists_become_one_host_per_line() {
        let out = to_described("# note\na.lan, b.lan every=5s tag=x; 10.0.0.1\nGroup: G\nc.lan\n");
        assert_eq!(out, "# note\na.lan\nb.lan every=5s tag=x\n10.0.0.1\nGroup: G\nc.lan\n");
        let before = parse_host_list("a.lan, b.lan every=5s tag=x; 10.0.0.1", &ParseOptions::default());
        let after = parse_host_list(&to_described("a.lan, b.lan every=5s tag=x; 10.0.0.1"), &ParseOptions { description_format: true, ..ParseOptions::default() });
        assert_eq!(before.hosts.len(), after.hosts.len());
        assert_eq!(before.hosts[1].overrides, after.hosts[1].overrides);
    }

    #[test]
    fn csv_cells_split_with_quotes() {
        assert_eq!(split_csv_row(r#"a,"b, c","say ""hi""", d "#, ','), ["a", "b, c", "say \"hi\"", "d"]);
        assert_eq!(split_csv_row("x;y;;z", ';'), ["x", "y", "", "z"]);
    }

    #[test]
    fn csv_from_another_ping_tool() {
        // The shape PingInfoView exports: many columns, host name and IP both present.
        let csv = "Host Name,IP Address,Reply IP Address,Succeed Count,Failed Count,Description\n\
                   nas.lan,192.168.1.20,192.168.1.20,120,0,\"NAS, in the closet\"\n\
                   ,192.168.1.1,192.168.1.1,120,0,Router\n\
                   ,,,,,nothing here\n";
        let import = import_text(csv);
        assert_eq!(import.format, "CSV with a header row");
        assert_eq!(import.lines, ["nas.lan NAS, in the closet", "192.168.1.1 Router"]);
        assert_eq!(import.hosts, 2);
        assert_eq!(import.skipped.len(), 1);
    }

    #[test]
    fn csv_in_pingtables_own_format_round_trips_groups_tags_and_intervals() {
        let csv = "host;description;group;tags;interval_secs;port\n\
                   web.lan;Web server;Servers;lan prod;5;443\n\
                   db.lan;Database;Servers;;;\n\
                   8.8.8.8;Google DNS;;;;\n\
                   vps.example.com;VPS;Cloud;;30;\n";
        let import = import_text(csv);
        assert_eq!(
            import.lines,
            ["8.8.8.8 Google DNS", "", "Group: Servers", "web.lan:443 Web server tag=lan tag=prod every=5s", "db.lan Database", "", "Group: Cloud", "vps.example.com VPS every=30s"]
        );
        let report = parse_host_list(&import.lines.join("\n"), &ParseOptions { description_format: true, ..ParseOptions::default() });
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert_eq!(report.hosts.len(), 4);
        assert_eq!(report.hosts[1].tags, ["lan", "prod"]);
        assert_eq!(report.hosts[1].group.as_deref(), Some("Servers"));
    }

    #[test]
    fn name_is_the_host_only_when_nothing_else_is() {
        let by_name = import_text("Name,Location\nprinter.lan,Office\n");
        assert_eq!(by_name.lines, ["Group: Office", "printer.lan"]);
        let with_ip = import_text("Name,IP\nFront door camera,10.0.0.50\n");
        assert_eq!(with_ip.lines, ["10.0.0.50 Front door camera"]);
    }

    #[test]
    fn headerless_csv_and_plain_lists() {
        let csv = import_text("10.0.0.1,Router,Core\n10.0.0.2,Switch,Core\nnot a host!,x,\n");
        assert_eq!(csv.format, "CSV without a header row");
        assert_eq!(csv.lines, ["Group: Core", "10.0.0.1 Router", "10.0.0.2 Switch"]);
        assert_eq!(csv.skipped.len(), 1);

        let plain = import_text("\u{feff}192.168.1.10 Main Server\n#192.168.1.14 Host 03\n192.168.1.30:443 HTTPS\n");
        assert_eq!(plain.format, "plain host list");
        assert_eq!(plain.hosts, 2);
        assert_eq!(plain.lines.len(), 3);
        assert_eq!(import_text("").format, "empty file");
    }

    #[test]
    fn description_words_that_look_like_options_are_dropped() {
        let import = import_text("host,description\nx.lan,rack=4 top shelf\n");
        assert_eq!(import.lines, ["x.lan top shelf"]);
    }

    #[test]
    fn group_words_that_look_like_options_are_dropped() {
        // Written into a `Group:` line, they would set an interval or tag for the whole group.
        let import = import_text("host,group\nx.lan,Lab every=1s tag=all\ny.lan,every=5s\n");
        assert_eq!(import.lines, ["y.lan", "", "Group: Lab", "x.lan"]);
        let report = parse_host_list(&import.lines.join("\n"), &ParseOptions { description_format: true, ..ParseOptions::default() });
        assert!(report.hosts.iter().all(|h| h.overrides.is_empty() && h.tags.is_empty()), "{:?}", report.hosts);
    }

    #[test]
    fn probe_and_timeout_columns_are_read() {
        let csv = "host,probe,timeout_ms,description\n\
                   web.lan,tcp:443,250,Web\n\
                   2001:db8::1,tcp:22,,SSH over IPv6\n\
                   ping.lan,icmp,,\n\
                   ssh.lan:2222,tcp:22,,Port in the host wins\n\
                   dash.lan,icmp,,'-minus first\n";
        let import = import_text(csv);
        assert_eq!(
            import.lines,
            [
                "web.lan:443 Web timeout=250ms",
                "[2001:db8::1]:22 SSH over IPv6",
                "ping.lan",
                "ssh.lan:2222 Port in the host wins",
                "dash.lan -minus first"
            ]
        );
    }
}
