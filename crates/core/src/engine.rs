//! Scheduler and per-host state.
//!
//! Every host has its own cadence: it is probed every `interval` seconds,
//! measured start to start, under one global cap on probes in flight. A host
//! whose probe is still running is never probed twice at once; if a beat is
//! missed it is skipped rather than made up in a burst.

use crate::model::{AlertEvent, AlertKind, Note, Outcome, Overrides, Probe, Run, Sample, Settings, Status};
use crate::parse::{parse_host_list, ParseOptions, ParsedHost, BLOCK_EVERY_SECS};
use crate::probe::{self, ProbeParams};
use crate::{netinfo, now_ms};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

pub const SCHEMA_VERSION: u32 = 1;
const SPARK_LEN: usize = 60;
const MAX_RUNS: usize = 500;
const MAX_EVENTS: usize = 200;
const MAX_NOTES: usize = 50;
const WORKER_IDLE: Duration = Duration::from_secs(20);
const HOUSEKEEPING_EVERY: Duration = Duration::from_secs(3);
/// Regular pinging spreads the first beats of a table over this long, at most
/// `START_GAP_MAX` apart, so the hosts do not all fire in the same instant of
/// every interval from then on.
const START_SPREAD: Duration = Duration::from_secs(1);
const START_GAP_MAX: Duration = Duration::from_millis(5);
/// A host that is due and has waited this many of its own intervals for a free
/// slot, and at least this long, has a result too old to pass for current.
const STALE_AFTER_BEATS: u32 = 3;
const STALE_AFTER_MIN: Duration = Duration::from_secs(10);
/// Most beats one failed probe may stand for.
const MAX_BEATS: u128 = 3600;
/// A name is looked up again this often, and sooner while its host is failing.
const RESOLVE_REFRESH: Duration = Duration::from_secs(300);
const RESOLVE_RETRY: Duration = Duration::from_secs(30);
const RESOLVE_AFTER_FAILS: u32 = 3;
/// A reverse lookup that found no name is tried again after this long.
const RDNS_RETRY: Duration = Duration::from_secs(3600);
/// Alert commands: how many run at once, how many may wait, and how long one may run.
const COMMANDS_AT_ONCE: usize = 4;
const COMMANDS_WAITING: usize = 64;
const COMMAND_TIME_LIMIT: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------------
// Statistics
// ---------------------------------------------------------------------------

/// Running totals for one host. Derived values (average, loss %) are
/// computed when a row is built, never stored.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HostStats {
    pub sent: u64,
    pub ok: u64,
    pub failed: u64,
    /// Beats that came due while a failed probe was still waiting for its
    /// timeout. Nothing was sent for them; they count as lost in the loss figure.
    pub missed: u64,
    pub consec_failed: u32,
    pub consec_ok: u32,
    pub max_consec_failed: u32,
    pub max_consec_failed_at_ms: Option<i64>,
    pub last: Option<Sample>,
    pub last_ok_at_ms: Option<i64>,
    pub last_failed_at_ms: Option<i64>,
    pub min_rtt_us: Option<u32>,
    pub max_rtt_us: Option<u32>,
    pub sum_rtt_us: u64,
    pub timed: u64,
    /// Smoothed mean difference between consecutive round-trip times.
    pub jitter_us: f32,
    prev_rtt_us: Option<u32>,
    /// True once `fail_trigger` consecutive probes failed, until
    /// `recover_trigger` consecutive probes succeed.
    pub down: bool,
}

impl HostStats {
    /// Fold one result in. Returns an alert when the host crosses the down
    /// or up threshold.
    pub fn apply(&mut self, sample: &Sample, fail_trigger: u32, recover_trigger: u32) -> Option<AlertKind> {
        self.sent += 1;
        self.last = Some(*sample);
        if sample.status.is_ok() {
            self.ok += 1;
            self.consec_ok += 1;
            self.consec_failed = 0;
            self.last_ok_at_ms = Some(sample.t_ms);
            if let Some(rtt) = sample.rtt_us {
                self.sum_rtt_us += rtt as u64;
                self.timed += 1;
                self.min_rtt_us = Some(self.min_rtt_us.map_or(rtt, |m| m.min(rtt)));
                self.max_rtt_us = Some(self.max_rtt_us.map_or(rtt, |m| m.max(rtt)));
                if let Some(prev) = self.prev_rtt_us {
                    let diff = (rtt as f32 - prev as f32).abs();
                    // The first difference is the estimate; later ones move it by a sixteenth.
                    if self.timed == 2 {
                        self.jitter_us = diff;
                    } else {
                        self.jitter_us += (diff - self.jitter_us) / 16.0;
                    }
                }
                self.prev_rtt_us = Some(rtt);
            }
            if self.down && self.consec_ok >= recover_trigger.max(1) {
                self.down = false;
                return Some(AlertKind::Up);
            }
        } else {
            self.failed += 1;
            self.missed += sample.beats.saturating_sub(1) as u64;
            self.consec_failed += 1;
            self.consec_ok = 0;
            self.last_failed_at_ms = Some(sample.t_ms);
            if self.consec_failed >= self.max_consec_failed {
                self.max_consec_failed = self.consec_failed;
                self.max_consec_failed_at_ms = Some(sample.t_ms);
            }
            if !self.down && self.consec_failed >= fail_trigger.max(1) {
                self.down = true;
                return Some(AlertKind::Down);
            }
        }
        None
    }

    pub fn avg_rtt_us(&self) -> Option<u32> {
        (self.timed > 0).then(|| (self.sum_rtt_us / self.timed) as u32)
    }

    /// Share of beats that got no answer. A failed probe counts for every beat
    /// it covered, so an outage weighs the same whatever the timeout is.
    pub fn failed_pct(&self) -> f32 {
        let beats = self.sent + self.missed;
        if beats == 0 {
            0.0
        } else {
            (self.failed + self.missed) as f32 * 100.0 / beats as f32
        }
    }
}

// ---------------------------------------------------------------------------
// Public views
// ---------------------------------------------------------------------------

/// One table row, ready to serialize. Field names carry their units.
#[derive(Clone, Debug, Serialize)]
pub struct HostRow {
    pub id: u32,
    /// Stable identity of the check: host plus probe. Survives restarts.
    pub key: String,
    pub order: u32,
    pub host: String,
    pub ip: Option<String>,
    pub reply_ip: Option<String>,
    pub hostname: Option<String>,
    pub mac: Option<String>,
    pub description: String,
    pub group: Option<String>,
    pub tags: Vec<String>,
    pub probe: Probe,
    pub enabled: bool,
    pub interval_secs: u32,
    pub timeout_ms: u32,
    /// True when the host has no interval or timeout of its own (from its
    /// line or its group), and so follows the global setting. Not sent: the
    /// CSV export leaves those values blank, so a file read on another
    /// computer follows that computer's settings.
    #[serde(skip)]
    pub interval_is_global: bool,
    #[serde(skip)]
    pub timeout_is_global: bool,
    /// The host's own packet options as host-list words (`size=1400 ttl=8`),
    /// for the CSV export. Empty when it has none. Not sent: `packet` says the
    /// same for people.
    #[serde(skip)]
    pub options: String,
    /// The line of the host list this host came from, counting from 1.
    pub line: u32,
    /// True when that line defines only this host, so removing the line removes just it.
    pub removable: bool,
    /// Set when this host's pings are customised, for example `1400 bytes · DF · TTL 8`.
    pub packet: Option<String>,
    /// `up`, `down`, `failing` (failed but under the down threshold),
    /// `recovering`, `pending` (no result yet) or `muted` (not pinged).
    pub state: &'static str,
    /// True when the state is old news: the host is due, and has waited a
    /// long while for a free slot because the table asks for more pings at
    /// once than `max_concurrent` allows.
    pub stale: bool,
    pub sent: u64,
    pub ok: u64,
    pub failed: u64,
    /// Beats skipped while a failed ping was still waiting for its timeout.
    /// They count as lost in `failed_pct`.
    pub missed: u64,
    pub failed_pct: f32,
    pub recent_loss_pct: f32,
    pub consec_failed: u32,
    pub max_consec_failed: u32,
    pub max_consec_failed_at_ms: Option<i64>,
    pub last_status: Option<Status>,
    pub last_status_label: Option<&'static str>,
    pub last_rtt_us: Option<u32>,
    pub last_ttl: Option<u8>,
    pub last_at_ms: Option<i64>,
    pub last_ok_at_ms: Option<i64>,
    pub last_failed_at_ms: Option<i64>,
    pub avg_rtt_us: Option<u32>,
    pub min_rtt_us: Option<u32>,
    pub max_rtt_us: Option<u32>,
    pub jitter_us: Option<u32>,
    /// Most recent round-trip times, oldest first. `null` marks a failure.
    pub spark: Vec<Option<u32>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Counts {
    pub total: u32,
    pub up: u32,
    pub down: u32,
    pub pending: u32,
    pub muted: u32,
}

/// The table, or the part of it that changed since a sequence number.
#[derive(Clone, Debug, Serialize)]
pub struct Snapshot {
    pub schema_version: u32,
    pub seq: u64,
    pub now_ms: i64,
    pub running: bool,
    /// Probes in flight right now.
    pub active: u32,
    /// Hosts that are due and waiting for a free slot. Above zero for long
    /// means the table asks for more than `max_concurrent` allows.
    pub waiting: u32,
    /// When regular pinging was last started, for the stopwatch. None while stopped.
    pub running_since_ms: Option<i64>,
    /// Set while a measurement runs: when it ends and how many hosts it covers.
    pub measure_ends_at_ms: Option<i64>,
    pub measure_hosts: u32,
    /// True when `hosts` is the whole table and replaces what the caller has.
    pub full: bool,
    pub counts: Counts,
    pub hosts: Vec<HostRow>,
    pub events: Vec<AlertEvent>,
    /// Things to tell the person watching that are not about one host.
    pub notes: Vec<Note>,
    pub warnings: Vec<String>,
}

/// Loss and average over the most recent stretch of a host's history.
#[derive(Clone, Debug, Serialize)]
pub struct WindowStats {
    pub label: &'static str,
    /// Beats in the window, and how many of them got no answer.
    pub sent: u32,
    pub failed: u32,
    pub loss_pct: f32,
    pub avg_rtt_us: Option<u32>,
    /// How much of the window the kept history really covers. Less than the
    /// window's length when the host has not been watched that long, or when
    /// the history holds fewer samples than the window needs.
    pub covered_ms: i64,
}

/// Everything known about one host, for the detail view.
#[derive(Clone, Debug, Serialize)]
pub struct HostDetail {
    pub schema_version: u32,
    pub row: HostRow,
    pub history: Vec<Sample>,
    /// Stretches of identical status, oldest first. Failed runs are outages.
    pub runs: Vec<Run>,
    pub p50_rtt_us: Option<u32>,
    pub p95_rtt_us: Option<u32>,
    pub p99_rtt_us: Option<u32>,
    pub windows: Vec<WindowStats>,
}

/// One host's result in a measurement.
#[derive(Clone, Debug, Serialize)]
pub struct MeasureHost {
    pub id: u32,
    pub host: String,
    pub description: String,
    /// Pings really sent, and how many of them got no answer.
    pub sent: u32,
    pub lost: u32,
    /// Beats the measurement called for: `sent` plus the beats skipped while
    /// a failed ping was still waiting for its timeout.
    pub expected: u32,
    /// Share of `expected` that got no answer.
    pub loss_pct: f32,
    pub min_rtt_us: Option<u32>,
    pub avg_rtt_us: Option<u32>,
    pub median_rtt_us: Option<u32>,
    pub p95_rtt_us: Option<u32>,
    pub max_rtt_us: Option<u32>,
    /// Mean difference between consecutive round-trip times.
    pub jitter_us: Option<u32>,
}

/// A timed test: chosen hosts pinged at a chosen rate for a set time.
#[derive(Clone, Debug, Serialize)]
pub struct MeasureReport {
    pub schema_version: u32,
    pub id: u64,
    pub running: bool,
    pub started_at_ms: i64,
    pub ends_at_ms: i64,
    /// When it actually ended; equal to `ends_at_ms` unless it was stopped early.
    pub ended_at_ms: Option<i64>,
    pub duration_secs: u32,
    pub per_second: u32,
    pub hosts: Vec<MeasureHost>,
}

#[derive(Clone)]
struct MeasureAcc {
    host: String,
    description: String,
    sent: u32,
    lost: u32,
    /// Beats skipped while a failed ping was outstanding.
    missed: u32,
    rtts: Vec<u32>,
}

#[derive(Clone)]
struct Measurement {
    id: u64,
    started_at_ms: i64,
    ends_at_ms: i64,
    ends: Instant,
    ended_at_ms: Option<i64>,
    duration_secs: u32,
    per_second: u32,
    order: Vec<u32>,
    hosts: HashMap<u32, MeasureAcc>,
}

impl Measurement {
    fn running(&self) -> bool {
        self.ended_at_ms.is_none()
    }

    fn report(&self) -> MeasureReport {
        let hosts = self
            .order
            .iter()
            .filter_map(|id| self.hosts.get(id).map(|acc| (*id, acc)))
            .map(|(id, acc)| {
                let mut sorted = acc.rtts.clone();
                sorted.sort_unstable();
                let pct = |p: f64| -> Option<u32> {
                    if sorted.is_empty() {
                        return None;
                    }
                    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
                    Some(sorted[rank.clamp(1, sorted.len()) - 1])
                };
                let jitter = (acc.rtts.len() > 1).then(|| {
                    let total: u64 = acc.rtts.windows(2).map(|w| w[0].abs_diff(w[1]) as u64).sum();
                    (total / (acc.rtts.len() as u64 - 1)) as u32
                });
                let expected = acc.sent + acc.missed;
                MeasureHost {
                    id,
                    host: acc.host.clone(),
                    description: acc.description.clone(),
                    sent: acc.sent,
                    lost: acc.lost,
                    expected,
                    loss_pct: if expected == 0 { 0.0 } else { (acc.lost + acc.missed) as f32 * 100.0 / expected as f32 },
                    min_rtt_us: sorted.first().copied(),
                    avg_rtt_us: (!sorted.is_empty()).then(|| (sorted.iter().map(|v| *v as u64).sum::<u64>() / sorted.len() as u64) as u32),
                    median_rtt_us: pct(50.0),
                    p95_rtt_us: pct(95.0),
                    max_rtt_us: sorted.last().copied(),
                    jitter_us: jitter,
                }
            })
            .collect();
        MeasureReport {
            schema_version: SCHEMA_VERSION,
            id: self.id,
            running: self.running(),
            started_at_ms: self.started_at_ms,
            ends_at_ms: self.ends_at_ms,
            ended_at_ms: self.ended_at_ms,
            duration_secs: self.duration_secs,
            per_second: self.per_second,
            hosts,
        }
    }
}

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

struct HostState {
    id: u32,
    key: String,
    order: u32,
    host: String,
    literal: Option<IpAddr>,
    probe: Probe,
    description: String,
    group: Option<String>,
    tags: Vec<String>,
    overrides: Overrides,
    enabled: bool,
    resolved: Option<IpAddr>,
    /// When the name was last looked up, whether or not that lookup worked.
    resolved_at: Option<Instant>,
    reply_ip: Option<IpAddr>,
    hostname: Option<String>,
    rdns_tried_at: Option<Instant>,
    mac: Option<String>,
    stats: HostStats,
    history: VecDeque<Sample>,
    runs: VecDeque<Run>,
    seq: u64,
    next_due: Option<Instant>,
    /// One extra ping asked for by hand. It does not move the regular beat.
    asap: Option<Instant>,
    in_flight: bool,
    /// Due for a long while and still waiting for a free slot: its last
    /// result is old and should not be read as current.
    stale: bool,
    /// Set while a measurement is pinging this host at its own rate.
    measure_interval: Option<Duration>,
    line: u32,
    alone_on_line: bool,
    /// One address of a block or range: pinged less often by default.
    from_block: bool,
}

impl HostState {
    fn new(id: u32, key: String, order: u32, parsed: &ParsedHost) -> Self {
        HostState {
            id,
            key,
            order,
            host: parsed.host.clone(),
            literal: parsed.addr,
            probe: parsed.probe,
            description: parsed.description.clone(),
            group: parsed.group.clone(),
            tags: parsed.tags.clone(),
            overrides: parsed.overrides.clone(),
            enabled: true,
            resolved: None,
            resolved_at: None,
            reply_ip: None,
            hostname: None,
            rdns_tried_at: None,
            mac: None,
            stats: HostStats::default(),
            history: VecDeque::new(),
            runs: VecDeque::new(),
            seq: 0,
            next_due: None,
            asap: None,
            in_flight: false,
            stale: false,
            measure_interval: None,
            line: parsed.line as u32,
            alone_on_line: true,
            from_block: parsed.from_block,
        }
    }

