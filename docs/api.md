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
| Address | `http://127.0.0.1:7464/api/v1` (the port can be changed with `--port` or in the settings file) |
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
Invoke-RestMethod http://127.0.0.1:7464/api/v1/portcheck -Method Post -Headers $h `
  -ContentType 'application/json' -Body '{"host":"example.com","ports":[22,443]}'
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
ports = call("/portcheck", {"host": "example.com", "ports": [22, 443]})
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

## Calls

| Call | Purpose |
|---|---|
| [`GET /state`](#the-table) | The table: every host and its numbers |
| [`POST /portcheck`](#port-check) | Try TCP ports on one host, once |
| [`POST /hosts/add`](#add-hosts) | Add hosts to the list |
| [`POST /hosts`](#act-on-hosts) | Mute, unmute, ping now, reset or remove several hosts |
| [`GET` / `POST /hosts/{id}`](#one-host) | One host's history; mute it, ping it now, reset it |
| [`POST /hosts/{id}/trace`](#trace-route) | Trace the route to a host |
| [`POST /control`](#start-and-stop) | Start, stop, ping every host once, reset counters |
| [`POST /measure`](#measure), `GET /measure`, `POST /measure/stop` | A timed measurement |
| [`GET` / `PUT /config`](#settings-and-host-list) | Read or replace settings and the host list |
| [`POST /parse`, `POST /import`](#check-a-list-before-using-it) | Check a host list, or turn a CSV into one, without applying it |
| [`GET /export`](#export) | The table as a JSON or CSV file |
| [`GET /source`, `GET /info`, `GET /defaults`](#about-this-computer-and-this-build) | This computer's addresses, build facts, default settings |

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
- The example leaves out some fields: `order`, `reply_ip`, `mac`, `line`,
  `removable`, `packet`, `recent_loss_pct`, `max_consec_failed`, `last_ttl`,
  `last_ok_at_ms`, `last_failed_at_ms`, `active`, `running_since_ms`.

To follow changes without polling fast, pass the `seq` you last saw:
`GET /state?since=11&wait_ms=20000` waits up to 20 seconds (25 at most) for
something to change and returns only the hosts that did, with
`"full": false`. A reply with `"full": true` replaces what you hold.

### Port check

`POST /portcheck` tries each port once and changes nothing.

```json
{ "host": "example.com", "ports": [22, 443], "ports_text": "8000-8002" }
```

`host` is one name or address; `example.com:8443` and `https://example.com`
add their port. `ports` and `ports_text` are both optional but one port must
come from somewhere. At most 64 ports.

```json
{
  "schema_version": 1,
  "host": "example.com",
  "target": "93.184.215.14",
  "from": "MY-PC",
  "source": "192.168.1.50",
  "timeout_ms": 3000,
  "checked_at_ms": 1794932547799,
  "open": 1,
  "results": [
    { "port": 22, "service": "SSH", "state": "no_answer", "status": "timeout", "rtt_us": null },
    { "port": 443, "service": "HTTPS", "state": "open", "status": "ok", "rtt_us": 12900 }
  ]
}
```

`state` is `open` (something is listening), `closed` (the host answered and
refused), `no_answer` (nothing came back: a firewall, or the host is off) or
`failed`. A name that does not resolve is a 409.

### Add hosts

`POST /hosts/add`

```json
{ "entries": ["nas.lan NAS in the closet"], "group": "Home lab", "ports": [445], "ping": true }
```

Each entry is one host, optionally followed by a description and options.
`group` is created if it does not exist; leave it out for no group. With
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
list).

### One host

`GET /hosts/{id}` returns `row` (as in the table), `history` (every sample
kept), `runs` (stretches of the same status; failed runs are outages),
`p50_rtt_us`, `p95_rtt_us`, `p99_rtt_us` and `windows` (loss over the last 5
minutes and hour).

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
and, per host, `sent`, `lost`, `loss_pct`, `min_rtt_us`, `avg_rtt_us`,
`median_rtt_us`, `p95_rtt_us`, `max_rtt_us` and `jitter_us`.
`POST /measure/stop` ends it early and keeps what it collected.

### Settings and host list

`GET /config` returns `settings`, `hosts_text` (the host list as typed),
`groups`, `autostart` and `config_path`.

`PUT /config` replaces settings and list together:

```json
{ "settings": { "...": "the settings object from GET /config" }, "hosts_text": "192.168.1.1\nnas.lan", "keep_stats": true, "autostart": true }
```

Send back the whole `settings` object you read, changed where you want it
changed. `keep_stats` keeps the history of hosts that are still in the list.
Replies with `hosts`, `warnings` (entries that were skipped) and
`save_error`.

Be careful with two settings, `on_down_command` and `on_up_command`: they
are commands pingtable runs. They are the reason the API needs a token.

### Check a list before using it

`POST /parse` with `{"text": "…"}` returns `count`, `groups`, `tcp` and
`warnings` for a host list without applying it.

`POST /import` with `{"text": "…contents of a CSV or text file…"}` returns
`format`, `hosts`, `skipped` and `text`: the file turned into host-list
lines, ready for `PUT /config` or `POST /hosts/add`.

### Export

`GET /export?format=json` or `format=csv` returns the table as a file.

### About this computer and this build

- `GET /source`: this computer's `name`, its `interfaces` and the addresses
  it sends from by default.
- `GET /info`: `version`, build facts, `config_path`, `log_path`.
- `GET /defaults`: the default settings and the example host list.
