//! One probe: an ICMP echo or a TCP connect.
//!
//! Round-trip time is always measured here with a monotonic clock around the
//! call, because the Windows ICMP API only reports whole milliseconds.

use crate::model::{Outcome, Status};
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct ProbeParams {
    pub timeout: Duration,
    pub payload_size: u16,
    pub ttl: Option<u8>,
    pub dont_fragment: bool,
    /// Type-of-service byte (DSCP in the top six bits). 0 leaves it unmarked.
    pub tos: u8,
    /// Bytes repeated to fill the ICMP payload. Zeros when unset.
    pub payload: Option<Vec<u8>>,
    pub source_v4: Option<Ipv4Addr>,
    pub source_v6: Option<Ipv6Addr>,
}

impl ProbeParams {
    /// The ICMP payload: `payload_size` bytes, the pattern repeated, or zeros.
    pub fn payload_bytes(&self) -> Vec<u8> {
        let size = self.payload_size as usize;
        match self.payload.as_deref() {
            Some(pattern) if !pattern.is_empty() => pattern.iter().copied().cycle().take(size).collect(),
            _ => vec![0u8; size],
        }
    }
}

impl Default for ProbeParams {
    fn default() -> Self {
        ProbeParams {
            timeout: Duration::from_millis(1000),
            payload_size: 32,
            ttl: None,
            dont_fragment: false,
            tos: 0,
            payload: None,
            source_v4: None,
            source_v6: None,
        }
    }
}

fn elapsed_us(start: Instant) -> u32 {
    start.elapsed().as_micros().min(u32::MAX as u128) as u32
}

/// Open a TCP connection and close it again. Nothing is sent.
pub fn tcp(addr: SocketAddr, params: &ProbeParams) -> Outcome {
    use socket2::{Domain, Protocol, Socket, Type};

    let socket = match Socket::new(Domain::for_address(addr), Type::STREAM, Some(Protocol::TCP)) {
        Ok(s) => s,
        Err(e) => return io_failure(&e),
    };
    let source: Option<IpAddr> = match addr {
        SocketAddr::V4(_) => params.source_v4.map(IpAddr::V4),
        SocketAddr::V6(_) => params.source_v6.map(IpAddr::V6),
    };
    if let Some(ip) = source {
        if let Err(e) = socket.bind(&SocketAddr::new(ip, 0).into()) {
            return io_failure(&e);
        }
    }
    let start = Instant::now();
    match socket.connect_timeout(&addr.into(), params.timeout) {
        Ok(()) => Outcome {
            status: Status::Ok,
            os_code: None,
            rtt_us: Some(elapsed_us(start)),
            ttl: None,
            reply_addr: Some(addr.ip()),
        },
        Err(e) => io_failure(&e),
    }
}

fn io_failure(e: &io::Error) -> Outcome {
    let status = match e.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => Status::Timeout,
        io::ErrorKind::ConnectionRefused => Status::Refused,
        io::ErrorKind::HostUnreachable => Status::HostUnreachable,
        io::ErrorKind::NetworkUnreachable => Status::NetUnreachable,
        io::ErrorKind::AddrNotAvailable | io::ErrorKind::InvalidInput => Status::BadDestination,
        _ => Status::Error,
    };
    Outcome { status, os_code: e.raw_os_error(), rtt_us: None, ttl: None, reply_addr: None }
}

/// Send one ICMP echo request and wait for the reply.
pub fn icmp(addr: IpAddr, params: &ProbeParams) -> Outcome {
    imp::icmp(addr, params)
}

// ---------------------------------------------------------------------------
// Windows: the IP Helper ICMP API. It needs no administrator rights.
// ---------------------------------------------------------------------------
#[cfg(windows)]
mod imp {
    use super::*;
    use std::ffi::c_void;
    use std::mem;
    use std::ptr;
    use windows_sys::Win32::Foundation::{GetLastError, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        Icmp6CreateFile, Icmp6SendEcho2, IcmpCloseHandle, IcmpCreateFile, IcmpSendEcho, IcmpSendEcho2Ex,
        ICMP_ECHO_REPLY, IP_OPTION_INFORMATION,
    };
    use windows_sys::Win32::Networking::WinSock::{AF_INET6, SOCKADDR_IN6};

    const IP_FLAG_DF: u8 = 0x2;
    const IP_STATUS_BASE: u32 = 11000;

    struct IcmpHandle(HANDLE);

    impl Drop for IcmpHandle {
        fn drop(&mut self) {
            unsafe {
                IcmpCloseHandle(self.0);
            }
        }
    }

