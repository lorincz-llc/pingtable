#!/usr/bin/env python3
"""Record a real pingtable run for the website demo (site/demo/).

Starts pingtable with the demo host list on a private port and config,
lets it warm up, then records the app's own API for a while: every reply
to the long poll the UI makes (GET /api/v1/state?since=...&wait_ms=...),
each host's history, and info, config, source, defaults, a parse of the
host list. The result is
sanitized (no machine name, user name, home paths, tokens or MAC
addresses; no traceroutes) and written as site/demo/recording.js, which
site/demo/replay.js plays back through the unmodified app interface. The
format is described at the top of site/demo/replay.js.

    scripts/record-demo.py --bin target/release/pingtable \\
        --router 192.168.1.1 --unused 192.168.1.250

    scripts/record-demo.py --rebuild work/raw.json   # re-encode, no pinging

--router is the home router and --unused an address in its network that
nothing answers on (the "old printer"). Without them the default gateway
from `ip route` is used; under WSL that is the virtual switch, so pass the
real router. The run also needs scripts/demo-flappy.py (the "dev server"
that keeps restarting) and a free local port for it.

Everything is checked at the end: the encoded recording is decoded again
and compared with what the app really sent, and the output is refused if
it still contains this machine's name, the user name or a home path.
"""

import argparse
import getpass
import ipaddress
import json
import os
import re
import secrets
import signal
import socket
import struct
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
DEFAULT_OUT = REPO / "site" / "demo" / "recording.js"
FORMAT = 1

# The neutral name the demo shows for the computer it was recorded on.
DEMO_MACHINE = "demo-pc"
DEMO_CONFIG_PATH = "~/.config/pingtable/pingtable.json"
DEMO_LOG_PATH = "~/.local/state/pingtable/pingtable.log"

# Row fields the replay works out from the host's history instead of
# storing them for every update. Both sides must agree; replay.js reads this
# list from the recording and derives exactly these. Anything the
# derivation gets wrong is stored as an explicit value (see derive_row).
DERIVED = [
    "sent", "ok", "failed", "missed", "failed_pct", "recent_loss_pct", "consec_failed",
    "max_consec_failed", "max_consec_failed_at_ms", "last_status", "last_status_label",
    "last_rtt_us", "last_ttl", "last_at_ms", "last_ok_at_ms", "last_failed_at_ms",
    "avg_rtt_us", "min_rtt_us", "max_rtt_us", "jitter_us", "spark",
]
SPARK_LEN = 60
# Top-level state fields that are not part of the timeline machinery.
TOP_SKIP = {"schema_version", "seq", "now_ms", "full", "hosts", "events", "notes"}


def host_list(router, unused, flappy_port, closed_port):
    """The demo's host list, in the order the story is told."""
    return f"""# The pingtable.net demo. Recorded by scripts/record-demo.py.
Group: This PC every=1s
127.0.0.1 Loopback
::1 Loopback, IPv6
127.0.0.1:{flappy_port} Dev server, restarting
127.0.0.1:{closed_port} Redis, not running

Group: Home network every=1s
{router} Router
{unused} Old printer, switched off every=2s

Group: Internet every=2s
1.1.1.1 Cloudflare DNS every=1s
8.8.8.8 Google DNS every=1s
9.9.9.9 Quad9 DNS
208.67.222.222 OpenDNS

Group: Websites every=3s
github.com:443 GitHub
github.com:22 GitHub, Git over SSH every=5s
cloudflare.com:443 Cloudflare
wikipedia.org:443 Wikipedia
example.com:443 Example site
example.com:22 Example site, SSH every=5s
amazon.com Amazon, ping
amazon.com:443 Amazon, HTTPS

Group: Far away every=3s
www.anu.edu.au Canberra, Australia
www.uq.edu.au Brisbane, Australia
www.vuw.ac.nz:443 Wellington, New Zealand
www.fing.edu.uy:443 Montevideo, Uruguay
www.ethz.ch Zurich, Switzerland
www.uio.no Oslo, Norway

Group: Broken on purpose every=5s
192.0.2.1 TEST-NET, never answers
no-such-host.invalid Name does not exist
"""


def log(*parts):
    print(time.strftime("%H:%M:%S"), *parts, file=sys.stderr, flush=True)


# ---------------------------------------------------------------- recording


