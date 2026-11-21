//! Render the table for people and for other programs.

use crate::engine::{HostRow, Snapshot, SCHEMA_VERSION};
use crate::model::Probe;
use crate::{netinfo, now_ms};
use serde::Serialize;

/// `2026-11-16T21:04:05.123Z` from Unix milliseconds.
pub fn rfc3339(t_ms: i64) -> String {
    let ms = t_ms.rem_euclid(1000);
    let secs = t_ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil date from day count (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{ms:03}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

pub fn probe_label(probe: Probe) -> String {
    match probe {
        Probe::Icmp => "icmp".to_string(),
        Probe::Tcp { port } => format!("tcp:{port}"),
    }
}

fn ms(us: Option<u32>) -> String {
    us.map(|us| format!("{:.3}", us as f64 / 1000.0)).unwrap_or_default()
}

fn time(t_ms: Option<i64>) -> String {
    t_ms.map(rfc3339).unwrap_or_default()
}

/// Quote a CSV cell when needed, and defuse cells a spreadsheet would run
/// as a formula.
fn csv_cell(text: &str) -> String {
    let risky = text.starts_with(['=', '+', '-', '@', '\t', '\r']);
    let text = if risky { format!("'{text}") } else { text.to_string() };
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text
    }
}

const CSV_HEADER: &[&str] = &[
    "host", "ip", "description", "group", "tags", "probe", "state", "last_status", "last_rtt_ms", "last_ttl",
    "avg_rtt_ms", "min_rtt_ms", "max_rtt_ms", "jitter_ms", "sent", "ok", "failed", "failed_pct",
    "recent_loss_pct", "consec_failed", "max_consec_failed", "last_ok_at", "last_failed_at", "reply_ip",
    "hostname", "mac", "interval_secs", "timeout_ms", "options", "exported_at", "from",
];

/// The table as CSV, one row per host, with when it was exported and from
/// which computer.
///
/// `interval_secs` and `timeout_ms` are left blank where the host follows the
/// global settings, and `options` holds its own packet options (`size=1400
/// ttl=8`), so that importing the file (see [`crate::hostlist::import_text`])
/// gives each host the values that were its own and no others.
pub fn to_csv(rows: &[HostRow]) -> String {
    csv_at(rows, now_ms(), &netinfo::local_name())
}

fn csv_at(rows: &[HostRow], exported_at_ms: i64, from: &str) -> String {
    let exported_at = rfc3339(exported_at_ms);
    let from = csv_cell(from);
    let mut out = CSV_HEADER.join(",");
    out.push_str("\r\n");
    for r in rows {
        let cells: Vec<String> = vec![
            csv_cell(&r.host),
            csv_cell(r.ip.as_deref().unwrap_or("")),
            csv_cell(&r.description),
            csv_cell(r.group.as_deref().unwrap_or("")),
            csv_cell(&r.tags.join(" ")),
            probe_label(r.probe),
            r.state.to_string(),
            csv_cell(r.last_status_label.unwrap_or("")),
            ms(r.last_rtt_us),
            r.last_ttl.map(|t| t.to_string()).unwrap_or_default(),
            ms(r.avg_rtt_us),
            ms(r.min_rtt_us),
            ms(r.max_rtt_us),
            ms(r.jitter_us),
            r.sent.to_string(),
            r.ok.to_string(),
            r.failed.to_string(),
            format!("{:.2}", r.failed_pct),
            format!("{:.2}", r.recent_loss_pct),
            r.consec_failed.to_string(),
            r.max_consec_failed.to_string(),
            time(r.last_ok_at_ms),
            time(r.last_failed_at_ms),
            csv_cell(r.reply_ip.as_deref().unwrap_or("")),
            csv_cell(r.hostname.as_deref().unwrap_or("")),
            r.mac.clone().unwrap_or_default(),
            if r.interval_is_global { String::new() } else { r.interval_secs.to_string() },
            if r.timeout_is_global { String::new() } else { r.timeout_ms.to_string() },
            csv_cell(&r.options),
            exported_at.clone(),
            from.clone(),
        ];
        out.push_str(&cells.join(","));
        out.push_str("\r\n");
    }
    out
}

#[derive(Serialize)]
struct JsonExport<'a> {
    schema_version: u32,
    exported_at: String,
    hosts: &'a [HostRow],
}

pub fn to_json(snapshot: &Snapshot) -> String {
    let doc = JsonExport { schema_version: SCHEMA_VERSION, exported_at: rfc3339(snapshot.now_ms), hosts: &snapshot.hosts };
    serde_json::to_string_pretty(&doc).unwrap_or_else(|_| "{}".to_string())
}

