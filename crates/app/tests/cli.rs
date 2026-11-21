//! The command-line program as a person or a script runs it.

#[cfg(unix)]
use std::path::PathBuf;
use std::process::Command;
#[cfg(unix)]
use std::time::{Duration, Instant};

/// Run `pingtable` with these arguments: exit status, standard output, standard error.
fn pingtable(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_pingtable")).args(args).output().expect("pingtable runs");
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

#[cfg(unix)]
/// A folder of its own for one test, removed when it is dropped.
struct Scratch(PathBuf);

#[cfg(unix)]
impl Scratch {
    fn new(name: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("pingtable-test-{}-cli-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
}

#[cfg(unix)]
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(unix)]
fn wait_for(what: &str, limit: Duration, done: impl Fn() -> bool) {
    let deadline = Instant::now() + limit;
    while !done() {
        assert!(Instant::now() < deadline, "gave up waiting: {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn retired_commands_report_a_usage_error() {
    for command in ["ports", "license", "licence"] {
        assert_eq!(pingtable(&[command]).0, 2);
    }
}

#[test]
fn once_checks_a_named_tcp_endpoint() {
    let service = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = service.local_addr().unwrap();
    let (code, output, errors) = pingtable(&["once", "--format", "json", &addr.to_string()]);
    assert_eq!(code, 0, "{errors}");
    let report: serde_json::Value = serde_json::from_str(&output).unwrap();
    let row = &report["hosts"][0];
    assert_eq!(row["probe"], serde_json::json!({ "kind": "tcp", "port": addr.port() }));
    assert_eq!(row["last_status"], "ok");
}

/// Ctrl+C on the table reaches the alert commands it is running, which run
/// in process groups of their own.
#[cfg(unix)]
#[test]
fn stopping_the_table_stops_its_alert_commands() {
    use std::process::Stdio;
    let dir = Scratch::new("signal");
    let config = dir.0.join("pingtable.json");
    // A port nothing listens on: the check is refused at once, so the host goes down.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    // A command no other test runs. `; true` keeps the shell, so the group has two programs.
    let marker = format!("sleep 297.{}", std::process::id() % 100_000);
    let settings = serde_json::json!({
        "token": "0123456789abcdef0123456789abcdef", "port": 0, "autostart": true,
        "hosts_text": format!("127.0.0.1:{closed}"),
        "settings": { "interval_secs": 1, "fail_trigger": 1, "on_down_command": format!("{marker}; true") },
    });
    std::fs::write(&config, settings.to_string()).unwrap();
    let running = || Command::new("pgrep").args(["-f", &marker]).output().is_ok_and(|out| !out.stdout.is_empty());

    /// Ends the table and the command whatever happens in the test.
    struct Cleanup(std::process::Child, String);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
            let _ = Command::new("pkill").args(["-f", &self.1]).status();
        }
    }
    let table = Command::new(env!("CARGO_BIN_EXE_pingtable"))
        .args(["--config", config.to_str().unwrap(), "--no-open"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut table = Cleanup(table, marker.clone());
    wait_for("the alert command to start", Duration::from_secs(20), running);
    let interrupted = Command::new("kill").args(["-INT", &table.0.id().to_string()]).status().unwrap();
    assert!(interrupted.success());
    let _ = table.0.wait();
    let gone = Instant::now() + Duration::from_secs(5);
    while running() && Instant::now() < gone {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(!running(), "the alert command outlived the table");
}