    fn ip(&self) -> Option<IpAddr> {
        self.literal.or(self.resolved)
    }

    fn interval(&self, settings: &Settings) -> Duration {
        let default = if self.from_block { settings.interval_secs.max(BLOCK_EVERY_SECS) } else { settings.interval_secs };
        Duration::from_secs(self.overrides.interval_secs.unwrap_or(default).max(1) as u64)
    }

    fn timeout_ms(&self, settings: &Settings) -> u32 {
        self.overrides.timeout_ms.unwrap_or(settings.timeout_ms)
    }

    fn packet_note(&self) -> Option<String> {
        let o = &self.overrides;
        if !o.shapes_packet() {
            return None;
        }
        let mut parts = Vec::new();
        if let Some(size) = o.payload_size {
            parts.push(format!("{size} bytes"));
        }
        if o.dont_fragment == Some(true) {
            parts.push("DF".to_string());
        }
        if let Some(ttl) = o.ttl {
            parts.push(format!("TTL {ttl}"));
        }
        if let Some(tos) = o.tos {
            parts.push(if tos % 4 == 0 { format!("DSCP {}", tos >> 2) } else { format!("ToS {tos}") });
        }
        if o.payload.is_some() {
            parts.push("custom data".to_string());
        }
        Some(parts.join(" · "))
    }

    /// The packet options as the host list writes them, so that they read back the same.
    fn option_words(&self) -> String {
        let o = &self.overrides;
        if !o.shapes_packet() {
            return String::new();
        }
        let mut words = Vec::new();
        if let Some(size) = o.payload_size {
            words.push(format!("size={size}"));
        }
        if let Some(ttl) = o.ttl {
            words.push(format!("ttl={ttl}"));
        }
        if let Some(df) = o.dont_fragment {
            words.push(format!("df={}", if df { "on" } else { "off" }));
        }
        if let Some(tos) = o.tos {
            words.push(if tos % 4 == 0 { format!("dscp={}", tos >> 2) } else { format!("tos={tos}") });
        }
        if let Some(data) = &o.payload {
            // As hex, whatever it was typed as: no spaces, commas or quotes to go wrong.
            words.push(format!("data=0x{}", data.iter().map(|b| format!("{b:02X}")).collect::<String>()));
        }
        words.join(" ")
    }

    /// Time between probes: the measurement's rate while one runs, else the host's own interval.
    fn cadence(&self, settings: &Settings) -> Duration {
        self.measure_interval.unwrap_or_else(|| self.interval(settings))
    }

    fn record(&mut self, sample: Sample, history_cap: usize) {
        while self.history.len() >= history_cap.max(1) {
            self.history.pop_front();
        }
        self.history.push_back(sample);
        match self.runs.back_mut() {
            Some(run) if run.status == sample.status => {
                run.end_ms = sample.t_ms;
                run.count += 1;
            }
            _ => {
                if self.runs.len() >= MAX_RUNS {
                    self.runs.pop_front();
                }
                self.runs.push_back(Run { status: sample.status, start_ms: sample.t_ms, end_ms: sample.t_ms, count: 1 });
            }
        }
    }

    fn state_name(&self) -> &'static str {
        if !self.enabled {
            return "muted";
        }
        match self.stats.last {
            None => "pending",
            Some(last) => match (last.status.is_ok(), self.stats.down) {
                (true, false) => "up",
                (true, true) => "recovering",
                (false, true) => "down",
                (false, false) => "failing",
            },
        }
    }

    fn row(&self, settings: &Settings) -> HostRow {
        let stats = &self.stats;
        let recent: Vec<&Sample> = self.history.iter().rev().take(SPARK_LEN).collect();
        let (mut beats, mut lost) = (0u32, 0u32);
        for s in &recent {
            beats += s.beats as u32;
            if !s.status.is_ok() {
                lost += s.beats as u32;
            }
        }
        let recent_loss_pct = if beats == 0 { 0.0 } else { lost as f32 * 100.0 / beats as f32 };
        let spark = recent.iter().rev().map(|s| if s.status.is_ok() { s.rtt_us } else { None }).collect();
        let interval_secs = self.interval(settings).as_secs() as u32;
        let timeout_ms = self.timeout_ms(settings);
        HostRow {
            id: self.id,
            key: self.key.clone(),
            order: self.order,
            host: self.host.clone(),
            ip: self.ip().map(|ip| ip.to_string()),
            reply_ip: self.reply_ip.map(|ip| ip.to_string()),
            hostname: self.hostname.clone(),
            mac: self.mac.clone(),
            description: self.description.clone(),
            group: self.group.clone(),
            tags: self.tags.clone(),
            probe: self.probe,
            enabled: self.enabled,
            interval_secs,
            timeout_ms,
            // Whether the value is the host's own, not whether it happens to
            // equal the global one: an own `every=1s` stays its own. A host
            // from a block has the block's slower default, which is not the
            // global setting either.
            interval_is_global: self.overrides.interval_secs.is_none() && !self.from_block,
            timeout_is_global: self.overrides.timeout_ms.is_none(),
            options: self.option_words(),
            line: self.line,
            removable: self.alone_on_line,
            packet: self.packet_note(),
            state: self.state_name(),
            stale: self.stale && self.enabled,
            sent: stats.sent,
            ok: stats.ok,
            failed: stats.failed,
            missed: stats.missed,
            failed_pct: stats.failed_pct(),
            recent_loss_pct,
            consec_failed: stats.consec_failed,
            max_consec_failed: stats.max_consec_failed,
            max_consec_failed_at_ms: stats.max_consec_failed_at_ms,
            last_status: stats.last.map(|s| s.status),
            last_status_label: stats.last.map(|s| s.status.label()),
            last_rtt_us: stats.last.and_then(|s| s.rtt_us),
            last_ttl: stats.last.and_then(|s| s.ttl),
            last_at_ms: stats.last.map(|s| s.t_ms),
            last_ok_at_ms: stats.last_ok_at_ms,
            last_failed_at_ms: stats.last_failed_at_ms,
            avg_rtt_us: stats.avg_rtt_us(),
            min_rtt_us: stats.min_rtt_us,
            max_rtt_us: stats.max_rtt_us,
            jitter_us: (stats.timed > 1).then_some(stats.jitter_us.round() as u32),
            spark,
        }
    }
}

struct State {
    settings: Settings,
    hosts_text: String,
    hosts: Vec<HostState>,
    index: HashMap<u32, usize>,
    next_id: u32,
    seq: u64,
    /// Sequence number at which the host list last changed shape.
    list_seq: u64,
    running: bool,
    /// One pass over every host was requested while not running.
    single_pass: bool,
    shutdown: bool,
    active: u32,
    /// Hosts that were due and found no free slot the last time the dispatcher looked.
    waiting: u32,
    events: VecDeque<AlertEvent>,
    notes: VecDeque<Note>,
    warnings: Vec<String>,
    /// When regular pinging was last started; None while stopped.
    running_since_ms: Option<i64>,
    /// The current or most recent measurement.
    measure: Option<Measurement>,
    next_measure_id: u64,
}

impl State {
    fn bump(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    fn reindex(&mut self) {
        self.index = self.hosts.iter().enumerate().map(|(i, h)| (h.id, i)).collect();
    }

    fn counts(&self) -> Counts {
        let mut c = Counts { total: self.hosts.len() as u32, ..Counts::default() };
        for h in &self.hosts {
            match h.state_name() {
                "muted" => c.muted += 1,
                "pending" => c.pending += 1,
                "down" | "failing" => c.down += 1,
                _ => c.up += 1,
            }
        }
        c
    }

    /// Record something for the person watching. Oldest notes fall off.
    fn note(&mut self, text: String) {
        let seq = self.bump();
        if self.notes.len() >= MAX_NOTES {
            self.notes.pop_front();
        }
        self.notes.push_back(Note { seq, t_ms: now_ms(), text });
    }

    fn snapshot(&self, since: u64) -> Snapshot {
        let full = since < self.list_seq || since > self.seq;
        Snapshot {
            schema_version: SCHEMA_VERSION,
            seq: self.seq,
            now_ms: now_ms(),
            running: self.running || self.single_pass,
            active: self.active,
            waiting: self.waiting,
            running_since_ms: self.running_since_ms,
            measure_ends_at_ms: self.measure.as_ref().filter(|m| m.running()).map(|m| m.ends_at_ms),
            measure_hosts: self.measure.as_ref().filter(|m| m.running()).map_or(0, |m| m.order.len() as u32),
            full,
            counts: self.counts(),
            hosts: self.hosts.iter().filter(|h| full || h.seq > since).map(|h| h.row(&self.settings)).collect(),
            events: self.events.iter().filter(|e| !full && e.seq > since).cloned().collect(),
            notes: self.notes.iter().filter(|n| !full && n.seq > since).cloned().collect(),
            warnings: if full { self.warnings.clone() } else { Vec::new() },
        }
    }
}

#[derive(Clone)]
struct JobCommon {
    payload_size: u16,
    ttl: Option<u8>,
    dont_fragment: bool,
    source_v4: Option<std::net::Ipv4Addr>,
    source_v6: Option<std::net::Ipv6Addr>,
    prefer_ipv6: bool,
}

struct Job {
    id: u32,
    host: String,
    /// The address to probe. None means the name is to be looked up first.
    addr: Option<IpAddr>,
    /// The address the name had last time, used when that lookup fails.
    fallback: Option<IpAddr>,
    probe: Probe,
    timeout_ms: u32,
    payload_size: u16,
    ttl: Option<u8>,
    dont_fragment: bool,
    tos: u8,
    payload: Option<Vec<u8>>,
    common: Arc<JobCommon>,
}

struct PoolQueue {
    jobs: VecDeque<Job>,
    /// Probe threads alive.
    workers: usize,
    /// Probe threads that are running a job right now.
    busy: usize,
    shutdown: bool,
}

/// Runs one alert command, stopping it after the time limit it is given.
type CommandRunner = Arc<dyn Fn(&str, &CommandEnv, Duration) -> Result<(), String> + Send + Sync>;

/// Alert commands waiting their turn.
struct CommandQueue {
    waiting: VecDeque<(String, CommandEnv)>,
    /// Threads that are running a command right now.
    running: usize,
    /// Commands left out because too many were already waiting.
    skipped: u32,
    /// How many run at once, how many may wait, and how long one may run.
    /// Always the constants above except in tests, which make them small.
    at_once: usize,
    most_waiting: usize,
    time_limit: Duration,
    /// What runs a command: the system shell, or a stand-in in tests.
    run: CommandRunner,
}

impl Default for CommandQueue {
    fn default() -> Self {
        CommandQueue {
            waiting: VecDeque::new(),
            running: 0,
            skipped: 0,
            at_once: COMMANDS_AT_ONCE,
            most_waiting: COMMANDS_WAITING,
            time_limit: COMMAND_TIME_LIMIT,
            run: Arc::new(run_command),
        }
    }
}

struct Inner {
    state: Mutex<State>,
    /// Wakes the dispatcher: something became due or a slot freed up.
    sched_cv: Condvar,
    /// Wakes observers waiting for the sequence number to move.
    change_cv: Condvar,
    pool: Mutex<PoolQueue>,
    pool_cv: Condvar,
    commands: Mutex<CommandQueue>,
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock_pool(&self) -> MutexGuard<'_, PoolQueue> {
        self.pool.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock_commands(&self) -> MutexGuard<'_, CommandQueue> {
        self.commands.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Tell the person watching something, and wake whoever is listening.
    fn tell(&self, text: String) {
        self.lock().note(text);
        self.change_cv.notify_all();
    }
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

/// Owns the host table and the threads that probe it. Cheap to clone.
#[derive(Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}

impl Engine {
    pub fn new(settings: Settings, hosts_text: &str) -> Engine {
        let inner = Arc::new(Inner {
            state: Mutex::new(State {
                settings: Settings::default(),
                hosts_text: String::new(),
                hosts: Vec::new(),
                index: HashMap::new(),
                next_id: 1,
                seq: 0,
                list_seq: 0,
                running: false,
                single_pass: false,
                shutdown: false,
                active: 0,
                waiting: 0,
                events: VecDeque::new(),
                notes: VecDeque::new(),
                warnings: Vec::new(),
                running_since_ms: None,
                measure: None,
                next_measure_id: 1,
            }),
            sched_cv: Condvar::new(),
            change_cv: Condvar::new(),
            pool: Mutex::new(PoolQueue { jobs: VecDeque::new(), workers: 0, busy: 0, shutdown: false }),
            pool_cv: Condvar::new(),
            commands: Mutex::new(CommandQueue::default()),
        });
        let engine = Engine { inner };
        engine.apply_config(settings, hosts_text, false);
        let for_dispatch = engine.inner.clone();
        thread::Builder::new()
            .name("pingtable-dispatch".into())
            .spawn(move || keep_running("dispatcher", &for_dispatch, dispatcher))
            .expect("start dispatcher thread");
        let for_housekeeping = engine.inner.clone();
        thread::Builder::new()
            .name("pingtable-housekeeping".into())
            .spawn(move || keep_running("housekeeper", &for_housekeeping, housekeeper))
            .expect("start housekeeping thread");
        engine
    }

    /// Current settings and the host list text they were built from.
    pub fn config(&self) -> (Settings, String) {
        let st = self.inner.lock();
        (st.settings.clone(), st.hosts_text.clone())
    }

