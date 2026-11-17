//! MCP server: pingtable's checks as tools an AI agent can call.
//!
//! `pingtable mcp` speaks the Model Context Protocol over standard input and
//! output: one JSON-RPC message per line in, one per line out. Nothing else
//! is ever written to standard output; faults go to standard error and the
//! log file.
//!
//! Both eras of the protocol are served. Revisions up to 2025-11-25 open with
//! an `initialize` handshake. Revision 2026-07-28 has none: every request
//! names its version, and `server/discover` lists what the server speaks.
//!
//! Four tools need nothing but this program (`ping`, `check_ports`,
//! `trace_route`, `network_info`). Two talk to a pingtable that is already
//! running on this computer, through its local API (`table_status`,
//! `table_add_hosts`).

use crate::{config, server};
use pingtable_core::probe::ProbeParams;
use pingtable_core::{netinfo, portcheck, trace, Engine, Probe, Settings};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::process::ExitCode;
use std::time::Duration;

/// The revision without a handshake.
const MODERN: &str = "2026-07-28";
/// Revisions that open with `initialize`, newest first.
const LEGACY: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
const VERSION_KEY: &str = "io.modelcontextprotocol/protocolVersion";
const SERVER_INFO_KEY: &str = "io.modelcontextprotocol/serverInfo";

/// Most hosts one `ping` call may expand to, and most rounds it may send.
const MAX_PING_HOSTS: usize = 256;
const MAX_PING_COUNT: u64 = 10;

const INSTRUCTIONS: &str = "pingtable checks whether machines and services on a network answer. \
Use `ping` to see if hosts are reachable and how fast; `check_ports` to see if a TCP service is listening \
(open), refused (closed) or silent (no_answer, usually a firewall or a host that is off); `trace_route` to \
find where a path stops; `network_info` for this computer's own addresses. `table_status` and \
`table_add_hosts` read and extend the table of a pingtable app that is already running here. Every check \
is sent from this computer, so results describe reachability from here. Only check hosts the user owns or \
is allowed to test, and prefer a few named ports to a sweep.";

type ToolResult = Result<(String, Value), String>;

/// The settings file named with `--config`, for the tools that find the
/// running app through it. Unset means the usual place.
static CONFIG: std::sync::OnceLock<Option<std::path::PathBuf>> = std::sync::OnceLock::new();

/// Read requests from standard input until it closes.
pub fn serve_stdio(config: Option<std::path::PathBuf>) -> ExitCode {
    let _ = CONFIG.set(config);
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if let Some(reply) = handle_line(&line) {
            if writeln!(out, "{reply}").and_then(|_| out.flush()).is_err() {
                break;
            }
        }
    }
    ExitCode::SUCCESS
}

/// Answer one line of input. None when the line needs no answer.
pub fn handle_line(line: &str) -> Option<String> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let reply = match serde_json::from_str::<Value>(line) {
        Ok(Value::Array(batch)) => {
            let replies: Vec<Value> = batch.iter().filter_map(handle).collect();
            if replies.is_empty() {
                return None;
            }
            Value::Array(replies)
        }
        Ok(message) => handle(&message)?,
        Err(_) => failure(Value::Null, -32700, "Parse error: the line is not JSON", None),
    };
    // serde_json never writes a raw newline inside a value, so this is one line.
    Some(reply.to_string())
}

fn server_info() -> Value {
    json!({ "name": "pingtable", "title": "pingtable", "version": env!("CARGO_PKG_VERSION") })
}

fn supported_versions() -> Vec<&'static str> {
    std::iter::once(MODERN).chain(LEGACY).collect()
}

fn failure(id: Value, code: i64, message: &str, data: Option<Value>) -> Value {
    let mut error = json!({ "code": code, "message": message });
    if let Some(data) = data {
        error["data"] = data;
    }
    json!({ "jsonrpc": "2.0", "id": id, "error": error })
}

