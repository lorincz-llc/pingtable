#!/usr/bin/env bash
# Writes the three winget manifest files for one released version, ready to
# be checked and then sent to the community repository (microsoft/winget-pkgs).
#
#   scripts/winget-manifest.sh            for the version in Cargo.toml
#   scripts/winget-manifest.sh 0.2.0      for another released version
#
# It needs that version's zip in dist/ (scripts/package-windows.sh makes it,
# or download it from the release) and writes
#   dist/winget/manifests/l/LorinczLLC/pingtable/VERSION/
#
# The zip must be the very file attached to the GitHub release: winget checks
# its SHA-256. Check the result on Windows before sending it:
#   winget validate --manifest <that folder>
# How to send it: docs/winget.md.
set -euo pipefail
cd "$(dirname "$0")/.."

python3 -I - "${1:-}" <<'PY'
import datetime, hashlib, pathlib, re, sys

root = pathlib.Path('.')
version = sys.argv[1] or re.search(r'^version = "([^"]+)"', (root / 'Cargo.toml').read_text(), re.M).group(1)
if not re.fullmatch(r'\d+\.\d+\.\d+', version):
    sys.exit(f'winget-manifest: "{version}" is not a version like 1.2.3')

identifier = 'LorinczLLC.pingtable'
schema = '1.28.0'  # the schema the community repository asks for
source = 'https://github.com/lorincz-llc/pingtable'
zip_name = f'pingtable-{version}-windows-x64.zip'
zip_path = root / 'dist' / zip_name
if not zip_path.is_file():
    sys.exit(f'winget-manifest: dist/{zip_name} is missing. Build it, or download it from {source}/releases/tag/v{version}')
digest = hashlib.sha256(zip_path.read_bytes()).hexdigest().upper()

# The day the version was released, from the changelog; today if it is not there.
dated = re.search(rf'^## {re.escape(version)} - (\d{{4}}-\d{{2}}-\d{{2}})', (root / 'CHANGELOG.md').read_text(), re.M)
released = dated.group(1) if dated else datetime.date.today().isoformat()

def header(kind):
    return f'# yaml-language-server: $schema=https://aka.ms/winget-manifest.{kind}.{schema}.schema.json\n\n'

files = {
    f'{identifier}.yaml': header('version') + f'''PackageIdentifier: {identifier}
PackageVersion: {version}
DefaultLocale: en-US
ManifestType: version
ManifestVersion: {schema}
''',
    # Both programs come out of the one zip. The folder goes on the PATH instead
    # of links being made, because pingtable.exe needs WebView2Loader.dll beside it.
    f'{identifier}.installer.yaml': header('installer') + f'''PackageIdentifier: {identifier}
PackageVersion: {version}
InstallerType: zip
NestedInstallerType: portable
NestedInstallerFiles:
- RelativeFilePath: pingtable\\pingtable.exe
  PortableCommandAlias: pingtable
- RelativeFilePath: pingtable\\pingtable-cli.exe
  PortableCommandAlias: pingtable-cli
ArchiveBinariesDependOnPath: true
ReleaseDate: {released}
Installers:
- Architecture: x64
  InstallerUrl: {source}/releases/download/v{version}/{zip_name}
  InstallerSha256: {digest}
ManifestType: installer
ManifestVersion: {schema}
''',
    f'{identifier}.locale.en-US.yaml': header('defaultLocale') + f'''PackageIdentifier: {identifier}
PackageVersion: {version}
PackageLocale: en-US
Publisher: Lorincz LLC
PublisherUrl: https://github.com/lorincz-llc
PublisherSupportUrl: {source}/issues
PackageName: pingtable
PackageUrl: {source}
License: Apache-2.0
LicenseUrl: {source}/blob/main/LICENSE
Copyright: Copyright 2026 Lorincz LLC
ShortDescription: Monitor hosts and named TCP services in one table.
Description: |-
  pingtable pings every machine you care about, once a second, and shows who is answering, how fast, and who just stopped.
  It traces routes, measures loss and jitter, and can alert when a monitored host or TCP service goes down.
  It comes as a window app (pingtable) and a command-line program (pingtable-cli) with JSON output, a local HTTP API and an MCP server for AI agents.
  No installer and no administrator rights are needed.
Moniker: pingtable
Tags:
- ping
- network
- monitoring
- tcp-monitoring
- traceroute
- cli
- mcp
ReleaseNotesUrl: {source}/releases/tag/v{version}
ManifestType: defaultLocale
ManifestVersion: {schema}
''',
}

out = root / 'dist' / 'winget' / 'manifests' / 'l' / 'LorinczLLC' / 'pingtable' / version
out.mkdir(parents=True, exist_ok=True)
for old in out.glob('*.yaml'):
    old.unlink()
for name, text in files.items():
    (out / name).write_text(text)
    print(f'  {out / name}')
print(f'winget-manifest: {identifier} {version}, zip SHA-256 {digest}')
print(f'Check it on Windows:  winget validate --manifest <{out}>')
PY