    /// Change only the alert commands, without reading the host list again.
    pub fn set_alert_commands(&self, on_down: &str, on_up: &str) {
        let mut st = self.inner.lock();
        st.settings.on_down_command = on_down.to_string();
        st.settings.on_up_command = on_up.to_string();
    }

    /// Replace settings and host list. With `keep_stats`, hosts that are
    /// still in the list keep their counters and history. Returns the
    /// parser's warnings.
    pub fn apply_config(&self, settings: Settings, hosts_text: &str, keep_stats: bool) -> Vec<String> {
        let settings = settings.sanitized();
        let report = parse_host_list(
            hosts_text,
            &ParseOptions {
                description_format: settings.description_format,
                cidr_skip_first: settings.cidr_skip_first,
                cidr_skip_last: settings.cidr_skip_last,
            },
        );

        let mut st = self.inner.lock();
        let mut old: HashMap<String, HostState> = if keep_stats {
            std::mem::take(&mut st.hosts).into_iter().map(|h| (h.key.clone(), h)).collect()
        } else {
            st.hosts.clear();
            HashMap::new()
        };
        let now = Instant::now();
        let running = st.running;
        let mut seen: HashMap<String, u32> = HashMap::new();
        let mut hosts = Vec::with_capacity(report.hosts.len());
        let mut fresh_at: Vec<usize> = Vec::new();
        for (i, parsed) in report.hosts.iter().enumerate() {
            let base = make_key(&parsed.host, parsed.probe);
            let n = seen.entry(base.clone()).or_insert(0);
            *n += 1;
            let key = if *n == 1 { base } else { format!("{base}#{n}") };
            let order = i as u32 + 1;
            let host = match old.remove(&key) {
                Some(mut kept) => {
                    kept.order = order;
                    kept.description = parsed.description.clone();
                    kept.group = parsed.group.clone();
                    kept.tags = parsed.tags.clone();
                    kept.overrides = parsed.overrides.clone();
                    kept.from_block = parsed.from_block;
                    kept.line = parsed.line as u32;
                    kept
                }
                None => {
                    let id = st.next_id;
                    st.next_id += 1;
                    fresh_at.push(hosts.len());
                    HostState::new(id, key, order, parsed)
                }
            };
            hosts.push(host);
        }
        // New hosts join a running table spread out, as when pinging starts,
        // not all in the same instant of every interval.
        if running {
            let gap = start_gap(fresh_at.len(), settings.repeat);
            for (place, &i) in fresh_at.iter().enumerate() {
                hosts[i].next_due = Some(now + gap * place as u32);
            }
        }
        // A line that expands to several hosts (a range, a block, or several
        // names in the loose form) cannot be removed one host at a time.
        let mut per_line: HashMap<u32, u32> = HashMap::new();
        for h in &hosts {
            *per_line.entry(h.line).or_insert(0) += 1;
        }
        for h in hosts.iter_mut() {
            h.alone_on_line = per_line[&h.line] == 1;
        }
        st.hosts = hosts;
        st.reindex();
        st.settings = settings;
        st.hosts_text = hosts_text.to_string();
        st.warnings = report.warnings.clone();
        st.list_seq = st.bump();
        drop(st);
        self.inner.sched_cv.notify_all();
        self.inner.change_cv.notify_all();
        report.warnings
    }

    /// Begin (or resume) pinging every enabled host.
    pub fn start(&self) {
        let mut st = self.inner.lock();
        if !st.running {
            st.running_since_ms = Some(now_ms());
        }
        st.running = true;
        st.single_pass = false;
        let now = Instant::now();
        let fresh = st.hosts.iter().filter(|h| h.enabled && h.next_due.is_none()).count();
        let gap = start_gap(fresh, st.settings.repeat);
        for (place, h) in st.hosts.iter_mut().filter(|h| h.enabled && h.next_due.is_none()).enumerate() {
            h.next_due = Some(now + gap * place as u32);
        }
        st.bump();
        drop(st);
        self.inner.sched_cv.notify_all();
        self.inner.change_cv.notify_all();
    }

    /// Stop everything: regular pinging and any measurement. Probes already in
    /// flight still report.
    pub fn stop(&self) {
        let mut st = self.inner.lock();
        // Stop means stop: a running measurement ends too, and keeps what it collected.
        end_measurement(&mut st);
        st.running = false;
        st.single_pass = false;
        st.running_since_ms = None;
        st.waiting = 0;
        for h in st.hosts.iter_mut() {
            h.next_due = None;
            h.asap = None;
        }
        st.bump();
        drop(st);
        self.inner.sched_cv.notify_all();
        self.inner.change_cv.notify_all();
    }

    /// Probe every enabled host right away, whether or not pinging is on.
    pub fn ping_now(&self) {
        let mut st = self.inner.lock();
        let now = Instant::now();
        if st.running {
            // One extra ping each. The regular beats stay where they are, so
            // the hosts are not pulled into step with one another.
            for h in st.hosts.iter_mut().filter(|h| h.enabled) {
                h.asap = Some(now);
            }
        } else {
            st.single_pass = true;
            for h in st.hosts.iter_mut().filter(|h| h.enabled) {
                h.next_due = Some(now);
            }
        }
        st.bump();
        drop(st);
        self.inner.sched_cv.notify_all();
        self.inner.change_cv.notify_all();
    }

    /// Clear every counter and all history. The host list stays.
    pub fn reset(&self) {
        let mut st = self.inner.lock();
        for h in st.hosts.iter_mut() {
            h.stats = HostStats::default();
            h.history.clear();
            h.runs.clear();
            h.stale = false;
        }
        st.events.clear();
        st.list_seq = st.bump();
        drop(st);
        self.inner.change_cv.notify_all();
    }

    /// Mute (`enabled = false`) or unmute one host. A muted host is not pinged
    /// and collects nothing. Returns false when the id is unknown.
    pub fn set_enabled(&self, id: u32, enabled: bool) -> bool {
        let mut st = self.inner.lock();
        let Some(&idx) = st.index.get(&id) else { return false };
        let seq = st.bump();
        let running = st.running;
        let h = &mut st.hosts[idx];
        h.enabled = enabled;
        h.next_due = (enabled && running).then(Instant::now);
        h.seq = seq;
        drop(st);
        self.inner.sched_cv.notify_all();
        self.inner.change_cv.notify_all();
        true
    }

    /// Mute or unmute several hosts at once. Returns how many changed.
    pub fn set_enabled_many(&self, ids: &[u32], enabled: bool) -> usize {
        let mut st = self.inner.lock();
        let seq = st.bump();
        let running = st.running;
        let now = Instant::now();
        // Hosts unmuted together while pinging join spread out, as when pinging
        // starts, not all in the same instant.
        let joining = if enabled && running { st.hosts.iter().filter(|h| !h.enabled && ids.contains(&h.id)).count() } else { 0 };
        let gap = start_gap(joining, st.settings.repeat);
        let mut changed = 0;
        for h in st.hosts.iter_mut().filter(|h| h.enabled != enabled && ids.contains(&h.id)) {
            h.enabled = enabled;
            h.next_due = (enabled && running).then(|| now + gap * changed as u32);
            h.seq = seq;
            changed += 1;
        }
        drop(st);
        self.inner.sched_cv.notify_all();
        self.inner.change_cv.notify_all();
        changed
    }

    /// Ping only the listed hosts: they are unmuted, every other host is
    /// muted, and pinging starts if it was stopped. Returns how many hosts
    /// are now being pinged.
    pub fn solo(&self, ids: &[u32]) -> usize {
        let mut st = self.inner.lock();
        let seq = st.bump();
        if !st.running {
            st.running_since_ms = Some(now_ms());
        }
        st.running = true;
        st.single_pass = false;
        let now = Instant::now();
        // The hosts that start now are spread out, as start() spreads them.
        let fresh = st.hosts.iter().filter(|h| h.next_due.is_none() && ids.contains(&h.id)).count();
        let gap = start_gap(fresh, st.settings.repeat);
        let mut place = 0u32;
        let mut pinging = 0;
        for h in st.hosts.iter_mut() {
            let wanted = ids.contains(&h.id);
            if wanted {
                pinging += 1;
            }
            if h.enabled != wanted {
                h.enabled = wanted;
                h.seq = seq;
            }
            if !wanted {
                h.next_due = None;
            } else if h.next_due.is_none() {
                h.next_due = Some(now + gap * place);
                place += 1;
            }
        }
        drop(st);
        self.inner.sched_cv.notify_all();
        self.inner.change_cv.notify_all();
        pinging
    }

    /// Unmute every host.
    pub fn unmute_all(&self) {
        let ids: Vec<u32> = self.inner.lock().hosts.iter().filter(|h| !h.enabled).map(|h| h.id).collect();
        self.set_enabled_many(&ids, true);
    }

    /// Keys of the hosts that are muted, for saving.
    pub fn muted_keys(&self) -> Vec<String> {
        self.inner.lock().hosts.iter().filter(|h| !h.enabled).map(|h| h.key.clone()).collect()
    }

    /// Mute every host whose key is listed. Others are left as they are.
    pub fn mute_keys(&self, keys: &[String]) {
        if keys.is_empty() {
            return;
        }
        let mut st = self.inner.lock();
        let seq = st.bump();
        for h in st.hosts.iter_mut().filter(|h| h.enabled && keys.contains(&h.key)) {
            h.enabled = false;
            h.next_due = None;
            h.seq = seq;
        }
        drop(st);
        self.inner.change_cv.notify_all();
    }

    /// Probe one host right away. Returns false when the id is unknown or the
    /// host is muted.
    pub fn ping_host(&self, id: u32) -> bool {
        let mut st = self.inner.lock();
        let Some(&idx) = st.index.get(&id) else { return false };
        if !st.hosts[idx].enabled {
            return false;
        }
        if st.running {
            st.hosts[idx].asap = Some(Instant::now());
        } else {
            st.single_pass = true;
            st.hosts[idx].next_due = Some(Instant::now());
        }
        st.bump();
        drop(st);
        self.inner.sched_cv.notify_all();
        self.inner.change_cv.notify_all();
        true
    }

    /// Clear one host's counters and history. Returns false when the id is unknown.
    pub fn reset_host(&self, id: u32) -> bool {
        let mut st = self.inner.lock();
        let Some(&idx) = st.index.get(&id) else { return false };
        let seq = st.bump();
        let h = &mut st.hosts[idx];
        h.stats = HostStats::default();
        h.history.clear();
        h.runs.clear();
        h.stale = false;
        h.seq = seq;
        drop(st);
        self.inner.change_cv.notify_all();
        true
    }

    /// Start a timed measurement: ping the listed hosts `per_second` times a
    /// second for `duration_secs`, whether or not regular pinging is on.
    /// Muted hosts are left out. Returns the measurement id, or None when no
    /// listed host can be pinged. A measurement already running is replaced.
    pub fn measure_start(&self, ids: &[u32], duration_secs: u32, per_second: u32) -> Option<u64> {
        let duration_secs = duration_secs.clamp(1, 3600);
        let per_second = per_second.clamp(1, 10);
        let interval = Duration::from_millis(1000 / per_second as u64);
        let mut st = self.inner.lock();
        end_measurement(&mut st);
        let now = Instant::now();
        let mut order = Vec::new();
        let mut hosts = HashMap::new();
        for h in st.hosts.iter_mut().filter(|h| h.enabled && ids.contains(&h.id)) {
            h.measure_interval = Some(interval);
            h.next_due = Some(now);
            order.push(h.id);
            hosts.insert(h.id, MeasureAcc { host: h.host.clone(), description: h.description.clone(), sent: 0, lost: 0, missed: 0, rtts: Vec::new() });
        }
        if order.is_empty() {
            return None;
        }
        let id = st.next_measure_id;
        st.next_measure_id += 1;
        let started_at_ms = now_ms();
        st.measure = Some(Measurement {
            id,
            started_at_ms,
            ends_at_ms: started_at_ms + duration_secs as i64 * 1000,
            ends: now + Duration::from_secs(duration_secs as u64),
            ended_at_ms: None,
            duration_secs,
            per_second,
            order,
            hosts,
        });
        st.bump();
        drop(st);
        self.inner.sched_cv.notify_all();
        self.inner.change_cv.notify_all();
        Some(id)
    }

    /// End the running measurement now. Its results so far are kept.
    pub fn measure_stop(&self) {
        let mut st = self.inner.lock();
        end_measurement(&mut st);
        st.bump();
        drop(st);
        self.inner.sched_cv.notify_all();
        self.inner.change_cv.notify_all();
    }

    /// The current or most recent measurement.
    pub fn measure_report(&self) -> Option<MeasureReport> {
        // Copy under the lock, sort outside it: a long measurement of many
        // hosts holds millions of samples, and probes must not wait on that.
        let copy = self.inner.lock().measure.clone();
        copy.as_ref().map(Measurement::report)
    }

    /// Lines of the host list that can be deleted to remove exactly these hosts.
    /// Hosts that share their line with others are left out.
    pub fn removable_lines(&self, ids: &[u32]) -> Vec<usize> {
        let st = self.inner.lock();
        st.hosts.iter().filter(|h| h.alone_on_line && ids.contains(&h.id)).map(|h| h.line as usize).collect()
    }

    /// How many hosts are in the table.
    pub fn host_count(&self) -> usize {
        self.inner.lock().hosts.len()
    }

    /// Current sequence number. It grows with every change.
    pub fn seq(&self) -> u64 {
        self.inner.lock().seq
    }

    /// Rows that changed after `since`. Pass 0 for the whole table.
    pub fn snapshot(&self, since: u64) -> Snapshot {
        self.inner.lock().snapshot(since)
    }

    /// Block until something changes after `since`, or `timeout` passes.
    pub fn wait_change(&self, since: u64, timeout: Duration) -> u64 {
        let deadline = Instant::now() + timeout;
        let mut st = self.inner.lock();
        while st.seq <= since && !st.shutdown {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            st = self.inner.change_cv.wait_timeout(st, left).unwrap_or_else(|p| p.into_inner()).0;
        }
        st.seq
    }

    /// Block until no probe is scheduled or in flight, or `timeout` passes.
    /// Returns true when the engine went idle.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut st = self.inner.lock();
        loop {
            let finite_run = st.single_pass || (st.running && !st.settings.repeat);
            let scheduled = (st.running || st.single_pass) && st.hosts.iter().any(|h| h.next_due.is_some() || h.asap.is_some());
            let busy = st.active > 0 || scheduled || finite_run;
            if !busy {
                return true;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            st = self.inner.change_cv.wait_timeout(st, left.min(Duration::from_millis(200))).unwrap_or_else(|p| p.into_inner()).0;
        }
    }

