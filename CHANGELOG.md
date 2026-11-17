# Changelog

What changed in each release, newest first. Versions follow
[semantic versioning](https://semver.org); before 1.0, anything may still
change between releases.

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
