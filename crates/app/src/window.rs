//! The native Windows window: a WebView2 view of the local interface.
//!
//! The window is only a frame. Pinging, settings and alerts all live in the
//! engine and the local server, exactly as in the browser flavour. Closing
//! the window ends the program, unless it was showing another pingtable that
//! was already running.

use crate::{config, server};
use pingtable_core::Engine;
use std::path::PathBuf;
use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::platform::windows::IconExtWindows;
use tao::window::{Icon, Theme, WindowBuilder};
use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_ICONINFORMATION, MB_OK};
use wry::{WebContext, WebViewBuilder};

enum UserEvent {
    /// The page changed its title (it carries the count of hosts that are down).
    Title(String),
    /// The page changed its look; the frame should match.
    Look(Look),
}

/// What the frame needs to know about the page's look: dark or light for the
/// title bar, and the background colour to show before the page has loaded.
#[derive(Clone, Copy, PartialEq)]
struct Look {
    dark: bool,
    background: (u8, u8, u8),
}

impl Look {
    const DEFAULT: Look = Look { dark: false, background: (255, 255, 255) };

    fn theme(self) -> Theme {
        if self.dark {
            Theme::Dark
        } else {
            Theme::Light
        }
    }

    fn rgba(self) -> (u8, u8, u8, u8) {
        (self.background.0, self.background.1, self.background.2, 255)
    }

    /// From the page's message, for example `{"theme":"dark","bg":"#071417"}`.
    fn from_json(text: &str) -> Option<Look> {
        let value: serde_json::Value = serde_json::from_str(text).ok()?;
        let dark = match value.get("theme")?.as_str()? {
            "dark" => true,
            "light" => false,
            _ => return None,
        };
        let background = parse_hex_colour(value.get("bg")?.as_str()?)?;
        Some(Look { dark, background })
    }

    fn to_json(self) -> String {
        let (r, g, b) = self.background;
        format!(r##"{{"theme":"{}","bg":"#{r:02x}{g:02x}{b:02x}"}}"##, if self.dark { "dark" } else { "light" })
    }
}

/// `#rgb` or `#rrggbb`.
fn parse_hex_colour(text: &str) -> Option<(u8, u8, u8)> {
    let hex = text.trim().strip_prefix('#')?;
    let digits: Vec<u8> = hex.chars().map(|c| c.to_digit(16).map(|d| d as u8)).collect::<Option<_>>()?;
    match digits.as_slice() {
        [r, g, b] => Some((r * 17, g * 17, b * 17)),
        [r1, r2, g1, g2, b1, b2] => Some((r1 * 16 + r2, g1 * 16 + g2, b1 * 16 + b2)),
        _ => None,
    }
}

fn local_data_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("pingtable")
}

/// The look is remembered so the next start opens in the right colours at once.
fn look_file() -> PathBuf {
    local_data_dir().join("window.json")
}

fn load_look() -> Look {
    std::fs::read_to_string(look_file()).ok().and_then(|text| Look::from_json(&text)).unwrap_or(Look::DEFAULT)
}

fn save_look(look: Look) {
    let path = look_file();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, look.to_json());
}

/// Tells the page it runs inside the window, so it can drop browser-only chrome.
const SHELL_SCRIPT: &str = "window.__PINGTABLE_SHELL__ = true;";

pub fn main() {
    crate::install_fault_log();
    if let Err(why) = run() {
        message_box(&why, MB_OK | MB_ICONERROR);
        std::process::exit(1);
    }
}

fn message_box(text: &str, flags: u32) {
    let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let (text, title) = (wide(text), wide("pingtable"));
    unsafe {
        MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), flags);
    }
}

struct Args {
    config: Option<PathBuf>,
    port: Option<u16>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args { config: None, port: None };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--config" => args.config = Some(PathBuf::from(it.next().ok_or("--config needs a file")?)),
            "--port" => args.port = Some(it.next().and_then(|v| v.parse().ok()).ok_or("--port needs a number")?),
            other => return Err(format!("Unknown option {other}.\n\nThe window accepts --config FILE and --port N.")),
        }
    }
    Ok(args)
}