    pub fn detail(&self, id: u32) -> Option<HostDetail> {
        let st = self.inner.lock();
        let h = &st.hosts[*st.index.get(&id)?];
        let mut times: Vec<u32> = h.history.iter().filter_map(|s| s.rtt_us).collect();
        times.sort_unstable();
        let pct = |p: f64| -> Option<u32> {
            if times.is_empty() {
                return None;
            }
            let rank = ((p / 100.0) * times.len() as f64).ceil() as usize;
            Some(times[rank.clamp(1, times.len()) - 1])
        };
        let now = now_ms();
        // The history is a fixed number of samples, not a length of time, so a
        // window may reach further back than what is kept. Say how far it got.
        let kept_ms = h.history.front().map_or(0, |oldest| (now - oldest.t_ms).max(0));
        let windows = [("1m", 60_000i64), ("5m", 300_000), ("1h", 3_600_000), ("24h", 86_400_000)]
            .iter()
            .map(|&(label, span)| {
                let mut w = WindowStats { label, sent: 0, failed: 0, loss_pct: 0.0, avg_rtt_us: None, covered_ms: kept_ms.min(span) };
                let mut sum = 0u64;
                let mut timed = 0u64;
                for s in h.history.iter().rev().take_while(|s| now - s.t_ms <= span) {
                    w.sent += s.beats as u32;
                    if !s.status.is_ok() {
                        w.failed += s.beats as u32;
                    }
                    if let Some(rtt) = s.rtt_us {
                        sum += rtt as u64;
                        timed += 1;
                    }
                }
                if w.sent > 0 {
                    w.loss_pct = w.failed as f32 * 100.0 / w.sent as f32;
                }
                w.avg_rtt_us = (timed > 0).then(|| (sum / timed) as u32);
                w
            })
            .collect();
        Some(HostDetail {
            schema_version: SCHEMA_VERSION,
            row: h.row(&st.settings),
            history: h.history.iter().copied().collect(),
            runs: h.runs.iter().copied().collect(),
            p50_rtt_us: pct(50.0),
            p95_rtt_us: pct(95.0),
            p99_rtt_us: pct(99.0),
            windows,
        })
    }

    /// Stop all threads. The engine cannot be used afterwards.
    pub fn shutdown(&self) {
        self.inner.lock().shutdown = true;
        self.inner.lock_pool().shutdown = true;
        self.inner.sched_cv.notify_all();
        self.inner.change_cv.notify_all();
        self.inner.pool_cv.notify_all();
    }
}

/// Close the running measurement, if any, and put its hosts back on their usual cadence.
fn end_measurement(st: &mut State) {
    let running = st.running;
    let Some(m) = st.measure.as_mut().filter(|m| m.running()) else { return };
    m.ended_at_ms = Some(now_ms().min(m.ends_at_ms));
    let now = Instant::now();
    // They go back spread out, as when pinging starts, not all at once.
    let back = if running { st.hosts.iter().filter(|h| h.measure_interval.is_some() && h.enabled).count() } else { 0 };
    let gap = start_gap(back, st.settings.repeat);
    let mut place = 0u32;
    for h in st.hosts.iter_mut().filter(|h| h.measure_interval.is_some()) {
        h.measure_interval = None;
        h.next_due = (running && h.enabled).then(|| {
            place += 1;
            now + gap * (place - 1)
        });
    }
}

/// How far apart to set the first beats of `count` hosts that start pinging
/// together. Regular pinging gives each host its own place in the interval. A
/// single pass has no next beat to spread, so it goes out at once.
fn start_gap(count: usize, repeat: bool) -> Duration {
    if repeat && count > 1 {
        (START_SPREAD / count.min(u32::MAX as usize) as u32).min(START_GAP_MAX)
    } else {
        Duration::ZERO
    }
}

fn make_key(host: &str, probe: Probe) -> String {
    match probe {
        Probe::Icmp => format!("{}|icmp", host.to_ascii_lowercase()),
        Probe::Tcp { port } => format!("{}|tcp:{port}", host.to_ascii_lowercase()),
    }
}

// ---------------------------------------------------------------------------
// Threads
// ---------------------------------------------------------------------------

/// Run one of the engine's background loops, and start it again if it ever
/// fails, so a fault cannot silently stop all pinging. The loop returns
/// normally only at shutdown.
fn keep_running(name: &str, inner: &Arc<Inner>, body: fn(Arc<Inner>)) {
    loop {
        let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| body(inner.clone())));
        if run.is_ok() || inner.lock().shutdown {
            return;
        }
        eprintln!("pingtable: the {name} stopped unexpectedly and was restarted");
        thread::sleep(Duration::from_millis(200));
    }
}

fn dispatcher(inner: Arc<Inner>) {
    let mut st = inner.lock();
    loop {
        if st.shutdown {
            return;
        }
        let step = plan(&mut st, Instant::now());
        if step.changed {
            inner.change_cv.notify_all();
        }
        if !step.jobs.is_empty() {
            submit(&inner, step.jobs, step.cap);
        }
        st = match step.wake {
            Some(when) => {
                let wait = when.saturating_duration_since(Instant::now()).max(Duration::from_millis(1));
                inner.sched_cv.wait_timeout(st, wait).unwrap_or_else(|p| p.into_inner()).0
            }
            None => inner.sched_cv.wait(st).unwrap_or_else(|p| p.into_inner()),
        };
    }
}

/// What the dispatcher should do at one moment.
struct Step {
    /// Probes to hand to the pool now.
    jobs: Vec<Job>,
    /// When to look again if nothing wakes the dispatcher sooner.
    wake: Option<Instant>,
    /// Something observers can see has changed.
    changed: bool,
    /// Most probe threads the pool may hold.
    cap: usize,
}

/// Decide which hosts to probe at `now`.
///
/// Every host that is due and not already being probed is a candidate. When
/// there are more candidates than free slots, the ones that have waited
/// longest go first. List order alone must not decide: a host that keeps
/// timing out is due again the moment its slot frees, and would take that slot
/// straight back from every host listed after it.
fn plan(st: &mut State, now: Instant) -> Step {
    fn sooner(wake: &mut Option<Instant>, when: Instant) {
        *wake = Some(wake.map_or(when, |w| w.min(when)));
    }
    let mut step = Step { jobs: Vec::new(), wake: None, changed: false, cap: st.settings.max_concurrent as usize };

    // A measurement ends on time even if nothing else wakes the dispatcher.
    if let Some(ends) = st.measure.as_ref().filter(|m| m.running()).map(|m| m.ends) {
        if now >= ends {
            end_measurement(st);
            st.bump();
            step.changed = true;
        } else {
            sooner(&mut step.wake, ends);
        }
    }
    let measuring = st.measure.as_ref().is_some_and(|m| m.running());
    if !(st.running || st.single_pass || measuring) {
        st.waiting = 0;
        return step;
    }

    let repeat = st.running && st.settings.repeat;
    let common = Arc::new(JobCommon {
        payload_size: st.settings.payload_size,
        ttl: st.settings.ttl,
        dont_fragment: st.settings.dont_fragment,
        source_v4: st.settings.source_v4,
        source_v6: st.settings.source_v6,
        prefer_ipv6: st.settings.prefer_ipv6,
    });
    let resolve_every = st.settings.resolve_every_ping;
    let mut any_scheduled = false;
    {
        let State { hosts, settings, active, waiting, .. } = &mut *st;

        // Who is due, and since when.
        let mut due_now: Vec<(Instant, usize)> = Vec::new();
        for (i, h) in hosts.iter_mut().enumerate() {
            if !h.enabled {
                h.next_due = None;
                h.asap = None;
                continue;
            }
            let due = match (h.next_due, h.asap) {
                (Some(beat), Some(asked)) => beat.min(asked),
                (Some(when), None) | (None, Some(when)) => when,
                (None, None) => continue,
            };
            any_scheduled = true;
            if h.in_flight {
                continue;
            }
            if due > now {
                sooner(&mut step.wake, due);
                continue;
            }
            due_now.push((due, i));
        }

        let free = settings.max_concurrent.saturating_sub(*active) as usize;
        if due_now.len() > free {
            // A stable sort: hosts that came due together keep their list order.
            due_now.sort_by_key(|&(due, _)| due);
        }
        // Those left over wait for a probe to finish, which wakes the dispatcher.
        *waiting = due_now.len().saturating_sub(free) as u32;

        for &(due, i) in due_now.iter().take(free) {
            let h = &mut hosts[i];
            h.in_flight = true;
            *active += 1;
            let asked = h.asap.take().is_some();
            let keeps_going = repeat || h.measure_interval.is_some();
            h.next_due = match h.next_due {
                // An extra ping asked for by hand leaves the regular beat where it was.
                Some(beat) if asked && beat > now => Some(beat),
                _ if keeps_going => {
                    let interval = h.cadence(settings);
                    let next = due + interval;
                    Some(if next <= now { now + interval } else { next })
                }
                _ => None,
            };
            if let Some(next) = h.next_due {
                sooner(&mut step.wake, next);
            }
            // A name is looked up again now and then, and sooner while its host
            // is failing, so a host that changed address is found again.
            let lookup_is_old = h.literal.is_none()
                && h.resolved.is_some()
                && h.resolved_at.is_none_or(|at| {
                    let age = now.saturating_duration_since(at);
                    age > RESOLVE_REFRESH || (h.stats.consec_failed >= RESOLVE_AFTER_FAILS && age > RESOLVE_RETRY)
                });
            step.jobs.push(Job {
                id: h.id,
                host: h.host.clone(),
                addr: h.literal.or(if resolve_every || lookup_is_old { None } else { h.resolved }),
                // "Look names up before every ping" means a failed lookup is a failed ping.
                fallback: if resolve_every { None } else { h.resolved },
                probe: h.probe,
                timeout_ms: h.timeout_ms(settings),
                payload_size: h.overrides.payload_size.unwrap_or(common.payload_size),
                ttl: h.overrides.ttl.or(common.ttl),
                dont_fragment: h.overrides.dont_fragment.unwrap_or(common.dont_fragment),
                tos: h.overrides.tos.unwrap_or(0),
                payload: h.overrides.payload.clone(),
                common: common.clone(),
            });
        }
    }

    if !any_scheduled && st.active == 0 && step.jobs.is_empty() {
        // Nothing left to do: a single pass or a no-repeat run is over.
        let was_active = st.single_pass || !st.settings.repeat;
        st.single_pass = false;
        if !st.settings.repeat {
            st.running = false;
        }
        if was_active {
            st.bump();
            step.changed = true;
        }
    }
    step
}

fn submit(inner: &Arc<Inner>, jobs: Vec<Job>, cap: usize) {
    let mut pool = inner.lock_pool();
    for job in jobs {
        pool.jobs.push_back(job);
        // Threads that are free to take a job, including ones just started.
        let free = pool.workers - pool.busy;
        if pool.jobs.len() > free && pool.workers < cap.max(1) {
            pool.workers += 1;
            let for_worker = inner.clone();
            let spawned = thread::Builder::new()
                .name("pingtable-probe".into())
                .stack_size(256 * 1024)
                .spawn(move || worker(for_worker));
            if spawned.is_err() {
                pool.workers -= 1;
            }
        }
    }
    drop(pool);
    inner.pool_cv.notify_all();
}

fn worker(inner: Arc<Inner>) {
    let mut pool = inner.lock_pool();
    loop {
        let job = loop {
            if let Some(job) = pool.jobs.pop_front() {
                break job;
            }
            if pool.shutdown {
                pool.workers -= 1;
                return;
            }
            let (guard, wait) = inner.pool_cv.wait_timeout(pool, WORKER_IDLE).unwrap_or_else(|p| p.into_inner());
            pool = guard;
            if wait.timed_out() && pool.jobs.is_empty() {
                pool.workers -= 1;
                return;
            }
        };
        pool.busy += 1;
        drop(pool);

        let t_ms = now_ms();
        let began = Instant::now();
        // A fault inside one probe is recorded as an error for that host. Without
        // this the thread would die still counted as busy, and the pool would shrink.
        let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_job(&job)))
            .unwrap_or(Done { outcome: Outcome::fail(Status::Error), resolved: None, looked_up: false });
        let took = began.elapsed();

        // Count this thread as free before the result is published: publishing
        // wakes the dispatcher, which may hand out the next job at once and
        // must not start an extra thread for it.
        inner.lock_pool().busy -= 1;

        let command = {
            let mut st = inner.lock();
            apply_result(&mut st, &job, done, took, t_ms)
        };
        inner.sched_cv.notify_all();
        inner.change_cv.notify_all();
        if let Some((command, env)) = command {
            queue_command(&inner, command, env);
        }

        pool = inner.lock_pool();
    }
}

/// What one probe came back with.
struct Done {
    outcome: Outcome,
    /// The address a lookup just gave, when one was made and it worked.
    resolved: Option<IpAddr>,
    /// A lookup was made, whether or not it worked.
    looked_up: bool,
}

fn run_job(job: &Job) -> Done {
    run_job_with(job, |host, prefer_ipv6| netinfo::resolve_within(host, prefer_ipv6, netinfo::LOOKUP_LIMIT))
}

/// [`run_job`] with the name lookup handed in, so tests need no resolver.
fn run_job_with(job: &Job, lookup: impl Fn(&str, bool) -> Option<IpAddr>) -> Done {
    let mut resolved = None;
    let mut looked_up = false;
    let ip = match job.addr {
        Some(ip) => ip,
        None => {
            looked_up = true;
            match lookup(&job.host, job.common.prefer_ipv6) {
                Some(ip) => {
                    resolved = Some(ip);
                    ip
                }
                // The name did not resolve this time. Carry on with the address it
                // had, so a resolver hiccup is not counted as the host being down.
                None => match job.fallback {
                    Some(ip) => ip,
                    None => return Done { outcome: Outcome::fail(Status::ResolveFailed), resolved: None, looked_up },
                },
            }
        }
    };
    let params = ProbeParams {
        timeout: Duration::from_millis(job.timeout_ms as u64),
        payload_size: job.payload_size,
        ttl: job.ttl,
        dont_fragment: job.dont_fragment,
        tos: job.tos,
        payload: job.payload.clone(),
        source_v4: job.common.source_v4,
        source_v6: job.common.source_v6,
    };
    let outcome = match job.probe {
        Probe::Icmp => probe::icmp(ip, &params),
        Probe::Tcp { port } => probe::tcp(SocketAddr::new(ip, port), &params),
    };
    Done { outcome, resolved, looked_up }
}

