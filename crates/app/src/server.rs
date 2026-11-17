//! Local HTTP server: the web UI and the JSON API under `/api/v1`.
//!
//! It listens on 127.0.0.1 only. Because the API can change settings that run
//! commands, three checks guard every API call:
//!
//! * the `Host` header must name this machine's loopback address (stops DNS
//!   rebinding),
//! * an `Origin` header, when present, must be this server (stops other web
//!   pages from calling in),
//! * the caller must present the token from the config file.

use crate::config::{self, Config};
use pingtable_core::parse::{parse_host_list, ParseOptions};
use pingtable_core::probe::ProbeParams;
use pingtable_core::{export, hostlist, netinfo, portcheck, trace, Engine, Settings};
use serde::Deserialize;
use serde_json::json;
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tiny_http::{Header, Method, Request, Response, Server};

const INDEX_HTML: &str = include_str!("../../../ui/index.html");
const APP_CSS: &str = include_str!("../../../ui/app.css");
const APP_JS: &str = include_str!("../../../ui/app.js");
const THEME_JS: &str = include_str!("../../../ui/theme.js");
const MANIFEST: &str = include_str!("../../../ui/manifest.webmanifest");
const ICON_SVG: &str = include_str!("../../../ui/icon.svg");

const WORKERS: usize = 16;
const MAX_BODY: u64 = 8 * 1024 * 1024;
const MAX_WAIT_MS: u64 = 25_000;
/// After the first change, wait a little so a burst arrives as one reply:
/// this long for a small table, growing with the number of hosts.
const COALESCE_MIN_MS: u64 = 120;
const COALESCE_MAX_MS: u64 = 800;

struct Ctx {
    engine: Engine,
    cfg: Mutex<Config>,
    path: PathBuf,
    port: u16,
    token: String,
    /// Serve UI files from this folder instead of the built-in copies.
    ui_dir: Option<PathBuf>,
}

/// Why the server could not start.
#[derive(Debug)]
pub enum StartError {
    /// Something already listens on the port. It may be another pingtable.
    PortInUse,
    Other(String),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartError::PortInUse => write!(f, "the port is already in use by another program; pick another with --port"),
            StartError::Other(why) => write!(f, "{why}"),
        }
    }
}

/// A started server. Dropping it does not stop it; the threads end with the process.
pub struct Running {
    /// Link to the interface, including the access token.
    pub url: String,
    handles: Vec<std::thread::JoinHandle<()>>,
}

impl Running {
    /// Block for as long as the server runs.
    pub fn join(self) {
        for handle in self.handles {
            let _ = handle.join();
        }
    }
}

pub fn link(port: u16, token: &str) -> String {
    format!("http://127.0.0.1:{port}/?t={token}")
}

/// Start listening on 127.0.0.1 and answer requests on background threads.
pub fn start(engine: Engine, cfg: Config, path: PathBuf) -> Result<Running, StartError> {
    let port = cfg.port;
    let server = Server::http(("127.0.0.1", port)).map_err(|why| {
        let in_use = why
            .downcast_ref::<std::io::Error>()
            .is_some_and(|e| e.kind() == std::io::ErrorKind::AddrInUse);
        if in_use {
            StartError::PortInUse
        } else {
            StartError::Other(format!("cannot listen on 127.0.0.1:{port}: {why}"))
        }
    })?;
    let url = link(port, &cfg.token);

    let ctx = Arc::new(Ctx {
        engine,
        token: cfg.token.clone(),
        cfg: Mutex::new(cfg),
        path,
        port,
        ui_dir: std::env::var_os("PINGTABLE_UI_DIR").map(PathBuf::from),
    });
    let server = Arc::new(server);
    let mut handles = Vec::new();
    for _ in 0..WORKERS {
        let server = server.clone();
        let ctx = ctx.clone();
        handles.push(std::thread::spawn(move || {
            while let Ok(request) = server.recv() {
                // A fault while answering one request ends that request only.
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handle(&ctx, request)));
            }
        }));
    }
    Ok(Running { url, handles })
}

/// True when a pingtable that accepts `token` already answers on `port`.
pub fn is_pingtable(port: u16, token: &str) -> bool {
    use std::io::Write;
    use std::net::{SocketAddr, TcpStream};
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_secs(2)) else { return false };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let request = format!(
        "GET /api/v1/info HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Pingtable-Token: {token}\r\nConnection: close\r\n\r\n"
    );
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut reply = String::new();
    let _ = stream.take(4096).read_to_string(&mut reply);
    reply.starts_with("HTTP/1.1 200") && reply.contains("\"version\"")
}