/// Start the engine and server, or find the pingtable that already runs.
/// Returns the link to show and the port it is on.
fn start_or_attach(args: &Args) -> Result<(String, u16), String> {
    let (path, mut cfg) = config::load(args.config.as_deref());
    if let Some(port) = args.port {
        cfg.set_port(port);
    }
    let (port, token, autostart) = (cfg.port, cfg.token.clone(), cfg.autostart);
    let engine = Engine::new(cfg.settings.clone(), &cfg.hosts_text);
    engine.mute_keys(&cfg.muted);
    match server::start(engine.clone(), cfg, path) {
        Ok(running) => {
            if autostart && !engine.snapshot(0).hosts.is_empty() {
                engine.start();
            }
            Ok((running.url, port))
        }
        Err(server::StartError::PortInUse) if server::is_pingtable(port, &token) => {
            engine.shutdown();
            Ok((server::link(port, &token), port))
        }
        Err(why) => Err(format!("pingtable could not start.\n\nPort {port}: {why}.")),
    }
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let (url, port) = start_or_attach(&args)?;

    let mut look = load_look();
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let window = WindowBuilder::new()
        .with_title("pingtable")
        .with_inner_size(LogicalSize::new(1360.0, 860.0))
        .with_min_inner_size(LogicalSize::new(340.0, 300.0))
        .with_window_icon(Icon::from_resource(1, None).ok())
        .with_theme(Some(look.theme()))
        .with_background_color(look.rgba())
        .build(&event_loop)
        .map_err(|why| format!("The window could not be created: {why}"))?;

    // WebView2 keeps its profile here; beside the exe would fail in read-only folders.
    let mut context = WebContext::new(Some(local_data_dir().join("webview")));
    let proxy = event_loop.create_proxy();
    let theme_proxy = event_loop.create_proxy();
    let origin = format!("http://127.0.0.1:{port}/");

    let built = WebViewBuilder::new_with_web_context(&mut context)
        .with_url(&url)
        .with_background_color(look.rgba())
        .with_initialization_script(SHELL_SCRIPT)
        .with_document_title_changed_handler(move |title| {
            let _ = proxy.send_event(UserEvent::Title(title));
        })
        .with_ipc_handler(move |message: wry::http::Request<String>| {
            if let Some(look) = Look::from_json(message.body()) {
                let _ = theme_proxy.send_event(UserEvent::Look(look));
            }
        })
        // The window shows pingtable and nothing else.
        .with_navigation_handler(move |target| target.starts_with(&origin))
        // Links to a host ("Open http://nas.lan") go to the default browser.
        .with_new_window_req_handler(|target, _| {
            let web = target.starts_with("http://") || target.starts_with("https://");
            // Host links are plain; refuse anything a command shell could misread.
            let plain = target.chars().all(|c| c.is_ascii_alphanumeric() || ":/.[]_-".contains(c));
            if web && plain {
                server::open_browser(&target);
            }
            wry::NewWindowResponse::Deny
        })
        .build(&window);

    let webview = match built {
        Ok(webview) => webview,
        Err(why) => {
            // No WebView2 runtime (rare on Windows 10 and 11): fall back to the browser.
            drop(window);
            server::open_browser(&url);
            message_box(
                &format!(
                    "pingtable is running and has opened in your browser.\n\n\
                     Its own window needs the Microsoft Edge WebView2 Runtime, which could not be started ({why}).\n\n\
                     Click OK to stop pingtable."
                ),
                MB_OK | MB_ICONINFORMATION,
            );
            return Ok(());
        }
    };

    event_loop.run(move |event, _, control_flow| {
        let _keep_alive = &webview;
        *control_flow = ControlFlow::Wait;
        match event {
            Event::UserEvent(UserEvent::Title(title)) => window.set_title(&title),
            Event::UserEvent(UserEvent::Look(new_look)) => {
                if new_look != look {
                    look = new_look;
                    window.set_theme(Some(look.theme()));
                    save_look(look);
                }
            }
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => *control_flow = ControlFlow::Exit,
            _ => {}
        }
    })
}