class Api:
    def __init__(self, port, token):
        self.base = f"http://127.0.0.1:{port}/api/v1/"
        self.token = token

    def call(self, path, body=None, method=None, timeout=40):
        data = json.dumps(body).encode() if body is not None else None
        req = urllib.request.Request(self.base + path, data=data, method=method or ("POST" if data else "GET"))
        req.add_header("X-Pingtable-Token", self.token)
        if data:
            req.add_header("Content-Type", "application/json")
        with urllib.request.urlopen(req, timeout=timeout) as reply:
            return json.load(reply)


def default_gateway():
    try:
        out = subprocess.run(["ip", "route"], capture_output=True, text=True, check=True).stdout
        m = re.search(r"^default via (\S+)", out, re.M)
        return m.group(1) if m else None
    except Exception:
        return None


def port_is_free(port):
    with socket.socket() as s:
        s.settimeout(0.5)
        return s.connect_ex(("127.0.0.1", port)) != 0


def record(args):
    work = Path(args.work).resolve()
    work.mkdir(parents=True, exist_ok=True)
    router = args.router or default_gateway()
    if not router:
        sys.exit("No --router given and no default gateway found.")
    unused = args.unused or str(ipaddress.ip_network(router + "/24", strict=False)[250])
    for port in (args.port, args.flappy_port, args.closed_port):
        if not port_is_free(port):
            sys.exit(f"Port {port} on 127.0.0.1 is in use; choose another.")
    hosts_text = host_list(router, unused, args.flappy_port, args.closed_port)
    token = secrets.token_hex(16)
    config = {
        "schema_version": 1,
        "port": args.port,
        "token": token,
        "autostart": True,
        "settings": {
            "interval_secs": 1, "repeat": True, "timeout_ms": 1000, "payload_size": 32, "ttl": None,
            "dont_fragment": False, "max_concurrent": 256, "resolve_every_ping": False, "reverse_dns": True,
            "prefer_ipv6": False, "source_v4": None, "source_v6": None, "read_mac": False,
            "history_per_host": 2000, "fail_trigger": 2, "recover_trigger": 1, "on_down_command": "",
            "on_up_command": "", "description_format": True, "cidr_skip_first": True, "cidr_skip_last": True,
        },
        "muted": [],
        "hosts_text": hosts_text,
    }
    config_path = work / "pt.json"
    config_path.write_text(json.dumps(config, indent=2))

    flappy = subprocess.Popen(
        [sys.executable, str(REPO / "scripts" / "demo-flappy.py"), "--port", str(args.flappy_port),
         "--up", str(args.flappy_up), "--down", str(args.flappy_down)],
        stdout=open(work / "flappy.log", "w"), stderr=subprocess.STDOUT)
    time.sleep(0.5)
    app = subprocess.Popen([args.bin, "--config", str(config_path), "--port", str(args.port), "--no-open"],
                           stdout=open(work / "pingtable.log", "w"), stderr=subprocess.STDOUT)
    api = Api(args.port, token)
    raw = {
        "format": "raw-1",
        "hosts_text": hosts_text,
        "port": args.port,
        "warmup_s": args.warmup,
        "duration_s": args.duration,
        "flappy": {"port": args.flappy_port, "up_s": args.flappy_up, "down_s": args.flappy_down},
    }
    try:
        for _ in range(100):
            try:
                api.call("info", timeout=2)
                break
            except Exception:
                time.sleep(0.1)
        else:
            sys.exit("pingtable did not start; see " + str(work / "pingtable.log"))
        log(f"pingtable is up on port {args.port}; warming up for {args.warmup} s")
        end_warmup = time.time() + args.warmup
        while time.time() < end_warmup:
            if app.poll() is not None:
                sys.exit("pingtable stopped during the warm-up")
            time.sleep(1)

        initial = api.call("state?since=0")
        raw["initial"] = initial
        t0 = initial["now_ms"]
        polls = []
        periodic = []
        stop = threading.Event()

        def details_loop():
            # For checking the replay only: what the detail panel really showed.
            while not stop.wait(args.detail_every):
                at = int(time.time() * 1000)
                got = {}
                for row in initial["hosts"]:
                    try:
                        got[row["id"]] = api.call(f"hosts/{row['id']}")
                    except Exception as err:
                        log("detail failed:", err)
                periodic.append({"t_ms": at, "details": got})

        thread = threading.Thread(target=details_loop, daemon=True)
        thread.start()
        log(f"recording for {args.duration} s")
        seq = initial["seq"]
        last_note = time.time()
        while True:
            snap = api.call(f"state?since={seq}&wait_ms=20000")
            polls.append(snap)
            seq = snap["seq"]
            if snap.get("full"):
                log("warning: a full snapshot arrived in the middle of the recording")
            if snap["now_ms"] - t0 >= args.duration * 1000:
                break
            if time.time() - last_note > 60:
                log(f"{(snap['now_ms'] - t0) / 1000:.0f} s, {len(polls)} replies")
                last_note = time.time()
        stop.set()
        thread.join(timeout=60)
        raw["polls"] = polls
        raw["details_periodic"] = periodic

        log("reading every host's history")
        raw["details_final"] = {str(r["id"]): api.call(f"hosts/{r['id']}") for r in initial["hosts"]}
        raw["info"] = api.call("info")
        raw["config"] = api.call("config")
        raw["source"] = api.call("source")
        raw["defaults"] = api.call("defaults")
        raw["parse"] = {
            "body": {"text": hosts_text, "description_format": True, "cidr_skip_first": True, "cidr_skip_last": True},
            "reply": api.call("parse", {"text": hosts_text, "description_format": True, "cidr_skip_first": True,
                                        "cidr_skip_last": True}),
        }
    finally:
        for proc in (app, flappy):
            if os.name == "nt":
                proc.terminate()
            else:
                proc.send_signal(signal.SIGINT)
        for proc in (app, flappy):
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
    raw_path = work / "raw.json"
    raw_path.write_text(json.dumps(raw))
    log(f"raw recording: {raw_path} ({raw_path.stat().st_size / 1e6:.1f} MB, {len(raw['polls'])} replies)")
    return raw


