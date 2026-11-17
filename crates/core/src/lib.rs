//! pingtable-core: everything that is not user interface.
//!
//! * [`parse`] turns a typed host list into targets.
//! * [`probe`] sends one ICMP echo or one TCP connect and reports the outcome.
//! * [`engine`] schedules rounds of probes and keeps per-host statistics.
//! * [`export`] renders the current table as JSON or CSV.
//! * [`portcheck`] tries a few TCP ports of one host, once, on request.

pub mod engine;
pub mod export;
pub mod hostlist;
pub mod model;
pub mod netinfo;
pub mod parse;
pub mod portcheck;
pub mod probe;
pub mod trace;

pub use engine::{Engine, HostDetail, HostRow, MeasureReport, Snapshot};
pub use model::{Outcome, Probe, Sample, Settings, Status};

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
