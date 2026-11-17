//! The command line: `pingtable [serve]`, `once`, `ports`, `api` and `mcp`.

use crate::{config, mcp, server};
use pingtable_core::probe::ProbeParams;
use pingtable_core::{export, netinfo, portcheck, Engine, Settings};
use serde_json::json;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

const HELP: &str = "\
pingtable - ping many hosts and watch them in one table

This is the command-line program (pingtable-cli.exe on Windows). The app in
its own window is pingtable.exe; both use the same settings and host list.

USAGE
  pingtable [serve] [OPTIONS]          run the local web UI (default)
  pingtable once [OPTIONS] [HOST...]   ping every host once and print the result
  pingtable ports [OPTIONS] HOST [PORT...]
                                       check which TCP ports answer on one host
  pingtable api [--format json]        show how to call this computer's pingtable
  pingtable mcp                        serve the checks to an AI agent (MCP, stdio)

OPTIONS
  --config FILE     use this config file (mcp, api: the running app to talk to)
  --port N          port for the web UI (default 7464)
  --no-open         do not open the browser
  --hosts FILE      once: read the host list from a file
  --format F        once: table (default), json or csv; ports: table or json
  --timeout MS      once, ports: per-probe timeout in milliseconds
                    (ports waits 3000 unless told otherwise)
  --described       once: host list is one \"address description\" per line
  -h, --help        show this help
  -V, --version     show the version

HOSTS
  example.com  192.168.1.10  2001:db8::1          ICMP ping
  example.com:443  [2001:db8::1]:22  https://x.y  TCP connect: is the port listening?
  10.0.0.1-10.0.0.20  10.0.0.0/24                 IPv4 range, CIDR block
  host every=5s timeout=300ms tag=lan             per-host options

PORTS
  pingtable ports example.com               the usual ports (22, 80, 443, 3389 ...)
  pingtable ports example.com 22 443        these ports
  pingtable ports 192.168.1.10 8000-8010    a short range (64 ports at most)
  One TCP connection is opened to each port and closed again; nothing is sent.
  open: something is listening.  closed: the host answered and refused.
  no answer: a firewall dropped it, or the host is off.
  Windows takes about two seconds to report a refusal, so with a timeout
  under 2500 a closed port can show as no answer.

AI AGENTS AND SCRIPTS
  pingtable mcp is a Model Context Protocol server on standard input and
  output. Point an agent at it with the command: pingtable-cli mcp
  It then has the tools ping, check_ports, trace_route, network_info,
  table_status and table_add_hosts. A script can use once and ports with
  --format json, or the local HTTP API: pingtable api prints its address
  and access token.
  Guide: https://github.com/lorincz-llc/pingtable/blob/main/docs/ai-agents.md

EXIT STATUS
  once, ports: 0 when every host is up or every port is open, 1 when not,
  2 when the command could not be understood.

pingtable is provided as is, without warranty of any kind, and you use it at
your own risk. Use it only on networks and hosts you are allowed to test.
Apache License 2.0. Full terms: LICENSE and DISCLAIMER beside the program, or
https://github.com/lorincz-llc/pingtable
";

#[derive(Default)]
struct Args {
    command: Option<String>,
    config: Option<PathBuf>,
    port: Option<u16>,
    no_open: bool,
    hosts_file: Option<PathBuf>,
    format: Option<String>,
    timeout_ms: Option<u32>,
    described: bool,
    hosts: Vec<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("pingtable {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "--config" => args.config = Some(PathBuf::from(value("--config")?)),
            "--port" => args.port = Some(value("--port")?.parse().map_err(|_| "--port needs a number".to_string())?),
            "--no-open" => args.no_open = true,
            "--hosts" => args.hosts_file = Some(PathBuf::from(value("--hosts")?)),
            "--format" => args.format = Some(value("--format")?),
            "--timeout" => {
                args.timeout_ms = Some(value("--timeout")?.parse().map_err(|_| "--timeout needs a number".to_string())?)
            }
            "--described" => args.described = true,
            "serve" | "once" | "ports" | "api" | "mcp" if args.command.is_none() && args.hosts.is_empty() => args.command = Some(arg),
            other if other.starts_with("--") => return Err(format!("unknown option {other}")),
            _ => args.hosts.push(arg),
        }
    }
    Ok(args)
}