/// Open a link in the default browser.
pub fn open_browser(url: &str) {
    use std::process::{Command, Stdio};
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", "", url]);
        c
    } else if is_wsl() {
        let mut c = Command::new("explorer.exe");
        c.arg(url);
        c
    } else if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        c.arg(url);
        c
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(url);
        c
    };
    let _ = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
}

/// Replace settings and host list, keep mutes for hosts that remain, and save
/// the config file. Returns the parser's warnings and a save error, if any.
fn apply_and_save(ctx: &Ctx, settings: Settings, hosts_text: &str, keep_stats: bool, autostart: Option<bool>) -> (Vec<String>, Option<String>) {
    let engine = &ctx.engine;
    let warnings = engine.apply_config(settings, hosts_text, keep_stats);
    let (settings, hosts_text) = engine.config();
    let mut cfg = ctx.cfg.lock().unwrap_or_else(|p| p.into_inner());
    engine.mute_keys(&cfg.muted);
    cfg.muted = engine.muted_keys();
    cfg.settings = settings;
    cfg.hosts_text = hosts_text;
    if let Some(autostart) = autostart {
        cfg.autostart = autostart;
    }
    let saved = config::save(&ctx.path, &cfg).err().map(|why| why.to_string());
    (warnings, saved)
}

/// Add hosts to the list from the table, without opening the list editor.
fn add_hosts(ctx: &Ctx, body: &AddBody) -> Result<serde_json::Value, String> {
    let entries: Vec<String> = body.entries.iter().map(|e| e.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|e| !e.is_empty()).collect();
    if entries.is_empty() {
        return Err("Type a host name or an address.".into());
    }
    let ports = chosen_ports(&body.ports, None, &body.ports_text)?;
    let entries: Vec<String> = entries.iter().flat_map(|e| portcheck::entries_for_ports(e, &ports, body.ping)).collect();
    let (mut settings, mut text) = ctx.engine.config();
    // A description needs the one-host-per-line form; switch the list to it when one is given.
    let has_description = entries.iter().any(|e| e.split(' ').skip(1).any(|w| !w.contains('=')));
    if !settings.description_format && has_description {
        text = hostlist::to_described(&text);
        settings.description_format = true;
    }
    let options = ParseOptions {
        description_format: settings.description_format,
        cidr_skip_first: settings.cidr_skip_first,
        cidr_skip_last: settings.cidr_skip_last,
    };
    let mut added = 0;
    for entry in &entries {
        if entry.starts_with('#') || entry.to_ascii_lowercase().starts_with("group:") {
            return Err(format!("\"{entry}\" is not a host. Pick the group below instead."));
        }
        let report = parse_host_list(entry, &options);
        if let Some(warning) = report.warnings.first() {
            return Err(warning.trim_start_matches("Line 1: ").to_string());
        }
        if report.hosts.is_empty() {
            return Err(format!("\"{entry}\" is not a host name or an address."));
        }
        added += report.hosts.len();
    }
    let group = body.group.as_deref().map(str::trim).filter(|g| !g.is_empty());
    let new_text = hostlist::add_entries(&text, group, &entries);
    let (_, saved) = apply_and_save(ctx, settings, &new_text, true, None);
    Ok(json!({ "ok": true, "added": added, "group": group, "save_error": saved }))
}

/// The ports a request asks for, from every place it can name them, without
/// repeats and within the limit of one check.
fn chosen_ports(listed: &[u16], named: Option<u16>, typed: &str) -> Result<Vec<u16>, String> {
    let mut ports: Vec<u16> = Vec::new();
    for port in named.into_iter().chain(listed.iter().copied()).chain(portcheck::parse_ports(typed)?) {
        if port == 0 {
            return Err("0 is not a port number (1 to 65535).".into());
        }
        if !ports.contains(&port) {
            ports.push(port);
        }
    }
    if ports.len() > portcheck::MAX_PORTS {
        return Err(format!("That is more than {} ports. Choose fewer at a time.", portcheck::MAX_PORTS));
    }
    Ok(ports)
}

