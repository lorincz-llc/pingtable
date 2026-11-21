# Changelog

What changed in each release, newest first. Versions follow
[semantic versioning](https://semver.org); before 1.0, anything may still
change between releases.

## 0.4.0 - 2026-11-21

Pingtable remains free and open source. This release adds four clipboard formats,
a recorded browser demo and fixes for large host lists, scheduling and imports.

### TCP monitoring

- Add a service as `host:port` to watch it in the table, or run
  `pingtable-cli once host:port` for one result.
- The standalone Port check window, `ports` command, `/api/v1/portcheck`
  endpoint and `check_ports` MCP tool have been removed. Existing TCP
  monitoring rows carry forward with their settings.

### The window

- **A new look: the NOC console.** Near-black with a luminous cyan in the
  dark; by day a light table under the same dark console bar. State squares
  glow softly and keep their shapes, failures show as tight status chips, and
  data is set in a heavier monospace. The window's title bar is dark in both
  looks, to match the bar.
- "Last ms" is coloured by latency: under 100 ms plain (cyan in the dark),
  100 to 199 ms amber, 200 ms and over magenta, so a far host never reads as
  a failed one.
- JetBrains Mono is built in for data, so every computer shows the same
  letterforms (SIL Open Font License 1.1; see THIRD-PARTY-NOTICES.md).
- **A denser table.** Words are set in the system sans-serif and hosts,
  addresses, numbers and times in monospace; every other row is striped
  instead of ruled, and rows are 26 pixels high instead of 32. Headings,
  buttons, tallies and windows are in sentence case. A failed ping reads
  "Timed out" or "No name" in the row instead of capitals. "Roomy rows" in the
  menu brings back taller rows.
- **Copy as text, Markdown, JSON or TSV.** Right-click a host, a group, a
  selection or an empty part of the table and copy it as a lined-up text
  table, a Markdown table, JSON with the API's field names, or tab-separated
  values that paste into a spreadsheet one value per cell (numbers without
  thousands separators). Text, Markdown and
  TSV copy the columns you chose, in their order, and the whole table follows the
  filter, search, sort and closed groups. Right-click a cell to copy just its
  value.
- Ctrl+C now copies the selected hosts as a text table, and Ctrl+Shift+C
  copies them as Markdown. "Copy summary" is still on the right-click menu,
  which now also works with the arrow keys.
- Right-click the detail panel to copy its figures, its timeline or every kept
  ping. Measure and Trace route have a "Copy as…" button with the
  same choices.
- A note that must not be missed, such as a settings file that could not be
  read, stays at the top of the window until it is dismissed.
- A CSV file saved from the menu starts with a byte-order mark, so Excel shows
  accented names correctly.
- A token in a link is saved only once pingtable accepts it, so an old or
  mistyped link no longer replaces a saved token that works, and it is saved
  even when pingtable was slow to answer at first.
- Failing and recovering hosts have their own square shapes (hollow and half
  filled), so they can be told from down without relying on colour.
- Light grey text is easier to read in both looks, including on a selected
  row; muted rows are dim grey. Keyboard focus in menus shows as a ring, and
  the "Lost contact" banner is easier to read.
- The main menu opens in a narrow window; before, it was cut off at 860
  pixels wide or less. Right-clicking a row that is partly scrolled out of
  view no longer closes its menu at once. Dragging a column heading no longer
  moves Host in front of the state square.
- "Add a new group…" shows the field for the group's name again; it never
  appeared. Where the browser blocks clipboard access and copying fails, the
  window now says so and how to copy by hand, instead of "Copied".
- The red button in a confirm window no longer turns light grey under the
  pointer, and unchecked boxes are easier to see in the dark look. The Check
  column is wide enough for "TCP 65535".
- Copies: emoji line up in text copies, Markdown escapes `&` and `$` and keeps
  notes apart, and JSON from the detail panel always gives `host` as the full
  row. The JSON envelope's address field is `from_address`.

### Pinging and results

- **Every host gets its turn.** When more hosts were due than could be pinged
  at once, the free places went to hosts in list order, and a host that timed
  out was due again the moment its place freed. With 256 or more hosts timing
  out, everything listed after them was never pinged again and kept its last
  state. Now the host that has waited longest goes first.
- The footer shows how many hosts are waiting for a free place, and a row
  whose result is old because it has waited too long is dimmed and marked.
  API: `waiting` in the table, `stale` in a row.
- **Loss counts time, not timeouts.** A host is never pinged twice at once, so
  while a failed ping waited for its timeout the beats after it were skipped
  and went uncounted. Measure at 10 pings a second on a host that was down
  half the time reported about 9% loss. A failed ping now counts for every
  beat it covered. Only regular pinging and measurements skip beats: a ping
  by hand while stopped, `pingtable-cli once` and the MCP `ping` count each
  ping once. API: `missed` in a row, `expected` in a measurement.
- The detail panel's "Loss, 1 hour" covered only as far back as the samples
  kept (33 minutes at the usual settings). The tile now says how far back it
  reaches, and median and 95th are marked as recent. API: `covered_ms`.
- A host name is looked up again every 5 minutes, and sooner while its host is
  failing, so a host that changes address is found again. If that lookup
  fails, the last address is used. A reverse lookup that found no name is
  tried again after an hour. Every lookup, including those for TCP rows,
  trace route and the MCP tools, gives up after 5 seconds.
- Regular pinging spreads the hosts' first beats out instead of sending every
  ping in the same instant of each second. "Ping every host once, now" no
  longer pulls them back into step. Hosts added while pinging runs, a new
  host list, hosts unmuted together, "Ping only these" and the end of a
  measurement are spread out the same way.
- On Linux, a ping that the system `ping` tool could not send at all (no
  permission inside a container, an option that version does not know) shows
  as "Not supported here" instead of "Timed out". "Network unreachable" and
  "Packet too big", which that tool reports as errors, now show as such
  instead of "Timed out".
- Jitter starts from the first difference between two pings instead of a
  sixteenth of it.
- A measurement of many hosts no longer holds up pinging while its report is
  put together.

### Host lists, import and export

- **Address blocks too big for a list are refused.** A block larger than a
  /16 was expanded in full before the 65,536-host limit applied: a /8 is 16.7
  million entries and could make pingtable stop with no explanation. It is
  now refused with a message before anything is expanded.
- A name that ends in a number, such as `192.168.001.010`, `192.168.1` or
  `10.0.0.010`, is refused with a message that says how to write the address.
  Before, the system's resolver read it in an old shorthand (a leading zero
  means octal), and another address was pinged: 192.168.1.8 for the first. A
  short range such as `10.0.0.1-20` now asks for both ends in full instead of
  becoming a name that never resolves.