pub fn main() -> ExitCode {
    crate::install_fault_log();
    let args = match parse_args() {
        Ok(args) => args,
        Err(why) => {
            eprintln!("pingtable: {why}\nTry: pingtable --help");
            return ExitCode::from(2);
        }
    };
    match args.command.as_deref() {
        Some("once") => once(args),
        Some("ports") => ports(args),
        Some("api") => api(args),
        Some("mcp") => mcp::serve_stdio(args.config),
        _ => serve(args),
    }
}

fn serve(args: Args) -> ExitCode {
    let (path, mut cfg) = config::load(args.config.as_deref());
    if let Some(port) = args.port {
        cfg.set_port(port);
    }
    let engine = Engine::new(cfg.settings.clone(), &cfg.hosts_text);
    engine.mute_keys(&cfg.muted);
    let (port, token, autostart) = (cfg.port, cfg.token.clone(), cfg.autostart);
    match server::start(engine.clone(), cfg, path.clone()) {
        Ok(running) => {
            if autostart && !engine.snapshot(0).hosts.is_empty() {
                engine.start();
            }
            println!("pingtable {} is running", env!("CARGO_PKG_VERSION"));
            println!("  open    {}", running.url);
            println!("  config  {}", path.display());
            println!("  stop    Ctrl+C");
            if !args.no_open {
                server::open_browser(&running.url);
            }
            running.join();
            ExitCode::SUCCESS
        }
        Err(server::StartError::PortInUse) if server::is_pingtable(port, &token) => {
            let url = server::link(port, &token);
            println!("pingtable is already running\n  open    {url}");
            if !args.no_open {
                server::open_browser(&url);
            }
            ExitCode::SUCCESS
        }
        Err(why) => {
            eprintln!("pingtable: {why}");
            ExitCode::from(1)
        }
    }
}

