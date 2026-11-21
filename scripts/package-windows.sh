#!/usr/bin/env bash
# Package the native-window and browser editions with their licenses and SHA-256
# checksums in dist/. The release notes use the matching CHANGELOG.md section.
# Use --no-build to package an existing dist/windows build, including signed files.
set -euo pipefail
cd "$(dirname "$0")/.."

case "$#:${1:-}" in
    0:)
        scripts/build-windows.sh
        if scripts/sign-windows.sh --configured; then
            scripts/sign-windows.sh
        else
            echo "package-windows: producing unsigned programs (docs/signing.md)"
        fi ;;
    1:--no-build) ;;
    *) echo "Usage: scripts/package-windows.sh [--no-build]" >&2; exit 2 ;;
esac

stage=$(mktemp -d dist/package-windows.XXXXXXXX)
trap 'rm -rf "$stage"' EXIT
python3 -I scripts/third-party.py --bundle dist/windows/third-party
python3 -I scripts/third-party.py --browser-bundle "$stage/browser"

python3 -I - "$stage" <<'PY'
import hashlib
from pathlib import Path
import re
import sys
from zipfile import ZIP_DEFLATED, ZipFile, ZipInfo

dist = Path('dist')
build = dist / 'windows'
stage = Path(sys.argv[1])
version = re.search(r'^version = "([^"]+)"', Path('Cargo.toml').read_text(), re.M)[1]
source = 'https://github.com/lorincz-llc/pingtable'

section = re.search(rf'^## {re.escape(version)}\b[^\n]*\n(.*?)(?=^## |\Z)',
                    Path('CHANGELOG.md').read_text(), re.M | re.S)
if not section:
    sys.exit(f'package-windows: CHANGELOG.md has no section for {version}')

# Do not attach a fresh version number to binaries left over from an older release.
for exe in ('pingtable.exe', 'pingtable-cli.exe'):
    version_record = f'ProductVersion\0{version}\0'.encode('utf-16-le')
    if version_record not in (build / exe).read_bytes():
        sys.exit(f'package-windows: {exe} does not have ProductVersion {version}. Rebuild first.')

# sign-windows.sh records the signed programs' digests when it finishes.
signed_digests = {}
signed_note = build / 'SIGNED.txt'
if signed_note.is_file():
    for line in signed_note.read_text().splitlines()[1:]:
        digest, _, path = line.partition('  ')
        signed_digests[path.strip()] = digest.strip()
signed = all(signed_digests.get(str(build / exe)) == hashlib.sha256((build / exe).read_bytes()).hexdigest()
             for exe in ('pingtable.exe', 'pingtable-cli.exe'))
signature = ('The Pingtable programs are code-signed by Lorincz LLC.' if signed else
             'The Pingtable programs are unsigned. Windows may show an “unknown publisher” warning.')

native_name = f'pingtable-{version}-windows-x64.zip'
browser_name = f'pingtable-{version}-browser-windows-x64.zip'
native_intro = """Native window edition
  Double-click pingtable.exe for the desktop window. Keep WebView2Loader.dll
  beside it. The window uses Microsoft's WebView2 Runtime. If the runtime is
  unavailable, the app opens its table in your default browser.
  pingtable-cli.exe runs the full browser table and command-line tools.
  The bundled Microsoft WebView2 loader is distributed under its vendor license."""
browser_intro = """Browser edition
  Double-click pingtable-cli.exe to open the full table in your default browser.
  Keep the terminal open while the table is in use. Press Ctrl+C in the terminal
  to stop Pingtable. This ZIP contains one executable and needs no WebView2 loader."""