    fn map_ip_status(code: u32) -> Status {
        match code {
            0 => Status::Ok,
            11002 => Status::NetUnreachable,
            11003 => Status::HostUnreachable,
            11004 => Status::ProtocolUnreachable,
            11005 => Status::PortUnreachable,
            11009 => Status::PacketTooBig,
            11010 => Status::Timeout,
            11013 | 11014 => Status::TtlExpired,
            11018 => Status::BadDestination,
            11050 => Status::GeneralFailure,
            _ => Status::Error,
        }
    }

    fn options(params: &ProbeParams, default_ttl: u8) -> Option<IP_OPTION_INFORMATION> {
        if params.ttl.is_none() && !params.dont_fragment && params.tos == 0 {
            return None;
        }
        Some(IP_OPTION_INFORMATION {
            Ttl: params.ttl.unwrap_or(default_ttl),
            Tos: params.tos,
            Flags: if params.dont_fragment { IP_FLAG_DF } else { 0 },
            OptionsSize: 0,
            OptionsData: ptr::null_mut(),
        })
    }

    fn timeout_ms(params: &ProbeParams) -> u32 {
        params.timeout.as_millis().clamp(1, u32::MAX as u128) as u32
    }

    pub fn icmp(addr: IpAddr, params: &ProbeParams) -> Outcome {
        match addr {
            IpAddr::V4(ip) => icmp_v4(ip, params),
            IpAddr::V6(ip) => icmp_v6(ip, params),
        }
    }

    fn icmp_v4(ip: Ipv4Addr, params: &ProbeParams) -> Outcome {
        let handle = unsafe { IcmpCreateFile() };
        if handle == INVALID_HANDLE_VALUE || handle.is_null() {
            return Outcome::fail_code(Status::Error, unsafe { GetLastError() } as i32);
        }
        let handle = IcmpHandle(handle);

        let payload = params.payload_bytes();
        // Room for the reply header, the echoed payload and an ICMP error message.
        let reply_len = mem::size_of::<ICMP_ECHO_REPLY>() + payload.len() + 64;
        let mut reply = vec![0u64; reply_len.div_ceil(8)];
        let opts = options(params, 128);
        let opts_ptr = opts.as_ref().map_or(ptr::null(), |o| o as *const IP_OPTION_INFORMATION);
        let dest = u32::from_ne_bytes(ip.octets());

        let start = Instant::now();
        let replies = unsafe {
            match params.source_v4 {
                Some(src) => IcmpSendEcho2Ex(
                    handle.0,
                    ptr::null_mut(),
                    None,
                    ptr::null(),
                    u32::from_ne_bytes(src.octets()),
                    dest,
                    payload.as_ptr() as *const c_void,
                    payload.len() as u16,
                    opts_ptr,
                    reply.as_mut_ptr() as *mut c_void,
                    (reply.len() * 8) as u32,
                    timeout_ms(params),
                ),
                None => IcmpSendEcho(
                    handle.0,
                    dest,
                    payload.as_ptr() as *const c_void,
                    payload.len() as u16,
                    opts_ptr,
                    reply.as_mut_ptr() as *mut c_void,
                    (reply.len() * 8) as u32,
                    timeout_ms(params),
                ),
            }
        };
        let rtt_us = elapsed_us(start);
        let last_error = if replies == 0 { unsafe { GetLastError() } } else { 0 };

        // The buffer is u64-aligned and at least one ICMP_ECHO_REPLY long.
        let echo: &ICMP_ECHO_REPLY = unsafe { &*(reply.as_ptr() as *const ICMP_ECHO_REPLY) };
        if replies == 0 {
            let code = if echo.Status >= IP_STATUS_BASE { echo.Status } else { last_error };
            let status = match map_ip_status(code) {
                Status::Ok => Status::Error,
                other => other,
            };
            // An ICMP error (for example TTL expired) still names the router that sent it.
            let reply_addr = (echo.Address != 0).then(|| IpAddr::V4(Ipv4Addr::from(echo.Address.to_ne_bytes())));
            return Outcome { reply_addr, ..Outcome::fail_code(status, code as i32) };
        }
        let reply_addr = Some(IpAddr::V4(Ipv4Addr::from(echo.Address.to_ne_bytes())));
        let status = map_ip_status(echo.Status);
        if status.is_ok() {
            Outcome { status, os_code: None, rtt_us: Some(rtt_us), ttl: Some(echo.Options.Ttl), reply_addr }
        } else {
            Outcome { status, os_code: Some(echo.Status as i32), rtt_us: None, ttl: None, reply_addr }
        }
    }

    fn sockaddr_v6(ip: Ipv6Addr) -> SOCKADDR_IN6 {
        let mut sa: SOCKADDR_IN6 = unsafe { mem::zeroed() };
        sa.sin6_family = AF_INET6;
        sa.sin6_addr.u.Byte = ip.octets();
        sa
    }

