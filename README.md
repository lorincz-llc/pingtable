# pingtable

[pingtable.net](https://pingtable.net)

Monitor ping and TCP connectivity across many hosts in one live table. Paste a
list such as `web-01.example:443`, `web-02.example:443` and `db.example:5432`
to see which ports accept connections, how long each connection takes and which
checks are refused or time out. Add a bare hostname to watch its ICMP replies
alongside those TCP checks.

One Rust program runs
the checks and serves the interface on `127.0.0.1`. The same interface runs in a native Windows window,
in a browser tab, or as an installed Chrome app, and the same checks run from
the command line, from a script through a local API, and from an AI agent
through an MCP server.

The app, command-line tools, local API and MCP server are free and open source.
Settings, the host list and muted hosts are saved locally.

## Download

Choose a ZIP for 64-bit Windows 10 or 11 from the
[latest release](https://github.com/lorincz-llc/pingtable/releases/latest).

- **Native window** (`pingtable-VERSION-windows-x64.zip`). Double-click
  `pingtable.exe`. The ZIP includes the CLI and Microsoft’s WebView2 loader.
- **Browser edition** (`pingtable-VERSION-browser-windows-x64.zip`).
  Double-click `pingtable-cli.exe` and keep its terminal open to run the full
  table in your default browser. This ZIP contains one executable, the README
  and license notices.

Both editions are free and run without installation or administrator rights.

Unpack it into a folder that only your account can change, such as
`%LOCALAPPDATA%\Programs\pingtable`. A folder you make yourself at the top of
a drive, such as `C:\Tools`, can normally be changed by every account on the
computer, and anyone who can replace the programs there can run code as you.

The release notes list both ZIP checksums and the programs’ signing status.
For unsigned programs, Windows may show an “unknown publisher” warning.

## Run it

The native-window ZIP holds two programs. They share one settings file and one host list.

| Program | What it is | Reach for it when |
|---|---|---|
| `pingtable.exe` | the app, in its own window | you want to watch the table |
| `pingtable-cli.exe` | the same program without the window, for a terminal | you want one quick answer, a script or scheduled task, or the table in a browser tab |

**The app.** Double-click `pingtable.exe`. Closing the window stops pingtable.
A new install starts with an example host list. "Restore default hosts" in the
host list window brings it back.

**The command line.** `pingtable-cli.exe` needs no window and no administrator
rights. Open a terminal in the folder (in File Explorer, type `cmd` in the
address bar and press Enter) and use one of its commands:

```
pingtable-cli once 192.168.1.1 example.com:443
```

pings the first host, checks the named TCP service, prints both results and exits:

```
HOST         IP             CHECK    STATE  STATUS              TIME ms  TTL  DESCRIPTION
192.168.1.1  192.168.1.1    icmp     UP     OK                     0.41   64
example.com  93.184.215.14  tcp:443  UP     OK                    12.80    -
```

```
pingtable-cli
```

runs pingtable without the window and opens the table in your default browser.
It prints a link such as `http://127.0.0.1:7464/?t=…`; the link carries an
access token, and the browser remembers it after the first visit. Ctrl+C stops
it. If the app is already running, this opens the browser on the running one.
On Linux the browser is given a small page that only your account can read
and that leads on to the link, because other accounts can see the arguments a
program is started with. If your browser cannot open that page (some sandboxed
browsers cannot), open the printed link yourself.

Two more commands are for programs rather than people: `pingtable-cli mcp`
serves the checks to an AI agent, and `pingtable-cli api` prints how to call
the local API. Both are described under
[AI agents and scripts](#ai-agents-and-scripts).

| Option | Applies to | Meaning |
|---|---|---|
| `--format table\|json\|csv` | `once` (all formats), `api` (table or JSON) | how the result is printed |
| `--timeout MS` | `once` | how long to wait for each answer |
| `--hosts FILE` | `once` | read the host list from a file |
| `--described` | `once` | the list is one "address description" per line |
| `--port N` | serving | port for the interface (default 7464) |
| `--no-open` | serving | do not open the browser |
| `--config FILE` | serving, `mcp`, `api` | use this settings file |
| `--allow-add` | `mcp` | also offer the tool that adds hosts to the running app's table |

`once` exits with status 0 when every host answers, 1 when a check fails, and
2 for invalid input. `pingtable-cli --help` lists every option.

To run from source, use `cargo run --release -p pingtable`. Append
`-- once example.com:443` for one TCP check.

## What it does

- **Table.** State, IP address, last and average round trip, a trend line of
  the last 60 pings, loss, reverse-lookup name and more. Columns can be shown,
  hidden, sorted, resized and reordered by dragging. Rows can be grouped. In a
  narrow window the less important columns give way, down to host, last and trend.
  Rows are compact; "Roomy rows" in the menu makes them taller.
- **TCP monitoring.** Add a service such as `example.com:443` to track its
  availability, latency and loss alongside the hosts you ping.
- **Adding hosts.** `+ Add`, the `A` key, right-click, or paste: type a host
  and an optional description, choose Ping, one or more ports, or both, and
  pick (or name) a group. "Remove from the list" takes a host out. The whole
  list can still be edited as text.
- **Importing.** `Import a file…` in the Add window, the menu, the right-click
  menu and the empty table, or drop a CSV or text file anywhere on the window.
- **Footer.** The computer and interface pings leave from, the interval and
  timeout, the ping rate and how many pings are in flight.
- **Detail panel.** Latency chart, percentiles, loss over the last 5 minutes
  and hour, and a timeline of outages for the selected host. Right-click it to
  copy the figures, the timeline or every kept ping.
- **Selection.** Click, Ctrl-click or Shift-click rows, then right-click for
  options: copy, ping now, mute, ping only these, measure, trace route, reset
  counters.
- **Copy and paste.** Right-click a host, a group, a selection or an empty
  part of the table to copy it as text (columns lined up, for a chat, a ticket
  or a terminal), Markdown (a table for GitHub or a wiki), JSON (the API's
  fields, for a program or an AI agent) or TSV (pastes into Excel or Google
  Sheets, one value per cell). Text, Markdown and TSV hold the columns you chose,
  in their order; "Copy table as" follows the filter, search, sort and closed
  groups. Right-click a cell to copy just its value; "Copy summary" gives a few
  plain lines about a host. Ctrl+C copies the selected hosts as a text table,
  Ctrl+Shift+C as Markdown. Measure and Trace route offer the same copy formats.
- **Mute.** A muted host is greyed out and not pinged. Mutes are remembered.
- **Measure.** Ping chosen hosts 1 to 10 times a second for a set time and get
  a report: loss, fastest, average, median, 95th percentile, slowest, jitter.
- **Trace route.** The routers between this computer and a host.
- **Alerts.** A host is down after N failed pings in a row and up after M good
  ones. Each crossing shows a notice, glows on the row, can play a sound and
  can run a command.
- **Two looks.** Light (the default) and dark, switched from the bar or with `T`.

## AI agents and scripts

- **AI agents: an MCP server.** `pingtable-cli mcp` gives any agent that
  speaks the Model Context Protocol the tools `ping`,
  `trace_route`, `network_info` and `table_status` (the running app's
  table). Started as `pingtable-cli mcp --allow-add` it also offers
  `table_add_hosts`, which adds a few hosts to that table. It works with
  hosted models and with models running on your own machine, and needs no
  account and no open port. Setting it up is one entry in the client's
  settings:

  ```json
  { "mcpServers": { "pingtable": { "command": "C:\\Users\\you\\AppData\\Local\\Programs\\pingtable\\pingtable-cli.exe", "args": ["mcp"] } } }
  ```

- **Agents and scripts with a shell.** `pingtable-cli once --format json HOST`
  prints one JSON document and sets the exit status.
- **Anything that speaks HTTP.** The local API does everything the window
  does. `pingtable-cli api` prints its address, the access token and example
  calls for curl and PowerShell.

[docs/ai-agents.md](docs/ai-agents.md) is the guide for agents: set-up, each
tool with its arguments and an example result, which tool to use when, and
the limits. [docs/api.md](docs/api.md) is the API reference.
[llms.txt](llms.txt) is the short version for a model to read first.

## Monitoring a TCP service

Add a service address such as `example.com:443` or `[2001:db8::1]:22` to the
host list, or choose TCP ports when adding a host. Each service gets its own
row with a latency trend, loss history and alerts. A successful TCP check
opens the connection and closes it immediately without sending application
data. The row reports a refusal if the host rejects the connection, or a
timeout if the configured wait elapses first. Use `timeout=3s` on a row
when the OS needs longer to report a refusal.

For a single result, run `pingtable-cli once example.com:443`.

## Importing

A file chosen with `Import a file…` (in the Add window, the menu, the
right-click menu, the empty table and the host list window), or dropped
anywhere on the window, is read as one of:

| File | How it is read |
|---|---|
| CSV with a header row | columns are matched by name: `host`, `hostname`, `address`, `ip`, `description`, `name`, `group`, `tags`, `port`, `probe`, `interval`, `timeout_ms` and common variants. Exports from pingtable and from other ping tools work as they are. |
| CSV without a header | host, description, group |
| Plain text | a host list, kept as it is |

Comma, semicolon and tab separators are all accepted. Rows without a usable
host are skipped and counted. Nothing changes until the list is applied.

## Host list

| You type | It does |
|---|---|
| `nas.lan` `192.168.1.10` `2001:db8::1` | ICMP ping |
| `example.com:443` `[2001:db8::1]:22` | TCP connection to the named service |
| `https://example.com` | TCP 443 (`http` is 80, `ssh` is 22) |
| `10.0.0.1-10.0.0.20` | every address in the range, pinged every 30 seconds |
| `10.0.0.0/24` | every address in the block (`/16` or smaller), pinged every 30 seconds |
| `Group: Home lab` | start a group |
| `# note` | ignored |

A name that ends in a number, such as `10.0.0.010`, is refused: write an
address as four numbers without leading zeros.

Options follow the host or group they apply to. A host's own option wins over
its group's, which wins over the global setting.

| Option | Meaning |
|---|---|
| `every=5s` | this host's own interval (`s`, `m`, `h`); the default is 1 second, or 30 seconds for a range or block |
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
  On Linux it is readable by your account only. A file that cannot be read
  (a typing slip, say) is never overwritten: it is renamed to
  `pingtable.json.unreadable-<date and time>`, pingtable starts with the
  default settings, and the window says so once. If it cannot be renamed,
  it stays where it is and nothing is saved over it until pingtable is
  closed. A file that cannot be opened at all (a folder in its place, no
  permission, another program holding it) stops pingtable from starting,
  with a message that says why. A running pingtable writes
  the port it really uses to `pingtable.json.run` beside it, which is how
  `pingtable-cli mcp` and `pingtable-cli api` find it.
- **Alert commands** run through the system shell (`cmd` on Windows, `sh`
  elsewhere), at most 4 at a time with up to 64 more waiting; any beyond that
  are left out and the window says how many. A command still running after
  60 seconds is stopped, together with the programs it started (on Linux,
  its process group; on Windows, a job object), and one that cannot be
  started is reported in the window. When pingtable itself is stopped (Ctrl+C,
  a closed terminal, `kill`), the commands still running are stopped with it. Values put into a command
  (`%HostName%`, `%Description%` and the rest) keep only ASCII letters,
  digits, spaces and `. _ : @ / + , = -`; anything else becomes `_`.
  `%Description%` and `%Group%`, which can come from an imported file or an
  agent, have every space turned into `_` as well, quotes or no quotes, and
  keep `-`, `/` and `@` only straight after a letter or a digit, so no word
  in them can start with one: `NAS - rack 2` becomes `NAS___rack_2`. They
  cannot add arguments of their own. For the exact text, have the command run
  a script that reads the environment variables `PINGTABLE_DESCRIPTION`,
  `PINGTABLE_GROUP`, `PINGTABLE_HOSTNAME` and so on; written as
  `%PINGTABLE_DESCRIPTION%` in a `cmd` command line they are not safe.
  Setting `"commands_locked": true` in `pingtable.json` keeps the commands as
  the file has them: the window and the API cannot change them. pingtable
  reads it from the file each time, so it counts even when set while
  pingtable runs.

## API

Everything the interface does goes through `/api/v1` on the same port. Send the
token in an `X-Pingtable-Token` header. Field names carry their units
(`rtt_us`, `t_ms`, `interval_secs`) and documents have `schema_version`.
`pingtable-cli api` prints the address and token for this computer, and
[docs/api.md](docs/api.md) has every call with examples.

| Call | Purpose |
|---|---|
| `GET /api/v1/state?since=N&wait_ms=M` | table rows changed since sequence `N`; waits up to `M` ms for a change. `state=down,failing&limit=100` sends only rows in those states, at most 100 |
| `GET /api/v1/hosts/{id}` | one host: history, status runs, percentiles, rolling loss |
| `POST /api/v1/hosts/{id}` | `{"enabled": false}` mutes; `{"action": "ping_now" \| "reset"}` |
| `POST /api/v1/hosts/{id}/trace` | trace the route to a host |
| `POST /api/v1/hosts` | `{"action": "mute" \| "unmute" \| "solo" \| "unmute_all" \| "ping_now" \| "reset" \| "remove", "ids": [...]}` |
| `POST /api/v1/hosts/add` | `{"entries": ["nas.lan NAS"], "group": "Home lab"}` adds hosts to a group (created if new). With `"ports": [22, 443]` each host gets one row for each port; `"ping": true` keeps the ping row too |
| `POST /api/v1/import` | `{"text": "..."}` turns a CSV or text file into host-list lines, without applying them |
| `POST /api/v1/measure` | `{"ids": [...], "duration_secs": 60, "per_second": 5}` starts a measurement |
| `GET /api/v1/measure`, `POST /api/v1/measure/stop` | its progress and result; end it early |
| `GET` / `PUT /api/v1/config` | read or change settings and host list; a `PUT` with only one of them leaves the other as it is, and one that sends back the `hosts_rev` it read is refused (409) if the list changed meanwhile |
| `GET /api/v1/defaults` | the example host list and default settings |
| `POST /api/v1/control` | `{"action": "start" \| "stop" \| "ping_now" \| "reset"}` |
| `POST /api/v1/parse` | check a host list without applying it |
| `GET /api/v1/source` | this computer's name and addresses |
| `GET /api/v1/export?format=json\|csv` | the table as a file |
| `GET /api/v1/info` | version and build facts |
| `GET /api/v1/hello?nonce=N` | proof that this server holds the token, without the token: see [docs/api.md](docs/api.md#checking-who-answers) |

The server listens on loopback only, rejects requests whose `Host` or `Origin`
is not this machine, and requires the token on every API call. The token
matters: the settings include commands that pingtable will run.

## Layout

| Path | What |
|---|---|
| `crates/core` | parser, list editing and import, probes, scheduler, statistics, measurements, traceroute, TCP monitoring, exports. No UI. |
| `crates/app` | two programs: `pingtable` (command line, config file, HTTP server, MCP server) and `pingtable-window` (the Windows window, a WebView2 frame around the same interface) |
| `crates/app/assets` | Windows icon and manifest; the icon is drawn by `scripts/make-icon.py` |
| `ui/` | the interface: plain HTML, CSS and JavaScript, no build step. Compiled into the binary. |
| `site/` | the pingtable.net page: plain HTML, CSS and JavaScript, no build step, same two looks as the app |
| `docs/` | the guide for AI agents, the API reference, and the release how-tos (signing, winget, Store) |
| `packaging/` | the Microsoft Store package description |
| `brand/` | theme record, the icon options sheet, and the images used on GitHub |

While working on the interface, serve it from disk so a browser refresh picks
up edits without rebuilding (a debug build only; a release build always serves
its own copy):

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
| `pingtable-cli.exe` | the command-line program for `once`, `api`, `mcp` and the browser table |

To package both editions with their licenses, SHA-256 files and release notes:

```bash
scripts/package-windows.sh
```

Three more steps belong to a release, each with its own page:

| Step | Script | Page |
|---|---|---|
| Sign the programs so Windows shows the publisher | `scripts/sign-windows.sh` (run by the package script once set up) | [docs/signing.md](docs/signing.md) |
| List the release in winget | `scripts/winget-manifest.sh` | [docs/winget.md](docs/winget.md) |
| Pack the Microsoft Store build | `scripts/package-msix.sh` | [docs/store.md](docs/store.md) |

The window needs the Microsoft Edge WebView2 Runtime, which Windows 11 includes.
Without it, pingtable opens in the default browser instead. A `pingtable.json`
beside the exe makes the folder portable; otherwise settings live in
`%APPDATA%\pingtable`.

No installer and no administrator rights are needed: the folder can sit in
any folder the user can write (best one that only that account can change; see
[Download](#download)), pinging uses the unprivileged Windows ICMP API, and the
program listens only on the loopback address.

## Signing: what to do

The packaging script records the signing status of the executables in both
ZIPs and in the release notes. See the [signing guide](docs/signing.md)
for publisher setup and verification.

## Security and privacy

- pingtable talks only to the hosts in your list and to your system's name
  resolver. It has no telemetry, no update check and no account. A license
  key is checked on this computer, by its form, and is never sent anywhere.
- Its interface is served on the loopback address only, and every API call
  needs the access token kept in the config file. Requests from other web
  pages and from other host names are refused.
- Alert commands are run by the system shell. Values substituted into them
  (`%HostName%` and the rest) are reduced to plain characters first, so text
  from an imported file or an agent cannot add commands or arguments of its
  own (details under [How it works](#how-it-works)). `"commands_locked": true`
  in the settings file keeps the commands from being changed at all.
- pingtable's own programs never send the token to whatever happens to listen
  on its port: they first ask it to prove that it holds the token.
- Please report a security problem privately to support@pingtable.net rather
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

hello@pingtable.net