- Hosts from a range or block are pinged every 30 seconds unless the list
  gives them or their group an `every=`, or the global interval is longer.
  Many addresses in a block never answer, and on a local network each of them
  costs a broadcast at every ping.
- In the example host list, the two checks of public servers run every 30
  seconds instead of every second.
- **Exported CSV goes back in as it came out.** Importing pingtable's own CSV
  turned TCP service monitors into pings, pinned every host to the interval it had, and
  lost timeouts and packet options. Import now reads the `probe`,
  `timeout_ms` and new `options` columns. The export leaves the interval and
  timeout blank only where a host has none of its own (an own `every=1s`
  stays, even when the global interval is one second), and carries packet
  options (`size=`, `ttl=`, `df=`, `dscp=`, `data=`) in `options`. Each row
  also says when it was exported and from which computer. API: `options`,
  `exported_at` and `from` are new last columns of `GET /export?format=csv`;
  `interval_secs` and `timeout_ms` are blank there when the host follows the
  global values.
- An imported file can no longer set a group's interval or tags through words
  such as `every=1s` in its group column.

### Settings and alert commands

- **A settings file that cannot be read is kept.** A stray comma, a
  byte-order mark or a file saved by PowerShell made pingtable start over with
  defaults. The file is now kept as `pingtable.json.unreadable-<date and
  time>`, pingtable starts with the defaults, and the window says so once.
  Files with a byte-order mark or in UTF-16 are now read as they are. On
  Linux the file is readable by your account only from the moment it is
  written. If the file cannot be moved aside, it stays in place and nothing
  is saved over it until pingtable is closed. A settings file that cannot be
  opened at all (a folder in its place, no permission, a file another program
  holds) stops pingtable from starting, with a message that says why. API: a
  note can have `sticky: true`.
- Settings and the host list window send back only what they show, so a host
  added while Settings is open (from another tab, or by an agent) is no longer
  removed when Settings is applied, and the other way round. API: in
  `PUT /config`, `settings` and `hosts_text` are each optional, and a partial
  `settings` changes only the fields it names.