    fn icmp_v6(ip: Ipv6Addr, params: &ProbeParams) -> Outcome {
        let handle = unsafe { Icmp6CreateFile() };
        if handle == INVALID_HANDLE_VALUE || handle.is_null() {
            return Outcome::fail_code(Status::Error, unsafe { GetLastError() } as i32);
        }
        let handle = IcmpHandle(handle);

        let payload = params.payload_bytes();
        // ICMPV6_ECHO_REPLY is 36 bytes: address (26, packed), pad, status, round-trip time.
        let mut reply = vec![0u8; 36 + payload.len() + 64];
        let source = sockaddr_v6(params.source_v6.unwrap_or(Ipv6Addr::UNSPECIFIED));
        let dest = sockaddr_v6(ip);
        let opts = options(params, 255);
        let opts_ptr = opts.as_ref().map_or(ptr::null(), |o| o as *const IP_OPTION_INFORMATION);

        let start = Instant::now();
        let replies = unsafe {
            Icmp6SendEcho2(
                handle.0,
                ptr::null_mut(),
                None,
                ptr::null(),
                &source,
                &dest,
                payload.as_ptr() as *const c_void,
                payload.len() as u16,
                opts_ptr,
                reply.as_mut_ptr() as *mut c_void,
                reply.len() as u32,
                timeout_ms(params),
            )
        };
        let rtt_us = elapsed_us(start);
        let last_error = if replies == 0 { unsafe { GetLastError() } } else { 0 };

        let reply_status = u32::from_ne_bytes([reply[28], reply[29], reply[30], reply[31]]);
        if replies == 0 {
            let code = if reply_status >= IP_STATUS_BASE { reply_status } else { last_error };
            let status = match map_ip_status(code) {
                Status::Ok => Status::Error,
                other => other,
            };
            return Outcome::fail_code(status, code as i32);
        }
        // sin6_port (2) and sin6_flowinfo (4) come first, then the 16 address bytes.
        let mut octets = [0u8; 16];
        octets.copy_from_slice(&reply[6..22]);
        let reply_addr = Some(IpAddr::V6(Ipv6Addr::from(octets)));
        let status = map_ip_status(reply_status);
        if status.is_ok() {
            Outcome { status, os_code: None, rtt_us: Some(rtt_us), ttl: None, reply_addr }
        } else {
            Outcome { status, os_code: Some(reply_status as i32), rtt_us: None, ttl: None, reply_addr }
        }
    }
}

// ---------------------------------------------------------------------------
// Everything else: run the system `ping` once. Raw ICMP needs privileges on
// Linux, and the system tool already has them. This path exists for
// development and for running the web flavour on a Linux box.
// ---------------------------------------------------------------------------
#[cfg(not(windows))]
mod imp {
    use super::*;
    use std::process::{Command, Stdio};

    pub fn icmp(addr: IpAddr, params: &ProbeParams) -> Outcome {
        let wait_secs = params.timeout.as_secs_f64().ceil().max(1.0) as u64;
        let mut cmd = Command::new("ping");
        cmd.arg("-n").arg("-c").arg("1").arg("-W").arg(wait_secs.to_string());
        cmd.arg("-s").arg(params.payload_size.to_string());
        if let Some(ttl) = params.ttl {
            cmd.arg("-t").arg(ttl.to_string());
        }
        if params.dont_fragment {
            cmd.arg("-M").arg("do");
        }
        if params.tos != 0 {
            cmd.arg("-Q").arg(params.tos.to_string());
        }
        if let Some(pattern) = params.payload.as_deref().filter(|p| !p.is_empty()) {
            // The system tool takes at most 16 pattern bytes.
            let hex: String = pattern.iter().take(16).map(|b| format!("{b:02x}")).collect();
            cmd.arg("-p").arg(hex);
        }
        match addr {
            IpAddr::V4(_) => {
                cmd.arg("-4");
                if let Some(src) = params.source_v4 {
                    cmd.arg("-I").arg(src.to_string());
                }
            }
            IpAddr::V6(_) => {
                cmd.arg("-6");
                if let Some(src) = params.source_v6 {
                    cmd.arg("-I").arg(src.to_string());
                }
            }
        }
        cmd.arg(addr.to_string());
        cmd.stdin(Stdio::null()).env("LC_ALL", "C");

        let start = Instant::now();
        let output = match cmd.output() {
            Ok(o) => o,
            Err(e) => {
                return Outcome { status: Status::Unsupported, os_code: e.raw_os_error(), ..Outcome::fail(Status::Unsupported) }
            }
        };
        let wall_us = elapsed_us(start);
        let text = String::from_utf8_lossy(&output.stdout);
        parse_ping_output(&text, output.status.success(), addr, wall_us, params.timeout)
    }

