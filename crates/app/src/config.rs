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
    /// Set by hand in this file only. While true, the alert commands in
    /// `settings` stay as written here: neither the API nor the interface can
    /// change them.
    pub commands_locked: bool,
    /// Something the person should be told about how this file was read, for
    /// example that it could not be and was set aside. Never saved.
    #[serde(skip)]
    pub notice: Option<String>,
    /// True when the file there could not be read and could not be moved
    /// aside either. It is left as it is: [`save`] writes nothing over it for
    /// the rest of the run. Never saved.
    #[serde(skip)]
    pub do_not_save: bool,
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
            commands_locked: false,
            notice: None,
            do_not_save: false,
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
/// The TCP checks of public servers run every 30 seconds: a connection a
/// second would only fill someone else's logs.
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
         one.one.one.one:443 Cloudflare HTTPS every=30s\n\
         https://example.com Example site every=30s\n\
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
pub(crate) fn new_token() -> String {
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
///
/// A file that cannot be used is never overwritten. One that was read but is
/// not valid settings is renamed to `pingtable.json.unreadable-<time>` and
/// pingtable starts from defaults, with `notice` saying so, so the host list
/// and commands in it are not lost. If it cannot be renamed, it is left where
/// it is and nothing is saved over it. One that cannot be opened at all (a
/// folder, no permission, held by another program) is an error: nothing can
/// be known about it, so pingtable does not start.
pub fn load(explicit: Option<&Path>) -> Result<(PathBuf, Config), String> {
    let path = path(explicit);
    let mut cfg = match read(&path) {
        Ok(Some(cfg)) => cfg,
        Ok(None) => Config::default(),
        Err(Unusable::Unopened(why)) => {
            return Err(format!(
                "The settings file {} could not be opened ({why}). pingtable did not start, so that nothing in it is lost. \
                 Make sure it is a file your account can read and that no other program holds it open, then start pingtable \
                 again, or name another file with --config.",
                path.display()
            ))
        }
        Err(Unusable::Invalid(why)) => {
            let (notice, moved) = set_aside(&path, &why);
            eprintln!("pingtable: {notice}");
            Config { notice: Some(notice), do_not_save: !moved, ..Config::default() }
        }
    };
    cfg.settings = cfg.settings.sanitized();
    if cfg.token.len() < 16 {
        cfg.token = new_token();
        if let Err(why) = save(&path, &cfg) {
            eprintln!("pingtable: cannot write {}: {why}", path.display());
        }
    }
    Ok((path, cfg))
}

/// Where the settings file is, or would be: `--config FILE`, or the usual place.
pub fn path(explicit: Option<&Path>) -> PathBuf {
    explicit.map(Path::to_path_buf).unwrap_or_else(default_path)
}

/// Read the config without creating or changing anything. None when there is
/// no usable config yet, which means pingtable has not run here.
pub fn peek(explicit: Option<&Path>) -> Option<(PathBuf, Config)> {
    let path = path(explicit);
    let cfg = read(&path).ok()??;
    (cfg.token.len() >= 16).then_some((path, cfg))
}

/// Why the config file that is there cannot be used, if it cannot. Nothing
/// is changed: for programs that only read it.
pub fn problem(explicit: Option<&Path>) -> Option<String> {
    let path = path(explicit);
    match read(&path) {
        Ok(_) => None,
        Err(Unusable::Invalid(why)) => Some(format!(
            "The settings file {} cannot be read ({why}). Fix it, or start pingtable, which keeps it aside and starts with the default settings.",
            path.display()
        )),
        Err(Unusable::Unopened(why)) => Some(format!(
            "The settings file {} could not be opened ({why}). Make sure it is a file your account can read and that no other program holds it open.",
            path.display()
        )),
    }
}

/// The settings file as it is on disk now, without creating or changing
/// anything. None when there is none, or it cannot be used.
pub fn on_disk(path: &Path) -> Option<Config> {
    read(path).ok().flatten()
}

/// Why a settings file that is there cannot be used.
#[derive(Debug)]
enum Unusable {
    /// It could not be opened or read: a folder, no permission, a file another
    /// program holds. What it holds is not known, so it is left alone.
    Unopened(String),
    /// It was read, and is not settings pingtable understands (a typing slip, say).
    Invalid(String),
}

/// The config in `path`. None when there is no file; an error says why a file
/// that is there cannot be used.
fn read(path: &Path) -> Result<Option<Config>, Unusable> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_dir() => return Err(Unusable::Unopened("it is a folder, not a file".into())),
        Ok(meta) if !meta.is_file() => return Err(Unusable::Unopened("it is not an ordinary file".into())),
        Ok(_) => {}
        Err(why) if why.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(why) => return Err(Unusable::Unopened(why.to_string())),
    }
    let bytes = std::fs::read(path).map_err(|why| Unusable::Unopened(why.to_string()))?;
    let text = decode(&bytes).map_err(Unusable::Invalid)?;
    serde_json::from_str(&text).map(Some).map_err(|why| Unusable::Invalid(why.to_string()))
}