fn success(id: Value, mut result: Value, modern: bool) -> Value {
    if modern {
        result["resultType"] = json!("complete");
        result["_meta"] = json!({ SERVER_INFO_KEY: server_info() });
    }
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn handle(message: &Value) -> Option<Value> {
    let id = message.get("id").cloned();
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        // Not a request. This server sends none of its own, so a stray
        // response is dropped; anything else with an id is told it is malformed.
        let is_response = message.get("result").is_some() || message.get("error").is_some();
        return match id {
            Some(id) if !is_response => Some(failure(id, -32600, "Invalid request: no method", None)),
            _ => None,
        };
    };
    // No id means a notification: act on nothing, answer nothing.
    let id = id?;
    let params = message.get("params").cloned().unwrap_or(Value::Null);

    let named = params.get("_meta").and_then(|meta| meta.get(VERSION_KEY)).and_then(Value::as_str);
    if let Some(version) = named {
        if !supported_versions().contains(&version) {
            let data = json!({ "supported": supported_versions(), "requested": version });
            return Some(failure(id, -32022, "Unsupported protocol version", Some(data)));
        }
    }
    let modern = named == Some(MODERN);

    Some(match method {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or("");
            let version = LEGACY.iter().copied().find(|v| *v == asked).unwrap_or(LEGACY[0]);
            let result = json!({
                "protocolVersion": version,
                "capabilities": { "tools": {} },
                "serverInfo": server_info(),
                "instructions": INSTRUCTIONS,
            });
            success(id, result, false)
        }
        "server/discover" => {
            let result = json!({
                "resultType": "complete",
                "supportedVersions": supported_versions(),
                "capabilities": { "tools": {} },
                "instructions": INSTRUCTIONS,
                "_meta": { SERVER_INFO_KEY: server_info() },
            });
            json!({ "jsonrpc": "2.0", "id": id, "result": result })
        }
        "ping" => success(id, json!({}), modern),
        "tools/list" => success(id, json!({ "tools": tools() }), modern),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let empty = json!({});
            let args = params.get("arguments").filter(|a| a.is_object()).unwrap_or(&empty);
            if !tools().iter().any(|tool| tool["name"] == name) {
                return Some(failure(id, -32602, &format!("Unknown tool: {name}"), None));
            }
            // A fault inside a tool must not take the server down with it.
            let outcome = std::panic::catch_unwind(|| call_tool(name, args))
                .unwrap_or_else(|_| Err("pingtable hit an internal fault running this tool. It is recorded in pingtable.log.".into()));
            let result = match outcome {
                Ok((text, data)) => json!({
                    "content": [{ "type": "text", "text": text }],
                    "structuredContent": data,
                    "isError": false,
                }),
                Err(why) => json!({ "content": [{ "type": "text", "text": why }], "isError": true }),
            };
            success(id, result, modern)
        }
        other => failure(id, -32601, &format!("Method not found: {other}"), None),
    })
}

/// The tools, as `tools/list` returns them.
fn tools() -> Vec<Value> {
    let read_only = json!({ "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": true });
    let local_read = json!({ "readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false });
    vec![
        json!({
            "name": "ping",
            "title": "Ping hosts",
            "description": "Ping one or more hosts from this computer and report which answered and how fast. \
An entry is a host name or an IP address (ICMP ping), or host:port (a TCP connection to that port, for hosts \
that block ICMP). Short IPv4 ranges (192.168.1.1-192.168.1.20) and blocks (192.168.1.0/28) expand to their \
addresses. Use count above 1 to measure loss. A host that does not answer may still be up: firewalls often \
drop pings.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "hosts": { "type": "array", "items": { "type": "string" }, "minItems": 1,
                        "description": "Hosts to ping, for example [\"192.168.1.1\", \"example.com\", \"example.com:443\"]. At most 256 after ranges expand." },
                    "count": { "type": "integer", "minimum": 1, "maximum": 10, "default": 1,
                        "description": "Pings sent to each host, one round after another." },
                    "timeout_ms": { "type": "integer", "minimum": 50, "maximum": 10000, "default": 1000,
                        "description": "How long to wait for each answer." }
                },
                "required": ["hosts"]
            },
            "annotations": read_only,
        }),
        json!({
            "name": "check_ports",
            "title": "Check TCP ports",
            "description": "Check whether TCP ports on one host are listening. Each port gets one connection attempt, \
closed the moment it opens; no data is sent. Result per port: open (something is listening), closed (the host \
answered and refused, so it is up but nothing listens there), or no_answer (nothing came back: a firewall \
dropped it, or the host is off). With no ports given, the usual ones are checked: 22, 80, 443, 3389, 445, 53, \
25, 3306, 5432, 1433, 8080, 8443. Takes up to 3 seconds when a port does not answer.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "host": { "type": "string", "description": "One host name or IP address, for example \"example.com\" or \"192.168.1.10\"." },
                    "ports": { "type": "array", "items": { "type": "integer", "minimum": 1, "maximum": 65535 }, "maxItems": 64,
                        "description": "Ports to check, for example [22, 443]. At most 64." },
                    "timeout_ms": { "type": "integer", "minimum": 200, "maximum": 10000, "default": 3000,
                        "description": "How long to wait for each port. Under 2500 a closed port on Windows can read as no_answer." }
                },
                "required": ["host"]
            },
            "annotations": read_only,
        }),
        json!({
            "name": "trace_route",
            "title": "Trace the route to a host",
            "description": "List the routers between this computer and a host, one line per hop, with the time each took \
to answer. Use it to see where a path stops when a host does not answer. Many routers stay silent, so gaps are \
normal; what matters is the last hop that answered.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "host": { "type": "string", "description": "One host name or IP address." },
                    "max_hops": { "type": "integer", "minimum": 1, "maximum": 30, "default": 30 },
                    "resolve_names": { "type": "boolean", "default": true, "description": "Look up a name for each router." }
                },
                "required": ["host"]
            },
            "annotations": read_only,
        }),
        json!({
            "name": "network_info",
            "title": "This computer's network",
            "description": "This computer's name, its network interfaces and addresses, and the address it sends from by \
default. Use it to learn which network the checks start from before reading their results. Sends nothing.",
            "inputSchema": { "type": "object", "properties": {} },
            "annotations": local_read,
        }),
        json!({
            "name": "table_status",
            "title": "Read the running table",
            "description": "Read the table of the pingtable app running on this computer: every host it watches, whether \
each is up or down, its last and average round-trip time and its loss. Use it to answer \"what is down right \
now\". Fails with a plain message when the app is not running; use ping for a one-off check then.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "only": { "type": "string", "enum": ["all", "down", "up"], "default": "all",
                        "description": "Which hosts to list. \"down\" includes hosts that are failing but not yet counted as down." }
                }
            },
            "annotations": local_read,
        }),
        json!({
            "name": "table_add_hosts",
            "title": "Add hosts to the running table",
            "description": "Add hosts to the table of the pingtable app running on this computer, so they are watched from \
now on. This changes the user's saved host list. An entry is a host name or address, optionally followed by a \
description (\"nas.lan NAS in the closet\"). Give ports to watch TCP ports instead of, or as well as, pinging.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "hosts": { "type": "array", "items": { "type": "string" }, "minItems": 1,
                        "description": "Entries to add, one host each." },
                    "group": { "type": "string", "description": "Group to put them in. Created if it does not exist. Omit for no group." },
                    "ports": { "type": "array", "items": { "type": "integer", "minimum": 1, "maximum": 65535 }, "maxItems": 64,
                        "description": "TCP ports to watch on each host: one row for each port." },
                    "ping": { "type": "boolean", "description": "With ports: also add the plain ping row. Defaults to false when ports are given." }
                },
                "required": ["hosts"]
            },
            "annotations": { "readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false },
        }),
    ]
}

