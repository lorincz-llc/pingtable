use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// How a target is checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Probe {
    /// ICMP echo request.
    Icmp,
    /// TCP connect to a port. No data is sent.
    Tcp { port: u16 },
}

/// Result class of one probe.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ok,
    Timeout,
    NetUnreachable,
    HostUnreachable,
    ProtocolUnreachable,
    PortUnreachable,
    TtlExpired,
    PacketTooBig,
    Refused,
    ResolveFailed,
    BadDestination,
    GeneralFailure,
    Unsupported,
    Error,
}

impl Status {
    pub fn is_ok(self) -> bool {
        self == Status::Ok
    }

    /// Short text shown in the table and in exports.
    pub fn label(self) -> &'static str {
        match self {
            Status::Ok => "OK",
            Status::Timeout => "Timed out",
            Status::NetUnreachable => "Network unreachable",
            Status::HostUnreachable => "Host unreachable",
            Status::ProtocolUnreachable => "Protocol unreachable",
            Status::PortUnreachable => "Port unreachable",
            Status::TtlExpired => "TTL expired",
            Status::PacketTooBig => "Packet too big",
            Status::Refused => "Connection refused",
            Status::ResolveFailed => "Name not resolved",
            Status::BadDestination => "Bad destination",
            Status::GeneralFailure => "General failure",
            Status::Unsupported => "Not supported here",
            Status::Error => "Error",
        }
    }
}

/// What one probe returned.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    pub status: Status,
    /// Raw operating-system or ICMP status code, when there is one.
    pub os_code: Option<i32>,
    pub rtt_us: Option<u32>,
    pub ttl: Option<u8>,
    pub reply_addr: Option<IpAddr>,
}

impl Outcome {
    pub fn fail(status: Status) -> Self {
        Outcome { status, os_code: None, rtt_us: None, ttl: None, reply_addr: None }
    }

    pub fn fail_code(status: Status, code: i32) -> Self {
        Outcome { status, os_code: Some(code), rtt_us: None, ttl: None, reply_addr: None }
    }
}

/// One recorded probe result. Field names carry their units.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    /// Unix time in milliseconds when the probe was sent.
    pub t_ms: i64,
    pub status: Status,
    /// Round-trip time in microseconds, for successful probes.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub rtt_us: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ttl: Option<u8>,
    /// Beats this probe stands for. A host is never probed twice at once, so
    /// while a failed probe waits for its timeout the beats that come due are
    /// skipped. They count as lost with it: 1 for an ordinary probe, more for
    /// a failure that outlasted its interval.
    #[serde(skip_serializing_if = "is_one", default = "one")]
    pub beats: u16,
}

fn is_one(n: &u16) -> bool {
    *n == 1
}

fn one() -> u16 {
    1
}

/// Settings that can be set for one host or one group and win over the
/// global value. Resolution order: host, then group, then global.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Overrides {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interval_secs: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u32>,
    /// ICMP payload size in bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload_size: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dont_fragment: Option<bool>,
    /// Type-of-service byte; DSCP is its top six bits.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tos: Option<u8>,
    /// Bytes repeated to fill the ICMP payload. Zeros when unset.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<Vec<u8>>,
}

impl Overrides {
    /// Values set here win; gaps are filled from `fallback`.
    pub fn or(self, fallback: Overrides) -> Overrides {
        Overrides {
            interval_secs: self.interval_secs.or(fallback.interval_secs),
            timeout_ms: self.timeout_ms.or(fallback.timeout_ms),
            payload_size: self.payload_size.or(fallback.payload_size),
            ttl: self.ttl.or(fallback.ttl),
            dont_fragment: self.dont_fragment.or(fallback.dont_fragment),
            tos: self.tos.or(fallback.tos),
            payload: self.payload.or(fallback.payload),
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == Overrides::default()
    }

    /// True when the packet itself is customised (size, TTL, flags, marking or data).
    pub fn shapes_packet(&self) -> bool {
        self.payload_size.is_some() || self.ttl.is_some() || self.dont_fragment.is_some() || self.tos.is_some() || self.payload.is_some()
    }
}

/// A stretch of consecutive probes that all had the same status.
/// A failed run is an outage.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub status: Status,
    pub start_ms: i64,
    pub end_ms: i64,
    pub count: u32,
}

/// Global settings. Every field has a default so old config files keep loading.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Seconds between two probes of the same host, start to start.
    pub interval_secs: u32,
    /// Keep pinging. When false, every host is probed once and pinging stops.
    pub repeat: bool,
    pub timeout_ms: u32,
    /// ICMP payload size in bytes.
    pub payload_size: u16,
    /// IP time-to-live / hop limit. None leaves the system default.
    pub ttl: Option<u8>,
    pub dont_fragment: bool,
    /// Upper limit on probes in flight at once.
    pub max_concurrent: u32,
    /// Look the name up again before every ping instead of once.
    pub resolve_every_ping: bool,
    /// Fill in host names for targets given as IP addresses.
    pub reverse_dns: bool,
    /// Try IPv6 first when a name has both kinds of address.
    pub prefer_ipv6: bool,
    pub source_v4: Option<Ipv4Addr>,
    pub source_v6: Option<Ipv6Addr>,
    /// Read MAC addresses from the system neighbour table after each round.
    pub read_mac: bool,
    /// Samples kept per host.
    pub history_per_host: u32,
    /// Consecutive failures before a host counts as down.
    pub fail_trigger: u32,
    /// Consecutive successes before a down host counts as up again.
    pub recover_trigger: u32,
    /// Command run when a host goes down. `%HostName%` style variables are replaced.
    pub on_down_command: String,
    /// Command run when a host comes back up.
    pub on_up_command: String,
    /// Host list uses one "address description" pair per line.
    pub description_format: bool,
    pub cidr_skip_first: bool,
    pub cidr_skip_last: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            interval_secs: 1,
            repeat: true,
            timeout_ms: 1000,
            payload_size: 32,
            ttl: None,
            dont_fragment: false,
            max_concurrent: 256,
            resolve_every_ping: false,
            reverse_dns: true,
            prefer_ipv6: false,
            source_v4: None,
            source_v6: None,
            read_mac: true,
            history_per_host: 2000,
            fail_trigger: 1,
            recover_trigger: 1,
            on_down_command: String::new(),
            on_up_command: String::new(),
            description_format: false,
            cidr_skip_first: true,
            cidr_skip_last: true,
        }
    }
}

impl Settings {
    /// Clamp values that would make the engine misbehave.
    pub fn sanitized(mut self) -> Self {
        self.interval_secs = self.interval_secs.clamp(1, 86_400);
        self.timeout_ms = self.timeout_ms.clamp(50, 60_000);
        self.max_concurrent = self.max_concurrent.clamp(1, 1000);
        self.history_per_host = self.history_per_host.clamp(10, 100_000);
        self.fail_trigger = self.fail_trigger.max(1);
        self.recover_trigger = self.recover_trigger.max(1);
        if self.payload_size == 0 {
            self.payload_size = 32;
        }
        self
    }
}

/// Raised when a host crosses the down or up threshold.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AlertEvent {
    pub seq: u64,
    pub t_ms: i64,
    pub host_id: u32,
    pub host: String,
    pub kind: AlertKind,
    pub status: Status,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AlertKind {
    Down,
    Up,
}

/// Something the person watching should be told that is not about one host:
/// an alert command that could not be started, or was stopped for running too long.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Note {
    pub seq: u64,
    pub t_ms: i64,
    pub text: String,
}
