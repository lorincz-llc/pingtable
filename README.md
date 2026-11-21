# pingtable

[pingtable.net](https://pingtable.net)

Ping many hosts and watch them in one table, and check whether the TCP ports
you care about are listening. One Rust program does the work and serves its own
interface on `127.0.0.1`. The same interface runs in a native Windows window,
in a browser tab, or as an installed Chrome app, and the same checks run from
the command line, from a script through a local API, and from an AI agent
through an MCP server.

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

The zip holds two programs. They share one settings file and one host list.

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

pings each host one time, prints the result and exits:

```
HOST         IP             CHECK    STATE  STATUS              TIME ms  TTL  DESCRIPTION
192.168.1.1  192.168.1.1    icmp     UP     OK                     0.41   64
example.com  93.184.215.14  tcp:443  UP     OK                    12.80    -
```

```
pingtable-cli ports example.com 22 80 443
```

checks which TCP ports are listening on one host (see [Port check](#port-check)):

```
example.com (93.184.215.14): 2 of 3 ports open
   22  SSH              no answer
   80  HTTP             open          12.6 ms
  443  HTTPS            open          12.9 ms
```

```
pingtable-cli
```

runs pingtable without the window and opens the table in your default browser.
It prints a link such as `http://127.0.0.1:7464/?t=…`; the link carries an
access token, and the browser remembers it after the first visit. Ctrl+C stops
it. If the app is already running, this opens the browser on the running one.

Two more commands are for programs rather than people: `pingtable-cli mcp`
serves the checks to an AI agent, and `pingtable-cli api` prints how to call
the local API. Both are described under
[AI agents and scripts](#ai-agents-and-scripts).

| Option | Applies to | Meaning |
|---|---|---|
| `--format table\|json\|csv` | `once`, `ports` and `api` (no `csv`) | how the result is printed |
| `--timeout MS` | `once`, `ports` | how long to wait for each answer |
| `--hosts FILE` | `once` | read the host list from a file |
| `--described` | `once` | the list is one "address description" per line |
| `--port N` | serving | port for the interface (default 7464) |
| `--no-open` | serving | do not open the browser |
| `--config FILE` | serving | use this settings file |

`once` and `ports` exit with status 0 only when every host answered or every
port was open, 1 when not, and 2 when the command could not be understood, so
a script or a scheduled task can act on the answer. `pingtable-cli --help`
lists everything.

**From source, on any system:** `cargo run --release -p pingtable` is the
command-line program; add `-- once …` or `-- ports …` for the other commands.

## What it does

- **Table.** State, IP address, last and average round trip, a trend line of
  the last 60 pings, loss, reverse-lookup name and more. Columns can be shown,
  hidden, sorted, resized and reordered by dragging. Rows can be grouped. In a
  narrow window the less important columns give way, down to host, last and trend.
- **Port check.** `PORTS`, the `O` key, or right-click a host: pick 22, 80,
  443, 3389 or any other ports and see within seconds which are open, which
  the host refused and which gave no answer. One click turns the ports into
  rows that are checked all the time. More under [Port check](#port-check).
- **Adding hosts.** `+ ADD`, the `A` key, right-click, or paste: type a host
  and an optional description, choose PING, one or more ports, or both, and
  pick (or name) a group. "Remove from the list" takes a host out. The whole
  list can still be edited as text.
- **Importing.** `IMPORT A FILE…` in the Add window, the menu, the right-click
  menu and the empty table, or drop a CSV or text file anywhere on the window.
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

## AI agents and scripts

pingtable is built to be driven by a program as easily as by a person.

- **AI agents: an MCP server.** `pingtable-cli mcp` gives any agent that
  speaks the Model Context Protocol six tools: `ping`, `check_ports`,
  `trace_route`, `network_info`, and `table_status` and `table_add_hosts`
  for the running app's table. It works with hosted models and with models
  running on your own machine, and needs no account and no open port. Setting
  it up is one entry in the client's settings:

  ```json
  { "mcpServers": { "pingtable": { "command": "C:\\Tools\\pingtable\\pingtable-cli.exe", "args": ["mcp"] } } }
  ```

- **Agents and scripts with a shell.** `pingtable-cli once --format json` and
  `pingtable-cli ports --format json` print one JSON document and set the
  exit status.
- **Anything that speaks HTTP.** The local API does everything the window
  does. `pingtable-cli api` prints its address, the access token and example
  calls for curl and PowerShell.

[docs/ai-agents.md](docs/ai-agents.md) is the guide for agents: set-up, each
tool with its arguments and an example result, which tool to use when, and
the limits. [docs/api.md](docs/api.md) is the API reference.
[llms.txt](llms.txt) is the short version for a model to read first.

## Port check

A port check answers one question: is anything listening on this TCP port of
this host? pingtable opens an ordinary TCP connection to the port and closes it
again the moment it opens. No data is sent, and no administrator rights are
needed.

| Result | What happened | What it usually means |
|---|---|---|
| **open** | the connection opened | a service is listening |
| **closed** | the host answered and refused | the host is up, nothing listens on that port |
| **no answer** | nothing came back before the timeout | a firewall dropped it, or the host is off |

Three ways to use it:

- **Once, on the spot.** `PORTS` on the bar, the `O` key, or "Check ports…"
  on a host's right-click menu. Choose from the usual ports or type others
  (`8080 9000-9005`), press CHECK, and copy the result if you need to pass it on.
- **All the time.** Add a host with a port (`example.com:443`), tick ports in
  the Add window, or press WATCH IN THE TABLE after a check. Each port is its
  own row, shown as `host:port`, with the same trend, loss and alerts as a ping.
- **From a terminal or a script.** `pingtable-cli ports HOST [PORT...]`.

One check takes one host and at most 64 ports; it is a check of the ports you
name, not a scanner. It is TCP only. Check only hosts you are allowed to test.

A check waits up to 3 seconds for each port, all at once. Windows takes about
two seconds to report that a host refused a connection, so a shorter wait
could not tell "closed" from "no answer". For the same reason a port row in
the table, with the usual 1 second timeout, shows a closed port as timed out:
down either way, but give that row `timeout=3s` if the difference matters.

## Importing

A file chosen with `IMPORT A FILE…` (in the Add window, the menu, the
right-click menu, the empty table and the host list window), or dropped
anywhere on the window, is read as one of:

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
| `example.com:443` `[2001:db8::1]:22` | port check: is that TCP port listening? (nothing is sent) |
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
`pingtable-cli api` prints the address and token for this computer, and
[docs/api.md](docs/api.md) has every call with examples.

| Call | Purpose |
|---|---|
| `GET /api/v1/state?since=N&wait_ms=M` | table rows changed since sequence `N`; waits up to `M` ms for a change |
| `GET /api/v1/hosts/{id}` | one host: history, status runs, percentiles, rolling loss |
| `POST /api/v1/hosts/{id}` | `{"enabled": false}` mutes; `{"action": "ping_now" \| "reset"}` |
| `POST /api/v1/hosts/{id}/trace` | trace the route to a host |
| `POST /api/v1/hosts` | `{"action": "mute" \| "unmute" \| "solo" \| "unmute_all" \| "ping_now" \| "reset" \| "remove", "ids": [...]}` |
| `POST /api/v1/hosts/add` | `{"entries": ["nas.lan NAS"], "group": "Home lab"}` adds hosts to a group (created if new). With `"ports": [22, 443]` each host gets one row for each port; `"ping": true` keeps the ping row too |
| `POST /api/v1/portcheck` | `{"host": "example.com", "ports": [22, 443], "ports_text": "8000-8010"}` tries each port once and returns `open`, `closed` or `no_answer` for each. Changes nothing |
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
| `crates/core` | parser, list editing and import, probes, scheduler, statistics, measurements, traceroute, port check, exports. No UI. |
| `crates/app` | two programs: `pingtable` (command line, config file, HTTP server, MCP server) and `pingtable-window` (the Windows window, a WebView2 frame around the same interface) |
| `crates/app/assets` | Windows icon and manifest; the icon is drawn by `scripts/make-icon.py` |
| `ui/` | the interface: plain HTML, CSS and JavaScript, no build step. Compiled into the binary. |
| `site/` | the pingtable.net page: plain HTML, CSS and JavaScript, no build step, same two looks as the app |
| `docs/` | the guide for AI agents, the API reference, and the release how-tos (signing, winget, Store) |
| `packaging/` | the Microsoft Store package description |
| `brand/` | theme record, the icon options sheet, and the images used on GitHub |

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
| `pingtable-cli.exe` | the command-line program: `once`, `ports`, `api`, `mcp`, or the table in a browser |

To make the zip for a release, with the license, notices and disclaimer
inside and the notes for the release page beside it:

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

No installer and no administrator rights are needed: the folder can sit
anywhere the user can write, pinging uses the unprivileged Windows ICMP API,
and the program listens only on the loopback address.

The released executables are not code-signed yet, so Windows may warn about an
unknown publisher when they are downloaded from the internet, and a policy that
blocks unsigned programs will block them. The signing step is ready and waits
for a signing account ([docs/signing.md](docs/signing.md)).

## Signing: what to do

A checklist for whoever makes the releases. Steps 1 to 5 are done once, in a
browser. Steps 6 to 8 are done once on the build machine. After that every
release is signed by the usual packaging command. Details and the full list of
regions are in [docs/signing.md](docs/signing.md); Microsoft's side was checked
on 17 Nov 2026 and may have changed.

**Have these ready before you start**

- A card for an Azure subscription. Signing costs about $9.99 a month.
- The company's website, live, on a domain the company owns.
- Mailboxes on that same domain. Microsoft writes to them during the check.
- The company's business identifier (tax number or D-U-N-S number) and its
  registered name and address, exactly as on the registration papers.
- Your own government photo ID, for the identity check on a person.

**Once, in the browser**

1. **Open an Azure subscription** at [portal.azure.com](https://portal.azure.com)
   if the company has none.
2. **Create an Artifact Signing account.** Search the portal for "Artifact
   Signing" (it may still be listed as "Trusted Signing"), create an account
   on the Basic plan, and pick a region near you.
   Write down the account name and the region's endpoint, for example
   `eus.codesigning.azure.net` for East US.
3. **Ask for identity validation** in that account: a new identity of the
   Organization kind, Public Trust. Fill in the company details and answer
   Microsoft's emails. This is the slow step: from a day to several weeks.
   Microsoft's pages disagree on whether a company needs three years of
   history, so a young company may be refused. If it is, see the fallback
   below.
4. **Create a certificate profile** of the Public Trust kind once the identity
   shows as completed. Write down the profile name. The company name on it is
   what Windows will show.
5. **Give yourself the right to sign.** On the profile, assign the role
   "Artifact Signing Certificate Profile Signer" to your own account, in the
   portal under Access control, or with:

   ```
   az role assignment create --assignee <your object id> --role "Artifact Signing Certificate Profile Signer" --scope "/subscriptions/<subscription id>/resourceGroups/<resource group>/providers/Microsoft.CodeSigning/codeSigningAccounts/<account name>/certificateProfiles/<profile name>"
   ```

**Once, on the build machine**

6. **Sign in with the Azure CLI:**

   ```bash
   az login
   ```

7. **Get jsign.** Download `jsign-7.6.jar` from
   [its release page](https://github.com/ebourg/jsign/releases/tag/7.6) and
   check it. It needs Java 8 or newer.

   ```bash
   sha256sum jsign-7.6.jar
   ```

   The answer should be
   `6cf58aa080e7ff82656f320cff8d4a3bff30408df8766806dcba3740dd839fa5`.

8. **Write `signing.env`** beside `Cargo.toml`, with the three names from
   steps 2 and 4. Git ignores the file and it holds no secret.

   ```
   PINGTABLE_SIGN_ENDPOINT=eus.codesigning.azure.net
   PINGTABLE_SIGN_ACCOUNT=<account name>
   PINGTABLE_SIGN_PROFILE=<profile name>
   JSIGN_JAR=/path/to/jsign-7.6.jar
   ```

   Add `JAVA=/path/to/java` if `java` is not on the PATH.

**Every release**

9. **Package as usual:**

   ```bash
   scripts/package-windows.sh
   ```

   It builds, signs, asks Windows whether each signature is valid, and stops
   if one is not. The last lines should say `signed`, and the zip's readme and
   the release notes then say so too.

10. **Check by hand** whenever you like:

    ```bash
    scripts/sign-windows.sh --verify
    ```

    Each program should show `ok` and "signed by" the company's name.

**After the first signed release**

- Send the winget listing ([docs/winget.md](docs/winget.md)).
- Expect the SmartScreen warning to fade over weeks, not at once: Windows
  builds trust in a publisher as people download its programs.

**If Microsoft refuses the company**

Buy an ordinary organisation code signing certificate with a cloud-held key
instead, for example from SSL.com with its eSigner service, at about $309 a
year. `scripts/sign-windows.sh` would need its jsign options changed for that
service; the rest stays the same.

## Security and privacy

- pingtable talks only to the hosts in your list and to your system's name
  resolver. It has no telemetry, no update check and no account.
- Its interface is served on the loopback address only, and every API call
  needs the access token kept in the config file. Requests from other web
  pages and from other host names are refused.
- Alert commands are run by the system shell. Values substituted into them
  (`%HostName%` and the rest) are reduced to plain characters first, so text
  from an imported file cannot add commands of its own.
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
