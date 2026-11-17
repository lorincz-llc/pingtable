# Changelog

What changed in each release, newest first. Versions follow
[semantic versioning](https://semver.org); before 1.0, anything may still
change between releases.

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