# ---------------------------------------------------------------- sanitizing


def forbidden_words(raw, work, extra):
    words = {}
    name = socket.gethostname()
    for n in {name, name.split(".")[0], socket.getfqdn()}:
        if n and n not in ("localhost",):
            words[n.lower()] = DEMO_MACHINE
    machine = (raw.get("info") or {}).get("machine")
    if machine:
        words[machine.lower()] = DEMO_MACHINE
    for n in ((raw.get("source") or {}).get("name"),):
        if n:
            words[n.lower()] = DEMO_MACHINE
    user = getpass.getuser()
    fail = {user.lower(), str(Path.home()).lower(), "/home/", "c:\\users\\", ".lan"}
    try:
        for line in open("/etc/resolv.conf"):
            parts = line.split()
            if parts and parts[0] in ("search", "domain"):
                fail.update(p.lower() for p in parts[1:])
    except OSError:
        pass
    token = None
    try:
        token = json.loads((Path(work) / "pt.json").read_text()).get("token")
    except Exception:
        pass
    fail.update(e.lower() for e in extra)
    if token:
        fail.add(token.lower())
    fail.discard("")
    return words, fail


def is_private(ip):
    try:
        a = ipaddress.ip_address(ip)
    except ValueError:
        return False
    return a.is_private or a.is_link_local or a in ipaddress.ip_network("100.64.0.0/10")


def keep_address(addr):
    """Addresses the demo may show for this computer: IPv4 private or loopback, IPv6 loopback only."""
    try:
        a = ipaddress.ip_address(addr)
    except ValueError:
        return False
    if a.version == 6:
        return a.is_loopback
    return a.is_private or a.is_loopback


def sanitize_row(row):
    row.pop("mac", None)
    row["mac"] = None
    name = row.get("hostname")
    ip = row.get("ip")
    if name and ip and is_private(ip) and not ipaddress.ip_address(ip).is_loopback:
        # A home device's own name says nothing useful and may say too much.
        row["hostname"] = None