def readme(intro):
    return f"""Pingtable {version} for Windows 10 and 11 (64-bit)
{source}

{intro}

Getting started
  Pingtable is free and open source. Unpack it in a folder only your account
  can change. You can run it without an installer or administrator rights.

Commands
  Open a terminal in the unpacked folder.
    pingtable-cli once 192.168.1.1 example.com:443
      Ping the first address and check the named TCP service once.
    pingtable-cli once --format json example.com:443
      Print a JSON result. The exit status is 0 if all checks succeed.
    pingtable-cli mcp
      Serve the checks to an AI agent over standard input and output.
    pingtable-cli api
      Print connection details for the local API.
    pingtable-cli --help
      List all commands and options.

Settings and support
  Pingtable stores settings in %APPDATA%\\pingtable. For portable settings,
  put a file named pingtable.json containing {{}} beside the program.
  {signature}
  Questions or issues: support@pingtable.net

Documents
  LICENSE.txt                      Apache License 2.0 for Pingtable
  NOTICE.txt                       Project attribution
  DISCLAIMER.txt                   Warranty and liability information
  THIRD-PARTY-NOTICES.md            Third-party licenses and attribution
  third-party/                     Additional toolchain notices

Copyright 2026 Lorincz LLC. hello@pingtable.net
"""

def write_zip(name, intro, programs, notices, extra):
    files = {exe: (build / exe).read_bytes() for exe in programs}
    texts = {
        'README.txt': readme(intro),
        'LICENSE.txt': Path('LICENSE').read_text(),
        'NOTICE.txt': Path('NOTICE').read_text(),
        'DISCLAIMER.txt': Path('DISCLAIMER.md').read_text(),
        'THIRD-PARTY-NOTICES.md': notices.read_text(),
    }
    for filename, contents in texts.items():
        files[filename] = contents.replace('\r\n', '\n').replace('\n', '\r\n').encode('utf-8')
    extras = sorted(path for path in extra.rglob('*') if path.is_file())
    if not extras:
        sys.exit(f'package-windows: missing toolchain notices in {extra}')
    for path in extras:
        files['third-party/' + path.relative_to(extra).as_posix()] = path.read_bytes()

    output = stage / name
    with ZipFile(output, 'w') as archive:
        for filename, content in files.items():
            # Fixed member metadata makes repacking the same inputs reproducible.
            item = ZipInfo('pingtable/' + filename)
            item.create_system = 3
            item.external_attr = ((0o100755 if filename in programs else 0o100644) << 16)
            archive.writestr(item, content, compress_type=ZIP_DEFLATED, compresslevel=9)
    digest = hashlib.sha256(output.read_bytes()).hexdigest()
    (stage / (name + '.sha256')).write_text(f'{digest}  {name}\n', encoding='ascii')
    print(f'{name}  {output.stat().st_size} bytes  sha256 {digest}')
    return digest

native = write_zip(native_name, native_intro, ['pingtable.exe', 'pingtable-cli.exe', 'WebView2Loader.dll'],
                   Path('THIRD-PARTY-NOTICES.md'), build / 'third-party')
browser = write_zip(browser_name, browser_intro, ['pingtable-cli.exe'],
                    stage / 'browser' / 'THIRD-PARTY-NOTICES.md', stage / 'browser' / 'third-party')

notes = f"""## Free Windows downloads

Both editions run the full live table, command-line tools, MCP server and local API on 64-bit Windows 10 and 11. Unpack your ZIP in a folder only your account can change.

- **Native window** — `{native_name}`. Double-click `pingtable.exe`. This archive includes Microsoft's WebView2 loader and the browser CLI.
- **Browser edition** — `{browser_name}`. Double-click `pingtable-cli.exe` for the same table in your default browser. This archive includes only the CLI and accompanying notices.

{signature}

| ZIP | SHA-256 |
|---|---|
| `{native_name}` | `{native}` |
| `{browser_name}` | `{browser}` |

## Changes in {version}

{section[1].strip()}

See the [README]({source}#readme) for usage and the [disclaimer]({source}/blob/main/DISCLAIMER.md) for warranty and liability information.
"""
(stage / 'release-notes.md').write_text(notes, encoding='utf-8')
for name in (native_name, browser_name, native_name + '.sha256', browser_name + '.sha256', 'release-notes.md'):
    (stage / name).replace(dist / name)
PY