/// Text from a file an editor may have saved with a byte-order mark, or as
/// UTF-16 (as Windows PowerShell does).
fn decode(bytes: &[u8]) -> Result<String, String> {
    let utf16 = |rest: &[u8], unit: fn([u8; 2]) -> u16| {
        let units: Vec<u16> = rest.chunks(2).map(|pair| unit([pair[0], *pair.get(1).unwrap_or(&0)])).collect();
        String::from_utf16(&units).map_err(|_| "it is not valid UTF-16 text".to_string())
    };
    if let Some(rest) = bytes.strip_prefix(b"\xEF\xBB\xBF") {
        String::from_utf8(rest.to_vec()).map_err(|_| "it is not UTF-8 text".into())
    } else if let Some(rest) = bytes.strip_prefix(b"\xFF\xFE") {
        utf16(rest, u16::from_le_bytes)
    } else if let Some(rest) = bytes.strip_prefix(b"\xFE\xFF") {
        utf16(rest, u16::from_be_bytes)
    } else {
        String::from_utf8(bytes.to_vec()).map_err(|_| "it is not UTF-8 text".into())
    }
}

/// Move an unreadable config out of the way, under a name that says when.
/// Returns what to tell the person, and whether the file was moved.
fn set_aside(path: &Path, why: &str) -> (String, bool) {
    let stamp: String = pingtable_core::export::rfc3339(pingtable_core::now_ms()).chars().filter(|c| c.is_ascii_digit() || *c == 'T').take(15).collect();
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| FILE_NAME.into());
    let mut aside = path.with_file_name(format!("{name}.unreadable-{stamp}"));
    let mut n = 2;
    while aside.exists() {
        aside = path.with_file_name(format!("{name}.unreadable-{stamp}-{n}"));
        n += 1;
    }
    match std::fs::rename(path, &aside) {
        Ok(()) => (
            format!(
                "The settings file {} could not be read ({why}), so pingtable started with the default settings and host list. \
                 The old file is kept as {}: to get your hosts back, fix it and put it back in place while pingtable is closed, \
                 or copy the host list out of it.",
                path.display(),
                aside.display()
            ),
            true,
        ),
        Err(also) => (
            format!(
                "The settings file {} could not be read ({why}) or moved aside ({also}), so pingtable started with the default \
                 settings and host list, and will not save anything over that file. Changes made now last until pingtable is \
                 closed. Fix or move that file, then start pingtable again.",
                path.display()
            ),
            false,
        ),
    }
}

/// Write the config through a temporary file so a crash cannot truncate it.
/// Refused when `cfg` says the file there is to be left alone.
pub fn save(path: &Path, cfg: &Config) -> std::io::Result<()> {
    if cfg.do_not_save {
        return Err(std::io::Error::other(
            "it could not be read or moved aside when pingtable started, so pingtable leaves it as it is until it is closed",
        ));
    }
    let text = serde_json::to_string_pretty(cfg).map_err(std::io::Error::other)?;
    write_private(path, text.as_bytes())
}

/// Replace a file atomically with an owner-only copy on filesystems that
/// support permissions. Portable FAT volumes do not have per-user access.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)?;
        }
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| FILE_NAME.into());
    // Every writer owns its temporary file. Another process can save the same
    // config while we are writing without replacing the bytes we will rename.
    let tmp = path.with_file_name(format!("{name}.{}.tmp", new_token()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Owner-only from the moment it exists, not after the token is in it.
        options.mode(0o600);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::{Foundation::GENERIC_WRITE, Storage::FileSystem::{FILE_READ_ATTRIBUTES, WRITE_DAC}};
        // No other handle can read the temporary file before its ACL is in place.
        options.share_mode(0).access_mode(GENERIC_WRITE | FILE_READ_ATTRIBUTES | WRITE_DAC);
    }
    let mut file = options.open(&tmp)?;
    #[cfg(windows)]
    let permissions = private_windows_file(&file);
    #[cfg(not(windows))]
    let permissions = Ok(());
    let result = permissions.and_then(|()| file.write_all(bytes)).and_then(|()| file.sync_all());
    drop(file);
    let result = result.and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(tmp);
    } else {
        // Older versions used one fixed temporary name. Retire a stale
        // copy, which may still contain an older token or key.
        let _ = std::fs::remove_file(path.with_file_name(format!("{name}.tmp")));
    }
    result
}