def sanitize(raw, words, fail):
    port = raw["port"]
    info = raw["info"]
    info["machine"] = DEMO_MACHINE
    info["config_path"] = DEMO_CONFIG_PATH
    info["log_path"] = DEMO_LOG_PATH
    src = raw["source"]
    src["name"] = DEMO_MACHINE
    src["interfaces"] = [i for i in src.get("interfaces", []) if keep_address(i.get("addr", ""))]
    for i in src["interfaces"]:
        i.pop("mac", None)
    if src.get("default_v4") and not keep_address(src["default_v4"]):
        src["default_v4"] = None
    if src.get("default_v6") and not keep_address(src["default_v6"]):
        src["default_v6"] = None
    cfg = raw["config"]
    cfg["config_path"] = DEMO_CONFIG_PATH
    cfg.pop("token", None)
    for key in ("on_down_command", "on_up_command"):
        cfg["settings"][key] = ""
    cfg["settings"]["source_v4"] = None
    cfg["settings"]["source_v6"] = None
    # The example list names the port pingtable ran on; show the usual one.
    raw["defaults"]["hosts_text"] = raw["defaults"]["hosts_text"].replace(f"127.0.0.1:{port}", "127.0.0.1:7464")
    for snap in [raw["initial"], *raw["polls"]]:
        for row in snap["hosts"]:
            sanitize_row(row)
    for detail in raw["details_final"].values():
        sanitize_row(detail["row"])
    raw.pop("details_periodic", None)  # not published

    def scrub(value):
        if isinstance(value, dict):
            return {k: scrub(v) for k, v in value.items() if k != "token"}
        if isinstance(value, list):
            return [scrub(v) for v in value]
        if isinstance(value, str):
            for word, neutral in words.items():
                value = re.sub(re.escape(word), neutral, value, flags=re.I)
            return value
        return value

    clean = scrub(raw)
    text = json.dumps(clean).lower()
    leaks = sorted(w for w in fail if w in text)
    if leaks:
        sys.exit(f"Refusing to write the recording: it still contains {leaks}")
    return clean


# ---------------------------------------------------------------- encoding


def f32(x):
    return struct.unpack("f", struct.pack("f", x))[0]


def round_half_away(x):
    return int(x + 0.5) if x >= 0 else -int(-x + 0.5)


class Stats:
    """A port of HostStats::apply (crates/core/src/engine.rs). replay.js has the same."""

    def __init__(self):
        self.sent = self.ok = self.failed = self.missed = 0
        self.consec_failed = self.consec_ok = self.max_consec_failed = 0
        self.max_consec_failed_at = None
        self.last = None
        self.last_ok_at = self.last_failed_at = None
        self.min = self.max = None
        self.sum = self.timed = 0
        self.jitter = 0.0
        self.prev = None

    def apply(self, s):
        self.sent += 1
        self.last = s
        if s["status"] == "ok":
            self.ok += 1
            self.consec_ok += 1
            self.consec_failed = 0
            self.last_ok_at = s["t_ms"]
            rtt = s.get("rtt_us")
            if rtt is not None:
                self.sum += rtt
                self.timed += 1
                self.min = rtt if self.min is None else min(self.min, rtt)
                self.max = rtt if self.max is None else max(self.max, rtt)
                if self.prev is not None:
                    diff = abs(f32(f32(rtt) - f32(self.prev)))
                    if self.timed == 2:
                        self.jitter = diff
                    else:
                        self.jitter = f32(self.jitter + f32(f32(diff - self.jitter) / 16.0))
                self.prev = rtt
        else:
            self.failed += 1
            self.missed += max(0, s.get("beats", 1) - 1)
            self.consec_failed += 1
            self.consec_ok = 0
            self.last_failed_at = s["t_ms"]
            if self.consec_failed >= self.max_consec_failed:
                self.max_consec_failed = self.consec_failed
                self.max_consec_failed_at = s["t_ms"]


def derive_fields(samples, n, labels):
    st = Stats()
    for s in samples[:n]:
        st.apply(s)
    recent = samples[max(0, n - SPARK_LEN):n]
    beats = sum(s.get("beats", 1) for s in recent)
    lost = sum(s.get("beats", 1) for s in recent if s["status"] != "ok")
    total = st.sent + st.missed
    last = st.last
    return {
        "sent": st.sent, "ok": st.ok, "failed": st.failed, "missed": st.missed,
        "failed_pct": 0.0 if total == 0 else f32(f32(f32(st.failed + st.missed) * 100.0) / f32(total)),
        "recent_loss_pct": 0.0 if beats == 0 else f32(f32(f32(lost) * 100.0) / f32(beats)),
        "consec_failed": st.consec_failed, "max_consec_failed": st.max_consec_failed,
        "max_consec_failed_at_ms": st.max_consec_failed_at,
        "last_status": last["status"] if last else None,
        "last_status_label": labels.get(last["status"], last["status"]) if last else None,
        "last_rtt_us": last.get("rtt_us") if last else None,
        "last_ttl": last.get("ttl") if last else None,
        "last_at_ms": last["t_ms"] if last else None,
        "last_ok_at_ms": st.last_ok_at, "last_failed_at_ms": st.last_failed_at,
        "avg_rtt_us": st.sum // st.timed if st.timed else None,
        "min_rtt_us": st.min, "max_rtt_us": st.max,
        "jitter_us": round_half_away(st.jitter) if st.timed > 1 else None,
        "spark": [s.get("rtt_us") if s["status"] == "ok" else None for s in recent],
    }


