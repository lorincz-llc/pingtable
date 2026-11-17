//! The config file: settings, host list, port and the API token.
//!
//! Location, first match wins:
//! 1. `--config FILE`
//! 2. `pingtable.json` beside the executable, if it exists (portable mode)
//! 3. the per-user config folder (`%APPDATA%\pingtable` on Windows,
//!    `$XDG_CONFIG_HOME/pingtable` or `~/.config/pingtable` elsewhere)

use pingtable_core::Settings;
use serde::{Deserialize, Serialize};
use std::hash::{BuildHasher, Hasher};
use std::path::{Path, PathBuf};

pub const DEFAULT_PORT: u16 = 7464;
const FILE_NAME: &str = "pingtable.json";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub schema_version: u32,
    pub port: u16,
    /// Secret the web UI must present. Anyone who can read this file can
    /// control pingtable, including the commands it runs on alerts.
    pub token: String,
    /// Start pinging as soon as the program starts.
    pub autostart: bool,
    pub settings: Settings,
    pub hosts_text: String,
    /// Keys (`host|icmp`, `host|tcp:443`) of hosts that are muted.
    pub muted: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            schema_version: 1,
            port: DEFAULT_PORT,
            token: String::new(),
            autostart: true,
            settings: default_settings(),
            hosts_text: default_hosts_text(DEFAULT_PORT),
            muted: Vec::new(),
        }
    }
}

impl Config {
    /// Change the port, keeping the example list's "pingtable itself" entry in step.
    pub fn set_port(&mut self, port: u16) {
        if self.hosts_text == default_hosts_text(self.port) {
            self.hosts_text = default_hosts_text(port);
        }
        self.port = port;
    }
}

/// The example host list. A new install starts with it, and "Restore default
/// hosts" brings it back. It shows every kind of entry: groups, ICMP over
/// IPv4 and IPv6, TCP checks, a URL, per-host options and two hosts that
/// fail on purpose (a reserved test address and a name that cannot resolve).
pub fn default_hosts_text(port: u16) -> String {
    format!(
        "Group: This PC every=2s tag=local\n\
         127.0.0.1 Loopback\n\
         ::1 Loopback, IPv6\n\
         127.0.0.1:{port} pingtable itself\n\
         \n\
         Group: Internet\n\
         1.1.1.1 Cloudflare DNS\n\
         8.8.8.8 Google DNS\n\
         9.9.9.9 Quad9 every=3s\n\
         one.one.one.one:443 Cloudflare HTTPS\n\
         https://example.com Example site\n\
         \n\
         Group: Broken on purpose\n\
         192.0.2.1 TEST-NET, never answers\n\
         no-such-host.invalid Bad name\n"
    )
}

/// Settings a new install starts with. They suit the example list: one host
/// per line with a description, one ping a second, down after 2 failures.
pub fn default_settings() -> Settings {
    Settings { interval_secs: 1, fail_trigger: 2, description_format: true, ..Settings::default() }
}

/// 128 random bits as hex, from the operating system's secure random source.
fn new_token() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        // No secure source (not expected on any supported system): fall back to
        // the standard library's per-process random keys rather than refuse to start.
        for chunk in bytes.chunks_mut(8) {
            let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
            hasher.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
            hasher.write_u32(std::process::id());
            chunk.copy_from_slice(&hasher.finish().to_ne_bytes());
        }
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn default_path() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let portable = dir.join(FILE_NAME);
            if portable.is_file() {
                return portable;
            }
        }
    }
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
    };
    base.unwrap_or_else(|| PathBuf::from(".")).join("pingtable").join(FILE_NAME)
}

/// Read the config, creating defaults (and a token) when there is none.
pub fn load(explicit: Option<&Path>) -> (PathBuf, Config) {
    let path = explicit.map(Path::to_path_buf).unwrap_or_else(default_path);
    let mut cfg = match std::fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<Config>(&text) {
            Ok(cfg) => cfg,
            Err(why) => {
                eprintln!("pingtable: {} is not valid ({why}); starting with defaults", path.display());
                Config::default()
            }
        },
        Err(_) => Config::default(),
    };
    cfg.settings = cfg.settings.sanitized();
    if cfg.token.len() < 16 {
        cfg.token = new_token();
        if let Err(why) = save(&path, &cfg) {
            eprintln!("pingtable: cannot write {}: {why}", path.display());
        }
    }
    (path, cfg)
}

/// Write the config through a temporary file so a crash cannot truncate it.
pub fn save(path: &Path, cfg: &Config) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)?;
        }
    }
    let text = serde_json::to_string_pretty(cfg).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pingtable_core::parse::{parse_host_list, ParseOptions};
    use pingtable_core::Probe;

    #[test]
    fn tokens_are_long_random_and_different() {
        let (a, b) = (new_token(), new_token());
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn default_host_list_parses_cleanly() {
        let settings = default_settings();
        let options = ParseOptions {
            description_format: settings.description_format,
            cidr_skip_first: settings.cidr_skip_first,
            cidr_skip_last: settings.cidr_skip_last,
        };
        let report = parse_host_list(&default_hosts_text(7464), &options);
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert_eq!(report.hosts.len(), 10);
        let groups: std::collections::BTreeSet<_> = report.hosts.iter().filter_map(|h| h.group.as_deref()).collect();
        assert_eq!(groups.len(), 3);
        assert_eq!(report.hosts[2].probe, Probe::Tcp { port: 7464 });
        assert_eq!(report.hosts[2].description, "pingtable itself");
        assert_eq!(report.hosts[0].overrides.interval_secs, Some(2));
        assert_eq!(report.hosts[5].overrides.interval_secs, Some(3));
        assert_eq!(report.hosts[7].probe, Probe::Tcp { port: 443 });
    }

    #[test]
    fn changing_the_port_updates_only_an_untouched_default_list() {
        let mut cfg = Config::default();
        cfg.set_port(9000);
        assert!(cfg.hosts_text.contains("127.0.0.1:9000 pingtable itself"));

        let mut edited = Config { hosts_text: "nas.lan".into(), ..Config::default() };
        edited.set_port(9000);
        assert_eq!(edited.hosts_text, "nas.lan");
        assert_eq!(edited.port, 9000);
    }

    #[test]
    fn an_empty_list_in_the_file_stays_empty() {
        let cfg: Config = serde_json::from_str(r#"{"hosts_text": ""}"#).unwrap();
        assert_eq!(cfg.hosts_text, "");
        let cfg: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.hosts_text, default_hosts_text(DEFAULT_PORT));
    }
}
