# The local HTTP API

Everything pingtable's window does goes through this API, so a script can do
it too. It exists while pingtable is running (`pingtable.exe`, or
`pingtable-cli` with no arguments), answers only on this computer, and needs
a token on every call.

An AI agent is usually better served by the MCP tools in
[ai-agents.md](ai-agents.md), which need no token handling.

## Getting started

```
pingtable-cli api
```

prints the address, the token and example calls for this computer. Add
`--format json` to read them from a program.

| | |
|---|---|
| Address | `http://127.0.0.1:7464/api/v1` (the port can be changed with `--port` or in the settings file; a running pingtable writes the one it uses to `pingtable.json.run` beside the settings file) |
| Token | Header `X-Pingtable-Token: <token>`. The token is the `token` value in `pingtable.json` |
| Bodies | JSON, with `Content-Type: application/json` |
| Errors | A status of 400 or above with `{"error": "a sentence saying what is wrong"}` |

Three calls in three languages:

```bash
curl -H "X-Pingtable-Token: $TOKEN" http://127.0.0.1:7464/api/v1/state
```

```powershell
$h = @{ 'X-Pingtable-Token' = $token }
Invoke-RestMethod http://127.0.0.1:7464/api/v1/state -Headers $h
Invoke-RestMethod http://127.0.0.1:7464/api/v1/source -Headers $h
```

```python
import json, subprocess, urllib.request

api = json.loads(subprocess.run(["pingtable-cli", "api", "--format", "json"],
                                capture_output=True, text=True, check=True).stdout)

def call(path, body=None):
    request = urllib.request.Request(
        api["base_url"] + path,
        data=json.dumps(body).encode() if body is not None else None,
        headers={api["header"]: api["token"], "Content-Type": "application/json"})
    with urllib.request.urlopen(request) as reply:
        return json.load(reply)

down = [h["host"] for h in call("/state")["hosts"] if h["state"] == "down"]
```

## Conventions

- Field names carry their units: `rtt_us` is microseconds, `t_ms` and
  `timeout_ms` are milliseconds, `interval_secs` is seconds. Times of day
  (`now_ms`, `last_at_ms`) are milliseconds since 1970-01-01 UTC.
- Documents carry `schema_version`. Fields may be added within a version;
  none is removed or changes meaning.
- A host in the table has a numeric `id`, valid until the host list changes,
  and a `key` (`example.com|tcp:443`) that survives restarts.
- The server listens on the loopback address only, refuses requests whose
  `Host` or `Origin` is not this computer, and requires the token. The token
  matters: the settings include commands that pingtable runs on alerts.
  Every reply carries `Cross-Origin-Resource-Policy: same-origin`.

## Calls