fn call_tool(name: &str, args: &Value) -> ToolResult {
    match name {
        "ping" => tool_ping(args),
        "check_ports" => tool_check_ports(args),
        "trace_route" => tool_trace_route(args),
        "network_info" => tool_network_info(),
        "table_status" => tool_table_status(args),
        "table_add_hosts" => tool_table_add_hosts(args),
        other => Err(format!("Unknown tool: {other}")),
    }
}

// ------------------------------------------------------------- arguments

/// A list of strings. One string on its own is taken as a list of one.
fn strings(args: &Value, key: &str) -> Result<Vec<String>, String> {
    let clean = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    let list: Vec<String> = match args.get(key) {
        Some(Value::String(one)) => vec![clean(one)],
        Some(Value::Array(items)) => {
            let mut list = Vec::with_capacity(items.len());
            for item in items {
                match item.as_str() {
                    Some(text) => list.push(clean(text)),
                    None => return Err(format!("\"{key}\" must be a list of text entries.")),
                }
            }
            list
        }
        Some(_) => return Err(format!("\"{key}\" must be a list of text entries.")),
        None => Vec::new(),
    };
    let list: Vec<String> = list.into_iter().filter(|entry| !entry.is_empty()).collect();
    if list.is_empty() {
        return Err(format!("Give at least one entry in \"{key}\"."));
    }
    Ok(list)
}

fn whole(args: &Value, key: &str, default: u64, low: u64, high: u64) -> Result<u64, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(value) => match value.as_u64().or_else(|| value.as_str().and_then(|text| text.trim().parse().ok())) {
            Some(n) if (low..=high).contains(&n) => Ok(n),
            _ => Err(format!("\"{key}\" must be a whole number from {low} to {high}.")),
        },
    }
}

/// Ports as a list of numbers; a string such as "22, 443" or "8000-8010" is read too.
fn ports(args: &Value) -> Result<Vec<u16>, String> {
    let typed = match args.get("ports") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Array(items)) => {
            let mut words = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    Value::Number(n) => words.push(n.to_string()),
                    Value::String(text) => words.push(text.clone()),
                    _ => return Err("\"ports\" must be a list of port numbers.".into()),
                }
            }
            words.join(" ")
        }
        Some(_) => return Err("\"ports\" must be a list of port numbers.".into()),
    };
    portcheck::parse_ports(&typed)
}

fn ms(us: Option<u32>) -> Option<f64> {
    us.map(|us| (us as f64 / 10.0).round() / 100.0)
}

fn ms_text(us: Option<u32>) -> String {
    ms(us).map(|v| format!("{v} ms")).unwrap_or_else(|| "no time".into())
}

