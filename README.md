# pingtable

[pingtable.net](https://pingtable.net)

Ping many hosts and watch them in one table. One Rust program does the pinging
and serves its own interface on `127.0.0.1`. The same interface runs in a
native Windows window, in a browser tab, or as an installed Chrome app.

Status: early, and moving fast. Everything described here runs. Nothing is
stored between runs except settings, the host list and which hosts are muted.

## Download

Windows 10 and 11, 64-bit: get the zip from the
[latest release](https://github.com/lorincz-llc/pingtable/releases/latest),
unpack it anywhere and double-click `pingtable.exe`. There is no installer and
no administrator rights are needed.

The programs are not code-signed yet, so Windows may show an "unknown
publisher" warning the first time. Each release lists the SHA-256 of its zip.

## Run it

**Windows.** Double-click `pingtable.exe`. Closing the window stops pingtable.
`pingtable-cli.exe` is the console program described next.

**Anywhere, in a browser:**

```bash
cargo run --release -p pingtable
```

It prints a link such as `http://127.0.0.1:7464/?t=…` and opens it. The link
carries an access token; the browser remembers it after the first visit.

Ping once from the terminal and exit (exit status 0 only if every host answered):

```bash
cargo run --release -p pingtable -- once 192.168.1.1 example.com:443 --format table
```

`--format` is `table`, `json` or `csv`. `pingtable --help` lists every option.

A new install starts with an example host list. "Restore default hosts" in the
hosts window brings it back.

## What it does

- **Table.** State, IP address, last and average round trip, a trend line of
  the last 60 pings, loss, reverse-lookup name and more. Columns can be shown,
  hidden, sorted, resized and reordered by dragging. Rows can be grouped. In a
  narrow window the less important columns give way, down to host, last and trend.
- **Adding hosts.** `+ ADD`, the `A` key, right-click, or paste: type a host
  and an optional description and pick (or name) a group. Drop a CSV or text
  file on the window to import it. "Remove from the list" takes a host out.
  The whole list can still be edited as text.
- **Footer.** The computer and interface pings leave from, the interval and
  timeout, the ping rate and how many pings are in flight.
- **Detail panel.** Latency chart, percentiles, loss over the last 5 minutes
  and hour, and a timeline of outages for the selected host.
- **Selection.** Click, Ctrl-click or Shift-click rows, then right-click for
  options: copy a summary, ping now, mute, ping only these, measure, trace
  route, reset counters.
- **Mute.** A muted host is greyed out and not pinged. Mutes are remembered.
- **Measure.** Ping chosen hosts 1 to 10 times a second for a set time and get
  a report: loss, fastest, average, median, 95th percentile, slowest, jitter.
- **Trace route.** The routers between this computer and a host.
- **Alerts.** A host is down after N failed pings in a row and up after M good
  ones. Each crossing shows a notice, glows on the row, can play a sound and
  can run a command.
- **Two looks.** Light (the default) and dark, switched from the bar or with `T`.

## Importing

A file dropped on the window, or chosen with "Import a file" in the host list
window, is read as one of:

| File | How it is read |
|---|---|
| CSV with a header row | columns are matched by name: `host`, `hostname`, `address`, `ip`, `description`, `name`, `group`, `tags`, `port`, `interval` and common variants. Exports from pingtable and from other ping tools work as they are. |
| CSV without a header | host, description, group |
| Plain text | a host list, kept as it is |

Comma, semicolon and tab separators are all accepted. Rows without a usable
host are skipped and counted. Nothing changes until the list is applied.

## Host list

| You type | It does |
|---|---|
| `nas.lan` `192.168.1.10` `2001:db8::1` | ICMP ping |
| `example.com:443` `[2001:db8::1]:22` | TCP connect to a port (nothing is sent) |
| `https://example.com` | TCP 443 (`http` is 80, `ssh` is 22) |
| `10.0.0.1-10.0.0.20` | every address in the range |
| `10.0.0.0/24` | every address in the block |
| `Group: Home lab` | start a group |
| `# note` | ignored |

Options follow the host or group they apply to. A host's own option wins over
its group's, which wins over the global setting.

| Option | Meaning |
|---|---|
| `every=5s` | this host's own interval (`s`, `m`, `h`); the default is 1 second |
| `timeout=300ms` | this host's own timeout |
| `tag=lan` | a tag; repeat for more |
| `size=1400` | ICMP payload size in bytes |
| `ttl=8` | hop limit |
| `df=on` | do not fragment |
| `dscp=46` or `tos=184` | traffic marking |
| `data=hello` or `data=0xBEEF` | payload contents, repeated to fill the packet |

With "one host per line, followed by a description" switched on, the first word
is the target and the rest of the line is its description.

## How it works

- **Scheduling.** Every host has its own cadence, start to start, under one cap
  on pings in flight. A host is never pinged twice at once; a missed beat is
  skipped, not made up.
- **ICMP.** On Windows it calls the system ICMP API (`IcmpSendEcho`,
  `Icmp6SendEcho2`), which needs no administrator rights. That API sets size,
  hop limit, flags, marking and payload, but not the source address of a
  packet or raw headers. Elsewhere pingtable runs the system `ping` once per
  probe. Round-trip time is measured by pingtable itself in microseconds.
- **Source.** Pings leave from the address the system picks, or from one
  chosen in the settings from this computer's interfaces.
- **MAC addresses** are read from the system neighbour table; pingtable never
  sends ARP.
- **Faults.** A fault inside one ping, one request or a background loop is
  contained: the ping is recorded as an error, the request ends, the loop
  restarts. Anything unexpected is written to `pingtable.log` in the per-user
  data folder (`%LOCALAPPDATA%\pingtable` on Windows).
- **Config** is one JSON file: `pingtable.json` beside the executable if it
  exists, otherwise the per-user config folder. `--config FILE` overrides.

## API

Everything the interface does goes through `/api/v1` on the same port. Send the
token in an `X-Pingtable-Token` header. Field names carry their units
(`rtt_us`, `t_ms`, `interval_secs`) and documents have `schema_version`.

| Call | Purpose |
|---|---|
| `GET /api/v1/state?since=N&wait_ms=M` | table rows changed since sequence `N`; waits up to `M` ms for a change |
| `GET /api/v1/hosts/{id}` | one host: history, status runs, percentiles, rolling loss |
| `POST /api/v1/hosts/{id}` | `{"enabled": false}` mutes; `{"action": "ping_now" \| "reset"}` |
| `POST /api/v1/hosts/{id}/trace` | trace the route to a host |
| `POST /api/v1/hosts` | `{"action": "mute" \| "unmute" \| "solo" \| "unmute_all" \| "ping_now" \| "reset" \| "remove", "ids": [...]}` |
| `POST /api/v1/hosts/add` | `{"entries": ["nas.lan NAS"], "group": "Home lab"}` adds hosts to a group (created if new) |
| `POST /api/v1/import` | `{"text": "..."}` turns a CSV or text file into host-list lines, without applying them |
| `POST /api/v1/measure` | `{"ids": [...], "duration_secs": 60, "per_second": 5}` starts a measurement |
| `GET /api/v1/measure`, `POST /api/v1/measure/stop` | its progress and result; end it early |
| `GET` / `PUT /api/v1/config` | read or replace settings and host list |
| `GET /api/v1/defaults` | the example host list and default settings |
| `POST /api/v1/control` | `{"action": "start" \| "stop" \| "ping_now" \| "reset"}` |
| `POST /api/v1/parse` | check a host list without applying it |
| `GET /api/v1/source` | this computer's name and addresses |
| `GET /api/v1/export?format=json\|csv` | the table as a file |
| `GET /api/v1/info` | version and build facts |

The server listens on loopback only, rejects requests whose `Host` or `Origin`
is not this machine, and requires the token on every API call. The token
matters: the settings include commands that pingtable will run.

## Layout

| Path | What |
|---|---|
| `crates/core` | parser, list editing and import, probes, scheduler, statistics, measurements, traceroute, exports. No UI. |
| `crates/app` | two programs: `pingtable` (command line, config file, HTTP server) and `pingtable-window` (the Windows window, a WebView2 frame around the same interface) |
| `crates/app/assets` | Windows icon and manifest; the icon is drawn by `scripts/make-icon.py` |
| `ui/` | the interface: plain HTML, CSS and JavaScript, no build step. Compiled into the binary. |
| `site/` | the pingtable.net page: plain HTML, CSS and JavaScript, no build step, same two looks as the app |
| `brand/` | theme record and the icon options sheet |

While working on the interface, serve it from disk so a browser refresh picks
up edits without rebuilding:

```bash
PINGTABLE_UI_DIR=$PWD/ui cargo run -p pingtable
```

## Tests

```bash
cargo test --workspace
```

## Building for Windows from Linux

One-time setup:

```bash
sudo apt install -y mingw-w64
```

```bash
rustup target add x86_64-pc-windows-gnu
```

Build and collect the package in `dist/windows`:

```bash
scripts/build-windows.sh
```

| File | What |
|---|---|
| `pingtable.exe` | the window app |
| `WebView2Loader.dll` | Microsoft's loader for the window; keep it beside `pingtable.exe` |
| `pingtable-cli.exe` | the console program (`serve`, `once`) |

To make the zip for a release, with the license, notices and disclaimer
inside and the notes for the release page beside it:

```bash
scripts/package-windows.sh
```

The window needs the Microsoft Edge WebView2 Runtime, which Windows 11 includes.
Without it, pingtable opens in the default browser instead. A `pingtable.json`
beside the exe makes the folder portable; otherwise settings live in
`%APPDATA%\pingtable`.

No installer and no administrator rights are needed: the folder can sit
anywhere the user can write, pinging uses the unprivileged Windows ICMP API,
and the program listens only on the loopback address.

The executables are not code-signed, so Windows may warn about an unknown
publisher when they are downloaded from the internet, and a policy that blocks
unsigned programs will block them.

## Security and privacy

- pingtable talks only to the hosts in your list and to your system's name
  resolver. It has no telemetry, no update check and no account.
- Its interface is served on the loopback address only, and every API call
  needs the access token kept in the config file. Requests from other web
  pages and from other host names are refused.
- Alert commands are run by the system shell. Values substituted into them
  (`%HostName%` and the rest) are reduced to plain characters first, so text
  from an imported file cannot add commands of its own.
- Please report a security problem privately to pingtable@lorincz.llc rather
  than in a public issue. More in [SECURITY.md](SECURITY.md).

## Disclaimer

pingtable is provided "as is", without warranty of any kind, express or
implied, and its authors and contributors accept no liability for any damage
or loss that comes from using it. You use it at your own risk.

Use it only on networks and hosts you own or are authorised to test. You are
responsible for the traffic it sends, for the commands it runs for you, and
for complying with the policies and laws that apply to you. It is not meant
for any use where a failure or a wrong reading could cause harm.

The full text is in [DISCLAIMER.md](DISCLAIMER.md). It restates sections 7 and
8 of the [license](LICENSE), which govern.

## Contributing

pingtable is open source and under active development. Each release is listed
in [CHANGELOG.md](CHANGELOG.md). Bug reports and ideas are welcome as
[issues](https://github.com/lorincz-llc/pingtable/issues); read
[CONTRIBUTING.md](CONTRIBUTING.md) before sending a pull request.

## License

Copyright 2026 Lorincz LLC. Licensed under the
[Apache License, Version 2.0](LICENSE). Third-party components are listed in
[THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md). "pingtable" and its mark
are not licensed for use as your own name or brand (section 6 of the license).

## Credits

pingtable@lorincz.llc