/// Keep the current account as the only entry in the access list, even
/// when the portable folder's permissions are inherited from a shared drive.
#[cfg(windows)]
fn private_windows_file(file: &std::fs::File) -> std::io::Result<()> {
    use std::io::Error;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{LocalFree, ERROR_SUCCESS};
    use windows_sys::Win32::Security::Authorization::{
        SetEntriesInAclW, SetSecurityInfo, EXPLICIT_ACCESS_W, GRANT_ACCESS, SE_FILE_OBJECT, TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
    };
    use windows_sys::Win32::Security::{GetTokenInformation, TokenUser, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, TOKEN_QUERY};
    use windows_sys::Win32::Storage::FileSystem::{GetVolumeInformationByHandleW, FILE_ALL_ACCESS};
    use windows_sys::Win32::System::SystemServices::{FILE_PERSISTENT_ACLS, SE_TOKEN_USER};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    // SAFETY: file stays open throughout; only the volume flags are requested.
    let mut flags = 0;
    if unsafe { GetVolumeInformationByHandleW(file.as_raw_handle(), null_mut(), 0, null_mut(), null_mut(), &mut flags, null_mut(), 0) } == 0 {
        return Err(Error::last_os_error());
    }
    if flags & FILE_PERSISTENT_ACLS == 0 {
        return Ok(());
    }

    // SAFETY: TokenUser fills SE_TOKEN_USER with the account SID and its
    // backing bytes. `user` stays live until the ACL has been constructed.
    let mut token = null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(Error::last_os_error());
    }
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    let mut user = SE_TOKEN_USER::default();
    let mut written = 0;
    if unsafe { GetTokenInformation(token.as_raw_handle(), TokenUser, (&raw mut user).cast(), std::mem::size_of_val(&user) as u32, &mut written) } == 0 {
        return Err(Error::last_os_error());
    }
    let entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: FILE_ALL_ACCESS,
        grfAccessMode: GRANT_ACCESS,
        Trustee: TRUSTEE_W {
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_USER,
            ptstrName: unsafe { user.Anonymous1.User.Sid.cast() },
            ..Default::default()
        },
        ..Default::default()
    };
    let mut acl = null_mut();
    // SAFETY: one valid entry and no previous ACL. Windows allocates the
    // new ACL; SetSecurityInfo copies it, then LocalFree releases it.
    let result = unsafe { SetEntriesInAclW(1, &entry, null(), &mut acl) };
    if result != ERROR_SUCCESS {
        return Err(Error::from_raw_os_error(result as i32));
    }
    let result = unsafe {
        SetSecurityInfo(file.as_raw_handle(), SE_FILE_OBJECT, DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, null_mut(), null_mut(), acl, null())
    };
    unsafe { LocalFree(acl.cast()) };
    if result != ERROR_SUCCESS {
        return Err(Error::from_raw_os_error(result as i32));
    }
    Ok(())
}

/// The small file a running pingtable writes beside its config, saying which
/// port it really listens on: `pingtable.json.run`.
pub fn run_file(path: &Path) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| FILE_NAME.into());
    path.with_file_name(format!("{name}.run"))
}

/// Record the port this pingtable listens on, for the programs that look for it.
pub fn write_run_file(path: &Path, port: u16) -> std::io::Result<()> {
    let doc = serde_json::json!({ "port": port, "pid": std::process::id(), "version": env!("CARGO_PKG_VERSION") });
    write_private(&run_file(path), doc.to_string().as_bytes())
}

