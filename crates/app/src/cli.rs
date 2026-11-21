//! Command-line entry points for the live table, one-shot checks and agents.

use crate::{config, mcp, server};
use pingtable_core::{export, Engine, Settings};
use serde_json::json;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

const HELP: &str = "\
pingtable - monitor ping and TCP connectivity across many hosts

This is the command-line program (pingtable-cli.exe on Windows). The app in
its own window is pingtable.exe; both use the same settings and host list.

USAGE
  pingtable [serve] [OPTIONS]          run the table, shown in the browser (default)
  pingtable once [OPTIONS] [HOST...]   ping every host once and print the result
  pingtable api [--format json]        show how to call this computer's pingtable
  pingtable mcp [--allow-add]          serve the checks to an AI agent (MCP, stdio)

OPTIONS
  --config FILE     use this config file (mcp, api: the running app to talk to)
  --allow-add       mcp: also offer table_add_hosts, which changes the saved list
  --port N          port for the web UI (default 7464)
  --no-open         do not open the browser
  --hosts FILE      once: read the host list from a file
  --format F        once: table (default), json or csv; api: table or json
  --timeout MS      once: per-probe timeout in milliseconds
  --described       once: host list is one \"address description\" per line
  -h, --help        show this help
  -V, --version     show the version

HOSTS
  example.com  192.168.1.10  2001:db8::1          ICMP ping
  example.com:443  [2001:db8::1]:22  https://x.y  TCP connect: is the port listening?
  10.0.0.1-10.0.0.20  10.0.0.0/24                 IPv4 range, CIDR block
  host every=5s timeout=300ms tag=lan             per-host options

AI AGENTS AND SCRIPTS
  pingtable mcp is a Model Context Protocol server on standard input and
  output. Point an agent at it with the command: pingtable-cli mcp
  It then has the tools ping, trace_route, network_info and
  table_status, and with --allow-add also table_add_hosts. A script can use
  once with --format json, or the local HTTP API: pingtable api
  prints its address and access token.
  Guide: https://github.com/lorincz-llc/pingtable/blob/main/docs/ai-agents.md

EXIT STATUS
  once: 0 when every host answers, 1 when not,
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
    allow_add: bool,
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
            "--allow-add" => args.allow_add = true,
            "serve" | "once" | "api" | "mcp" if args.command.is_none() && args.hosts.is_empty() => args.command = Some(arg),
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
        Some("api") => api(args),
        Some("mcp") => mcp::serve_stdio(mcp::Options { config: args.config, allow_add: args.allow_add }),
        _ if args.hosts.is_empty() => serve(args),
        _ => {
            eprintln!("pingtable: unexpected argument {}\nTry: pingtable --help", args.hosts[0]);
            ExitCode::from(2)
        }
    }
}

fn serve(args: Args) -> ExitCode {
    let (path, mut cfg) = match config::load(args.config.as_deref()) {
        Ok(loaded) => loaded,
        Err(why) => {
            eprintln!("pingtable: {why}");
            return ExitCode::from(1);
        }
    };
    if let Some(port) = args.port {
        cfg.set_port(port);
    }
    // Ctrl+C, a closed terminal or a kill reaches the alert commands too.
    pingtable_core::engine::pass_stop_signals_to_commands();
    let engine = Engine::new(cfg.settings.clone(), &cfg.hosts_text);
    engine.mute_keys(&cfg.muted);
    let (port, token, autostart) = (cfg.port, cfg.token.clone(), cfg.autostart);
    match server::start(engine.clone(), cfg, path.clone()) {
        Ok(running) => {
            server::autostart(&engine, autostart);
            println!("pingtable {} is running", env!("CARGO_PKG_VERSION"));
            println!("  open    {}", running.url);
            println!("  config  {}", path.display());
            println!("  stop    Ctrl+C");
            if !args.no_open {
                server::open_ui(running.port, &token);
            }
            running.join();
            ExitCode::SUCCESS
        }
        // Only a pingtable that proves it holds the token is treated as one.
        Err(server::StartError::PortInUse) => match server::prove(port, &token) {
            Ok(()) => {
                let url = server::link(port, &token);
                println!("pingtable is already running\n  open    {url}");
                if !args.no_open {
                    server::open_ui(port, &token);
                }
                ExitCode::SUCCESS
            }
            Err(why) => {
                eprintln!("pingtable: {}", server::port_taken(port, &why));
                ExitCode::from(1)
            }
        },
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

/// `pingtable api`: how to call the local API of this install.
fn api(args: Args) -> ExitCode {
    // Read only, unless there is no settings file yet: then one is made, with a token.
    let explicit = args.config.as_deref();
    let (path, mut cfg) = match config::peek(explicit) {
        Some(found) => found,
        None => match config::problem(explicit) {
            Some(why) => {
                eprintln!("pingtable: {why}");
                return ExitCode::from(1);
            }
            None => match config::load(explicit) {
                Ok(loaded) => loaded,
                Err(why) => {
                    eprintln!("pingtable: {why}");
                    return ExitCode::from(1);
                }
            },
        },
    };
    if let Some(port) = args.port {
        cfg.set_port(port);
    }
    // Where it really runs (its run file says), if it proves it is this pingtable.
    let found = match args.port {
        Some(port) => server::prove(port, &cfg.token).map(|_| port).map_err(|why| (why, port)),
        None => server::find_local(&path, &cfg),
    };
    let running = found.is_ok();
    let port = found.as_ref().map_or(cfg.port, |port| *port);
    let base = format!("http://127.0.0.1:{port}/api/v1");
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
            println!(
                "  running   {}",
                match found.as_ref().map_err(|(why, _)| why) {
                    Ok(_) => "yes",
                    Err(server::LocalError::Unproven) => "no: another program answers on that port, so it was not given the token",
                    Err(server::LocalError::Older) => "no: an older pingtable answers on that port; close it and start this version",
                    Err(server::LocalError::NoReply) => "no: something on that port did not answer in time; try again in a moment",
                    Err(_) => "no: start pingtable.exe, or run pingtable-cli with no arguments",
                }
            );
            println!("  settings  {}", path.display());
            println!();
            println!("The token is a password for this computer's pingtable. Keep it to yourself.");
            println!();
            println!("Read the table:");
            println!("  curl -H \"X-Pingtable-Token: {token}\" {base}/state");
            println!("  Invoke-RestMethod {base}/state -Headers @{{'X-Pingtable-Token'='{token}'}}");
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