- The host list window no longer drops hosts added while it was open (from
  the Add window, an import or an agent): if the list
  changed meanwhile, Apply says so and offers "Load it again"; what you typed
  goes to the clipboard and Ctrl+Z brings it back. API:
  `hosts_rev` in `GET /config` and in `PUT /config` replies; a `PUT` that
  sends an old `hosts_rev` is refused with 409 and changes nothing. It starts
  at a new random number each run, so a window left open across a restart
  cannot save an old list either.
- Muting or unmuting a host while settings or the list are being applied is
  no longer undone by them.
- API: `PUT /config` refuses a field or a setting it does not know and names
  the one meant (`timeout_ms`, not `timeout`), instead of ignoring it and
  saying ok. It accepts everything `GET /config` returns and ignores the
  read-only fields, so a script can read, change and send back. Sending back
  the host list already in use keeps the counters.
- Alert commands:
  - On Windows a command with quotes in it (a program path with a space, a
    quoted argument) now runs as typed, and the window app no longer opens a
    console for each one.
  - At most 4 run at once and 64 wait; one that runs for more than 60 seconds
    is stopped, together with the programs it started (its process group on
    Linux, a job object on Windows). On Linux, Ctrl+C, a closed terminal or
    `kill` also stops the commands the table is running. A command that could not be started is reported in the app
    instead of being dropped silently.
  - `%Description%` and `%Group%` can no longer add arguments to the command:
    their spaces always become `_`, quotes or no quotes, and they keep `-`,
    `/` and `@` only straight after a letter or a digit, so no word in them
    starts with one. Letters outside ASCII become `_`. For the exact text,
    run a script that reads `PINGTABLE_DESCRIPTION` and `PINGTABLE_GROUP`.
  - `"commands_locked": true` in `pingtable.json` keeps the commands as the
    file has them; the window and the API cannot change them, and Settings
    shows them greyed out. It counts as soon as it is saved in the file, even
    while pingtable runs; commands edited in the file meanwhile are what the
    table uses, and no mute, unmute or save writes them back changed. API:
    `commands_locked` in `GET /config`.
  - Settings explains what `%Description%` and `%Group%` become in a command.

### Security

- pingtable's own programs no longer send the access token to whatever
  listens on its port. `pingtable-cli api`, the MCP tools and a second start
  first ask the server to prove it holds the token, and look for it on the
  port it wrote down when it started (`pingtable.json.run`), so starting with
  `--port` no longer sends the tools to the wrong place. API:
  `GET /api/v1/hello?nonce=`, the one call without a token. Tools from this
  release do not attach to a running 0.3.0.
- When something else holds pingtable's port, a second start,
  `pingtable-cli api` and the MCP tools now say whether it is another
  program, an older pingtable, or something that did not answer in time. The
  MCP tools also say why a settings file cannot be used, instead of saying
  pingtable has not been started.
- On Linux the browser is given a page only your account can read, instead of
  a command line with the token in it (except under WSL, where the Windows
  browser is still given the link on its command line). Replies carry
  `Cross-Origin-Resource-Policy: same-origin`. `PINGTABLE_UI_DIR` works only
  in debug builds.

### For AI agents (MCP)

- `table_add_hosts` is offered only when the server is started as
  `pingtable mcp --allow-add`. It takes at most 16 plain hosts and 64 rows per
  call, refuses probe options, inline ports, ranges and blocks in host names, and no longer starts
  pinging.
- `ping` takes plain names and addresses and at most one range or block. A host given twice is pinged once.
- An argument a tool does not have is an error that names the expected argument. Every call ends within 30 seconds,
  and `ping` returns what came back by then.
- `trace_route` looks up router names only when asked (`resolve_names`, now
  off by default), and shows only names that look like host names.
  Descriptions and group names in `table_status` are cleaned of control and
  invisible characters and cut short. `table_status` takes `limit`.
- Under revision 2026-07-28, `server/discover` and `tools/list` carry `ttlMs`
  and `cacheScope`, as that revision requires.
- `table_status` asks the app only for the rows it lists, so a very large
  table no longer comes back as "The table is empty"; a reply that cannot be
  read is an error. API: `GET /state` takes `state=` and `limit=` (not
  together with `since=`), and then adds `matched`.

### Website