/// Fixed-width text table for the terminal.
pub fn to_text(rows: &[HostRow]) -> String {
    let header = ["HOST", "IP", "CHECK", "STATE", "STATUS", "TIME ms", "TTL", "DESCRIPTION"];
    let mut table: Vec<[String; 8]> = Vec::with_capacity(rows.len());
    for r in rows {
        table.push([
            r.host.clone(),
            r.ip.clone().unwrap_or_default(),
            probe_label(r.probe),
            r.state.to_uppercase(),
            r.last_status_label.unwrap_or("-").to_string(),
            r.last_rtt_us.map(|us| format!("{:.2}", us as f64 / 1000.0)).unwrap_or_else(|| "-".into()),
            r.last_ttl.map(|t| t.to_string()).unwrap_or_else(|| "-".into()),
            r.description.clone(),
        ]);
    }
    let mut widths: Vec<usize> = header.iter().map(|h| h.len()).collect();
    for row in &table {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }
    let mut out = String::new();
    let mut line = |cells: &[String]| {
        let mut text = String::new();
        for (i, cell) in cells.iter().enumerate() {
            let pad = widths[i].saturating_sub(cell.chars().count());
            // Right-align the numeric columns.
            if i == 5 || i == 6 {
                text.push_str(&" ".repeat(pad));
                text.push_str(cell);
            } else {
                text.push_str(cell);
                text.push_str(&" ".repeat(pad));
            }
            text.push_str("  ");
        }
        out.push_str(text.trim_end());
        out.push('\n');
    };
    line(&header.map(String::from));
    for row in &table {
        line(row);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;
    use crate::hostlist::import_text;
    use crate::model::Settings;

    fn settings() -> Settings {
        Settings { interval_secs: 2, timeout_ms: 800, description_format: true, reverse_dns: false, read_mac: false, ..Settings::default() }
    }

    fn rows_of(list: &str) -> Vec<HostRow> {
        let engine = Engine::new(settings(), list);
        engine.shutdown();
        engine.snapshot(0).hosts
    }

    #[test]
    fn csv_says_when_and_from_where_and_leaves_global_values_blank() {
        let csv = csv_at(&rows_of("a.lan\nb.lan:22 every=10s timeout=300ms\n"), 1_794_864_645_123, "desk.lan");
        let lines: Vec<&str> = csv.split("\r\n").collect();
        assert!(lines[0].ends_with(",interval_secs,timeout_ms,options,exported_at,from"), "{}", lines[0]);
        assert!(lines[1].ends_with(",,,,2026-11-16T21:30:45.123Z,desk.lan"), "{}", lines[1]);
        assert!(lines[2].ends_with(",10,300,,2026-11-16T21:30:45.123Z,desk.lan"), "{}", lines[2]);
        assert!(lines[1].starts_with("a.lan,") && lines[2].contains(",tcp:22,"));
        assert_eq!(lines.len(), 4, "a header, two rows and the end of the last line");
        // The computer's name is a cell like any other.
        assert!(csv_at(&rows_of("a.lan"), 0, "=desk, two").ends_with(",1970-01-01T00:00:00.000Z,\"'=desk, two\"\r\n"));
    }

    #[test]
    fn csv_goes_back_in_as_the_same_hosts() {
        let list = "router.lan Router\n\
                    192.0.2.7:8443 Admin page every=10s\n\
                    [2001:db8::1]:443 Web over IPv6 timeout=300ms\n\
                    2001:db8::2 Plain IPv6\n\
                    Group: Servers tag=lab every=5s\n\
                    nas.lan NAS, in the closet tag=storage\n\
                    web.lan:443 -backup link timeout=2s\n\
                    Group: Edge\n\
                    192.0.2.16/30\n\
                    edge.lan every=2s timeout=800ms\n\
                    plain.lan\n\
                    big.lan Big size=1400 ttl=8 df=on dscp=46 data=hello\n\
                    odd.lan tos=5 df=off data=0xBEEF\n";
        let before = rows_of(list);
        assert_eq!(before.len(), 12);
        let import = import_text(&to_csv(&before));
        assert_eq!(import.format, "CSV with a header row");
        assert!(import.skipped.is_empty(), "{:?}", import.skipped);
        let after = rows_of(&import.lines.join("\n"));

        let shape = |r: &HostRow| {
            (r.host.clone(), r.probe, r.description.clone(), r.group.clone(), r.tags.clone(), r.interval_secs, r.timeout_ms, r.packet.clone())
        };
        assert_eq!(after.iter().map(shape).collect::<Vec<_>>(), before.iter().map(shape).collect::<Vec<_>>(), "{:#?}", import.lines);
        let line = |host: &str| import.lines.iter().find(|l| l.starts_with(host)).unwrap().as_str();
        // A host's own values are kept, even when they equal the global ones here.
        assert_eq!(line("edge.lan"), "edge.lan every=2s timeout=800ms");
        // Values that were the global ones stay unpinned, so they follow the settings.
        assert_eq!(line("plain.lan"), "plain.lan");
        assert_eq!(line("192.0.2.17"), "192.0.2.17 every=30s");
        assert_eq!(line("big.lan"), "big.lan Big size=1400 ttl=8 df=on dscp=46 data=0x68656C6C6F");
        assert_eq!(line("odd.lan"), "odd.lan df=off tos=5 data=0xBEEF");
    }

    #[test]
    fn an_options_column_gives_packet_options_and_nothing_else() {
        let import = import_text("host,options\nx.lan,size=100 every=1s tag=all ttl=4 junk ,\ny.lan,\"df=on,data=a size=70000\"\n");
        assert_eq!(import.lines, ["x.lan size=100 ttl=4", "y.lan"]);
    }

    #[test]
    fn rfc3339_known_dates() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(rfc3339(951_782_400_000), "2000-02-29T00:00:00.000Z");
        assert_eq!(rfc3339(1_794_864_645_123), "2026-11-16T21:30:45.123Z");
        assert_eq!(rfc3339(-1), "1969-12-31T23:59:59.999Z");
    }

    #[test]
    fn csv_cells_are_quoted_and_defused() {
        assert_eq!(csv_cell("plain"), "plain");
        assert_eq!(csv_cell("a,b"), "\"a,b\"");
        assert_eq!(csv_cell("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_cell("=cmd|' /C calc'!A0"), "'=cmd|' /C calc'!A0");
        assert_eq!(csv_cell("-1,2"), "\"'-1,2\"");
    }
}