def same(a, b):
    if isinstance(a, float) or isinstance(b, float):
        if a is None or b is None:
            return a is b
        return abs(a - b) <= 1e-4 * max(1.0, abs(a), abs(b))
    return a == b


def to_relative(value, t0, keys):
    """Absolute times (keys ending in _ms holding a Unix time) become ms after t0."""
    if isinstance(value, dict):
        out = {}
        for k, v in value.items():
            if k.endswith("_ms") and isinstance(v, int) and v > 10**12:
                keys.add(k)
                out[k] = v - t0
            else:
                out[k] = to_relative(v, t0, keys)
        return out
    if isinstance(value, list):
        return [to_relative(v, t0, keys) for v in value]
    return value


def encode_history(samples, statuses):
    """Columns: time steps, one status letter per sample, round trips of OK samples, TTL runs, beats."""
    t, prev = [], 0
    letters = []
    rtts = []
    ttl_runs = []
    beats = {}
    for i, s in enumerate(samples):
        t.append(s["t_ms"] - prev)
        prev = s["t_ms"]
        letters.append(statuses[s["status"]])
        if s["status"] == "ok":
            rtts.append(s.get("rtt_us"))
        ttl = s.get("ttl")
        if ttl_runs and ttl_runs[-1][1] == ttl:
            ttl_runs[-1][0] += 1
        else:
            ttl_runs.append([1, ttl])
        if s.get("beats", 1) != 1:
            beats[str(i)] = s["beats"]
    out = {"t": t, "s": "".join(letters), "r": rtts, "ttl": ttl_runs}
    if beats:
        out["b"] = beats
    return out