type CommandEnv = Vec<(&'static str, String)>;

fn apply_result(st: &mut State, job: &Job, done: Done, took: Duration, t_ms: i64) -> Option<(String, CommandEnv)> {
    st.active = st.active.saturating_sub(1);
    let idx = *st.index.get(&job.id)?;
    let seq = st.bump();
    let fail_trigger = st.settings.fail_trigger;
    let recover_trigger = st.settings.recover_trigger;
    let history_cap = st.settings.history_per_host as usize;
    let cadence = st.hosts[idx].cadence(&st.settings);
    // Beats are skipped only on a schedule that repeats: regular pinging, or a
    // measurement. A single pass (a ping by hand while stopped, `once`, the MCP
    // `ping`) sends each probe when the last one is back, and skips nothing.
    let repeating = (st.running && st.settings.repeat) || st.hosts[idx].measure_interval.is_some();
    let Done { outcome, resolved, looked_up } = done;

    let h = &mut st.hosts[idx];
    h.in_flight = false;
    h.stale = false;
    h.seq = seq;
    if looked_up {
        h.resolved_at = Some(Instant::now());
    }
    if let Some(ip) = resolved {
        if h.literal.is_none() && h.resolved != Some(ip) {
            h.resolved = Some(ip);
            h.mac = None;
        }
    }
    if outcome.reply_addr.is_some() {
        h.reply_ip = outcome.reply_addr;
    }
    // A host is never probed twice at once, so a failed probe that outlasted
    // its interval kept the beats after it from being sent. It stands for all
    // of them: loss then measures time without an answer, not how many
    // timeouts happened to fit into it.
    let beats = if outcome.status.is_ok() || !repeating {
        1
    } else {
        let per_beat = cadence.as_micros().max(1);
        ((took.as_micros() + per_beat / 2) / per_beat).clamp(1, MAX_BEATS) as u16
    };
    let sample = Sample { t_ms, status: outcome.status, rtt_us: outcome.rtt_us, ttl: outcome.ttl, beats };
    let alert = h.stats.apply(&sample, fail_trigger, recover_trigger);
    h.record(sample, history_cap);
    if let Some(acc) = st.measure.as_mut().filter(|m| m.running() && t_ms >= m.started_at_ms).and_then(|m| m.hosts.get_mut(&job.id)) {
        acc.sent += 1;
        match outcome.rtt_us {
            Some(rtt) if outcome.status.is_ok() => acc.rtts.push(rtt),
            _ => {
                acc.lost += 1;
                acc.missed += (beats - 1) as u32;
            }
        }
    }
    let h = &mut st.hosts[idx];

    let kind = alert?;
    let env: CommandEnv = vec![
        ("HostName", h.host.clone()),
        ("IPAddress", h.ip().map(|ip| ip.to_string()).unwrap_or_default()),
        ("Description", h.description.clone()),
        ("Group", h.group.clone().unwrap_or_default()),
        ("Status", outcome.status.label().to_string()),
        ("SucceedCount", h.stats.ok.to_string()),
        ("FailedCount", h.stats.failed.to_string()),
        ("ConsecutiveFailedCount", h.stats.consec_failed.to_string()),
        ("LastPingTime", outcome.rtt_us.map(|us| format!("{:.3}", us as f64 / 1000.0)).unwrap_or_default()),
    ];
    let event = AlertEvent { seq, t_ms, host_id: h.id, host: h.host.clone(), kind, status: outcome.status };
    if st.events.len() >= MAX_EVENTS {
        st.events.pop_front();
    }
    st.events.push_back(event);

    let template = match kind {
        AlertKind::Down => &st.settings.on_down_command,
        AlertKind::Up => &st.settings.on_up_command,
    };
    if template.trim().is_empty() {
        return None;
    }
    Some((substitute(template, &env), env))
}

/// Values that come from a host list, an imported file or an agent, rather
/// than from pingtable itself.
const FREE_TEXT: [&str; 2] = ["Description", "Group"];

/// Replace `%Name%` variables. Unknown names are left as written.
///
/// The command line is run by a shell, and some values (a description from an
/// imported file, say) were not typed by the person who wrote the command. So:
///
/// * every character a shell could act on is replaced with `_`;
/// * in free text a space also becomes `_`, wherever the variable is. Quotes
///   in the command are no help: `\"`, `^"` and a second shell (`eval`,
///   `ssh host "…"`, `powershell -Command "…"`) each read them their own way;
/// * in free text, `-`, `/` and `@` are kept only straight after a letter or
///   a digit, so no word in it can start with one. Programs read those as an
///   option, a switch or "take this from a file".
///
/// The `PINGTABLE_` environment variables carry the exact text for commands
/// that need it. A script can read them safely. Written as `%PINGTABLE_NAME%`
/// in a `cmd` command line they are not safe: `cmd` expands them before it
/// looks for `&` and `|`.
fn substitute(template: &str, env: &CommandEnv) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    'scan: while let Some(c) = rest.chars().next() {
        if let Some(after) = rest.strip_prefix('%') {
            for (name, value) in env {
                if let Some(tail) = after.strip_prefix(*name).and_then(|t| t.strip_prefix('%')) {
                    out.push_str(&shell_safe(value, FREE_TEXT.contains(name)));
                    rest = tail;
                    continue 'scan;
                }
            }
        }
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

fn shell_safe(value: &str, free_text: bool) -> String {
    let mut safe = String::with_capacity(value.len());
    let mut after_word = false;
    for c in value.chars() {
        let kept = match c {
            c if c.is_ascii_alphanumeric() => c,
            '-' | '/' | '@' if free_text && !after_word => '_',
            c if "._:@/+,=-".contains(c) => c,
            ' ' if !free_text => ' ',
            _ => '_',
        };
        after_word = kept.is_ascii_alphanumeric();
        safe.push(kept);
    }
    safe
}

/// Hand an alert command to the runners. A few run at once and the rest wait
/// their turn, so an outage of the whole network does not start a shell for
/// every host in the same second.
fn queue_command(inner: &Arc<Inner>, command: String, env: CommandEnv) {
    let mut queue = inner.lock_commands();
    if queue.running >= queue.at_once {
        if queue.waiting.len() >= queue.most_waiting {
            queue.skipped += 1;
        } else {
            queue.waiting.push_back((command, env));
        }
        return;
    }
    queue.running += 1;
    drop(queue);
    let for_runner = inner.clone();
    let started = thread::Builder::new().name("pingtable-command".into()).spawn(move || command_runner(for_runner, command, env));
    if started.is_err() {
        inner.lock_commands().running -= 1;
        inner.tell("An alert command could not be started: no thread could be created for it.".into());
    }
}

/// Run one command, then whatever is waiting, until nothing is.
fn command_runner(inner: Arc<Inner>, mut command: String, mut env: CommandEnv) {
    let (run, time_limit) = {
        let queue = inner.lock_commands();
        (queue.run.clone(), queue.time_limit)
    };
    loop {
        if let Err(why) = run(&command, &env, time_limit) {
            inner.tell(why);
        }
        let mut queue = inner.lock_commands();
        match queue.waiting.pop_front() {
            Some((next, next_env)) => {
                command = next;
                env = next_env;
            }
            None => {
                queue.running -= 1;
                let skipped = std::mem::take(&mut queue.skipped);
                let most_waiting = queue.most_waiting;
                drop(queue);
                match skipped {
                    0 => {}
                    1 => inner.tell(format!("1 alert command was not run because {most_waiting} others were already waiting.")),
                    n => inner.tell(format!("{n} alert commands were not run because {most_waiting} others were already waiting.")),
                }
                return;
            }
        }
    }
}

/// The system shell, set to run one command line.
#[cfg(windows)]
fn shell(command: &str) -> std::process::Command {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Console::GetConsoleWindow;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut cmd = std::process::Command::new("cmd");
    // The line must reach cmd exactly as it was typed. `arg` would escape its
    // quotes the way C programs expect, which cmd does not understand. With
    // /S, cmd strips only the outer pair added here. /D skips AutoRun commands.
    cmd.raw_arg("/D /S /C").raw_arg(format!("\"{command}\""));
    // The window app has no console; without this, Windows opens one for every command.
    if unsafe { GetConsoleWindow() }.is_null() {
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

#[cfg(not(windows))]
fn shell(command: &str) -> std::process::Command {
    use std::os::unix::process::CommandExt;
    let mut cmd = std::process::Command::new("sh");
    cmd.arg("-c").arg(command);
    // A process group of its own, so a stop reaches what the shell started too.
    cmd.process_group(0);
    cmd
}

/// Everything one alert command started, so that a stop ends all of it and
/// not only the shell: `sh -c "a; b"` forks `a`, and `cmd /C` never hands
/// over to the program it starts.
///
/// On Unix that is the shell's own process group. A program that leaves the
/// group on purpose (a daemon) is out of reach.
#[cfg(not(windows))]
struct CommandTree {
    group: libc::pid_t,
    /// Its place in `COMMAND_GROUPS`, while it has one.
    place: Option<usize>,
}

/// The process groups of the alert commands that run now, so that a stop of
/// pingtable itself reaches them (see [`pass_stop_signals_to_commands`]).
/// 0 marks a free place. At most 4 commands run at once, so 16 places are
/// plenty; a command that finds none is only left off the list.
#[cfg(not(windows))]
static COMMAND_GROUPS: [std::sync::atomic::AtomicI32; 16] = [const { std::sync::atomic::AtomicI32::new(0) }; 16];

/// Make a stop of this program reach the alert commands it is running.
///
/// On Unix every alert command runs in a process group of its own, so that
/// its time limit can end everything it started. A Ctrl+C, a closed terminal
/// or a `kill` reaches only pingtable's group, so without this the commands
/// would live on after pingtable. After this call, SIGINT, SIGTERM and SIGHUP
/// are passed on to every command still running, and then do to pingtable
/// what they would have done. One that was set to be ignored (as `nohup` does
/// with a hang-up) stays ignored. Call it once, in the program that runs the
/// table. On Windows the commands already end with pingtable, through their
/// job objects, so this does nothing there.
pub fn pass_stop_signals_to_commands() {
    #[cfg(not(windows))]
    for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        let handler = pass_on as extern "C" fn(libc::c_int) as libc::sighandler_t;
        // SAFETY: the handler only reads atomics and calls killpg, signal and
        // raise, which may all be called from a signal handler.
        unsafe {
            if libc::signal(signal, handler) == libc::SIG_IGN {
                libc::signal(signal, libc::SIG_IGN);
            }
        }
    }
}

/// The signal handler: pass the signal on to the alert commands, then take it
/// the usual way.
#[cfg(not(windows))]
extern "C" fn pass_on(signal: libc::c_int) {
    for place in &COMMAND_GROUPS {
        let group = place.load(std::sync::atomic::Ordering::SeqCst);
        if group > 0 {
            // SAFETY: plain numbers; killpg may be called from a signal handler.
            unsafe { libc::killpg(group, signal) };
        }
    }
    // SAFETY: as above. The signal is blocked while this runs, so the one
    // raised here is taken, with its usual effect, as soon as this returns.
    unsafe {
        libc::signal(signal, libc::SIG_DFL);
        libc::raise(signal);
    }
}

#[cfg(not(windows))]
impl CommandTree {
    fn hold(child: &std::process::Child) -> CommandTree {
        use std::sync::atomic::Ordering::SeqCst;
        let group = child.id() as libc::pid_t;
        let place = COMMAND_GROUPS.iter().position(|place| place.compare_exchange(0, group, SeqCst, SeqCst).is_ok());
        CommandTree { group, place }
    }

    fn stop(&self, child: &mut std::process::Child) {
        // The shell has not been waited for yet, so its group id is still its own.
        // SAFETY: killpg takes plain numbers and touches no memory of ours.
        if unsafe { libc::killpg(self.group, libc::SIGKILL) } != 0 {
            let _ = child.kill();
        }
    }

    /// The shell ended by itself: a program it left running in the background is the user's.
    fn release(self) {}
}

#[cfg(not(windows))]
impl Drop for CommandTree {
    fn drop(&mut self) {
        if let Some(place) = self.place {
            COMMAND_GROUPS[place].store(0, std::sync::atomic::Ordering::SeqCst);
        }
    }
}

/// On Windows, a Job Object that the shell is put in as soon as it starts.
/// The programs it starts join the job too. The job ends them all when it is
/// told to, and when its last handle closes, so they also end with pingtable.
/// A program started in the few milliseconds before the shell joins is not
/// in it.
#[cfg(windows)]
struct CommandTree {
    /// Null when there is no job (it could not be made or joined); then only
    /// the shell itself can be stopped.
    job: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
impl CommandTree {
    fn hold(child: &std::process::Child) -> CommandTree {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };
        // SAFETY: a new, unnamed job with default security; the handle is ours and checked for null.
        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return CommandTree { job };
        }
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: `limits` is the structure this information class takes, with its true size; the
        // process handle belongs to `child`, which outlives the call.
        let joined = unsafe {
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            ) != 0
                && AssignProcessToJobObject(job, child.as_raw_handle()) != 0
        };
        if !joined {
            // SAFETY: the job handle is ours, and no process is in the job.
            unsafe { CloseHandle(job) };
            return CommandTree { job: std::ptr::null_mut() };
        }
        CommandTree { job }
    }

    fn stop(&self, child: &mut std::process::Child) {
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;
        // SAFETY: the job handle is ours and open until drop.
        if self.job.is_null() || unsafe { TerminateJobObject(self.job, 1) } == 0 {
            let _ = child.kill();
        }
    }

    /// The shell ended by itself: a program it left running (with `start`) is
    /// the user's, so the job lets go of it instead of ending it on close.
    fn release(self) {
        use windows_sys::Win32::System::JobObjects::{JobObjectExtendedLimitInformation, SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION};
        if self.job.is_null() {
            return;
        }
        let limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        // SAFETY: as in `hold`; no limit flags at all this time.
        unsafe {
            SetInformationJobObject(
                self.job,
                JobObjectExtendedLimitInformation,
                &limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION as *const core::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
        }
    }
}

#[cfg(windows)]
impl Drop for CommandTree {
    fn drop(&mut self) {
        if !self.job.is_null() {
            // SAFETY: the job handle is ours and closed only here.
            unsafe { windows_sys::Win32::Foundation::CloseHandle(self.job) };
        }
    }
}

/// Run one alert command and wait for it to end. One that outlasts
/// `time_limit` (`COMMAND_TIME_LIMIT` in the app) is stopped, together with
/// the programs it started.
fn run_command(command: &str, env: &CommandEnv, time_limit: Duration) -> Result<(), String> {
    let mut cmd = shell(command);
    for (name, value) in env {
        // A NUL cannot be passed in the environment; left in, the command would not start.
        cmd.env(format!("PINGTABLE_{}", name.to_ascii_uppercase()), value.replace('\0', " "));
    }
    cmd.stdin(std::process::Stdio::null());
    let mut child = cmd.spawn().map_err(|why| format!("An alert command could not be started: {why}."))?;
    let tree = CommandTree::hold(&child);
    let deadline = Instant::now() + time_limit;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                tree.release();
                return Ok(());
            }
            Ok(None) if Instant::now() >= deadline => {
                tree.stop(&mut child);
                let _ = child.wait();
                return Err(format!("An alert command ran for more than {} seconds and was stopped.", time_limit.as_secs()));
            }
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(why) => {
                tree.stop(&mut child);
                let _ = child.wait();
                return Err(format!("An alert command could not be watched, so it was stopped: {why}."));
            }
        }
    }
}

/// Flag hosts that have been due for several of their own intervals and still
/// have no slot, and clear the flag on hosts that are no longer waiting.
/// Returns true when any row changed.
fn mark_stale(st: &mut State, now: Instant) -> bool {
    let live = st.running;
    let mut flipped: Vec<usize> = Vec::new();
    for (i, h) in st.hosts.iter().enumerate() {
        let waited_long = live
            && h.enabled
            && !h.in_flight
            && h.next_due.is_some_and(|due| now.saturating_duration_since(due) > (h.cadence(&st.settings) * STALE_AFTER_BEATS).max(STALE_AFTER_MIN));
        if waited_long != h.stale {
            flipped.push(i);
        }
    }
    if flipped.is_empty() {
        return false;
    }
    let seq = st.bump();
    for i in flipped {
        let h = &mut st.hosts[i];
        h.stale = !h.stale;
        h.seq = seq;
    }
    true
}