fn check_name(probe: Probe) -> String {
    match probe {
        Probe::Icmp => "icmp".into(),
        Probe::Tcp { port } => format!("tcp:{port}"),
    }
}

fn shown(host: &str, ip: Option<&str>) -> String {
    match ip {
        Some(ip) if ip != host => format!("{host} ({ip})"),
        _ => host.to_string(),
    }
}

// ----------------------------------------------------------------- tools

fn tool_ping(args: &Value) -> ToolResult {
    let hosts = strings(args, "hosts")?;
    let count = whole(args, "count", 1, 1, MAX_PING_COUNT)?;
    let timeout_ms = whole(args, "timeout_ms", 1000, 50, 10_000)? as u32;
    // One host to an entry. Words after it may only be options such as timeout=300ms.
    if let Some(bad) = hosts.iter().find(|h| h.starts_with('#') || h.to_ascii_lowercase().starts_with("group:") || h.split(' ').skip(1).any(|word| !word.contains('='))) {
        return Err(format!("\"{bad}\" is not one host. Give each host as its own entry, with no spaces in it."));
    }

    // One pass per round, no name lookups for addresses, no alert commands.
    let settings = Settings { repeat: false, reverse_dns: false, read_mac: false, timeout_ms, on_down_command: String::new(), on_up_command: String::new(), ..Settings::default() };
    let engine = Engine::new(settings.clone(), &hosts.join("\n"));
    let first = engine.snapshot(0);
    let total = first.hosts.len();
    if total == 0 || total > MAX_PING_HOSTS {
        engine.shutdown();
        return Err(if total == 0 {
            first.warnings.first().map(|w| w.trim_start_matches("Line 1: ").to_string()).unwrap_or_else(|| "None of those is a host name or an address.".into())
        } else {
            format!("That is {total} hosts. Ping at most {MAX_PING_HOSTS} in one call; split the list.")
        });
    }

    let waves = (total as u64).div_ceil(settings.max_concurrent.max(1) as u64);
    let budget = Duration::from_millis(waves * timeout_ms as u64) + Duration::from_secs(20);
    let mut finished = true;
    for round in 0..count {
        engine.ping_now();
        if !engine.wait_idle(budget) {
            finished = false;
            break;
        }
        if round + 1 < count {
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    let snapshot = engine.snapshot(0);
    engine.shutdown();

    let mut lines = Vec::with_capacity(total + 2);
    let mut rows = Vec::with_capacity(total);
    let mut answered = 0;
    for r in &snapshot.hosts {
        let up = r.ok > 0;
        answered += up as usize;
        let who = format!("{} {}", shown(&r.host, r.ip.as_deref()), check_name(r.probe));
        let status = r.last_status_label.unwrap_or("No result");
        lines.push(if up && count == 1 {
            format!("- {who}: up, {}", ms_text(r.last_rtt_us))
        } else if up {
            format!("- {who}: up, {} of {} answered, average {}, slowest {}", r.ok, r.sent, ms_text(r.avg_rtt_us), ms_text(r.max_rtt_us))
        } else if r.last_status == Some(pingtable_core::Status::Refused) {
            format!("- {who}: port closed (the host answered and refused the connection)")
        } else {
            format!("- {who}: no answer ({status})")
        });
        rows.push(json!({
            "host": r.host, "ip": r.ip, "check": check_name(r.probe), "up": up, "status": status,
            "sent": r.sent, "answered": r.ok, "loss_pct": (r.failed_pct as f64 * 10.0).round() / 10.0,
            "last_ms": ms(r.last_rtt_us), "avg_ms": ms(r.avg_rtt_us), "min_ms": ms(r.min_rtt_us), "max_ms": ms(r.max_rtt_us),
        }));
    }
    let mut text = format!("{answered} of {total} {} answered, pinged from {}.\n{}", if total == 1 { "host" } else { "hosts" }, netinfo::local_name(), lines.join("\n"));
    for warning in &snapshot.warnings {
        text.push_str(&format!("\nSkipped: {warning}"));
    }
    if !finished {
        text.push_str("\nSome hosts had no result before the time ran out.");
    }
    Ok((text, json!({ "from": netinfo::local_name(), "count": count, "timeout_ms": timeout_ms, "answered": answered, "total": total, "complete": finished, "hosts": rows, "skipped": snapshot.warnings })))
}

fn tool_check_ports(args: &Value) -> ToolResult {
    let typed = args.get("host").and_then(Value::as_str).unwrap_or("");
    let (host, addr, named) = portcheck::split_target(typed)?;
    let timeout_ms = whole(args, "timeout_ms", portcheck::MIN_TIMEOUT_MS as u64, 200, 10_000)? as u32;
    let mut wanted: Vec<u16> = named.into_iter().collect();
    wanted.extend(ports(args)?.into_iter().filter(|port| Some(*port) != named));
    if wanted.is_empty() {
        wanted = portcheck::COMMON_PORTS.to_vec();
    }
    if wanted.len() > portcheck::MAX_PORTS {
        return Err(format!("That is more than {} ports. Check fewer at a time.", portcheck::MAX_PORTS));
    }
    let settings = Settings::default();
    let Some(target) = addr.or_else(|| netinfo::resolve(&host, settings.prefer_ipv6)) else {
        return Err(format!("The name {host} does not resolve, so there is nothing to check."));
    };
    let params = ProbeParams { timeout: Duration::from_millis(timeout_ms as u64), ..ProbeParams::default() };
    let results = portcheck::check(target, &wanted, &params);
    let open = results.iter().filter(|r| r.state == portcheck::PortState::Open).count();

    let ip = target.to_string();
    let mut text = format!("{}: {open} of {} {} open, checked from {}.", shown(&host, Some(&ip)), results.len(), if results.len() == 1 { "port" } else { "ports" }, netinfo::local_name());
    let mut rows = Vec::with_capacity(results.len());
    for r in &results {
        let state = match r.state {
            portcheck::PortState::Open => "open",
            portcheck::PortState::Closed => "closed",
            portcheck::PortState::NoAnswer => "no_answer",
            portcheck::PortState::Failed => "failed",
        };
        let service = r.service.map(|s| format!(" {s}")).unwrap_or_default();
        let detail = match r.state {
            portcheck::PortState::Open => format!("open, {}", ms_text(r.rtt_us)),
            portcheck::PortState::Closed => "closed (the host refused the connection)".into(),
            portcheck::PortState::NoAnswer => "no answer (filtered by a firewall, or the host is off)".into(),
            portcheck::PortState::Failed => format!("failed ({})", r.status.label()),
        };
        text.push_str(&format!("\n- {}{service}: {detail}", r.port));
        rows.push(json!({ "port": r.port, "service": r.service, "state": state, "status": r.status.label(), "ms": ms(r.rtt_us) }));
    }
    Ok((text, json!({ "host": host, "ip": ip, "from": netinfo::local_name(), "timeout_ms": timeout_ms, "open": open, "total": results.len(), "ports": rows })))
}

fn tool_trace_route(args: &Value) -> ToolResult {
    let typed = args.get("host").and_then(Value::as_str).unwrap_or("");
    let (host, addr, _) = portcheck::split_target(typed)?;
    let max_hops = whole(args, "max_hops", trace::MAX_HOPS as u64, 1, trace::MAX_HOPS as u64)? as u8;
    let names = args.get("resolve_names").and_then(Value::as_bool).unwrap_or(true);
    let Some(target) = addr.or_else(|| netinfo::resolve(&host, Settings::default().prefer_ipv6)) else {
        return Err(format!("The name {host} does not resolve, so there is nothing to trace."));
    };
    let hops = trace::traceroute(target, max_hops, Duration::from_millis(1500), names);
    let reached = hops.last().is_some_and(|hop| hop.reached);
    let ip = target.to_string();
    let mut text = if reached {
        format!("Route to {}: reached in {} {}, traced from {}.", shown(&host, Some(&ip)), hops.len(), if hops.len() == 1 { "hop" } else { "hops" }, netinfo::local_name())
    } else {
        format!("Route to {}: not reached within {max_hops} hops, traced from {}. The last router that answered is where to look.", shown(&host, Some(&ip)), netinfo::local_name())
    };
    let mut rows = Vec::with_capacity(hops.len());
    for hop in &hops {
        let line = match hop.addr {
            Some(addr) => format!("\n{:>2}  {addr}  {}  {}", hop.ttl, hop.name.as_deref().unwrap_or("-"), ms_text(hop.rtt_us)),
            None => format!("\n{:>2}  *  no answer", hop.ttl),
        };
        text.push_str(&line);
        rows.push(json!({ "hop": hop.ttl, "address": hop.addr, "name": hop.name, "ms": ms(hop.rtt_us), "is_destination": hop.reached }));
    }
    Ok((text, json!({ "host": host, "ip": ip, "from": netinfo::local_name(), "reached": reached, "hops": rows })))
}

fn tool_network_info() -> ToolResult {
    let name = netinfo::local_name();
    let interfaces = netinfo::interfaces();
    // Asking for a route sends nothing. These are documentation addresses.
    let out_v4 = "192.0.2.1".parse().ok().and_then(netinfo::source_for).filter(|a| !a.is_unspecified());
    let out_v6 = "2001:db8::1".parse().ok().and_then(netinfo::source_for).filter(|a| !a.is_unspecified());
    let mut text = format!("This computer is {name}.");
    match out_v4 {
        Some(addr) => text.push_str(&format!(" It sends IPv4 from {addr} by default.")),
        None => text.push_str(" It has no IPv4 route out."),
    }
    if let Some(addr) = out_v6 {
        text.push_str(&format!(" It sends IPv6 from {addr}."));
    }
    let mut rows = Vec::with_capacity(interfaces.len());
    for i in &interfaces {
        text.push_str(&format!("\n- {}: {}{}", i.name, i.addr, if i.loopback { " (loopback)" } else { "" }));
        rows.push(json!({ "interface": i.name, "address": i.addr, "loopback": i.loopback }));
    }
    Ok((text, json!({ "computer": name, "default_source_ipv4": out_v4, "default_source_ipv6": out_v6, "interfaces": rows })))
}

/// Call the local API of a pingtable running on this computer.
fn local_api(method: &str, path: &str, body: Option<&Value>) -> Result<Value, String> {
    let Some((_, cfg)) = config::peek(CONFIG.get().and_then(|path| path.as_deref())) else {
        return Err("pingtable has not been started on this computer yet, so there is no table to read. Start pingtable.exe once, or use the ping tool for a one-off check.".into());
    };
    let (status, text) = server::call_local(cfg.port, &cfg.token, method, path, body.map(Value::to_string).as_deref())
        .map_err(|_| "The pingtable app is not running on this computer, so there is no table to read. Start pingtable.exe (or run pingtable-cli with no arguments), or use the ping tool for a one-off check.".to_string())?;
    let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if status != 200 {
        let why = value.get("error").and_then(Value::as_str).unwrap_or("the request was refused");
        return Err(format!("pingtable answered {status}: {why}"));
    }
    Ok(value)
}

fn tool_table_status(args: &Value) -> ToolResult {
    let only = args.get("only").and_then(Value::as_str).unwrap_or("all");
    if !["all", "down", "up"].contains(&only) {
        return Err("\"only\" must be all, down or up.".into());
    }
    let state = local_api("GET", "/api/v1/state", None)?;
    let counts = &state["counts"];
    let running = state["running"].as_bool().unwrap_or(false);
    let empty = Vec::new();
    let all = state["hosts"].as_array().unwrap_or(&empty);
    let us = |row: &Value, key: &str| ms(row[key].as_u64().map(|v| v.min(u32::MAX as u64) as u32));

    let mut rows = Vec::new();
    let mut lines = Vec::new();
    for row in all {
        let state_name = row["state"].as_str().unwrap_or("pending");
        let is_up = state_name == "up";
        let troubled = matches!(state_name, "down" | "failing" | "recovering");
        if (only == "down" && !troubled) || (only == "up" && !is_up) {
            continue;
        }
        let host = row["host"].as_str().unwrap_or("");
        let check = match (row["probe"]["kind"].as_str(), row["probe"]["port"].as_u64()) {
            (Some("tcp"), Some(port)) => format!("tcp:{port}"),
            _ => "icmp".into(),
        };
        let description = row["description"].as_str().unwrap_or("");
        let status = row["last_status_label"].as_str().unwrap_or("no result yet");
        let who = format!("{} {check}{}", shown(host, row["ip"].as_str()), if description.is_empty() { String::new() } else { format!(" \"{description}\"") });
        lines.push(if is_up {
            format!("- {who}: up, last {}, loss {}%", us(row, "last_rtt_us").map(|v| format!("{v} ms")).unwrap_or_else(|| "no time".into()), row["failed_pct"].as_f64().map(|v| (v * 10.0).round() / 10.0).unwrap_or(0.0))
        } else if troubled {
            format!("- {who}: {state_name}, {status}, {} failed in a row", row["consec_failed"].as_u64().unwrap_or(0))
        } else {
            format!("- {who}: {state_name}")
        });
        rows.push(json!({
            "host": host, "ip": row["ip"], "check": check, "description": description, "group": row["group"], "state": state_name,
            "status": status, "failed_in_a_row": row["consec_failed"], "sent": row["sent"], "loss_pct": row["failed_pct"],
            "last_ms": us(row, "last_rtt_us"), "avg_ms": us(row, "avg_rtt_us"),
        }));
    }
    let mut text = format!(
        "pingtable on {} is {}: {} hosts, {} up, {} down, {} muted.",
        netinfo::local_name(), if running { "pinging" } else { "stopped (counts are from the last run)" },
        counts["total"], counts["up"], counts["down"], counts["muted"]
    );
    const SHOWN: usize = 100;
    if lines.is_empty() {
        text.push_str(match only { "down" => "\nNothing is down.", "up" => "\nNothing is up.", _ => "\nThe table is empty." });
    }
    for line in lines.iter().take(SHOWN) {
        text.push('\n');
        text.push_str(line);
    }
    if lines.len() > SHOWN {
        text.push_str(&format!("\n… and {} more; the structured result has every one.", lines.len() - SHOWN));
    }
    Ok((text, json!({ "computer": netinfo::local_name(), "running": running, "counts": counts, "listed": only, "hosts": rows })))
}

fn tool_table_add_hosts(args: &Value) -> ToolResult {
    let hosts = strings(args, "hosts")?;
    let wanted = ports(args)?;
    let group = args.get("group").and_then(Value::as_str).map(str::trim).filter(|g| !g.is_empty());
    let ping = args.get("ping").and_then(Value::as_bool).unwrap_or(wanted.is_empty());
    let body = json!({ "entries": hosts, "group": group, "ports": wanted, "ping": ping });
    let reply = local_api("POST", "/api/v1/hosts/add", Some(&body))?;
    let added = reply["added"].as_u64().unwrap_or(0);
    let mut text = format!("Added {added} {} to the table{}.", if added == 1 { "row" } else { "rows" }, group.map(|g| format!(", in group {g}")).unwrap_or_default());
    if let Some(why) = reply["save_error"].as_str() {
        text.push_str(&format!(" The settings file could not be written ({why}), so they will be gone after a restart."));
    }
    // Like the app's own Add window: start pinging if the table was stopped.
    let running = local_api("GET", "/api/v1/state", None).ok().and_then(|s| s["running"].as_bool()).unwrap_or(true);
    if !running {
        let started = local_api("POST", "/api/v1/control", Some(&json!({ "action": "start" }))).is_ok();
        text.push_str(if started { " Pinging was stopped, so it has been started." } else { " Pinging is stopped; press START in the app." });
    }
    Ok((text, json!({ "added": added, "group": group, "ports": wanted, "ping": ping })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn ask(request: Value) -> Value {
        let reply = handle_line(&request.to_string()).expect("a request gets an answer");
        assert!(!reply.contains('\n'), "one message, one line");
        serde_json::from_str(&reply).unwrap()
    }

    fn call(name: &str, arguments: Value) -> Value {
        let reply = ask(json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": { "name": name, "arguments": arguments } }));
        assert_eq!(reply["id"], 7);
        reply["result"].clone()
    }

    #[test]
    fn the_handshake_agrees_on_a_version_the_client_knows() {
        let reply = ask(json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "t", "version": "0" } } }));
        assert_eq!(reply["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(reply["result"]["serverInfo"]["name"], "pingtable");
        assert!(reply["result"]["capabilities"]["tools"].is_object());
        assert!(reply["result"]["instructions"].as_str().unwrap().contains("check_ports"));
        // A version this server does not know gets its newest handshake revision.
        let reply = ask(json!({ "jsonrpc": "2.0", "id": 2, "method": "initialize", "params": { "protocolVersion": "1999-01-01" } }));
        assert_eq!(reply["result"]["protocolVersion"], "2025-11-25");
    }

    #[test]
    fn notifications_and_blank_lines_get_no_answer() {
        assert_eq!(handle_line(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string()), None);
        assert_eq!(handle_line(&json!({ "jsonrpc": "2.0", "method": "notifications/cancelled", "params": { "requestId": 3 } }).to_string()), None);
        assert_eq!(handle_line("   "), None);
        assert_eq!(handle_line(&json!({ "jsonrpc": "2.0", "id": 9, "result": {} }).to_string()), None);
    }

    #[test]
    fn bad_input_is_answered_not_fatal() {
        let reply: Value = serde_json::from_str(&handle_line("{not json").unwrap()).unwrap();
        assert_eq!(reply["error"]["code"], -32700);
        assert_eq!(ask(json!({ "jsonrpc": "2.0", "id": 1, "method": "resources/list" }))["error"]["code"], -32601);
        assert_eq!(ask(json!({ "jsonrpc": "2.0", "id": 1 }))["error"]["code"], -32600);
        assert_eq!(ask(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": "format_disk" } }))["error"]["code"], -32602);
        assert_eq!(ask(json!({ "jsonrpc": "2.0", "id": "a", "method": "ping" }))["result"], json!({}));
    }

    #[test]
    fn the_handshake_free_revision_is_served() {
        let meta = json!({ VERSION_KEY: MODERN });
        let reply = ask(json!({ "jsonrpc": "2.0", "id": "d", "method": "server/discover", "params": { "_meta": meta } }));
        assert_eq!(reply["result"]["resultType"], "complete");
        assert!(reply["result"]["supportedVersions"].as_array().unwrap().contains(&json!(MODERN)));
        assert_eq!(reply["result"]["_meta"][SERVER_INFO_KEY]["name"], "pingtable");
        let reply = ask(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": { "_meta": meta } }));
        assert_eq!(reply["result"]["resultType"], "complete");
        assert_eq!(reply["result"]["tools"].as_array().unwrap().len(), 6);
        // A handshake client gets no fields from the newer revision.
        let reply = ask(json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list" }));
        assert!(reply["result"].get("resultType").is_none());
        // An unknown version is refused with the list of known ones.
        let reply = ask(json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/list", "params": { "_meta": { VERSION_KEY: "1900-01-01" } } }));
        assert_eq!(reply["error"]["code"], -32022);
        assert_eq!(reply["error"]["data"]["requested"], "1900-01-01");
        assert!(reply["error"]["data"]["supported"].as_array().unwrap().contains(&json!("2025-11-25")));
    }

    #[test]
    fn every_tool_is_described_for_a_model() {
        for tool in tools() {
            let name = tool["name"].as_str().unwrap();
            assert!(tool["description"].as_str().unwrap().len() > 80, "{name} needs a real description");
            assert_eq!(tool["inputSchema"]["type"], "object", "{name}");
            assert!(tool["annotations"]["readOnlyHint"].is_boolean(), "{name}");
            for (key, property) in tool["inputSchema"]["properties"].as_object().unwrap() {
                assert!(property.get("type").is_some(), "{name}.{key} has a type");
            }
        }
        assert!(tools().iter().all(|t| t["name"] == "table_add_hosts" || t["annotations"]["readOnlyHint"] == true));
    }

    #[test]
    fn ping_reports_each_host() {
        let result = call("ping", json!({ "hosts": ["127.0.0.1"], "count": 2 }));
        assert_eq!(result["isError"], false, "{result}");
        let data = &result["structuredContent"];
        assert_eq!((data["total"].as_u64(), data["answered"].as_u64()), (Some(1), Some(1)), "{data}");
        assert_eq!(data["hosts"][0]["sent"], 2);
        assert_eq!(data["hosts"][0]["check"], "icmp");
        assert!(result["content"][0]["text"].as_str().unwrap().contains("1 of 1 host answered"));
    }

    #[test]
    fn ping_refuses_what_it_should() {
        assert_eq!(call("ping", json!({}))["isError"], true);
        assert_eq!(call("ping", json!({ "hosts": ["not a host!"] }))["isError"], true);
        assert_eq!(call("ping", json!({ "hosts": ["bad_host!"] }))["isError"], true);
        assert_eq!(call("ping", json!({ "hosts": ["127.0.0.1 192.0.2.1"] }))["isError"], true);
        assert_eq!(call("ping", json!({ "hosts": ["# note"] }))["isError"], true);
        assert_eq!(call("ping", json!({ "hosts": ["127.0.0.1 timeout=300ms"] }))["isError"], false);
        assert_eq!(call("ping", json!({ "hosts": ["127.0.0.1"], "count": 99 }))["isError"], true);
        let big = call("ping", json!({ "hosts": ["10.0.0.0/20"] }));
        assert_eq!(big["isError"], true);
        assert!(big["content"][0]["text"].as_str().unwrap().contains("at most 256"));
    }

    #[test]
    fn check_ports_tells_open_from_closed() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let open = listener.local_addr().unwrap().port();
        let closed = {
            let gone = TcpListener::bind("127.0.0.1:0").unwrap();
            gone.local_addr().unwrap().port()
        };
        let result = call("check_ports", json!({ "host": "127.0.0.1", "ports": [open, closed] }));
        assert_eq!(result["isError"], false, "{result}");
        let data = &result["structuredContent"];
        assert_eq!(data["open"], 1);
        assert_eq!((data["ports"][0]["port"].as_u64(), data["ports"][0]["state"].as_str()), (Some(open as u64), Some("open")));
        assert_eq!(data["ports"][1]["state"], "closed");
        assert!(result["content"][0]["text"].as_str().unwrap().contains("1 of 2 ports open"));
        // One host only, and ports must be ports.
        assert_eq!(call("check_ports", json!({ "host": "10.0.0.0/30" }))["isError"], true);
        assert_eq!(call("check_ports", json!({ "host": "127.0.0.1", "ports": [70000] }))["isError"], true);
        assert_eq!(call("check_ports", json!({ "host": "127.0.0.1", "ports": "22, 80" }))["isError"], false);
    }

    #[test]
    fn network_info_names_this_computer() {
        let result = call("network_info", json!({}));
        assert_eq!(result["isError"], false);
        assert!(result["structuredContent"]["computer"].as_str().is_some_and(|name| !name.is_empty()));
        assert!(result["structuredContent"]["interfaces"].is_array());
    }

    #[test]
    fn a_batch_is_answered_as_a_batch() {
        let line = json!([{ "jsonrpc": "2.0", "id": 1, "method": "ping" }, { "jsonrpc": "2.0", "method": "notifications/initialized" }, { "jsonrpc": "2.0", "id": 2, "method": "ping" }]).to_string();
        let reply: Value = serde_json::from_str(&handle_line(&line).unwrap()).unwrap();
        assert_eq!(reply.as_array().unwrap().len(), 2);
    }
}