def build(raw, out_path, extra_forbidden=()):
    raw = dict(raw)
    work = raw.pop("work", ".")
    words, fail = forbidden_words(raw, work, extra_forbidden)
    t0 = raw["initial"]["now_ms"]
    # Keep exactly the recorded length, so the replay's loop matches the dev server's cycle.
    duration_ms = int(raw["duration_s"] * 1000)
    raw["polls"] = [p for p in raw["polls"] if p["now_ms"] - t0 <= duration_ms]
    clean = sanitize(json.loads(json.dumps(raw)), words, fail)
    time_keys = set()
    rel = to_relative(clean, t0, time_keys)
    # History samples carry t_ms too; to_relative has turned them already.
    histories = {int(k): d["history"] for k, d in rel["details_final"].items()}
    cap = rel["config"]["settings"].get("history_per_host", 2000)
    for hid, h in histories.items():
        if len(h) >= cap:
            log(f"warning: host {hid} filled its history ({cap}); the replay stores more values for it")

    labels = {}
    for snap in [rel["initial"], *rel["polls"]]:
        for row in snap["hosts"]:
            if row.get("last_status") and row.get("last_status_label"):
                labels[row["last_status"]] = row["last_status_label"]
    status_names = sorted({s["status"] for h in histories.values() for s in h} | set(labels) | {"ok"})
    letters = "oabcdefghijklmnpqrstuvwxyz"
    status_list = ["ok"] + [s for s in status_names if s != "ok"]
    statuses = {name: letters[i] for i, name in enumerate(status_list)}
    index_by_time = {hid: {s["t_ms"]: i for i, s in enumerate(h)} for hid, h in histories.items()}

    stats = {"updates": 0, "explicit": 0, "explicit_fields": {}}

    def count_of(hid, row):
        if row.get("last_at_ms") is None:
            return 0
        i = index_by_time[hid].get(row["last_at_ms"])
        return None if i is None else i + 1

    def split_row(hid, row, prev):
        """(n, fields to store): stored fields are what changed since prev plus what derivation misses."""
        n = count_of(hid, row)
        derived = derive_fields(histories[hid], n or 0, labels)
        fields = {}
        for k, v in row.items():
            if k in ("id",):
                continue
            if k in DERIVED and n is not None:
                if not same(derived[k], v):
                    fields[k] = v
                    stats["explicit_fields"][k] = stats["explicit_fields"].get(k, 0) + 1
            elif prev is None or k not in prev or not same(prev[k], v):
                fields[k] = v
        return (n if n is not None else -1), fields

    rows_prev = {}
    initial_rows = []
    for row in rel["initial"]["hosts"]:
        hid = row["id"]
        n, fields = split_row(hid, row, None)
        fields = {k: v for k, v in fields.items()}
        initial_rows.append([hid, n, fields])
        rows_prev[hid] = row
    initial_top = {k: v for k, v in rel["initial"].items() if k not in TOP_SKIP}

    timeline = []
    last_t = 0
    top_known = dict(initial_top)
    top_actual = dict(initial_top)
    active_at = 0

    def push(t, entry):
        nonlocal last_t
        timeline.append([t - last_t, *entry])
        last_t = t

    for snap in rel["polls"]:
        t = snap["now_ms"]
        top = {k: v for k, v in snap.items() if k not in TOP_SKIP}
        top_actual.update(top)
        diff = {k: v for k, v in top_actual.items() if k != "active" and not same(top_known.get(k), v)}
        if top_actual.get("active") != top_known.get("active") and (diff or t - active_at >= 1000):
            diff["active"] = top_actual["active"]
            active_at = t
        if diff:
            push(t, [2, diff])
            top_known.update(diff)
        for row in snap["hosts"]:
            hid = row["id"]
            n, fields = split_row(hid, row, rows_prev.get(hid))
            rows_prev[hid] = row
            stats["updates"] += 1
            if any(k in DERIVED for k in fields):
                stats["explicit"] += 1
            push(t, [0, hid, n, fields] if fields else [0, hid, n])
        for event in snap.get("events", []):
            push(t, [1, event])
        for note in snap.get("notes", []):
            push(t, [3, note])

    recording = {
        "format": FORMAT,
        "about": "A real pingtable run, recorded by scripts/record-demo.py and replayed by replay.js.",
        "recorded_on": time.strftime("%Y-%m-%d", time.gmtime(t0 / 1000)),
        "version": rel["info"].get("version"),
        "warmup_ms": int(raw["warmup_s"] * 1000),
        "duration_ms": duration_ms,
        "time_keys": sorted(time_keys),
        "derived": DERIVED,
        "spark_len": SPARK_LEN,
        "statuses": status_list,
        "labels": labels,
        "static": {
            "info": rel["info"], "config": rel["config"], "source": rel["source"],
            "defaults": rel["defaults"], "parse": rel["parse"],
        },
        "initial": {"top": initial_top, "rows": initial_rows},
        "history": {str(hid): encode_history(h, statuses) for hid, h in histories.items()},
        "timeline": timeline,
    }
    verify(recording, raw, t0)
    body = json.dumps(recording, separators=(",", ":"), ensure_ascii=False)
    leaks = sorted(w for w in fail if w in body.lower())
    if leaks:
        sys.exit(f"Refusing to write the recording: it still contains {leaks}")
    out_path = Path(out_path)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(
        "/* Generated by scripts/record-demo.py: a real pingtable run, recorded on "
        f"{recording['recorded_on']} and replayed by replay.js. Do not edit by hand. */\n"
        f"window.PINGTABLE_RECORDING = {body};\n", encoding="utf-8")
    size = out_path.stat().st_size
    log(f"wrote {out_path} ({size / 1e3:.0f} kB): {len(timeline)} timeline entries, "
        f"{stats['updates']} row updates, {stats['explicit']} with values the history could not give: "
        f"{stats['explicit_fields']}")
    return recording


# ---------------------------------------------------------------- checking


def decode_history(enc, statuses):
    out, t, r = [], 0, iter(enc["r"])
    ttls = []
    for count, ttl in enc["ttl"]:
        ttls.extend([ttl] * count)
    for i, (dt, letter) in enumerate(zip(enc["t"], enc["s"])):
        t += dt
        s = {"t_ms": t, "status": statuses[letter]}
        if s["status"] == "ok":
            s["rtt_us"] = next(r)
        if ttls[i] is not None:
            s["ttl"] = ttls[i]
        b = enc.get("b", {}).get(str(i))
        if b:
            s["beats"] = b
        out.append(s)
    return out