| Call | Purpose |
|---|---|
| [`GET /state`](#the-table) | The table: every host and its numbers |
| [`POST /hosts/add`](#add-hosts) | Add hosts to the list |
| [`POST /hosts`](#act-on-hosts) | Mute, unmute, ping now, reset or remove several hosts |
| [`GET` / `POST /hosts/{id}`](#one-host) | One host's history; mute it, ping it now, reset it |
| [`POST /hosts/{id}/trace`](#trace-route) | Trace the route to a host |
| [`POST /control`](#start-and-stop) | Start, stop, ping every host once, reset counters |
| [`POST /measure`](#measure), `GET /measure`, `POST /measure/stop` | A timed measurement |
| [`GET` / `PUT /config`](#settings-and-host-list) | Read or change settings and the host list |
| [`POST /parse`, `POST /import`](#check-a-list-before-using-it) | Check a host list, or turn a CSV into one, without applying it |
| [`GET /export`](#export) | The table as a JSON or CSV file |
| [`GET /source`, `GET /info`, `GET /defaults`](#about-this-computer-and-this-build) | This computer's addresses, build facts, default settings |
| [`GET /hello`](#checking-who-answers) | Proof that the server holds the token. The one call that needs no token |

Paths below are relative to `/api/v1`.

### The table

`GET /state` returns the whole table.

```json
{
  "schema_version": 1,
  "seq": 11,
  "now_ms": 1794932547799,
  "running": true,
  "counts": { "total": 2, "up": 2, "down": 0, "pending": 0, "muted": 0 },
  "full": true,
  "hosts": [
    {
      "id": 1,
      "key": "192.168.1.1|icmp",
      "host": "192.168.1.1",
      "ip": "192.168.1.1",
      "hostname": "router.lan",
      "description": "Router",
      "group": "Home lab",
      "tags": [],
      "probe": { "kind": "icmp" },
      "enabled": true,
      "interval_secs": 1,
      "timeout_ms": 1000,
      "state": "up",
      "sent": 4,
      "ok": 4,
      "failed": 0,
      "failed_pct": 0.0,
      "consec_failed": 0,
      "last_status": "ok",
      "last_status_label": "OK",
      "last_rtt_us": 420,
      "last_at_ms": 1794932547792,
      "avg_rtt_us": 410,
      "min_rtt_us": 390,
      "max_rtt_us": 450,
      "jitter_us": 20,
      "spark": [410, 400, 420]
    }
  ],
  "events": [],
  "warnings": []
}
```

- `state` is `up`, `down`, `failing` (failed, but not yet often enough to
  count as down), `recovering`, `pending` (no result yet) or `muted`.
- `probe` is `{"kind": "icmp"}` or `{"kind": "tcp", "port": 443}`.
- `last_status` is `ok`, `timeout`, `refused`, `resolve_failed`,
  `host_unreachable`, `net_unreachable` and a few rarer ones;
  `last_status_label` is the same in words.
- `spark` is the most recent round-trip times, oldest first; `null` marks a
  failed ping.
- A row's `stale` is true when its state is old news: the host is due and has
  waited a long while for a free slot, so it has not been pinged lately.
- `failed_pct` counts beats, not only pings. A host is never pinged twice at
  once, so while a failed ping waits for its timeout the beats that come due
  are skipped; they count as lost with it, and `missed` says how many there
  were. With a timeout no longer than the interval, `missed` stays 0. So it
  does for a ping by hand while pinging is stopped (`ping_now`), which has no
  beats to skip.
- `active` is how many pings are in flight. `waiting` is how many hosts are
  due and waiting for a free slot; above zero for long means the table asks
  for more pings at once than "pings in flight at once" allows.
- `events` (a host went down or came back) and `notes` (things that are not
  about one host, such as an alert command that could not be started) come
  only in replies to a `since=` request. Each has `seq` and `t_ms`; a note
  has `text`. A note with `"sticky": true` matters beyond the moment and is
  best shown until it is dismissed: the settings file could not be read, and
  pingtable started with the defaults. That note goes once, to the first
  `since=` request after the start.
- The example leaves out some fields: `order`, `reply_ip`, `mac`, `line`,
  `removable`, `packet`, `recent_loss_pct`, `max_consec_failed`, `last_ttl`,
  `last_ok_at_ms`, `last_failed_at_ms`, `stale`, `missed`, `active`,
  `waiting`, `running_since_ms`, `notes`.

To follow changes without polling fast, pass the `seq` you last saw:
`GET /state?since=11&wait_ms=20000` waits up to 20 seconds (25 at most) for
something to change and returns only the hosts that did, with
`"full": false`. A reply with `"full": true` replaces what you hold.

A big table (a whole /16 is 65,536 rows) makes a reply of tens of megabytes.
To read part of it, `GET /state?state=down,failing&limit=100` sends only the
rows in those states (any of the six, split by commas), at most `limit` of
them, in table order, and adds `matched`: how many rows were in those states
before the limit. Either can be used alone. `counts` still covers the whole
table. An unknown state is a 400.

`state` and `limit` read part of the whole table, so they cannot be used
with `since`: a reply of changes filtered that way would never report a row
that left the states asked for, and the rows a limit cut off would be lost
for good. Such a request is a 400. Read the table with them, or follow
changes with `since` and without them.

### Add hosts

`POST /hosts/add`

```json
{ "entries": ["nas.lan NAS in the closet"], "group": "Home lab", "ports": [445], "ping": true }
```

Each entry is one host, optionally followed by a description and options.
`group` is created if it does not exist; leave it out for no group. A group
name must be one line: one with a line break in it is refused. With
`ports` (or `ports_text`), every entry that names no port of its own gets one
row for each port, and `ping` keeps the plain ping row too. Replies
`{"ok": true, "added": 2, "group": "Home lab", "save_error": null}`. The list
is saved; `save_error` says so if the settings file could not be written.

### Act on hosts

`POST /hosts`

```json
{ "action": "mute", "ids": [3, 4] }
```

`action` is `mute`, `unmute`, `solo` (ping only these), `unmute_all`,
`ping_now`, `reset` (clear their counters) or `remove` (take them out of the
list). `solo` starts pinging if it is stopped.

### One host

`GET /hosts/{id}` returns `row` (as in the table), `history` (every sample
kept), `runs` (stretches of the same status; failed runs are outages),
`p50_rtt_us`, `p95_rtt_us`, `p99_rtt_us` and `windows` (loss over the last
minute, 5 minutes, hour and day).

The history holds a fixed number of samples (2,000 unless changed in the
settings), not a length of time, so a window may ask for more than is kept.
Each window therefore carries `covered_ms`: how far back it really reaches.
The percentiles are over the kept history; the averages in `row` are since
the counters were last reset. A sample has `beats` when a failed ping stood
for more than one beat (see `missed` above).

`POST /hosts/{id}` takes `{"enabled": false}` to mute or `true` to unmute,
and `{"action": "ping_now"}` or `{"action": "reset"}`.

### Trace route

`POST /hosts/{id}/trace` with an empty body `{}`. Takes a second or two.

```json
{
  "schema_version": 1,
  "host": "example.com",
  "target": "93.184.215.14",
  "from": "MY-PC",
  "source": "192.168.1.50",
  "reached": true,
  "hops": [
    { "ttl": 1, "addr": "192.168.1.1", "name": "router.lan", "rtt_us": 520, "status": "ttl_expired", "reached": false },
    { "ttl": 2, "addr": null, "name": null, "rtt_us": null, "status": "timeout", "reached": false }
  ]
}
```

### Start and stop

`POST /control` with `{"action": "start"}`, `"stop"`, `"ping_now"` (every
host once, whether or not pinging is on) or `"reset"` (clear every counter).

### Measure

`POST /measure`

```json
{ "ids": [1, 2], "duration_secs": 60, "per_second": 5 }
```

pings the chosen hosts 1 to 10 times a second for a set time. `GET /measure`
returns `{"measurement": …}` with `running`, `started_at_ms`, `ends_at_ms`
and, per host, `sent`, `lost`, `expected`, `loss_pct`, `min_rtt_us`,
`avg_rtt_us`, `median_rtt_us`, `p95_rtt_us`, `max_rtt_us` and `jitter_us`.
`sent` and `lost` are pings that really went out. `expected` adds the beats
skipped while a failed ping waited for its timeout, and `loss_pct` is the
share of `expected` that got no answer, so a host that is down for half the
measurement reads 50% whatever the timeout is.
`POST /measure/stop` ends it early and keeps what it collected.

### Settings and host list

`GET /config` returns `settings`, `hosts_text` (the host list as typed),
`hosts_rev`, `groups`, `autostart`, `commands_locked` and `config_path`.
`hosts_rev` is a number that changes whenever the host list does, by any
path: this call, `POST /hosts/add`, removing hosts, or an agent. Each start
of pingtable begins it at a new random number (below 2^48, so it stays a
whole number in JavaScript), so a list read before a restart is not taken
for one read after it.

`PUT /config` changes settings, the list, or both. Every part is optional,
and what is left out stays as it is:

```json
{ "settings": { "timeout_ms": 500 } }
```

```json
{ "hosts_text": "192.168.1.1\nnas.lan", "hosts_rev": 7, "keep_stats": true }
```

```json
{ "settings": { "...": "the settings object from GET /config" }, "hosts_text": "192.168.1.1\nnas.lan", "keep_stats": true, "autostart": true }
```

- `settings` holds the fields to change; the others keep their values. The
  whole object read from `GET /config` works too.
- `hosts_text` replaces the whole list. Leave it out to keep the list as it
  is, including hosts added meanwhile by another window or an agent.
- `hosts_rev` is the one `GET /config` gave with the list you changed. If
  the list has changed since, nothing is applied and the reply is a 409 with
  `error` and the current `hosts_rev`: load the list again and make the
  change to that, so a host added meanwhile is not lost. Without `hosts_rev`
  the list is applied as it is sent.
- `keep_stats` keeps the history of hosts that stay in the list. It defaults
  to true when `hosts_text` is left out or is the list in use, and to false
  when another list is sent.
- `autostart` is whether pinging starts with the program.
- The other fields `GET /config` gives (`schema_version`, `groups`,
  `commands_locked`, `config_path`) are taken and left as they are, so a
  script can send back what it read with one value changed. Sending
  `"commands_locked": false` does not unlock the commands; only the file can.

Any other name, or one that is not a setting, is a 400 that names the one
meant: `{"settings": {"timeout": 500}}` asks "Did you mean `timeout_ms`?"
and changes nothing.

Replies with `hosts`, `hosts_rev` (after the change), `warnings` (entries
that were skipped), `commands_locked` and `save_error`.

Be careful with two settings, `on_down_command` and `on_up_command`: they
are commands pingtable runs. They are the reason the API needs a token.
With `"commands_locked": true` in `pingtable.json` (set by hand in the file;
the API cannot set it) they stay as the file has them: a `PUT` that changes
them changes everything else it asks for and leaves the commands alone.
pingtable reads `commands_locked` and the commands from the file at each
change, mutes included, so the lock counts even when it was set while
pingtable runs, and commands edited in the file meanwhile are kept and used.

### Check a list before using it

`POST /parse` with `{"text": "…"}` returns `count`, `groups`, `tcp` and
`warnings` for a host list without applying it.

`POST /import` with `{"text": "…contents of a CSV or text file…"}` returns
`format`, `hosts`, `skipped` and `text`: the file turned into host-list
lines, ready for `PUT /config` or `POST /hosts/add`.

### Export

`GET /export?format=json` or `format=csv` returns the table as a file.
The CSV leaves `interval_secs` and `timeout_ms` blank where the host has no
value of its own and follows the global settings, puts the host's own packet
options in `options` (`size=1400 ttl=8 df=on dscp=46 data=0x…`, as in the
host list), and ends each row with `exported_at` and `from` (this computer's
name). `POST /import` turns it back into the same hosts: the same targets,
descriptions, groups, tags, intervals, timeouts and packet options. Group
options are given to each host of the group instead, and a host from a
block comes back as a host of its own, with the interval it had.

### About this computer and this build

- `GET /source`: this computer's `name`, its `interfaces` and the addresses
  it sends from by default.
- `GET /info`: `version`, build facts, `config_path`, `log_path`, `license`
  (the program's source license, "Apache License 2.0").
- `GET /defaults`: the default settings and the example host list.

### Checking who answers

`GET /hello?nonce=N` needs no token. `N` is 16 to 128 letters, digits, `-`
or `_`, made up fresh for each check. The reply is

```json
{ "schema_version": 1, "proof": "3f1c…" }
```

where `proof` is HMAC-SHA256, keyed with the token, of the text
`hello-v1`, the port the server listens on and `N`, one per line
(`"hello-v1\n7464\n" + N`), written as 64 lower-case hex digits.

A client that holds the token works out the same value and compares it
before it sends the token anywhere. Something else listening on the port
cannot give the right answer, so it never learns the token. Because the port
is part of it, a listener on another port cannot pass the question on to the
real pingtable and hand back its answer. pingtable's own programs
(`pingtable-cli api`, the MCP tools, a second start of the app) do this, and
send their request on the same connection that gave the proof.

```python
import hashlib, hmac, json, secrets, urllib.request

nonce = secrets.token_hex(16)
with urllib.request.urlopen(f"http://127.0.0.1:{port}/api/v1/hello?nonce={nonce}") as reply:
    proof = json.load(reply)["proof"]
expected = hmac.new(token.encode(), f"hello-v1\n{port}\n{nonce}".encode(), hashlib.sha256).hexdigest()
assert hmac.compare_digest(proof, expected), "not this computer's pingtable"
```

To find the port, read `port` from `pingtable.json.run` beside the settings
file (written each time pingtable starts; it may be out of date if pingtable
has stopped), then fall back to `port` in `pingtable.json`.