fn once(args: Args) -> ExitCode {
    let mut text = args.hosts.join("\n");
    if let Some(file) = &args.hosts_file {
        match std::fs::read_to_string(file) {
            Ok(body) => {
                text.push('\n');
                text.push_str(&body);
            }
            Err(why) => {
                eprintln!("pingtable: cannot read {}: {why}", file.display());
                return ExitCode::from(2);
            }
        }
    }
    if text.trim().is_empty() {
        eprintln!("pingtable: no hosts given\nTry: pingtable once example.com 192.168.1.1:22");
        return ExitCode::from(2);
    }

    let mut settings = Settings { repeat: false, reverse_dns: false, read_mac: false, ..Settings::default() };
    settings.description_format = args.described;
    if let Some(ms) = args.timeout_ms {
        settings.timeout_ms = ms;
    }
    let engine = Engine::new(settings.clone(), &text);
    let first = engine.snapshot(0);
    for warning in &first.warnings {
        eprintln!("pingtable: {warning}");
    }
    if first.hosts.is_empty() {
        eprintln!("pingtable: nothing to ping");
        return ExitCode::from(2);
    }

    // Worst case: every host times out, `max_concurrent` at a time.
    let waves = (first.hosts.len() as u64).div_ceil(settings.max_concurrent as u64);
    let budget = Duration::from_millis(waves * settings.timeout_ms as u64) + Duration::from_secs(20);
    engine.start();
    let finished = engine.wait_idle(budget);
    let snapshot = engine.snapshot(0);
    engine.shutdown();
    if !finished {
        eprintln!("pingtable: gave up waiting; some hosts have no result");
    }

    match args.format.as_deref().unwrap_or("table") {
        "json" => println!("{}", export::to_json(&snapshot)),
        "csv" => print!("{}", export::to_csv(&snapshot.hosts)),
        "table" => print!("{}", export::to_text(&snapshot.hosts)),
        other => {
            eprintln!("pingtable: unknown format \"{other}\" (use table, json or csv)");
            return ExitCode::from(2);
        }
    }
    if snapshot.hosts.iter().all(|h| h.state == "up") {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// `pingtable ports HOST [PORT...]`: try a few TCP ports of one host, once.
fn ports(args: Args) -> ExitCode {
    let usage = |why: &str| {
        eprintln!("pingtable: {why}\nTry: pingtable ports example.com 22 443");
        ExitCode::from(2)
    };
    let Some((first, rest)) = args.hosts.split_first() else {
        return usage("no host given");
    };
    let (host, addr, named) = match portcheck::split_target(first) {
        Ok(target) => target,
        Err(why) => return usage(&why),
    };
    let mut wanted: Vec<u16> = named.into_iter().collect();
    match portcheck::parse_ports(&rest.join(" ")) {
        Ok(more) => wanted.extend(more.into_iter().filter(|port| Some(*port) != named)),
        Err(why) => return usage(&why),
    }
    if wanted.is_empty() {
        wanted = portcheck::COMMON_PORTS.to_vec();
    }
    if wanted.len() > portcheck::MAX_PORTS {
        return usage(&format!("that is more than {} ports", portcheck::MAX_PORTS));
    }
    let format = args.format.as_deref().unwrap_or("table");
    if format != "table" && format != "json" {
        return usage(&format!("unknown format \"{format}\" (use table or json)"));
    }

    let settings = Settings::default();
    let Some(target) = addr.or_else(|| netinfo::resolve(&host, settings.prefer_ipv6)) else {
        eprintln!("pingtable: the name {host} does not resolve");
        return ExitCode::from(1);
    };
    let timeout_ms = args.timeout_ms.unwrap_or(portcheck::MIN_TIMEOUT_MS).clamp(50, 60_000);
    let params = ProbeParams { timeout: Duration::from_millis(timeout_ms as u64), ..ProbeParams::default() };
    let results = portcheck::check(target, &wanted, &params);
    let open = results.iter().filter(|r| r.state == portcheck::PortState::Open).count();

    if format == "json" {
        let doc = json!({ "schema_version": 1, "host": host, "target": target, "timeout_ms": timeout_ms, "open": open, "results": results });
        println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_default());
    } else {
        let shown = if host == target.to_string() { host.clone() } else { format!("{host} ({target})") };
        println!("{shown}: {open} of {} {} open", results.len(), if results.len() == 1 { "port" } else { "ports" });
        for r in &results {
            let time = r.rtt_us.map(|us| format!("{:.1} ms", us as f64 / 1000.0)).unwrap_or_default();
            let line = format!("{:>5}  {:<16} {:<10} {:>10}", r.port, r.service.unwrap_or(""), r.state.label(), time);
            println!("{}", line.trim_end());
        }
    }
    if open == results.len() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// `pingtable api`: how to call the local API of this install.
fn api(args: Args) -> ExitCode {
    let (path, mut cfg) = config::load(args.config.as_deref());
    if let Some(port) = args.port {
        cfg.set_port(port);
    }
    let base = format!("http://127.0.0.1:{}/api/v1", cfg.port);
    let running = server::is_pingtable(cfg.port, &cfg.token);
    match args.format.as_deref().unwrap_or("table") {
        "json" => {
            let doc = json!({
                "schema_version": 1, "base_url": base, "header": "X-Pingtable-Token", "token": cfg.token,
                "running": running, "config_path": path.display().to_string(),
            });
            println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_default());
        }
        "table" => {
            let token = &cfg.token;
            println!("pingtable API on this computer");
            println!("  address   {base}");
            println!("  header    X-Pingtable-Token: {token}");
            println!("  running   {}", if running { "yes" } else { "no: start pingtable.exe, or run pingtable-cli with no arguments" });
            println!("  settings  {}", path.display());
            println!();
            println!("The token is a password for this computer's pingtable. Keep it to yourself.");
            println!();
            println!("Read the table:");
            println!("  curl -H \"X-Pingtable-Token: {token}\" {base}/state");
            println!("  Invoke-RestMethod {base}/state -Headers @{{'X-Pingtable-Token'='{token}'}}");
            println!();
            println!("Check ports on a host:");
            println!("  curl -H \"X-Pingtable-Token: {token}\" -H \"Content-Type: application/json\" \\");
            println!("       -d \"{{\\\"host\\\":\\\"example.com\\\",\\\"ports\\\":[22,443]}}\" {base}/portcheck");
            println!();
            println!("Every call: {}/blob/main/docs/api.md", crate::SOURCE);
        }
        other => {
            eprintln!("pingtable: unknown format \"{other}\" (use table or json)");
            return ExitCode::from(2);
        }
    }
    ExitCode::SUCCESS
}
