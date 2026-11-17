//! Scheduler and per-host state.
//!
//! Every host has its own cadence: it is probed every `interval` seconds,
//! measured start to start, under one global cap on probes in flight. A host
//! whose probe is still running is never probed twice at once; if a beat is
//! missed it is skipped rather than made up in a burst.

use crate::model::{AlertEvent, AlertKind, Outcome, Overrides, Probe, Run, Sample, Settings, Status};
use crate::parse::{parse_host_list, ParseOptions, ParsedHost};
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
const WORKER_IDLE: Duration = Duration::from_secs(20);
const HOUSEKEEPING_EVERY: Duration = Duration::from_secs(3);

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
                    self.jitter_us += (diff - self.jitter_us) / 16.0;
                }
                self.prev_rtt_us = Some(rtt);
            }
            if self.down && self.consec_ok >= recover_trigger.max(1) {
                self.down = false;
                return Some(AlertKind::Up);
            }
        } else {
            self.failed += 1;
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

    pub fn failed_pct(&self) -> f32 {
        if self.sent == 0 {
            0.0
        } else {
            self.failed as f32 * 100.0 / self.sent as f32
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
    /// The line of the host list this host came from, counting from 1.
    pub line: u32,
    /// True when that line defines only this host, so removing the line removes just it.
    pub removable: bool,
    /// Set when this host's pings are customised, for example `1400 bytes · DF · TTL 8`.
    pub packet: Option<String>,
    /// `up`, `down`, `failing` (failed but under the down threshold),
    /// `recovering`, `pending` (no result yet) or `muted` (not pinged).
    pub state: &'static str,
    pub sent: u64,
    pub ok: u64,
    pub failed: u64,
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
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct WindowStats {
    pub label: &'static str,
    pub sent: u32,
    pub failed: u32,
    pub loss_pct: f32,
    pub avg_rtt_us: Option<u32>,
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
    pub sent: u32,
    pub lost: u32,
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

struct MeasureAcc {
    host: String,
    description: String,
    sent: u32,
    lost: u32,
    rtts: Vec<u32>,
}

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
                MeasureHost {
                    id,
                    host: acc.host.clone(),
                    description: acc.description.clone(),
                    sent: acc.sent,
                    lost: acc.lost,
                    loss_pct: if acc.sent == 0 { 0.0 } else { acc.lost as f32 * 100.0 / acc.sent as f32 },
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
    reply_ip: Option<IpAddr>,
    hostname: Option<String>,
    rdns_tried: bool,
    mac: Option<String>,
    stats: HostStats,
    history: VecDeque<Sample>,
    runs: VecDeque<Run>,
    seq: u64,
    next_due: Option<Instant>,
    in_flight: bool,
    /// Set while a measurement is pinging this host at its own rate.
    measure_interval: Option<Duration>,
    line: u32,
    alone_on_line: bool,
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
            reply_ip: None,
            hostname: None,
            rdns_tried: false,
            mac: None,
            stats: HostStats::default(),
            history: VecDeque::new(),
            runs: VecDeque::new(),
            seq: 0,
            next_due: None,
            in_flight: false,
            measure_interval: None,
            line: parsed.line as u32,
            alone_on_line: true,
        }
    }

    fn ip(&self) -> Option<IpAddr> {
        self.literal.or(self.resolved)
    }

    fn interval(&self, settings: &Settings) -> Duration {
        Duration::from_secs(self.overrides.interval_secs.unwrap_or(settings.interval_secs).max(1) as u64)
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
        let recent_failed = recent.iter().filter(|s| !s.status.is_ok()).count();
        let recent_loss_pct = if recent.is_empty() { 0.0 } else { recent_failed as f32 * 100.0 / recent.len() as f32 };
        let spark = recent.iter().rev().map(|s| if s.status.is_ok() { s.rtt_us } else { None }).collect();
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
            interval_secs: self.interval(settings).as_secs() as u32,
            timeout_ms: self.timeout_ms(settings),
            line: self.line,
            removable: self.alone_on_line,
            packet: self.packet_note(),
            state: self.state_name(),
            sent: stats.sent,
            ok: stats.ok,
            failed: stats.failed,
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
    events: VecDeque<AlertEvent>,
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

    fn snapshot(&self, since: u64) -> Snapshot {
        let full = since < self.list_seq || since > self.seq;
        Snapshot {
            schema_version: SCHEMA_VERSION,
            seq: self.seq,
            now_ms: now_ms(),
            running: self.running || self.single_pass,
            active: self.active,
            running_since_ms: self.running_since_ms,
            measure_ends_at_ms: self.measure.as_ref().filter(|m| m.running()).map(|m| m.ends_at_ms),
            measure_hosts: self.measure.as_ref().filter(|m| m.running()).map_or(0, |m| m.order.len() as u32),
            full,
            counts: self.counts(),
            hosts: self.hosts.iter().filter(|h| full || h.seq > since).map(|h| h.row(&self.settings)).collect(),
            events: self.events.iter().filter(|e| !full && e.seq > since).cloned().collect(),
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
    addr: Option<IpAddr>,
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

struct Inner {
    state: Mutex<State>,
    /// Wakes the dispatcher: something became due or a slot freed up.
    sched_cv: Condvar,
    /// Wakes observers waiting for the sequence number to move.
    change_cv: Condvar,
    pool: Mutex<PoolQueue>,
    pool_cv: Condvar,
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock_pool(&self) -> MutexGuard<'_, PoolQueue> {
        self.pool.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
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
                events: VecDeque::new(),
                warnings: Vec::new(),
                running_since_ms: None,
                measure: None,
                next_measure_id: 1,
            }),
            sched_cv: Condvar::new(),
            change_cv: Condvar::new(),
            pool: Mutex::new(PoolQueue { jobs: VecDeque::new(), workers: 0, busy: 0, shutdown: false }),
            pool_cv: Condvar::new(),
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
                    kept.line = parsed.line as u32;
                    kept
                }
                None => {
                    let id = st.next_id;
                    st.next_id += 1;
                    let mut fresh = HostState::new(id, key, order, parsed);
                    if running {
                        fresh.next_due = Some(now);
                    }
                    fresh
                }
            };
            hosts.push(host);
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
        for h in st.hosts.iter_mut().filter(|h| h.enabled && h.next_due.is_none()) {
            h.next_due = Some(now);
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
        for h in st.hosts.iter_mut() {
            h.next_due = None;
        }
        st.bump();
        drop(st);
        self.inner.sched_cv.notify_all();
        self.inner.change_cv.notify_all();
    }

    /// Probe every enabled host right away, whether or not pinging is on.
    pub fn ping_now(&self) {
        let mut st = self.inner.lock();
        if !st.running {
            st.single_pass = true;
        }
        let now = Instant::now();
        for h in st.hosts.iter_mut().filter(|h| h.enabled) {
            h.next_due = Some(now);
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
        let mut changed = 0;
        for h in st.hosts.iter_mut().filter(|h| h.enabled != enabled && ids.contains(&h.id)) {
            h.enabled = enabled;
            h.next_due = (enabled && running).then_some(now);
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
                h.next_due = Some(now);
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
        if !st.running {
            st.single_pass = true;
        }
        st.hosts[idx].next_due = Some(Instant::now());
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
            hosts.insert(h.id, MeasureAcc { host: h.host.clone(), description: h.description.clone(), sent: 0, lost: 0, rtts: Vec::new() });
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
        self.inner.lock().measure.as_ref().map(Measurement::report)
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
            let scheduled = (st.running || st.single_pass) && st.hosts.iter().any(|h| h.next_due.is_some());
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
        let windows = [("1m", 60_000i64), ("5m", 300_000), ("1h", 3_600_000), ("24h", 86_400_000)]
            .iter()
            .map(|&(label, span)| {
                let mut w = WindowStats { label, sent: 0, failed: 0, loss_pct: 0.0, avg_rtt_us: None };
                let mut sum = 0u64;
                let mut timed = 0u64;
                for s in h.history.iter().rev().take_while(|s| now - s.t_ms <= span) {
                    w.sent += 1;
                    if !s.status.is_ok() {
                        w.failed += 1;
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
    for h in st.hosts.iter_mut().filter(|h| h.measure_interval.is_some()) {
        h.measure_interval = None;
        h.next_due = (running && h.enabled).then_some(now);
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
        let now = Instant::now();
        let mut earliest: Option<Instant> = None;
        let mut jobs: Vec<Job> = Vec::new();

        // A measurement ends on time even if nothing else wakes this thread.
        let measure_ends = st.measure.as_ref().filter(|m| m.running()).map(|m| m.ends);
        if let Some(ends) = measure_ends {
            if now >= ends {
                end_measurement(&mut st);
                st.bump();
                inner.change_cv.notify_all();
            } else {
                earliest = Some(ends);
            }
        }
        let measuring = st.measure.as_ref().is_some_and(|m| m.running());

        if st.running || st.single_pass || measuring {
            let repeat = st.running && st.settings.repeat;
            let cap = st.settings.max_concurrent;
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
            let State { hosts, settings, active, .. } = &mut *st;
            for h in hosts.iter_mut() {
                let Some(due) = h.next_due else { continue };
                if !h.enabled {
                    h.next_due = None;
                    continue;
                }
                any_scheduled = true;
                if h.in_flight {
                    continue;
                }
                if due > now {
                    earliest = Some(earliest.map_or(due, |e| e.min(due)));
                    continue;
                }
                if *active >= cap {
                    continue; // a finishing probe wakes us
                }
                h.in_flight = true;
                *active += 1;
                h.next_due = if repeat || h.measure_interval.is_some() {
                    let interval = h.cadence(settings);
                    let next = due + interval;
                    Some(if next <= now { now + interval } else { next })
                } else {
                    None
                };
                if let Some(next) = h.next_due {
                    earliest = Some(earliest.map_or(next, |e| e.min(next)));
                }
                jobs.push(Job {
                    id: h.id,
                    host: h.host.clone(),
                    addr: h.literal.or(if resolve_every { None } else { h.resolved }),
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
            if !any_scheduled && st.active == 0 && jobs.is_empty() {
                // Nothing left to do: a single pass or a no-repeat run is over.
                let was_active = st.single_pass || !st.settings.repeat;
                st.single_pass = false;
                if !st.settings.repeat {
                    st.running = false;
                }
                if was_active {
                    st.bump();
                    inner.change_cv.notify_all();
                }
            }
            if !jobs.is_empty() {
                let cap = cap as usize;
                submit(&inner, jobs, cap);
            }
        }

        st = match earliest {
            Some(when) => {
                let wait = when.saturating_duration_since(Instant::now()).max(Duration::from_millis(1));
                inner.sched_cv.wait_timeout(st, wait).unwrap_or_else(|p| p.into_inner()).0
            }
            None => inner.sched_cv.wait(st).unwrap_or_else(|p| p.into_inner()),
        };
    }
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
        // A fault inside one probe is recorded as an error for that host. Without
        // this the thread would die still counted as busy, and the pool would shrink.
        let (outcome, resolved) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_job(&job)))
            .unwrap_or((Outcome::fail(Status::Error), None));

        // Count this thread as free before the result is published: publishing
        // wakes the dispatcher, which may hand out the next job at once and
        // must not start an extra thread for it.
        inner.lock_pool().busy -= 1;

        let command = {
            let mut st = inner.lock();
            apply_result(&mut st, &job, outcome, resolved, t_ms)
        };
        inner.sched_cv.notify_all();
        inner.change_cv.notify_all();
        if let Some((command, env)) = command {
            run_command(&command, &env);
        }

        pool = inner.lock_pool();
    }
}

fn run_job(job: &Job) -> (Outcome, Option<IpAddr>) {
    let ip = match job.addr {
        Some(ip) => ip,
        None => match netinfo::resolve(&job.host, job.common.prefer_ipv6) {
            Some(ip) => ip,
            None => return (Outcome::fail(Status::ResolveFailed), None),
        },
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
    (outcome, Some(ip))
}

type CommandEnv = Vec<(&'static str, String)>;

fn apply_result(
    st: &mut State,
    job: &Job,
    outcome: Outcome,
    resolved: Option<IpAddr>,
    t_ms: i64,
) -> Option<(String, CommandEnv)> {
    st.active = st.active.saturating_sub(1);
    let idx = *st.index.get(&job.id)?;
    let seq = st.bump();
    let fail_trigger = st.settings.fail_trigger;
    let recover_trigger = st.settings.recover_trigger;
    let history_cap = st.settings.history_per_host as usize;

    let h = &mut st.hosts[idx];
    h.in_flight = false;
    h.seq = seq;
    if let Some(ip) = resolved {
        if h.literal.is_none() && h.resolved != Some(ip) {
            h.resolved = Some(ip);
            h.mac = None;
        }
    }
    if outcome.reply_addr.is_some() {
        h.reply_ip = outcome.reply_addr;
    }
    let sample = Sample { t_ms, status: outcome.status, rtt_us: outcome.rtt_us, ttl: outcome.ttl };
    let alert = h.stats.apply(&sample, fail_trigger, recover_trigger);
    h.record(sample, history_cap);
    if let Some(acc) = st.measure.as_mut().filter(|m| m.running() && t_ms >= m.started_at_ms).and_then(|m| m.hosts.get_mut(&job.id)) {
        acc.sent += 1;
        match outcome.rtt_us {
            Some(rtt) if outcome.status.is_ok() => acc.rtts.push(rtt),
            _ => acc.lost += 1,
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

/// Replace `%Name%` variables. Unknown names are left as written.
///
/// The command line is run by a shell, and the values (a description from an
/// imported file, say) are not always typed by the person who wrote the
/// command. So every character a shell could act on is replaced with `_`
/// before it goes in. The `PINGTABLE_` environment variables carry the exact
/// text for commands that need it.
fn substitute(template: &str, env: &CommandEnv) -> String {
    let mut out = template.to_string();
    for (name, value) in env {
        out = out.replace(&format!("%{name}%"), &shell_safe(value));
    }
    out
}

fn shell_safe(value: &str) -> String {
    value.chars().map(|c| if c.is_alphanumeric() || " ._:@/+,=-".contains(c) { c } else { '_' }).collect()
}

fn run_command(command: &str, env: &CommandEnv) {
    let mut cmd = if cfg!(windows) {
        let mut c = std::process::Command::new("cmd");
        c.arg("/C").arg(command);
        c
    } else {
        let mut c = std::process::Command::new("sh");
        c.arg("-c").arg(command);
        c
    };
    for (name, value) in env {
        cmd.env(format!("PINGTABLE_{}", name.to_ascii_uppercase()), value);
    }
    cmd.stdin(std::process::Stdio::null());
    if let Ok(mut child) = cmd.spawn() {
        // Reap it so finished commands do not pile up as zombies.
        let _ = thread::Builder::new().name("pingtable-command".into()).spawn(move || {
            let _ = child.wait();
        });
    }
}

/// Fills in MAC addresses and reverse-DNS names in the background.
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
            if st.seq == last_seq {
                continue;
            }
            last_seq = st.seq;
            let candidates: Vec<(u32, IpAddr)> = if st.settings.reverse_dns {
                st.hosts
                    .iter()
                    .filter(|h| h.hostname.is_none() && !h.rdns_tried && h.stats.sent > 0)
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
                    candidates.iter().map(|&(id, ip)| scope.spawn(move || (id, netinfo::reverse(ip)))).collect();
                handles.into_iter().filter_map(|h| h.join().ok()).collect()
            });
            let mut st = inner.lock();
            let seq = st.bump();
            for (id, name) in names {
                if let Some(&idx) = st.index.get(&id) {
                    let h = &mut st.hosts[idx];
                    h.rdns_tried = true;
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
        assert!(line.starts_with("echo x _ calc.exe _ del _._ _ out _id_ __id_ _PATH_ _q_ _ rm -rf _"), "{line}");
        assert!(!line.contains(['&', '|', '>', '<', '`', '$', '%', '"', ';', '\n', '!', '~', '*', '(', ')']), "{line}");
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