    pub(super) fn parse_ping_output(
        text: &str,
        success: bool,
        addr: IpAddr,
        wall_us: u32,
        timeout: Duration,
    ) -> Outcome {
        if success {
            let ms = field(text, "time=").and_then(|v| v.parse::<f64>().ok());
            let ttl = field(text, "ttl=").or_else(|| field(text, "hlim=")).and_then(|v| v.parse::<u8>().ok());
            let rtt_us = ms.map(|ms| (ms * 1000.0).round() as u32).unwrap_or(wall_us);
            // The system tool rounds its wait up to whole seconds; hold it to our timeout.
            if rtt_us as u128 > timeout.as_micros() {
                return Outcome::fail(Status::Timeout);
            }
            return Outcome { status: Status::Ok, os_code: None, rtt_us: Some(rtt_us), ttl, reply_addr: Some(addr) };
        }
        let lower = text.to_ascii_lowercase();
        let status = if lower.contains("host unreachable") {
            Status::HostUnreachable
        } else if lower.contains("net unreachable") || lower.contains("network is unreachable") {
            Status::NetUnreachable
        } else if lower.contains("port unreachable") {
            Status::PortUnreachable
        } else if lower.contains("time to live exceeded") || lower.contains("hop limit") {
            Status::TtlExpired
        } else if lower.contains("frag needed") || lower.contains("message too long") {
            Status::PacketTooBig
        } else {
            Status::Timeout
        };
        // "From 10.0.0.1 icmp_seq=1 Time to live exceeded" names the router that answered.
        let reply_addr = text
            .lines()
            .find_map(|line| line.trim().strip_prefix("From "))
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|word| word.trim_end_matches(':').parse::<IpAddr>().ok());
        Outcome { reply_addr, ..Outcome::fail(status) }
    }

    /// The token after `key`, up to the next space.
    fn field<'a>(text: &'a str, key: &str) -> Option<&'a str> {
        let rest = &text[text.find(key)? + key.len()..];
        Some(rest.split(|c: char| c.is_whitespace()).next().unwrap_or(rest))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        const OK: &str = "PING 1.1.1.1 (1.1.1.1) 56(84) bytes of data.\n64 bytes from 1.1.1.1: icmp_seq=1 ttl=57 time=3.42 ms\n\n--- 1.1.1.1 ping statistics ---\n1 packets transmitted, 1 received, 0% packet loss, time 0ms\n";

        #[test]
        fn reads_time_and_ttl() {
            let o = parse_ping_output(OK, true, "1.1.1.1".parse().unwrap(), 9000, Duration::from_secs(1));
            assert_eq!(o.status, Status::Ok);
            assert_eq!(o.rtt_us, Some(3420));
            assert_eq!(o.ttl, Some(57));
        }

        #[test]
        fn slow_reply_counts_as_timeout() {
            let o = parse_ping_output(OK, true, "1.1.1.1".parse().unwrap(), 9000, Duration::from_millis(2));
            assert_eq!(o.status, Status::Timeout);
        }

        #[test]
        fn failures_are_classified() {
            let addr: IpAddr = "10.9.9.9".parse().unwrap();
            let t = Duration::from_secs(1);
            let o = parse_ping_output("From 10.0.0.1 icmp_seq=1 Destination Host Unreachable\n", false, addr, 0, t);
            assert_eq!(o.status, Status::HostUnreachable);
            let o = parse_ping_output("1 packets transmitted, 0 received, 100% packet loss\n", false, addr, 0, t);
            assert_eq!(o.status, Status::Timeout);
            let o = parse_ping_output("PING 1.1.1.1\nFrom 192.168.1.1 icmp_seq=1 Time to live exceeded\n", false, addr, 0, t);
            assert_eq!((o.status, o.reply_addr), (Status::TtlExpired, Some("192.168.1.1".parse().unwrap())));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn tcp_connect_to_listening_port_succeeds() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let o = tcp(addr, &ProbeParams::default());
        assert_eq!(o.status, Status::Ok);
        assert!(o.rtt_us.is_some());
    }

    #[test]
    fn tcp_connect_to_closed_port_is_refused() {
        let addr = {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap()
        };
        let o = tcp(addr, &ProbeParams::default());
        assert_eq!(o.status, Status::Refused);
        assert_eq!(o.rtt_us, None);
    }

    #[test]
    fn payload_is_sized_and_patterned() {
        let zeros = ProbeParams { payload_size: 5, ..ProbeParams::default() };
        assert_eq!(zeros.payload_bytes(), [0, 0, 0, 0, 0]);
        let patterned = ProbeParams { payload_size: 5, payload: Some(vec![0xAB, 0xCD]), ..ProbeParams::default() };
        assert_eq!(patterned.payload_bytes(), [0xAB, 0xCD, 0xAB, 0xCD, 0xAB]);
        let empty = ProbeParams { payload_size: 0, payload: Some(vec![1]), ..ProbeParams::default() };
        assert!(empty.payload_bytes().is_empty());
    }
}