- **A live demo on pingtable.net.** The site now shows pingtable's own
  interface replaying a real 10-minute run recorded from a home connection:
  26 hosts on this PC, the home network and the internet, with a service that
  keeps restarting, a refused port, a printer that is off, a site that ignores
  ping but answers on 443, and hosts on the far side of the world. Click a row
  for its history; right-click it to copy it. Nothing is pinged from the
  visitor's browser. The made-up sample table is gone.
- The demo is sealed in: it runs in a sandboxed frame with no access to the
  page, its storage or the network, and cannot open windows, follow links or
  send anything anywhere. Its debug settings work only on the developer's own
  computer. `scripts/build-site-demo.sh` refuses to build a demo page that
  would break any of this.
- pingtable.net now matches the window: sans-serif words, monospace data,
  near-black text and sentence case. The live demo starts on the first
  screen, and the features mention copying as text, Markdown, JSON or TSV.
- `scripts/record-demo.py` records a new run for the demo. It checks the
  result against what the app sent and refuses to write it if it still holds
  this computer's name, the user name or a home path.
  `scripts/build-site-demo.sh` copies the current `ui/` into `site/demo/`.

### Building

- Every change is built, tested and checked with clippy on Windows and Linux,
  with a fixed Rust release (1.98.0) so a newer clippy cannot fail a build
  that changed nothing, and the dependencies are checked against the RustSec
  advisory database.

## 0.3.0 - 2026-11-17

- **For AI agents: an MCP server.** `pingtable-cli mcp` serves six tools over
  standard input and output: `ping`, `check_ports`, `trace_route`,
  `network_info`, `table_status` and `table_add_hosts`. It works with hosted
  and local models, needs no account and opens no port. Protocol revisions
  2024-11-05 through 2026-07-28 are served.
- `pingtable-cli api` prints the address and token of the local API, with
  example calls for curl and PowerShell, or as JSON for a program.
- New guides: `docs/ai-agents.md` (set-up, every tool with an example, which
  tool to use when), `docs/api.md` (every API call with examples) and
  `llms.txt`.
- For releases: `scripts/sign-windows.sh` signs the programs once a signing
  account exists, `scripts/winget-manifest.sh` writes the winget manifests,
  and `scripts/package-msix.sh` packs the Microsoft Store build. Each has a
  page in `docs/`. This release itself is still unsigned.

## 0.2.0 - 2026-11-17

- **Port check.** A new window (`PORTS` on the bar, the `O` key, or
  right-click a host) tries the TCP ports you choose on one host and says for
  each whether it is open, closed or gave no answer. Common ports are one
  click; others can be typed, with short ranges. WATCH IN THE TABLE turns the
  ports into rows.
- The Add window offers PING and the common ports as choices, so a host can be
  added as a ping, as one or more port checks, or both, without typing
  `host:port`.
- Rows that check a port show it beside the host (`example.com:443`).
- `IMPORT A FILE…` is now in the Add window, the menu, the right-click menu
  and the empty table, not only in the host list window.
- Command line: `pingtable-cli ports HOST [PORT...]`. The help and the README
  explain what `pingtable-cli.exe` is and how to use it.
- API: `POST /api/v1/portcheck`; `POST /api/v1/hosts/add` accepts `ports`,
  `ports_text` and `ping`.
- The pingtable.net page is open to search engines and describes port checks
  and the command line.

## 0.1.1 - 2026-11-17

No change to how pingtable pings or what it shows.

- A full disclaimer, in `DISCLAIMER.md` and in the Windows package as
  `DISCLAIMER.txt`. The About box, the command-line help, the README and the
  website point to it.
- `SECURITY.md`, `CONTRIBUTING.md` and this changelog.
- `scripts/package-windows.sh` builds the release zip and its notes.
- The pingtable.net page, in `site/`.

## 0.1.0 - 2026-11-17

First release.

- Ping many hosts over ICMP or TCP and watch them in one table, with a trend
  line, loss and averages for each.
- Runs as a Windows app in its own window, in a browser tab, or from the
  command line. No installer and no administrator rights.
- Host list with groups, ranges, address blocks and per-host options; quick
  add, remove and CSV or text import.
- Detail panel, mute, "ping only these", timed measurements with a report,
  trace route, alerts with sound and commands.
- Light and dark looks, resizable and reorderable columns, a narrow layout.
- A local HTTP API on the loopback address, protected by a token.