def verify(rec, raw, t0):
    """Decode the recording the way replay.js does and compare every row update with what the app sent."""
    statuses = {letter: name for letter, name in zip("oabcdefghijklmnpqrstuvwxyz", rec["statuses"])}
    hist = {int(k): decode_history(v, statuses) for k, v in rec["history"].items()}

    def expect(row):
        out = dict(row)
        out["mac"] = None
        for k in rec["time_keys"]:
            if isinstance(out.get(k), int) and out[k] > 10**12:
                out[k] -= t0
        return out

    def compare(where, got, want):
        for k, v in want.items():
            if k == "hostname":
                continue  # sanitized
            g = got.get(k)
            if isinstance(v, list) and isinstance(g, list):
                ok = len(v) == len(g) and all(same(a, b) for a, b in zip(v, g))
            else:
                ok = same(g, v)
            if not ok:
                raise SystemExit(f"Recording check failed at {where}, host {want.get('id')}, field {k}: "
                                 f"replay {g!r}, app {v!r}")

    rows = {}
    for (hid, n, fields), want in zip(rec["initial"]["rows"], raw["initial"]["hosts"]):
        rows[hid] = {"id": hid, **fields, **materialize(hist[hid], n, fields, rec)}
        compare("start", rows[hid], expect(want))
    updates = [e for e in rec["timeline"] if e[1] == 0]
    raw_updates = [row for snap in raw["polls"] for row in snap["hosts"]]
    if len(updates) != len(raw_updates):
        raise SystemExit(f"Recording check failed: {len(updates)} updates, the app sent {len(raw_updates)}")
    for i, (entry, want) in enumerate(zip(updates, raw_updates)):
        hid, n = entry[2], entry[3]
        fields = entry[4] if len(entry) > 4 else {}
        rows[hid].update(fields)
        rows[hid].update(materialize(hist[hid], n, fields, rec))
        compare(f"update {i}", rows[hid], expect(want))
    log(f"checked {len(raw_updates)} row updates against what the app sent: all match")


def materialize(samples, n, fields, rec):
    """The derived fields for a row at n samples, except those stored explicitly."""
    if n < 0:
        return {}
    derived = derive_fields(samples, n, rec["labels"])
    return {k: v for k, v in derived.items() if k not in fields}


# ---------------------------------------------------------------- main


def main():
    ap = argparse.ArgumentParser(description="Record a real pingtable run for the website demo.",
                                 formatter_class=argparse.RawDescriptionHelpFormatter, epilog=__doc__)
    ap.add_argument("--bin", default=str(REPO / "target" / "release" / "pingtable"), help="the pingtable binary")
    ap.add_argument("--router", help="the home router's address")
    ap.add_argument("--unused", help="an address in the home network that nothing answers on")
    ap.add_argument("--port", type=int, default=7491, help="port for pingtable's web UI during the recording")
    ap.add_argument("--flappy-port", type=int, default=8180, help="port of the restarting dev server")
    ap.add_argument("--flappy-up", type=float, default=55)
    ap.add_argument("--flappy-down", type=float, default=20)
    ap.add_argument("--closed-port", type=int, default=6379, help="a local port nothing listens on")
    ap.add_argument("--warmup", type=float, default=120, help="seconds of pinging before the recording starts")
    ap.add_argument("--duration", type=float, default=600, help="seconds recorded")
    ap.add_argument("--detail-every", type=float, default=30, help="seconds between detail reads kept for checking")
    ap.add_argument("--work", default=str(REPO / "target" / "demo-recording"), help="folder for the raw recording and logs")
    ap.add_argument("--out", default=str(DEFAULT_OUT))
    ap.add_argument("--rebuild", metavar="RAW", help="encode this raw.json again instead of recording")
    ap.add_argument("--forbid", action="append", default=[], help="another word that must not appear in the output")
    args = ap.parse_args()
    if args.rebuild:
        raw = json.loads(Path(args.rebuild).read_text())
        raw["work"] = str(Path(args.rebuild).resolve().parent)
    else:
        raw = record(args)
        raw["work"] = str(Path(args.work).resolve())
    build(raw, args.out, args.forbid)


if __name__ == "__main__":
    main()
