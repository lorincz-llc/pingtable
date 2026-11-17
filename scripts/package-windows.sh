#!/usr/bin/env bash
# Builds the Windows programs and makes the files for a release:
#
#   dist/pingtable-VERSION-windows-x64.zip   a folder named "pingtable" holding the
#                                            programs, a short readme, the license,
#                                            the notices and the disclaimer
#   dist/release-notes.md                    text for the release page, with the
#                                            zip's SHA-256
#
# VERSION is the workspace version in Cargo.toml. The notes take their list of
# changes from that version's section of CHANGELOG.md, so write that first.
#
#   scripts/package-windows.sh             build, then package
#   scripts/package-windows.sh --no-build  package what is already in dist/windows
set -euo pipefail
cd "$(dirname "$0")/.."

if [ "${1:-}" != "--no-build" ]; then
    scripts/build-windows.sh
fi

python3 -I - <<'PY'
import hashlib, pathlib, re, sys, zipfile

root = pathlib.Path('.')
version = re.search(r'^version = "([^"]+)"', (root / 'Cargo.toml').read_text(), re.M).group(1)
name = f'pingtable-{version}-windows-x64.zip'
source = 'https://github.com/lorincz-llc/pingtable'

# This version's changes, from the changelog.
changelog = (root / 'CHANGELOG.md').read_text()
section = re.search(rf'^## {re.escape(version)}\b[^\n]*\n(.*?)(?=^## |\Z)', changelog, re.M | re.S)
if not section:
    sys.exit(f'package-windows: CHANGELOG.md has no section for {version}')
changes = section.group(1).strip()

readme = f'''pingtable {version} for Windows 10 and 11 (64-bit)
{source}

Start it
  Double-click pingtable.exe. Keep WebView2Loader.dll in the same folder.
  No installer and no administrator rights are needed; the folder can sit
  anywhere you can write.

What is here
  pingtable.exe        the app, in its own window
  WebView2Loader.dll   Microsoft's loader for that window
  pingtable-cli.exe    the same program without the window, for a terminal
  LICENSE.txt          Apache License 2.0
  DISCLAIMER.txt       no warranty, no liability, your responsibilities
  NOTICE.txt, THIRD-PARTY-NOTICES.md

The command line
  pingtable-cli.exe does the same work without a window. It suits a quick
  answer, a script or a scheduled task. Open a terminal in this folder (in
  File Explorer, type cmd in the address bar and press Enter), then:

    pingtable-cli once 192.168.1.1 example.com:443
        ping each host one time, print the result and exit

    pingtable-cli ports example.com 22 80 443
        check which TCP ports are listening on one host:
        open, closed (refused) or no answer

    pingtable-cli
        run without the window and show the table in your browser

    pingtable-cli --help
        every command and option

  "once" and "ports" end with status 0 only when every host answered or
  every port was open, so a script can act on the result. Add --format json
  for output a program can read.

Good to know
  The programs are not code-signed yet, so Windows may show an "unknown
  publisher" warning the first time you run them.
  Settings are kept in %APPDATA%\\pingtable. To keep them beside the program
  instead, put a file named pingtable.json in this folder.

pingtable is provided "as is", without warranty of any kind, and you use it at
your own risk. Use it only on networks and hosts you own or are authorised to
test. Read DISCLAIMER.txt before you use it.

Copyright 2026 Lorincz LLC. pingtable@lorincz.llc
'''

def windows_text(text):
    """Text files in the zip use Windows line ends, so any editor shows them properly."""
    return text.replace('\r\n', '\n').replace('\n', '\r\n').encode()

programs = ['pingtable.exe', 'WebView2Loader.dll', 'pingtable-cli.exe']
texts = {
    'README.txt': readme,
    'LICENSE.txt': (root / 'LICENSE').read_text(),
    'DISCLAIMER.txt': (root / 'DISCLAIMER.md').read_text(),
    'NOTICE.txt': (root / 'NOTICE').read_text(),
    'THIRD-PARTY-NOTICES.md': (root / 'THIRD-PARTY-NOTICES.md').read_text(),
}

out = root / 'dist' / name
with zipfile.ZipFile(out, 'w', zipfile.ZIP_DEFLATED, compresslevel=9) as z:
    for program in programs:
        path = root / 'dist' / 'windows' / program
        if not path.is_file():
            sys.exit(f'package-windows: {path} is missing; run scripts/build-windows.sh')
        z.write(path, f'pingtable/{program}')
    for filename, text in texts.items():
        z.writestr(f'pingtable/{filename}', windows_text(text))

digest = hashlib.sha256(out.read_bytes()).hexdigest()
(root / 'dist' / 'release-notes.md').write_text(f'''{changes}

**Download:** `{name}` below, for Windows 10 and 11 (64-bit). Unpack it anywhere and double-click `pingtable.exe`. No installer and no administrator rights are needed.

The programs are not code-signed yet, so Windows may show an "unknown publisher" warning the first time.

SHA-256 of the zip: `{digest}`

What it does is described in the [README]({source}#readme).

pingtable is provided "as is", without warranty of any kind, and you use it at your own risk. Use it only on networks and hosts you own or are authorised to test. Full text: [DISCLAIMER.md]({source}/blob/main/DISCLAIMER.md).
''')

print(f'{out}  {out.stat().st_size} bytes')
print(f'sha256  {digest}')
with zipfile.ZipFile(out) as z:
    for item in z.infolist():
        print(f'  {item.file_size:>8}  {item.filename}')
print('dist/release-notes.md written')
PY