/// Try a few TCP ports of one host, once, and reply with what answered.
/// Nothing is added to the table and no counter changes.
fn port_check(ctx: &Ctx, request: Request, body: &PortCheckBody) {
    let (host, addr, named) = match portcheck::split_target(&body.host) {
        Ok(target) => target,
        Err(why) => return reply_error(request, 400, &why),
    };
    let ports = match chosen_ports(&body.ports, named, &body.ports_text) {
        Ok(ports) if ports.is_empty() => return reply_error(request, 400, "Choose at least one port."),
        Ok(ports) => ports,
        Err(why) => return reply_error(request, 400, &why),
    };
    let (settings, _) = ctx.engine.config();
    let Some(target) = addr.or_else(|| netinfo::resolve(&host, settings.prefer_ipv6)) else {
        return reply_error(request, 409, &format!("The name {host} does not resolve, so there is nothing to check."));
    };
    // Long enough for Windows to report a refusal, whatever the ping timeout is.
    let timeout_ms = settings.timeout_ms.clamp(portcheck::MIN_TIMEOUT_MS, 10_000);
    let params = ProbeParams {
        timeout: Duration::from_millis(timeout_ms as u64),
        source_v4: settings.source_v4,
        source_v6: settings.source_v6,
        ..ProbeParams::default()
    };
    let results = portcheck::check(target, &ports, &params);
    reply_json(
        request,
        200,
        &json!({
            "schema_version": 1,
            "host": host,
            "target": target,
            "from": netinfo::local_name(),
            "source": netinfo::source_for(target),
            "timeout_ms": timeout_ms,
            "checked_at_ms": pingtable_core::now_ms(),
            "open": results.iter().filter(|r| r.state == portcheck::PortState::Open).count(),
            "results": results,
        }),
    );
}

/// Trace the route to one host and reply with the hops.
fn trace_host(ctx: &Ctx, request: Request, id: u32) {
    let Some(detail) = ctx.engine.detail(id) else {
        return reply_error(request, 404, "No such host.");
    };
    let (settings, _) = ctx.engine.config();
    let target = detail
        .row
        .ip
        .as_deref()
        .and_then(|ip| ip.parse().ok())
        .or_else(|| netinfo::resolve(&detail.row.host, settings.prefer_ipv6));
    let Some(target) = target else {
        return reply_error(request, 409, "The name does not resolve, so there is nothing to trace.");
    };
    let timeout = Duration::from_millis(settings.timeout_ms.clamp(500, 2000) as u64);
    let hops = trace::traceroute(target, trace::MAX_HOPS, timeout, true);
    reply_json(
        request,
        200,
        &json!({
            "schema_version": 1,
            "host": detail.row.host,
            "target": target,
            "from": netinfo::local_name(),
            "source": netinfo::source_for(target),
            "reached": hops.last().is_some_and(|h| h.reached),
            "hops": hops,
        }),
    );
}

fn is_wsl() -> bool {
    std::fs::read_to_string("/proc/version").map(|v| v.to_ascii_lowercase().contains("microsoft")).unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Request plumbing
// ---------------------------------------------------------------------------

fn header<'a>(request: &'a Request, name: &'static str) -> Option<&'a str> {
    request.headers().iter().find(|h| h.field.equiv(name)).map(|h| h.value.as_str())
}

fn query_param<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query.split('&').filter_map(|pair| pair.split_once('=')).find(|(k, _)| *k == name).map(|(_, v)| v)
}

fn loopback_names(port: u16) -> [String; 3] {
    [format!("127.0.0.1:{port}"), format!("localhost:{port}"), format!("[::1]:{port}")]
}

fn host_allowed(request: &Request, port: u16) -> bool {
    match header(request, "Host") {
        Some(host) => loopback_names(port).iter().any(|ok| host.eq_ignore_ascii_case(ok)),
        None => false,
    }
}

fn origin_allowed(request: &Request, port: u16) -> bool {
    match header(request, "Origin") {
        Some(origin) => loopback_names(port).iter().any(|ok| origin.eq_ignore_ascii_case(&format!("http://{ok}"))),
        None => true,
    }
}

