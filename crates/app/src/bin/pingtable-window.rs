//! pingtable in its own window. Windows only for now; elsewhere use the
//! `pingtable` program and a browser.

#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    #[cfg(windows)]
    pingtable::window::main();

    #[cfg(not(windows))]
    {
        eprintln!("pingtable-window is the Windows window app. Here, run `pingtable` and use the browser.");
        std::process::exit(2);
    }
}
