//! pingtable application: config file, local HTTP server, command line and
//! (on Windows) the native window.
//!
//! Two programs are built from this crate:
//!
//! * `pingtable`, a console program: `pingtable [serve]`, `once`,
//!   `api`, `mcp` (the checks as tools for an AI agent).
//! * `pingtable-window`, a Windows GUI program that shows the same interface
//!   in its own window.

/// Who pingtable is credited to. Shown in the About box and in the exe details.
pub const CREDITS: &str = "hello@pingtable.net";

/// The project's home on the web.
pub const WEBSITE: &str = "https://pingtable.net";

/// Where the source code and downloads are.
pub const SOURCE: &str = "https://github.com/lorincz-llc/pingtable";

pub mod cli;
pub mod config;
pub mod mcp;
pub mod server;
#[cfg(windows)]
pub mod window;

/// Where problems are written: `pingtable.log` in the per-user data folder.
pub fn log_path() -> std::path::PathBuf {
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from)
    } else {
        std::env::var_os("XDG_STATE_HOME")
            .map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".local").join("state")))
    };
    base.unwrap_or_else(std::env::temp_dir).join("pingtable").join("pingtable.log")
}

/// Record unexpected faults in the log file (and on the console) instead of
/// losing them. The window program has no console, so the file is the only
/// place such a message can go.
pub fn install_fault_log() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        use std::io::Write;
        let path = log_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // Keep the file small: start over once it passes 1 MB.
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > 1_000_000) {
            let _ = std::fs::remove_file(&path);
        }
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
            let when = pingtable_core::export::rfc3339(pingtable_core::now_ms());
            let thread = std::thread::current().name().unwrap_or("unnamed").to_string();
            let _ = writeln!(file, "{when} pingtable {} fault in thread {thread}: {info}", env!("CARGO_PKG_VERSION"));
        }
        previous(info);
    }));
}