/// Marks hosts that have waited too long for a slot, and fills in MAC
/// addresses and reverse-DNS names in the background.
fn housekeeper(inner: Arc<Inner>) {
    let mut last_seq = 0u64;
    loop {
        let (read_mac, reverse_dns, candidates) = {
            let deadline = Instant::now() + HOUSEKEEPING_EVERY;
            let mut st = inner.lock();
            loop {
                if st.shutdown {
                    return;
                }
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                st = inner.change_cv.wait_timeout(st, left).unwrap_or_else(|p| p.into_inner()).0;
            }
            if mark_stale(&mut st, Instant::now()) {
                inner.change_cv.notify_all();
            }
            if st.seq == last_seq {
                continue;
            }
            last_seq = st.seq;
            let candidates: Vec<(u32, IpAddr)> = if st.settings.reverse_dns {
                st.hosts
                    .iter()
                    .filter(|h| h.hostname.is_none() && h.stats.sent > 0 && h.rdns_tried_at.is_none_or(|at| at.elapsed() > RDNS_RETRY))
                    .filter_map(|h| h.literal.map(|ip| (h.id, ip)))
                    .take(8)
                    .collect()
            } else {
                Vec::new()
            };
            (st.settings.read_mac, st.settings.reverse_dns, candidates)
        };

        if read_mac {
            let table = netinfo::neighbours();
            if !table.is_empty() {
                let mut st = inner.lock();
                let mut changed = Vec::new();
                for (i, h) in st.hosts.iter().enumerate() {
                    if let Some(mac) = h.ip().and_then(|ip| table.get(&ip)) {
                        let text = netinfo::format_mac(mac);
                        if h.mac.as_deref() != Some(text.as_str()) {
                            changed.push((i, text));
                        }
                    }
                }
                if !changed.is_empty() {
                    let seq = st.bump();
                    for (i, text) in changed {
                        st.hosts[i].mac = Some(text);
                        st.hosts[i].seq = seq;
                    }
                    last_seq = seq;
                    drop(st);
                    inner.change_cv.notify_all();
                }
            }
        }

        if reverse_dns && !candidates.is_empty() {
            let names: Vec<(u32, Option<String>)> = thread::scope(|scope| {
                let handles: Vec<_> =
                    candidates.iter().map(|&(id, ip)| scope.spawn(move || (id, netinfo::reverse_within(ip, netinfo::LOOKUP_LIMIT)))).collect();
                handles.into_iter().filter_map(|h| h.join().ok()).collect()
            });
            let mut st = inner.lock();
            let seq = st.bump();
            for (id, name) in names {
                if let Some(&idx) = st.index.get(&id) {
                    let h = &mut st.hosts[idx];
                    h.rdns_tried_at = Some(Instant::now());
                    if name.is_some() {
                        h.hostname = name;
                        h.seq = seq;
                    }
                }
            }
            // Leave last_seq behind so the next pass picks up remaining hosts.
            drop(st);
            inner.change_cv.notify_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn sample(t_ms: i64, ok: bool, rtt_us: u32) -> Sample {
        Sample {
            t_ms,
            status: if ok { Status::Ok } else { Status::Timeout },
            rtt_us: ok.then_some(rtt_us),
            ttl: None,
            beats: 1,
        }
    }

    #[test]
    fn stats_count_and_time() {
        let mut s = HostStats::default();
        for (i, (ok, rtt)) in [(true, 1000), (true, 3000), (false, 0), (true, 2000)].into_iter().enumerate() {
            s.apply(&sample(i as i64, ok, rtt), 1, 1);
        }
        assert_eq!((s.sent, s.ok, s.failed), (4, 3, 1));
        assert_eq!(s.avg_rtt_us(), Some(2000));
        assert_eq!((s.min_rtt_us, s.max_rtt_us), (Some(1000), Some(3000)));
        assert_eq!(s.failed_pct(), 25.0);
        assert_eq!((s.consec_failed, s.max_consec_failed, s.max_consec_failed_at_ms), (0, 1, Some(2)));
        assert!(s.jitter_us > 0.0);
    }

    #[test]
    fn down_and_up_respect_thresholds() {
        let mut s = HostStats::default();
        assert_eq!(s.apply(&sample(1, true, 500), 3, 2), None);
        assert_eq!(s.apply(&sample(2, false, 0), 3, 2), None);
        assert_eq!(s.apply(&sample(3, false, 0), 3, 2), None);
        assert_eq!(s.apply(&sample(4, false, 0), 3, 2), Some(AlertKind::Down));
        assert_eq!(s.apply(&sample(5, false, 0), 3, 2), None, "one alert per outage");
        assert_eq!(s.apply(&sample(6, true, 500), 3, 2), None);
        assert!(s.down);
        assert_eq!(s.apply(&sample(7, true, 500), 3, 2), Some(AlertKind::Up));
        assert!(!s.down);
        assert_eq!(s.max_consec_failed, 4);
    }

    #[test]
    fn short_blip_under_threshold_never_alerts() {
        let mut s = HostStats::default();
        for t in 0..10 {
            let ok = t % 2 == 0;
            assert_eq!(s.apply(&sample(t, ok, 100), 2, 1), None);
        }
        assert!(!s.down);
    }

    #[test]
    fn variables_are_substituted() {
        let env: CommandEnv = vec![("HostName", "nas.lan".into()), ("Status", "Timed out".into())];
        assert_eq!(substitute("notify %HostName% is %Status% (%Unknown%)", &env), "notify nas.lan is Timed out (%Unknown%)");

        // A value cannot smuggle in a second command, a pipe, a redirect or a variable.
        let hostile: CommandEnv = vec![("Description", "x & calc.exe | del *.* > out `id` $(id) %PATH% \"q\" ; rm -rf ~\n!".into())];
        let line = substitute("echo %Description%", &hostile);
        assert!(!line.contains(['&', '|', '>', '<', '`', '$', '%', '"', ';', '\n', '!', '~', '*', '(', ')']), "{line}");
        // Outside quotes it cannot add an argument either: it stays one word.
        assert_eq!(line.split(' ').count(), 2, "{line}");
        assert!(line.starts_with("echo x___calc.exe"), "{line}");
    }

    #[test]
    fn free_text_cannot_add_arguments_or_options() {
        let env = |text: &str| -> CommandEnv { vec![("Description", text.into()), ("Group", "-rf /".into()), ("Status", "Timed out".into())] };

        // One word, and no word in it reads as an option, a switch or a file reference.
        let line = substitute("curl -d %Description% https://hook.example", &env("x -o /tmp/p https://attacker.example/p"));
        assert_eq!(line, "curl -d x__o__tmp/p_https:__attacker.example/p https://hook.example");
        assert_eq!(substitute("curl -d %Description% u", &env("@/etc/hostname")), "curl -d __etc/hostname u");
        assert_eq!(substitute("rm %Group%", &env("")), "rm _rf__");
        assert_eq!(substitute("tool %Description%", &env("/S /Q")), "tool _S__Q");
        assert_eq!(substitute("tool %Description%", &env("a=-o b,@f c+/d")), "tool a=_o_b,_f_c+_d");
        // Inside a word they stay.
        assert_eq!(substitute("say %Description%", &env("web-1 TCP/IP user@nas.lan 10.0.0.0/24")), "say web-1_TCP/IP_user@nas.lan_10.0.0.0/24");

        // Quotes in the command change nothing: a space is never kept.
        assert_eq!(substitute("notify \"%Description% is down\"", &env("NAS in the closet")), "notify \"NAS_in_the_closet is down\"");
        assert_eq!(substitute("notify \"%Description%\"", &env("-o /tmp/p")), "notify \"_o__tmp/p\"");
        assert_eq!(substitute("notify \"%Description%\"", &env("a\" -x \"b")), "notify \"a___x__b\"");
        assert_eq!(substitute("notify '%Description%'", &env("a b")), "notify 'a_b'");
        assert_eq!(substitute(r#"curl -d {\"text\":\"%Description%\"} u"#, &env("x -o /tmp/p")), r#"curl -d {\"text\":\"x__o__tmp/p\"} u"#);

        // pingtable's own values are not free text and keep their spaces.
        assert_eq!(substitute("say %Status%", &env("")), "say Timed out");
        // Letters outside ASCII are replaced: some programs turn a few of them into quotes.
        assert_eq!(substitute("say %Description%", &env("caf\u{e9} \u{2ba}x")), "say caf___x");
        // A value is put in once; what it contains is not looked at again.
        assert_eq!(substitute("%Description%", &env("%Status%")), "_Status_");
    }

    #[cfg(unix)]
    #[test]
    fn free_text_stays_one_argument_whatever_quotes_the_command_has() {
        // `\"` is not a quote to sh, so a rule that kept spaces "inside quotes" let these through.
        let env: CommandEnv = vec![("Description", "x -o /tmp/pingtable-test-pwned https://attacker.example/p".into())];
        let line = substitute(r#"printf '[%s]\n' -d {\"text\":\"%Description%\"} https://hook.example/x"#, &env);
        let out = std::process::Command::new("sh").arg("-c").arg(&line).output().unwrap();
        let args = String::from_utf8_lossy(&out.stdout).into_owned();
        assert_eq!(
            args.lines().collect::<Vec<_>>(),
            ["[-d]", r#"[{"text":"x__o__tmp/pingtable-test-pwned_https:__attacker.example/p"}]"#, "[https://hook.example/x]"],
            "{line}"
        );
    }

    fn state_of(settings: Settings, list: &str) -> State {
        let engine = Engine::new(settings, list);
        engine.shutdown();
        let mut st = engine.inner.lock();
        let hosts = std::mem::take(&mut st.hosts);
        let mut copy = State {
            settings: st.settings.clone(),
            hosts_text: String::new(),
            hosts,
            index: HashMap::new(),
            next_id: 100,
            seq: 0,
            list_seq: 0,
            running: true,
            single_pass: false,
            shutdown: false,
            active: 0,
            waiting: 0,
            events: VecDeque::new(),
            notes: VecDeque::new(),
            warnings: Vec::new(),
            running_since_ms: None,
            measure: None,
            next_measure_id: 1,
        };
        copy.reindex();
        copy
    }

    /// Pretend the probes handed out by `plan` came back.
    fn finish(st: &mut State, jobs: &[Job]) {
        for job in jobs {
            let idx = st.index[&job.id];
            st.hosts[idx].in_flight = false;
            st.active -= 1;
        }
    }

    #[test]
    fn a_freed_slot_goes_to_the_host_that_waited_longest() {
        // One slot, three hosts that all time out after exactly one interval:
        // the case where the first host is due again the moment its slot frees.
        let settings = Settings { max_concurrent: 1, interval_secs: 1, ..quick_settings() };
        let mut st = state_of(settings, "192.0.2.1 192.0.2.2 192.0.2.3");
        let t0 = Instant::now();
        for h in st.hosts.iter_mut() {
            h.next_due = Some(t0);
        }
        let ids: Vec<u32> = st.hosts.iter().map(|h| h.id).collect();

        let mut order = Vec::new();
        for beat in 0..6u32 {
            let now = t0 + Duration::from_secs(beat as u64);
            let step = plan(&mut st, now);
            assert_eq!(step.jobs.len(), 1, "one slot, one probe");
            assert_eq!(st.waiting, 2, "the other two wait");
            order.push(step.jobs[0].id);
            finish(&mut st, &step.jobs);
        }
        assert_eq!(order, [ids[0], ids[1], ids[2], ids[0], ids[1], ids[2]], "every host gets its turn");
    }

    #[test]
    fn hosts_that_fit_are_all_probed_and_nothing_waits() {
        let settings = Settings { max_concurrent: 8, ..quick_settings() };
        let mut st = state_of(settings, "192.0.2.1 192.0.2.2 192.0.2.3");
        let t0 = Instant::now();
        for h in st.hosts.iter_mut() {
            h.next_due = Some(t0);
        }
        let step = plan(&mut st, t0);
        assert_eq!((step.jobs.len(), st.waiting, st.active), (3, 0, 3));
        // All in flight: nothing more is handed out, and the dispatcher sleeps
        // until a probe finishes and wakes it.
        let again = plan(&mut st, t0 + Duration::from_millis(10));
        assert!(again.jobs.is_empty());
        assert_eq!(again.wake, None);
    }

    #[test]
    fn an_extra_ping_leaves_the_regular_beat_alone() {
        let mut st = state_of(quick_settings(), "192.0.2.1");
        let t0 = Instant::now();
        let beat = t0 + Duration::from_millis(700);
        st.hosts[0].next_due = Some(beat);
        st.hosts[0].asap = Some(t0);
        let step = plan(&mut st, t0);
        assert_eq!(step.jobs.len(), 1, "pinged at once");
        assert_eq!((st.hosts[0].next_due, st.hosts[0].asap), (Some(beat), None), "and still due at its own beat");
    }

    #[test]
    fn a_host_left_waiting_is_marked_stale_and_then_cleared() {
        let settings = Settings { max_concurrent: 1, interval_secs: 1, ..quick_settings() };
        let mut st = state_of(settings, "192.0.2.1 192.0.2.2");
        let t0 = Instant::now();
        let seen = sample(0, true, 500);
        for h in st.hosts.iter_mut() {
            h.next_due = Some(t0);
            h.stats.apply(&seen, 1, 1);
        }
        let stale = |st: &State| st.hosts.iter().map(|h| h.row(&st.settings).stale).collect::<Vec<_>>();
        assert!(!mark_stale(&mut st, t0 + Duration::from_secs(9)), "a few seconds late is not stale");
        assert!(mark_stale(&mut st, t0 + Duration::from_secs(11)));
        assert_eq!(stale(&st), [true, true]);
        // The last known state still stands; the flag says it is old.
        assert_eq!(st.hosts.iter().map(|h| h.state_name()).collect::<Vec<_>>(), ["up", "up"]);

        // One is being probed again and the other has a fresh beat ahead of it.
        st.hosts[0].in_flight = true;
        st.hosts[1].next_due = Some(t0 + Duration::from_secs(12));
        assert!(mark_stale(&mut st, t0 + Duration::from_secs(11)));
        assert_eq!(stale(&st), [false, false]);

        // Stopped pinging is not staleness: the rows are simply not being probed.
        st.hosts[0].in_flight = false;
        st.running = false;
        assert!(!mark_stale(&mut st, t0 + Duration::from_secs(60)));
    }

    /// A ping of the first host of `st`, as `plan` would hand it out.
    fn icmp_job(st: &State, timeout_ms: u32) -> Job {
        Job {
            id: st.hosts[0].id,
            host: st.hosts[0].host.clone(),
            addr: None,
            fallback: None,
            probe: Probe::Icmp,
            timeout_ms,
            payload_size: 32,
            ttl: None,
            dont_fragment: false,
            tos: 0,
            payload: None,
            common: Arc::new(JobCommon { payload_size: 32, ttl: None, dont_fragment: false, source_v4: None, source_v6: None, prefer_ipv6: false }),
        }
    }

    fn timed_out() -> Done {
        Done { outcome: Outcome::fail(Status::Timeout), resolved: None, looked_up: false }
    }

    fn answered() -> Done {
        Done {
            outcome: Outcome { status: Status::Ok, os_code: None, rtt_us: Some(900), ttl: None, reply_addr: None },
            resolved: None,
            looked_up: false,
        }
    }

    #[test]
    fn a_failed_probe_counts_for_every_beat_it_covered() {
        // Ten pings a second with a one-second timeout: each timeout hides ten beats.
        let mut st = state_of(quick_settings(), "192.0.2.1");
        st.hosts[0].measure_interval = Some(Duration::from_millis(100));
        let job = icmp_job(&st, 1000);
        // Down for three seconds, then up for three seconds' worth of answers.
        for t in 0..3 {
            st.active = 1;
            apply_result(&mut st, &job, timed_out(), Duration::from_millis(1003), t);
        }
        for t in 3..33 {
            st.active = 1;
            apply_result(&mut st, &job, answered(), Duration::from_millis(1), t);
        }
        let row = st.hosts[0].row(&st.settings);
        assert_eq!((row.sent, row.failed, row.missed), (33, 3, 27), "33 pings went out, and 27 more beats passed unanswered");
        assert_eq!(row.failed_pct, 50.0, "down for half the time is 50% loss, not 3 in 33");
        assert_eq!(st.hosts[0].history.front().unwrap().beats, 10);

        // A failure that returns at once (no route, say) is one beat like any other.
        st.active = 1;
        apply_result(&mut st, &job, Done { outcome: Outcome::fail(Status::NetUnreachable), resolved: None, looked_up: false }, Duration::from_millis(2), 40);
        assert_eq!(st.hosts[0].stats.missed, 27);
    }

    #[test]
    fn a_single_pass_counts_one_beat_per_ping() {
        // As the MCP `ping`, `pingtable once` and a ping by hand while stopped do:
        // one pass per round, with the usual 1 s interval and a 5 s timeout.
        let mut st = state_of(Settings { timeout_ms: 5000, ..quick_settings() }, "192.0.2.1");
        st.running = false;
        st.single_pass = true;
        let job = icmp_job(&st, 5000);
        st.active = 1;
        apply_result(&mut st, &job, answered(), Duration::from_millis(1), 0);
        st.active = 1;
        apply_result(&mut st, &job, timed_out(), Duration::from_millis(5003), 1);
        let row = st.hosts[0].row(&st.settings);
        assert_eq!((row.sent, row.failed, row.missed), (2, 1, 0), "nothing was due while the timeout ran");
        assert_eq!(row.failed_pct, 50.0, "1 of 2 pings lost");

        // A run that does not repeat is a single pass as well.
        let mut once = state_of(Settings { repeat: false, ..quick_settings() }, "192.0.2.1");
        let job = icmp_job(&once, 5000);
        once.active = 1;
        apply_result(&mut once, &job, timed_out(), Duration::from_millis(5003), 0);
        assert_eq!(once.hosts[0].stats.missed, 0);
        // Regular pinging does skip the beats a timeout covers.
        let mut regular = state_of(quick_settings(), "192.0.2.1");
        let job = icmp_job(&regular, 5000);
        regular.active = 1;
        apply_result(&mut regular, &job, timed_out(), Duration::from_millis(5003), 0);
        assert_eq!(regular.hosts[0].stats.missed, 4);
    }

    #[test]
    fn a_name_is_looked_up_again_after_five_minutes_or_three_failures() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut st = state_of(quick_settings(), &format!("moved.lan:{port}"));
        let old: IpAddr = "127.0.0.1".parse().unwrap();
        let t0 = Instant::now();
        st.hosts[0].resolved = Some(old);
        st.hosts[0].resolved_at = Some(t0);
        let secs = Duration::from_secs;

        // Plan one probe `age` after the last lookup, with `failed` failures in a
        // row, and say what it was given: the address to use (None means look the
        // name up first) and the address to fall back on.
        let probe_at = |st: &mut State, when: Instant, failed: u32| {
            st.hosts[0].stats.consec_failed = failed;
            st.hosts[0].next_due = Some(when);
            let step = plan(st, when);
            finish(st, &step.jobs);
            assert_eq!(step.jobs.len(), 1);
            (step.jobs[0].addr, step.jobs[0].fallback)
        };
        assert_eq!(probe_at(&mut st, t0 + secs(10), 0), (Some(old), Some(old)), "a recent lookup is used as it is");
        assert_eq!(probe_at(&mut st, t0 + secs(299), 2), (Some(old), Some(old)));
        assert_eq!(probe_at(&mut st, t0 + secs(301), 0), (None, Some(old)), "after 5 minutes, looked up again");
        assert_eq!(probe_at(&mut st, t0 + secs(20), 3), (Some(old), Some(old)), "failing, but looked up 20 s ago");
        assert_eq!(probe_at(&mut st, t0 + secs(31), 3), (None, Some(old)), "after 3 failures, sooner");

        // The new lookup fails: the probe goes to the address the name had, and
        // the host keeps it. The failed lookup still counts as the last one.
        st.hosts[0].next_due = Some(t0 + secs(301));
        let step = plan(&mut st, t0 + secs(301));
        let done = run_job_with(&step.jobs[0], |_, _| None);
        assert!(done.looked_up && done.resolved.is_none());
        assert_eq!(done.outcome.status, Status::Ok, "probed at the last address");
        apply_result(&mut st, &step.jobs[0], done, Duration::from_millis(1), 1);
        assert_eq!(st.hosts[0].resolved, Some(old), "a failed refresh keeps the last address");
        let looked_at = st.hosts[0].resolved_at.unwrap();
        assert!(looked_at >= t0);
        assert_eq!(probe_at(&mut st, looked_at + secs(10), 0), (Some(old), Some(old)), "and is not tried again at once");

        // A lookup that finds a new address is used, and the old MAC goes with the old address.
        st.hosts[0].next_due = Some(looked_at + secs(301));
        let step = plan(&mut st, looked_at + secs(301));
        let done = run_job_with(&step.jobs[0], |name, _| (name == "moved.lan").then_some(old));
        assert_eq!((done.resolved, done.outcome.status), (Some(old), Status::Ok));
        let moved: IpAddr = "192.0.2.20".parse().unwrap();
        st.hosts[0].mac = Some("02:00:00:00:00:01".into());
        let answered = Outcome { status: Status::Ok, os_code: None, rtt_us: Some(900), ttl: None, reply_addr: None };
        apply_result(&mut st, &step.jobs[0], Done { outcome: answered, resolved: Some(moved), looked_up: true }, Duration::from_millis(1), 2);
        assert_eq!((st.hosts[0].resolved, st.hosts[0].mac.as_deref()), (Some(moved), None));

        // A name that never resolved has nothing to fall back on.
        let mut job = step.jobs.into_iter().next().unwrap();
        job.fallback = None;
        let done = run_job_with(&job, |_, _| None);
        assert_eq!(done.outcome.status, Status::ResolveFailed);

        // "Look names up before every ping": every probe looks, and a failed lookup is a failed ping.
        let mut every = state_of(Settings { resolve_every_ping: true, ..quick_settings() }, "moved.lan");
        every.hosts[0].resolved = Some(old);
        every.hosts[0].resolved_at = Some(t0);
        assert_eq!(probe_at(&mut every, t0 + secs(1), 0), (None, None));
    }

    #[test]
    fn hosts_from_a_block_are_pinged_every_30_seconds_unless_told_otherwise() {
        let list = "192.0.2.0/30\n192.0.2.9\n192.0.2.16/30 every=5s\nGroup: Slow every=10s\n192.0.2.32/30\n192.0.2.40-192.0.2.41";
        let st = state_of(Settings { interval_secs: 1, ..quick_settings() }, list);
        let every: Vec<u32> = st.hosts.iter().map(|h| h.row(&st.settings).interval_secs).collect();
        assert_eq!(every, [30, 30, 1, 5, 5, 10, 10, 10, 10]);
        // A global interval longer than that still wins.
        let st = state_of(Settings { interval_secs: 60, ..quick_settings() }, "192.0.2.0/30 192.0.2.9");
        let every: Vec<u32> = st.hosts.iter().map(|h| h.row(&st.settings).interval_secs).collect();
        assert_eq!(every, [60, 60, 60]);
    }

    #[test]
    fn hosts_added_while_pinging_are_spread_out() {
        let engine = Engine::new(quick_settings(), "192.0.2.1");
        // No dispatcher, so the beats stay where they are put.
        engine.shutdown();
        engine.start();
        let list: Vec<String> = (1..=100).map(|n| format!("192.0.2.{n}")).collect();
        // As a new list sent to PUT /config does: every host is new.
        engine.apply_config(quick_settings(), &list.join("\n"), false);
        let due: Vec<Instant> = engine.inner.lock().hosts.iter().map(|h| h.next_due.expect("due")).collect();
        assert_eq!(due.len(), 100);
        assert!(due.windows(2).all(|w| w[1] - w[0] == START_GAP_MAX), "5 ms apart, not all at once");

        // A run that does not repeat sends them all at once, as start() does.
        engine.apply_config(Settings { repeat: false, ..quick_settings() }, &list.join("\n"), false);
        let due: Vec<Instant> = engine.inner.lock().hosts.iter().map(|h| h.next_due.expect("due")).collect();
        assert!(due.windows(2).all(|w| w[0] == w[1]));
    }

    /// The beats of the hosts that are due, in table order.
    fn due_times(engine: &Engine) -> Vec<Instant> {
        engine.inner.lock().hosts.iter().filter_map(|h| h.next_due).collect()
    }

    #[test]
    fn hosts_unmuted_soloed_or_back_from_a_measurement_are_spread_out() {
        let list: Vec<String> = (1..=100).map(|n| format!("192.0.2.{n}")).collect();
        let spread = |due: &[Instant], what: &str| {
            assert_eq!(due.len(), 100, "{what}");
            assert!(due.windows(2).all(|w| w[1] - w[0] == START_GAP_MAX), "{what}: 5 ms apart, not all at once");
        };
        // No dispatcher, so the beats stay where they are put.
        let engine = Engine::new(quick_settings(), &list.join("\n"));
        engine.shutdown();
        let ids: Vec<u32> = engine.snapshot(0).hosts.iter().map(|h| h.id).collect();

        // Unmute every host while pinging.
        engine.set_enabled_many(&ids, false);
        engine.start();
        assert!(due_times(&engine).is_empty());
        engine.unmute_all();
        spread(&due_times(&engine), "unmute all");

        // Ping only these, from stopped.
        engine.stop();
        engine.solo(&ids);
        spread(&due_times(&engine), "solo");

        // The end of a measurement puts the hosts back on their cadence.
        engine.measure_start(&ids, 60, 10);
        engine.measure_stop();
        spread(&due_times(&engine), "end of a measurement");
    }

    /// Wait up to 10 s for `done` to hold.
    fn wait_for(what: &str, done: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(Instant::now() < deadline, "gave up waiting: {what}");
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn notes_of(inner: &Inner) -> Vec<String> {
        inner.lock().notes.iter().map(|n| n.text.clone()).collect()
    }

    #[test]
    fn alert_commands_run_4_at_a_time_and_64_wait() {
        let engine = Engine::new(quick_settings(), "");
        engine.shutdown();
        let inner = engine.inner.clone();
        // A stand-in for the shell that holds every command until the gate opens.
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let ran: Arc<Mutex<Vec<(String, Duration)>>> = Arc::default();
        {
            let (gate, ran) = (gate.clone(), ran.clone());
            inner.lock_commands().run = Arc::new(move |command: &str, _: &CommandEnv, limit: Duration| -> Result<(), String> {
                ran.lock().unwrap().push((command.to_string(), limit));
                let (open, cv) = &*gate;
                let mut open = open.lock().unwrap();
                while !*open {
                    open = cv.wait(open).unwrap();
                }
                if command == "c5" {
                    Err("An alert command could not be started: test.".into())
                } else {
                    Ok(())
                }
            });
        }
        for n in 0..100 {
            queue_command(&inner, format!("c{n}"), Vec::new());
        }
        wait_for("four commands to start", || ran.lock().unwrap().len() == 4);
        {
            let queue = inner.lock_commands();
            assert_eq!((queue.running, queue.waiting.len(), queue.skipped), (4, 64, 32));
        }
        let mut first: Vec<String> = ran.lock().unwrap().iter().map(|(c, _)| c.clone()).collect();
        first.sort();
        assert_eq!(first, ["c0", "c1", "c2", "c3"]);
        assert!(notes_of(&inner).is_empty(), "nothing to report while they run");

        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        let skipped = "32 alert commands were not run because 64 others were already waiting.";
        wait_for("the queue to empty", || inner.lock_commands().running == 0 && notes_of(&inner).iter().any(|n| n == skipped));
        let ran = ran.lock().unwrap();
        let mut names: Vec<u32> = ran.iter().map(|(c, _)| c[1..].parse().unwrap()).collect();
        names.sort_unstable();
        assert_eq!(names, (0..68).collect::<Vec<u32>>(), "the first 68 ran, each once");
        assert!(ran.iter().all(|(_, limit)| *limit == Duration::from_secs(60)), "each with the 60 s limit");
        let notes = notes_of(&inner);
        assert_eq!(notes.iter().filter(|n| *n == skipped).count(), 1, "{notes:?}");
        assert!(notes.iter().any(|n| n == "An alert command could not be started: test."), "{notes:?}");
        assert!(inner.lock_commands().waiting.is_empty());
    }

    fn slow_command() -> &'static str {
        if cfg!(windows) {
            "ping -n 4 127.0.0.1 >NUL"
        } else {
            "sleep 3"
        }
    }

    #[test]
    fn an_alert_command_that_runs_too_long_is_stopped() {
        let began = Instant::now();
        let result = run_command(slow_command(), &Vec::new(), Duration::from_millis(200));
        assert!(result.as_ref().is_err_and(|why| why.ends_with("was stopped.")), "{result:?}");
        assert!(began.elapsed() < Duration::from_secs(2), "stopped after {:?}", began.elapsed());
        let quick = if cfg!(windows) { "echo ok >NUL" } else { "true" };
        assert_eq!(run_command(quick, &Vec::new(), Duration::from_secs(20)), Ok(()));

        // Through the queue, with its own limit, the person watching is told.
        let engine = Engine::new(quick_settings(), "");
        engine.shutdown();
        engine.inner.lock_commands().time_limit = Duration::from_millis(200);
        queue_command(&engine.inner, slow_command().to_string(), Vec::new());
        wait_for("the command to be stopped", || notes_of(&engine.inner).iter().any(|n| n.ends_with("was stopped.")));
    }

    /// A folder of its own for one test, removed when it is dropped.
    struct Scratch(std::path::PathBuf);

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

    #[cfg(target_os = "linux")]
    #[test]
    fn a_stopped_command_takes_its_programs_with_it() {
        let dir = Scratch::new("command-tree");
        let pid_file = dir.0.join("sleep.pid");
        // Two commands, so the shell starts sleep as a program of its own instead of becoming it.
        let command = format!("sleep 30 & echo $! > '{}'; wait; true", pid_file.display());
        let result = run_command(&command, &Vec::new(), Duration::from_millis(500));
        assert!(result.as_ref().is_err_and(|why| why.ends_with("was stopped.")), "{result:?}");
        let pid: u32 = std::fs::read_to_string(&pid_file).unwrap().trim().parse().unwrap();
        // Gone, or a zombie waiting to be collected (which has no command line left).
        let running = || std::fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|line| !line.is_empty());
        wait_for("the program the shell started to end", || !running());
    }

    /// A stop of pingtable reaches the commands that run (the signals themselves
    /// are tried on the real program, in `crates/app/tests/alert_commands.rs`).
    #[cfg(unix)]
    #[test]
    fn a_running_command_is_listed_for_the_stop_signals_until_it_ends() {
        use std::sync::atomic::Ordering::SeqCst;
        let listed = |group: i32| COMMAND_GROUPS.iter().any(|place| place.load(SeqCst) == group);
        let mut child = shell("sleep 30; true").spawn().unwrap();
        let group = child.id() as i32;
        let tree = CommandTree::hold(&child);
        assert!(listed(group), "listed while it runs");
        tree.stop(&mut child);
        let _ = child.wait();
        drop(tree);
        assert!(!listed(group), "let go once it has ended");

        let mut child = shell("true").spawn().unwrap();
        let group = child.id() as i32;
        let tree = CommandTree::hold(&child);
        let _ = child.wait();
        tree.release();
        assert!(!listed(group), "one that ended by itself is let go too");
    }

    #[cfg(windows)]
    #[test]
    fn a_stopped_command_takes_its_programs_with_it() {
        let dir = Scratch::new("command-tree");
        let out = dir.0.join("ping.txt");
        // cmd opens the file for ping, which keeps it open (and so undeletable) while it runs.
        let command = format!("ping -n 30 127.0.0.1 > \"{}\"", out.display());
        let result = run_command(&command, &Vec::new(), Duration::from_millis(1500));
        assert!(result.as_ref().is_err_and(|why| why.ends_with("was stopped.")), "{result:?}");
        let removed = || match std::fs::remove_file(&out) {
            Ok(()) => true,
            Err(why) => why.kind() == std::io::ErrorKind::NotFound,
        };
        wait_for("the program the shell started to end", removed);
    }

    #[test]
    fn jitter_starts_at_the_first_difference() {
        let mut s = HostStats::default();
        s.apply(&sample(1, true, 10_000), 1, 1);
        s.apply(&sample(2, true, 20_000), 1, 1);
        assert_eq!(s.jitter_us, 10_000.0);
        s.apply(&sample(3, true, 20_000), 1, 1);
        assert!(s.jitter_us < 10_000.0, "later samples smooth it");
    }

    fn quick_settings() -> Settings {
        Settings { timeout_ms: 500, reverse_dns: false, read_mac: false, ..Settings::default() }
    }

    #[test]
    fn single_run_probes_each_host_once_and_stops() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let open = listener.local_addr().unwrap().port();
        let closed = {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let settings = Settings { repeat: false, ..quick_settings() };
        let engine = Engine::new(settings, &format!("127.0.0.1:{open} 127.0.0.1:{closed}"));
        engine.start();
        assert!(engine.wait_idle(Duration::from_secs(10)));
        let snap = engine.snapshot(0);
        assert!(snap.full);
        assert!(!snap.running, "a no-repeat run stops by itself");
        assert_eq!(snap.hosts.len(), 2);
        assert_eq!((snap.hosts[0].sent, snap.hosts[0].state), (1, "up"));
        assert!(snap.hosts[0].last_rtt_us.is_some());
        assert_eq!((snap.hosts[1].sent, snap.hosts[1].state), (1, "down"));
        assert_eq!(snap.hosts[1].last_status, Some(Status::Refused));
        assert_eq!(snap.counts, Counts { total: 2, up: 1, down: 1, pending: 0, muted: 0 });
        assert_eq!(snap.hosts[0].key, format!("127.0.0.1|tcp:{open}"));
        assert_eq!((snap.hosts[0].line, snap.hosts[0].removable), (1, false), "two hosts share line 1");
        assert!(engine.removable_lines(&[snap.hosts[0].id]).is_empty());
        engine.shutdown();
    }

    #[test]
    fn ping_now_works_while_stopped_and_deltas_are_small() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let engine = Engine::new(quick_settings(), &format!("127.0.0.1:{port} 127.0.0.1:{port}"));
        let before = engine.snapshot(0).seq;
        engine.ping_now();
        assert!(engine.wait_idle(Duration::from_secs(10)));
        let snap = engine.snapshot(0);
        assert_eq!(snap.hosts.iter().map(|h| h.sent).collect::<Vec<_>>(), [1, 1]);
        assert_eq!(snap.hosts[1].key, format!("127.0.0.1|tcp:{port}#2"), "duplicate targets get distinct keys");
        assert!(!snap.running);

        // Disable one host; only that row comes back in the delta.
        let seq = snap.seq;
        assert!(engine.set_enabled(snap.hosts[0].id, false));
        let delta = engine.snapshot(seq);
        assert!(!delta.full);
        assert_eq!(delta.hosts.len(), 1);
        assert_eq!(delta.hosts[0].state, "muted");
        assert!(seq > before);
        engine.shutdown();
    }

    #[test]
    fn per_host_interval_beats_the_global_one() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let settings = Settings { interval_secs: 30, ..quick_settings() };
        let engine = Engine::new(settings, &format!("127.0.0.1:{port} every=1s\nlocalhost:{port}"));
        engine.start();
        thread::sleep(Duration::from_millis(2400));
        let snap = engine.snapshot(0);
        assert_eq!(snap.hosts[0].interval_secs, 1);
        assert!((3..=4).contains(&snap.hosts[0].sent), "fast host sent {}", snap.hosts[0].sent);
        assert_eq!(snap.hosts[1].sent, 1, "slow host keeps the 30 s default");
        assert!(snap.running);
        engine.stop();
        assert!(engine.wait_idle(Duration::from_secs(5)));
        engine.shutdown();
    }

    #[test]
    fn keep_stats_carries_history_across_a_config_change() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let engine = Engine::new(quick_settings(), &format!("127.0.0.1:{port}"));
        engine.ping_now();
        assert!(engine.wait_idle(Duration::from_secs(10)));
        let id = engine.snapshot(0).hosts[0].id;

        let warnings = engine.apply_config(quick_settings(), &format!("localhost:{port} 127.0.0.1:{port} oops!"), true);
        assert_eq!(warnings.len(), 1);
        let snap = engine.snapshot(0);
        assert_eq!(snap.hosts.len(), 2);
        assert_eq!((snap.hosts[0].sent, snap.hosts[0].order), (0, 1));
        assert_eq!((snap.hosts[1].id, snap.hosts[1].sent, snap.hosts[1].order), (id, 1, 2));
        assert_eq!(snap.warnings.len(), 1);

        assert_eq!(engine.removable_lines(&[id]), Vec::<usize>::new(), "it shares line 1 with another host");
        let detail = engine.detail(id).unwrap();
        assert_eq!(detail.history.len(), 1);
        assert_eq!(detail.runs.len(), 1);
        assert!(detail.p95_rtt_us.is_some());
        assert_eq!(detail.windows[0].sent, 1);

        engine.apply_config(quick_settings(), &format!("127.0.0.1:{port}"), false);
        assert_eq!(engine.snapshot(0).hosts[0].sent, 0, "without keep_stats everything starts over");
        engine.shutdown();
    }

    #[test]
    fn one_host_can_be_pinged_and_reset_on_its_own() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let engine = Engine::new(quick_settings(), &format!("127.0.0.1:{port} localhost:{port}"));
        let ids: Vec<u32> = engine.snapshot(0).hosts.iter().map(|h| h.id).collect();

        assert!(engine.ping_host(ids[1]));
        assert!(engine.wait_idle(Duration::from_secs(10)));
        assert!(engine.ping_host(ids[1]));
        assert!(engine.wait_idle(Duration::from_secs(10)));
        let snap = engine.snapshot(0);
        assert_eq!(snap.hosts.iter().map(|h| h.sent).collect::<Vec<_>>(), [0, 2], "only the chosen host was pinged");
        assert!(!snap.running);

        let seq = snap.seq;
        assert!(engine.reset_host(ids[1]));
        let delta = engine.snapshot(seq);
        assert_eq!(delta.hosts.len(), 1);
        assert_eq!((delta.hosts[0].sent, delta.hosts[0].state), (0, "pending"));
        assert!(engine.detail(ids[1]).unwrap().history.is_empty());

        assert!(!engine.ping_host(9999) && !engine.reset_host(9999));
        engine.set_enabled(ids[0], false);
        assert!(!engine.ping_host(ids[0]), "a muted host is not pinged");

        // Muting by key (how the saved list is applied at start) and reading it back.
        assert_eq!(engine.muted_keys(), [format!("127.0.0.1|tcp:{port}")]);
        engine.mute_keys(&[format!("localhost|tcp:{port}"), "gone.lan|icmp".to_string()]);
        assert_eq!(engine.muted_keys().len(), 2);
        assert_eq!(engine.snapshot(0).counts.muted, 2);
        engine.shutdown();
    }

    #[test]
    fn solo_pings_only_the_chosen_hosts() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let settings = Settings { repeat: false, ..quick_settings() };
        let engine = Engine::new(settings, &format!("127.0.0.1:{port} localhost:{port} 127.0.0.1:{port}"));
        let ids: Vec<u32> = engine.snapshot(0).hosts.iter().map(|h| h.id).collect();

        // Stopped, then start just one host.
        assert_eq!(engine.solo(&[ids[1]]), 1);
        assert!(engine.wait_idle(Duration::from_secs(10)));
        let snap = engine.snapshot(0);
        assert_eq!(snap.hosts.iter().map(|h| h.sent).collect::<Vec<_>>(), [0, 1, 0]);
        assert_eq!(snap.hosts.iter().map(|h| h.state).collect::<Vec<_>>(), ["muted", "up", "muted"]);
        assert_eq!(snap.counts.muted, 2);

        assert_eq!(engine.set_enabled_many(&[ids[0], ids[1]], true), 1, "only the muted one changes");
        assert_eq!(engine.set_enabled_many(&[ids[0], ids[1]], false), 2);
        engine.unmute_all();
        assert_eq!(engine.snapshot(0).counts.muted, 0);
        assert!(engine.muted_keys().is_empty());
        engine.shutdown();
    }

    #[test]
    fn measurement_runs_at_its_own_rate_for_a_set_time() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let engine = Engine::new(quick_settings(), &format!("127.0.0.1:{port} localhost:{port}"));
        let ids: Vec<u32> = engine.snapshot(0).hosts.iter().map(|h| h.id).collect();
        assert!(engine.measure_report().is_none());

        // Not pinging, yet a measurement of one host at 10 a second for 1 second runs.
        let id = engine.measure_start(&[ids[0]], 1, 10).unwrap();
        let live = engine.snapshot(0);
        assert_eq!((live.measure_hosts, live.measure_ends_at_ms.is_some()), (1, true));
        thread::sleep(Duration::from_millis(1400));
        let report = engine.measure_report().unwrap();
        assert_eq!((report.id, report.running, report.per_second, report.duration_secs), (id, false, 10, 1));
        assert_eq!(report.hosts.len(), 1);
        let h = &report.hosts[0];
        assert!((8..=11).contains(&h.sent), "sent {}", h.sent);
        assert_eq!(h.lost, 0);
        assert!(h.median_rtt_us.is_some() && h.p95_rtt_us >= h.median_rtt_us && h.max_rtt_us >= h.p95_rtt_us);
        assert!(h.jitter_us.is_some());

        // Afterwards the host is quiet again and the other host was never touched.
        let after = engine.snapshot(0);
        let sent_then = after.hosts[0].sent;
        assert_eq!((after.hosts[1].sent, after.measure_hosts, after.running), (0, 0, false));
        thread::sleep(Duration::from_millis(400));
        assert_eq!(engine.snapshot(0).hosts[0].sent, sent_then);

        // Stopping early keeps what was collected; muted hosts cannot be measured.
        engine.measure_start(&ids, 60, 5).unwrap();
        thread::sleep(Duration::from_millis(500));
        engine.measure_stop();
        let early = engine.measure_report().unwrap();
        assert!(!early.running && early.ended_at_ms.unwrap() < early.ends_at_ms);
        assert_eq!(early.hosts.len(), 2);
        // The main Stop ends a measurement as well, and nothing is pinged afterwards.
        engine.start();
        engine.measure_start(&ids, 60, 5).unwrap();
        thread::sleep(Duration::from_millis(300));
        engine.stop();
        let stopped = engine.snapshot(0);
        assert_eq!((stopped.running, stopped.measure_hosts, stopped.running_since_ms), (false, 0, None));
        assert!(!engine.measure_report().unwrap().running);
        assert!(engine.wait_idle(Duration::from_secs(5)));
        let sent: Vec<u64> = engine.snapshot(0).hosts.iter().map(|h| h.sent).collect();
        thread::sleep(Duration::from_millis(500));
        assert_eq!(engine.snapshot(0).hosts.iter().map(|h| h.sent).collect::<Vec<_>>(), sent);

        engine.set_enabled(ids[0], false);
        engine.set_enabled(ids[1], false);
        assert!(engine.measure_start(&ids, 5, 1).is_none());
        engine.shutdown();
    }

    #[test]
    fn probe_threads_are_reused_not_multiplied() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let list: Vec<String> = (0..4).map(|_| format!("127.0.0.1:{port}")).collect();
        let engine = Engine::new(quick_settings(), &list.join(" "));
        for _ in 0..25 {
            engine.ping_now();
            assert!(engine.wait_idle(Duration::from_secs(10)));
        }
        let pool = engine.inner.lock_pool();
        assert!(pool.workers <= 4, "{} probe threads for 4 hosts", pool.workers);
        assert_eq!(pool.busy, 0);
        drop(pool);
        assert!(engine.snapshot(0).hosts.iter().all(|h| h.sent == 25));
        engine.shutdown();
    }

    #[test]
    fn concurrency_cap_still_finishes_every_host() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let list: Vec<String> = (0..40).map(|_| format!("127.0.0.1:{port}")).collect();
        let settings = Settings { repeat: false, max_concurrent: 3, ..quick_settings() };
        let engine = Engine::new(settings, &list.join(" "));
        engine.start();
        assert!(engine.wait_idle(Duration::from_secs(20)));
        let snap = engine.snapshot(0);
        assert_eq!(snap.hosts.len(), 40);
        assert!(snap.hosts.iter().all(|h| h.sent == 1), "every host probed exactly once");
        assert_eq!(snap.active, 0);
        engine.shutdown();
    }
}
