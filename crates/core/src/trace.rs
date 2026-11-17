//! Traceroute: find the routers between here and a host.
//!
//! One ICMP echo is sent for every hop count from 1 up, all at the same time.
//! A router that drops a packet because its hop count ran out says so, which
//! names that router; the first echo that reaches the host ends the path.

use crate::model::Status;
use crate::netinfo;
use crate::probe::{self, ProbeParams};
use serde::Serialize;
use std::net::IpAddr;
use std::thread;
use std::time::{Duration, Instant};

pub const MAX_HOPS: u8 = 30;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Hop {
    /// Hop number, starting at 1.
    pub ttl: u8,
    /// The router (or the host itself) that answered. None when nothing answered.
    pub addr: Option<IpAddr>,
    /// Name found by reverse lookup, when asked for.
    pub name: Option<String>,
    pub rtt_us: Option<u32>,
    pub status: Status,
    /// True for the hop that is the destination.
    pub reached: bool,
}

/// Trace the path to `target`. Takes about one `timeout`, because every hop
/// is probed at once.
pub fn traceroute(target: IpAddr, max_hops: u8, timeout: Duration, resolve_names: bool) -> Vec<Hop> {
    let max_hops = max_hops.clamp(1, MAX_HOPS);
    let probe_hop = |ttl: u8| -> Hop {
        let params = ProbeParams { timeout, ttl: Some(ttl), ..ProbeParams::default() };
        let start = Instant::now();
        let outcome = probe::icmp(target, &params);
        let elapsed = start.elapsed().as_micros().min(u32::MAX as u128) as u32;
        let reached = outcome.status.is_ok();
        let answered = reached || outcome.reply_addr.is_some();
        Hop {
            ttl,
            addr: if reached { Some(target) } else { outcome.reply_addr },
            name: None,
            rtt_us: if reached { outcome.rtt_us } else { answered.then_some(elapsed) },
            status: outcome.status,
            reached,
        }
    };

    let mut hops: Vec<Hop> = thread::scope(|scope| {
        let handles: Vec<_> = (1..=max_hops).map(|ttl| scope.spawn(move || probe_hop(ttl))).collect();
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    });
    hops.sort_by_key(|h| h.ttl);

    // The path ends at the first hop that reached the host. If none did, keep
    // a few silent hops after the last router that answered and drop the rest.
    if let Some(end) = hops.iter().position(|h| h.reached) {
        hops.truncate(end + 1);
    } else {
        let last_answer = hops.iter().rposition(|h| h.addr.is_some()).map_or(0, |i| i + 1);
        hops.truncate((last_answer + 3).min(hops.len()));
    }

    if resolve_names {
        let names: Vec<Option<String>> = thread::scope(|scope| {
            let handles: Vec<_> = hops.iter().map(|h| h.addr).map(|addr| scope.spawn(move || addr.and_then(netinfo::reverse))).collect();
            handles.into_iter().map(|h| h.join().ok().flatten()).collect()
        });
        for (hop, name) in hops.iter_mut().zip(names) {
            hop.name = name;
        }
    }
    hops
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_is_one_hop_away() {
        let hops = traceroute("127.0.0.1".parse().unwrap(), 5, Duration::from_millis(1000), false);
        assert_eq!(hops.len(), 1, "{hops:?}");
        assert!(hops[0].reached);
        assert_eq!((hops[0].ttl, hops[0].addr), (1, Some("127.0.0.1".parse().unwrap())));
        assert!(hops[0].rtt_us.is_some());
    }
}