fn same_secret(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn reply(request: Request, status: u16, content_type: &str, body: Vec<u8>, extra: &[(&str, &str)]) {
    let mut response = Response::from_data(body).with_status_code(status);
    let mut add = |name: &str, value: &str| {
        if let Ok(h) = Header::from_bytes(name.as_bytes(), value.as_bytes()) {
            response.add_header(h);
        }
    };
    add("Content-Type", content_type);
    add("Cache-Control", "no-store");
    add("X-Content-Type-Options", "nosniff");
    add("Referrer-Policy", "no-referrer");
    for (name, value) in extra {
        add(name, value);
    }
    let _ = request.respond(response);
}

fn reply_json(request: Request, status: u16, value: &impl serde::Serialize) {
    let body = serde_json::to_vec(value).unwrap_or_else(|_| b"{}".to_vec());
    reply(request, status, "application/json; charset=utf-8", body, &[]);
}

fn reply_error(request: Request, status: u16, message: &str) {
    reply_json(request, status, &json!({ "error": message }));
}

fn read_json<T: for<'de> Deserialize<'de>>(request: &mut Request) -> Result<T, String> {
    let mut body = String::new();
    request
        .as_reader()
        .take(MAX_BODY)
        .read_to_string(&mut body)
        .map_err(|why| format!("could not read the request body: {why}"))?;
    serde_json::from_str(&body).map_err(|why| format!("request body is not valid: {why}"))
}

// ---------------------------------------------------------------------------
// Routing
// ---------------------------------------------------------------------------

fn handle(ctx: &Ctx, request: Request) {
    let url = request.url().to_string();
    let (path, query) = url.split_once('?').unwrap_or((url.as_str(), ""));

    if !host_allowed(&request, ctx.port) {
        return reply_error(request, 403, "This server only answers on its loopback address.");
    }
    if !path.starts_with("/api/") {
        return serve_static(ctx, request, path);
    }
    if !origin_allowed(&request, ctx.port) {
        return reply_error(request, 403, "Cross-origin requests are not allowed.");
    }
    let presented = header(&request, "X-Pingtable-Token").or_else(|| query_param(query, "t")).unwrap_or("");
    if !same_secret(presented, &ctx.token) {
        return reply_error(request, 401, "Missing or wrong token. Open the link pingtable printed when it started.");
    }
    api(ctx, request, path, query);
}

fn serve_static(ctx: &Ctx, request: Request, path: &str) {
    if *request.method() != Method::Get {
        return reply_error(request, 405, "Method not allowed.");
    }
    let (file, content_type, built_in) = match path {
        "/" | "/index.html" => ("index.html", "text/html; charset=utf-8", INDEX_HTML),
        "/app.css" => ("app.css", "text/css; charset=utf-8", APP_CSS),
        "/app.js" => ("app.js", "text/javascript; charset=utf-8", APP_JS),
        "/theme.js" => ("theme.js", "text/javascript; charset=utf-8", THEME_JS),
        "/manifest.webmanifest" => ("manifest.webmanifest", "application/manifest+json", MANIFEST),
        "/icon.svg" | "/favicon.ico" => ("icon.svg", "image/svg+xml", ICON_SVG),
        _ => return reply_error(request, 404, "Not found."),
    };
    let body = ctx
        .ui_dir
        .as_ref()
        .and_then(|dir| std::fs::read(dir.join(file)).ok())
        .unwrap_or_else(|| built_in.as_bytes().to_vec());
    let csp = "default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; \
               object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'";
    reply(request, 200, content_type, body, &[("Content-Security-Policy", csp)]);
}

#[derive(Deserialize)]
struct ConfigBody {
    settings: Settings,
    hosts_text: String,
    #[serde(default)]
    keep_stats: bool,
    autostart: Option<bool>,
}

#[derive(Deserialize)]
struct ControlBody {
    action: String,
}

/// Body of `POST /api/v1/hosts/{id}`: turn the host on or off, or act on it.
#[derive(Deserialize)]
struct HostBody {
    enabled: Option<bool>,
    /// `ping_now` or `reset`.
    action: Option<String>,
}

/// Body of `POST /api/v1/hosts`: one action on several hosts.
#[derive(Deserialize)]
struct BulkBody {
    #[serde(default)]
    ids: Vec<u32>,
    /// `mute`, `unmute`, `solo`, `unmute_all`, `ping_now` or `reset`.
    action: String,
}

/// Body of `POST /api/v1/hosts/add`.
#[derive(Deserialize)]
struct AddBody {
    /// One host per entry, optionally followed by a description and options.
    entries: Vec<String>,
    /// Existing or new group name; none adds the hosts ungrouped.
    group: Option<String>,
    /// TCP ports to watch on every entry that names no port of its own: one
    /// row for each. Empty adds the entries as typed.
    #[serde(default)]
    ports: Vec<u16>,
    /// More ports as a person types them: `8080 9000-9005`.
    #[serde(default)]
    ports_text: String,
    /// With ports: keep the plain ping row as well.
    #[serde(default)]
    ping: bool,
}

/// Body of `POST /api/v1/portcheck`.
#[derive(Deserialize)]
struct PortCheckBody {
    /// One host name or address. A port or scheme in it is checked too.
    host: String,
    #[serde(default)]
    ports: Vec<u16>,
    /// More ports as a person types them: `22, 80 443`, `8000-8010`.
    #[serde(default)]
    ports_text: String,
}

#[derive(Deserialize)]
struct ImportBody {
    text: String,
}

/// Body of `POST /api/v1/measure`.
#[derive(Deserialize)]
struct MeasureBody {
    ids: Vec<u32>,
    duration_secs: u32,
    per_second: u32,
}

#[derive(Deserialize)]
struct ParseBody {
    text: String,
    #[serde(flatten)]
    options: ParseOptions,
}

fn api(ctx: &Ctx, mut request: Request, path: &str, query: &str) {
    let method = request.method().clone();
    let engine = &ctx.engine;

    match (&method, path) {
        (Method::Get, "/api/v1/state") => {
            let since: u64 = query_param(query, "since").and_then(|v| v.parse().ok()).unwrap_or(0);
            let wait_ms: u64 = query_param(query, "wait_ms").and_then(|v| v.parse().ok()).unwrap_or(0).min(MAX_WAIT_MS);
            if wait_ms > 0 && since > 0 && since <= engine.seq() {
                let seq = engine.wait_change(since, Duration::from_millis(wait_ms));
                if seq > since {
                    // 200 hosts at one ping a second would otherwise mean a reply per ping.
                    let hosts = engine.host_count() as u64;
                    std::thread::sleep(Duration::from_millis((COALESCE_MIN_MS + hosts * 2).min(COALESCE_MAX_MS)));
                }
            }
            reply_json(request, 200, &engine.snapshot(since));
        }

        (Method::Get, "/api/v1/config") => {
            let (settings, hosts_text) = engine.config();
            let autostart = ctx.cfg.lock().map(|c| c.autostart).unwrap_or(true);
            reply_json(
                request,
                200,
                &json!({
                    "schema_version": 1,
                    "settings": settings,
                    "hosts_text": hosts_text,
                    "groups": hostlist::group_names(&hosts_text),
                    "autostart": autostart,
                    "config_path": ctx.path.display().to_string(),
                }),
            );
        }

        (Method::Put, "/api/v1/config") => {
            let body: ConfigBody = match read_json(&mut request) {
                Ok(body) => body,
                Err(why) => return reply_error(request, 400, &why),
            };
            let (warnings, saved) = apply_and_save(ctx, body.settings, &body.hosts_text, body.keep_stats, body.autostart);
            let hosts = engine.snapshot(0).counts.total;
            reply_json(
                request,
                200,
                &json!({
                    "ok": true,
                    "hosts": hosts,
                    "warnings": warnings,
                    "save_error": saved,
                }),
            );
        }

        (Method::Post, "/api/v1/hosts/add") => {
            let body: AddBody = match read_json(&mut request) {
                Ok(body) => body,
                Err(why) => return reply_error(request, 400, &why),
            };
            match add_hosts(ctx, &body) {
                Ok(reply) => reply_json(request, 200, &reply),
                Err(why) => reply_error(request, 400, &why),
            }
        }

        (Method::Post, "/api/v1/portcheck") => {
            let body: PortCheckBody = match read_json(&mut request) {
                Ok(body) => body,
                Err(why) => return reply_error(request, 400, &why),
            };
            port_check(ctx, request, &body);
        }

        (Method::Post, "/api/v1/import") => {
            let body: ImportBody = match read_json(&mut request) {
                Ok(body) => body,
                Err(why) => return reply_error(request, 400, &why),
            };
            let import = hostlist::import_text(&body.text);
            reply_json(
                request,
                200,
                &json!({
                    "format": import.format,
                    "text": import.lines.join("\n"),
                    "hosts": import.hosts,
                    "skipped": import.skipped,
                }),
            );
        }

        (Method::Post, "/api/v1/control") => {
            let body: ControlBody = match read_json(&mut request) {
                Ok(body) => body,
                Err(why) => return reply_error(request, 400, &why),
            };
            match body.action.as_str() {
                "start" => engine.start(),
                "stop" => engine.stop(),
                "ping_now" => engine.ping_now(),
                "reset" => engine.reset(),
                other => return reply_error(request, 400, &format!("Unknown action \"{other}\".")),
            }
            reply_json(request, 200, &json!({ "ok": true }));
        }

        (Method::Post, "/api/v1/parse") => {
            let body: ParseBody = match read_json(&mut request) {
                Ok(body) => body,
                Err(why) => return reply_error(request, 400, &why),
            };
            let report = parse_host_list(&body.text, &body.options);
            let groups: std::collections::BTreeSet<&str> = report.hosts.iter().filter_map(|h| h.group.as_deref()).collect();
            reply_json(
                request,
                200,
                &json!({
                    "count": report.hosts.len(),
                    "groups": groups.len(),
                    "tcp": report.hosts.iter().filter(|h| h.probe != pingtable_core::Probe::Icmp).count(),
                    "warnings": report.warnings,
                }),
            );
        }

        (Method::Get, "/api/v1/export") => {
            let snapshot = engine.snapshot(0);
            let stamp = export::rfc3339(snapshot.now_ms).replace([':', '-'], "");
            let stamp = &stamp[..15.min(stamp.len())];
            match query_param(query, "format").unwrap_or("json") {
                "csv" => reply(
                    request,
                    200,
                    "text/csv; charset=utf-8",
                    export::to_csv(&snapshot.hosts).into_bytes(),
                    &[("Content-Disposition", &format!("attachment; filename=\"pingtable-{stamp}.csv\""))],
                ),
                "json" => reply(
                    request,
                    200,
                    "application/json; charset=utf-8",
                    export::to_json(&snapshot).into_bytes(),
                    &[("Content-Disposition", &format!("attachment; filename=\"pingtable-{stamp}.json\""))],
                ),
                other => reply_error(request, 400, &format!("Unknown format \"{other}\". Use json or csv.")),
            }
        }

        (Method::Post, "/api/v1/hosts") => {
            let body: BulkBody = match read_json(&mut request) {
                Ok(body) => body,
                Err(why) => return reply_error(request, 400, &why),
            };
            let mut mutes_changed = true;
            match body.action.as_str() {
                "mute" => {
                    engine.set_enabled_many(&body.ids, false);
                }
                "unmute" => {
                    engine.set_enabled_many(&body.ids, true);
                }
                "solo" => {
                    engine.solo(&body.ids);
                }
                "unmute_all" => engine.unmute_all(),
                "remove" => {
                    // Delete the list lines of hosts that have a line to themselves.
                    let lines = engine.removable_lines(&body.ids);
                    let shared = body.ids.len().saturating_sub(lines.len());
                    if !lines.is_empty() {
                        let (settings, text) = engine.config();
                        let (_, saved) = apply_and_save(ctx, settings, &hostlist::remove_lines(&text, &lines), true, None);
                        if let Some(why) = saved {
                            return reply_error(request, 500, &format!("Removed, but the settings file could not be written: {why}"));
                        }
                    }
                    return reply_json(request, 200, &json!({ "ok": true, "removed": lines.len(), "shared": shared }));
                }
                "ping_now" => {
                    mutes_changed = false;
                    for id in &body.ids {
                        engine.ping_host(*id);
                    }
                }
                "reset" => {
                    mutes_changed = false;
                    for id in &body.ids {
                        engine.reset_host(*id);
                    }
                }
                other => return reply_error(request, 400, &format!("Unknown action \"{other}\".")),
            }
            if mutes_changed {
                let mut cfg = ctx.cfg.lock().unwrap_or_else(|p| p.into_inner());
                cfg.muted = engine.muted_keys();
                let _ = config::save(&ctx.path, &cfg);
            }
            reply_json(request, 200, &json!({ "ok": true }));
        }

        (Method::Post, "/api/v1/measure") => {
            let body: MeasureBody = match read_json(&mut request) {
                Ok(body) => body,
                Err(why) => return reply_error(request, 400, &why),
            };
            match engine.measure_start(&body.ids, body.duration_secs, body.per_second) {
                Some(id) => reply_json(request, 200, &json!({ "ok": true, "id": id })),
                None => reply_error(request, 409, "None of those hosts can be measured. Unmute them first."),
            }
        }

        (Method::Post, "/api/v1/measure/stop") => {
            engine.measure_stop();
            reply_json(request, 200, &json!({ "ok": true }));
        }

        (Method::Get, "/api/v1/measure") => reply_json(request, 200, &json!({ "measurement": engine.measure_report() })),

        (Method::Get, "/api/v1/source") => {
            let default_v4 = netinfo::source_for("1.1.1.1".parse().expect("address"));
            let default_v6 = netinfo::source_for("2606:4700:4700::1111".parse().expect("address"));
            reply_json(
                request,
                200,
                &json!({
                    "name": netinfo::local_name(),
                    "interfaces": netinfo::interfaces(),
                    "default_v4": default_v4,
                    "default_v6": default_v6,
                }),
            );
        }

        (Method::Get, "/api/v1/defaults") => reply_json(
            request,
            200,
            &json!({
                "schema_version": 1,
                "settings": config::default_settings(),
                "hosts_text": config::default_hosts_text(ctx.port),
            }),
        ),

        (Method::Get, "/api/v1/info") => reply_json(
            request,
            200,
            &json!({
                "version": env!("CARGO_PKG_VERSION"),
                "machine": netinfo::local_name(),
                "os": std::env::consts::OS,
                "arch": std::env::consts::ARCH,
                "rustc": env!("PINGTABLE_RUSTC"),
                "target": env!("PINGTABLE_TARGET"),
                "icmp": if cfg!(windows) { "the Windows ICMP API" } else { "the system ping command" },
                "config_path": ctx.path.display().to_string(),
                "log_path": crate::log_path().display().to_string(),
                "credits": crate::CREDITS,
                "website": crate::WEBSITE,
                "source": crate::SOURCE,
                "license": "Apache License 2.0",
                "libraries": ["tiny_http", "serde", "socket2", "dns-lookup", "windows-sys", "tao", "wry"],
            }),
        ),

        (_, p) if p.starts_with("/api/v1/hosts/") => {
            let rest = &p["/api/v1/hosts/".len()..];
            if let Some(id) = rest.strip_suffix("/trace").and_then(|id| id.parse::<u32>().ok()) {
                if method != Method::Post {
                    return reply_error(request, 405, "Method not allowed.");
                }
                return trace_host(ctx, request, id);
            }
            let Ok(id) = rest.parse::<u32>() else {
                return reply_error(request, 404, "No such host.");
            };
            match method {
                Method::Get => match engine.detail(id) {
                    Some(detail) => reply_json(request, 200, &detail),
                    None => reply_error(request, 404, "No such host."),
                },
                Method::Post => {
                    let body: HostBody = match read_json(&mut request) {
                        Ok(body) => body,
                        Err(why) => return reply_error(request, 400, &why),
                    };
                    if engine.detail(id).is_none() {
                        return reply_error(request, 404, "No such host.");
                    }
                    if let Some(enabled) = body.enabled {
                        engine.set_enabled(id, enabled);
                        // Remember who is muted for the next start.
                        let mut cfg = ctx.cfg.lock().unwrap_or_else(|p| p.into_inner());
                        cfg.muted = engine.muted_keys();
                        let _ = config::save(&ctx.path, &cfg);
                    }
                    match body.action.as_deref() {
                        None => {}
                        Some("ping_now") => {
                            if !engine.ping_host(id) {
                                return reply_error(request, 409, "This host is muted.");
                            }
                        }
                        Some("reset") => {
                            engine.reset_host(id);
                        }
                        Some(other) => return reply_error(request, 400, &format!("Unknown action \"{other}\".")),
                    }
                    reply_json(request, 200, &json!({ "ok": true }));
                }
                _ => reply_error(request, 405, "Method not allowed."),
            }
        }

        _ => reply_error(request, 404, "Not found."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_are_gathered_from_every_place_once() {
        assert_eq!(chosen_ports(&[443, 22], Some(8443), "22 9000-9001").unwrap(), [8443, 443, 22, 9000, 9001]);
        assert!(chosen_ports(&[], None, "").unwrap().is_empty());
        assert!(chosen_ports(&[0], None, "").unwrap_err().contains("not a port number"));
        assert!(chosen_ports(&[], None, "https").unwrap_err().contains("not a port number"));
        let many: Vec<u16> = (1..=65).collect();
        assert!(chosen_ports(&many, None, "").unwrap_err().contains("more than 64"));
    }
}