/// Ports where the pingtable using this config may listen, most likely first:
/// the one in its run file, then the one in the config. Whatever answers on
/// them still has to prove it holds the token before it is told anything.
pub fn ports_to_try(path: &Path, cfg: &Config) -> Vec<u16> {
    let recorded = std::fs::read_to_string(run_file(path))
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|doc| doc["port"].as_u64())
        .and_then(|port| u16::try_from(port).ok())
        .filter(|port| *port != 0);
    let mut ports: Vec<u16> = recorded.into_iter().collect();
    if !ports.contains(&cfg.port) {
        ports.push(cfg.port);
    }
    ports
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
        // Public servers are checked every 30 seconds, not every second.
        for public in &report.hosts[6..=7] {
            assert!(matches!(public.probe, Probe::Tcp { .. }), "{public:?}");
            assert_eq!(public.overrides.interval_secs, Some(30), "{}", public.host);
        }
        assert_eq!(report.hosts[6].description, "Cloudflare HTTPS");
    }

    /// A folder of its own for one test, removed when it is dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            let dir = std::env::temp_dir().join(format!("pingtable-test-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn files_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }

    #[test]
    fn text_with_a_byte_order_mark_or_in_utf16_is_read() {
        let json = r#"{"port": 7000}"#;
        assert_eq!(decode(json.as_bytes()).unwrap(), json);
        assert_eq!(decode(&[b"\xEF\xBB\xBF".as_slice(), json.as_bytes()].concat()).unwrap(), json);
        let le: Vec<u8> = b"\xFF\xFE".iter().copied().chain(json.encode_utf16().flat_map(u16::to_le_bytes)).collect();
        assert_eq!(decode(&le).unwrap(), json);
        let be: Vec<u8> = b"\xFE\xFF".iter().copied().chain(json.encode_utf16().flat_map(u16::to_be_bytes)).collect();
        assert_eq!(decode(&be).unwrap(), json);
        assert!(decode(b"\xC3\x28").unwrap_err().contains("not UTF-8"));
    }

    #[test]
    fn a_file_with_a_byte_order_mark_loads_as_it_is() {
        let dir = Scratch::new("bom");
        let path = dir.0.join(FILE_NAME);
        let token = "0123456789abcdef0123456789abcdef";
        std::fs::write(&path, format!("\u{feff}{{\"token\": \"{token}\", \"hosts_text\": \"nas.lan\"}}")).unwrap();
        let (_, cfg) = load(Some(&path)).unwrap();
        assert_eq!((cfg.token.as_str(), cfg.hosts_text.as_str()), (token, "nas.lan"));
        assert!(cfg.notice.is_none());
        assert_eq!(files_in(&dir.0), [FILE_NAME]);
        assert!(peek(Some(&path)).is_some());
    }

    #[test]
    fn an_unreadable_file_is_kept_aside_not_overwritten() {
        let dir = Scratch::new("unreadable");
        let path = dir.0.join(FILE_NAME);
        let broken = "{\"hosts_text\": \"nas.lan\\nprinter.lan\",, \"token\": \"x\"}";
        std::fs::write(&path, broken).unwrap();
        assert!(peek(Some(&path)).is_none(), "peek changes nothing and finds nothing usable");
        assert!(problem(Some(&path)).is_some_and(|why| why.contains("cannot be read")));
        assert_eq!(files_in(&dir.0), [FILE_NAME]);

        let (_, cfg) = load(Some(&path)).unwrap();
        assert_eq!(cfg.hosts_text, default_hosts_text(DEFAULT_PORT));
        assert_eq!(cfg.token.len(), 32);
        assert!(!cfg.do_not_save);
        let files = files_in(&dir.0);
        assert_eq!(files.len(), 2, "{files:?}");
        let aside = files.iter().find(|f| f.starts_with("pingtable.json.unreadable-")).expect("the old file is kept");
        assert_eq!(std::fs::read_to_string(dir.0.join(aside)).unwrap(), broken);
        let notice = cfg.notice.expect("a notice for the interface");
        assert!(notice.contains(aside.as_str()) && notice.contains("default settings"), "{notice}");
        // The new file is a good one, and the notice is not saved in it.
        assert!(problem(Some(&path)).is_none());
        let (_, again) = load(Some(&path)).unwrap();
        assert!(again.notice.is_none());
        assert_eq!(again.token, cfg.token);
    }

    #[test]
    fn a_folder_in_the_place_of_the_file_is_left_alone() {
        let dir = Scratch::new("folder");
        let path = dir.0.join(FILE_NAME);
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("keep.txt"), "mine").unwrap();
        let why = load(Some(&path)).unwrap_err();
        assert!(why.contains("could not be opened") && why.contains("folder") && why.contains("did not start"), "{why}");
        assert_eq!(files_in(&dir.0), [FILE_NAME], "nothing renamed and nothing written beside it");
        assert_eq!(std::fs::read_to_string(path.join("keep.txt")).unwrap(), "mine");
        assert!(problem(Some(&path)).is_some_and(|why| why.contains("could not be opened")));
        assert!(peek(Some(&path)).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn a_file_that_cannot_be_opened_stops_the_start_and_is_kept() {
        use std::os::unix::fs::PermissionsExt;
        let dir = Scratch::new("closed");
        let path = dir.0.join(FILE_NAME);
        std::fs::write(&path, "{\"hosts_text\": \"nas.lan\"}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read(&path).is_ok() {
            return; // Run as root, which reads it anyway: nothing to show here.
        }
        let why = load(Some(&path)).unwrap_err();
        assert!(why.contains("could not be opened"), "{why}");
        assert_eq!(files_in(&dir.0), [FILE_NAME]);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"hosts_text\": \"nas.lan\"}");
    }

    #[cfg(unix)]
    #[test]
    fn a_broken_file_that_cannot_be_moved_aside_is_never_saved_over() {
        use std::os::unix::fs::PermissionsExt;
        let dir = Scratch::new("stuck");
        let path = dir.0.join(FILE_NAME);
        let broken = "{\"hosts_text\": \"nas.lan\",, }";
        std::fs::write(&path, broken).unwrap();
        // A folder that can be read but not changed: the file reads, and cannot be renamed.
        std::fs::set_permissions(&dir.0, std::fs::Permissions::from_mode(0o555)).unwrap();
        let stuck = std::fs::rename(&path, dir.0.join("probe")).is_err();
        let loaded = load(Some(&path));
        std::fs::set_permissions(&dir.0, std::fs::Permissions::from_mode(0o755)).unwrap();
        if !stuck {
            return; // Run as root, which can rename it anyway.
        }
        let (_, cfg) = loaded.unwrap();
        assert!(cfg.do_not_save);
        assert!(cfg.notice.as_deref().is_some_and(|n| n.contains("will not save anything over that file")), "{:?}", cfg.notice);
        // Now that the folder could be written, still nothing goes over the file.
        let refused = save(&path, &cfg).unwrap_err().to_string();
        assert!(refused.contains("leaves it as it is"), "{refused}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);
        assert_eq!(files_in(&dir.0), [FILE_NAME]);
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = Scratch::new("private");
        let path = dir.0.join(FILE_NAME);
        // A temporary file left behind by a crash, readable by everyone.
        std::fs::write(dir.0.join("pingtable.json.tmp"), "old").unwrap();
        std::fs::set_permissions(dir.0.join("pingtable.json.tmp"), std::fs::Permissions::from_mode(0o644)).unwrap();
        save(&path, &Config::default()).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        write_run_file(&path, 7521).unwrap();
        assert_eq!(std::fs::metadata(run_file(&path)).unwrap().permissions().mode() & 0o777, 0o600);
        // One that cannot even be written to is replaced too, not reused.
        std::fs::write(dir.0.join("pingtable.json.tmp"), "old").unwrap();
        std::fs::set_permissions(dir.0.join("pingtable.json.tmp"), std::fs::Permissions::from_mode(0o444)).unwrap();
        save(&path, &Config { hosts_text: "nas.lan".into(), ..Config::default() }).unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(on_disk(&path).unwrap().hosts_text, "nas.lan");
        assert_eq!(files_in(&dir.0), [FILE_NAME, "pingtable.json.run"]);
    }

    #[test]
    fn the_run_file_port_is_tried_first() {
        let dir = Scratch::new("runfile");
        let path = dir.0.join(FILE_NAME);
        let cfg = Config { port: 7520, ..Config::default() };
        assert_eq!(ports_to_try(&path, &cfg), [7520]);
        write_run_file(&path, 7529).unwrap();
        assert_eq!(run_file(&path), dir.0.join("pingtable.json.run"));
        assert_eq!(ports_to_try(&path, &cfg), [7529, 7520]);
        write_run_file(&path, 7520).unwrap();
        assert_eq!(ports_to_try(&path, &cfg), [7520]);
        std::fs::write(run_file(&path), "not json").unwrap();
        assert_eq!(ports_to_try(&path, &cfg), [7520]);
    }

    #[test]
    fn commands_locked_is_read_from_the_file() {
        let cfg: Config = serde_json::from_str(r#"{"commands_locked": true}"#).unwrap();
        assert!(cfg.commands_locked);
        assert!(!Config::default().commands_locked);
        assert!(serde_json::to_string(&Config { notice: Some("x".into()), ..Config::default() }).unwrap().contains("\"commands_locked\":false"));
        let saved = serde_json::to_string(&Config { notice: Some("x".into()), do_not_save: true, ..Config::default() }).unwrap();
        assert!(!saved.contains("notice") && !saved.contains("do_not_save"), "{saved}");
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
