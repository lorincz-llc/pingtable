//! The command line: `pingtable [serve]` and `pingtable once`.

use crate::{config, server};
use pingtable_core::{export, Engine, Settings};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

const HELP: &str = "\
pingtable - ping many hosts and watch them in one table

USAGE
  pingtable [serve] [OPTIONS]          run the local web UI (default)
  pingtable once [OPTIONS] [HOST...]   ping every host once and print the result

OPTIONS
  --config FILE     use this config file
  --port N          port for the web UI (default 7464)
  --no-open         do not open the browser
  --hosts FILE      once: read the host list from a file
  --format F        once: table (default), json or csv
  --timeout MS      once: per-probe timeout in milliseconds
  --described       once: host list is one \"address description\" per line
  -h, --help        show this help
  -V, --version     show the version

HOSTS
  example.com  192.168.1.10  2001:db8::1          ICMP ping
  example.com:443  [2001:db8::1]:22  https://x.y  TCP connect
  10.0.0.1-10.0.0.20  10.0.0.0/24                 IPv4 range, CIDR block
  host every=5s timeout=300ms tag=lan             per-host options
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
            "serve" | "once" if args.command.is_none() && args.hosts.is_empty() => args.command = Some(arg),
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
