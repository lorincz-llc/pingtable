#!/usr/bin/env bash
# Packs the Windows programs as an MSIX package for the Microsoft Store.
#
#   scripts/package-msix.sh             build, then pack
#   scripts/package-msix.sh --no-build  pack what is already in dist/windows
#
# Result: dist/pingtable-VERSION-x64.msix. The Store signs the package itself,
# so it is uploaded unsigned and needs no certificate of yours.
#
# The package must carry the identity Partner Center gives the app. Put the
# three values in the environment or in store.env beside Cargo.toml (KEY=value
# lines; it is not committed):
#   PINGTABLE_STORE_NAME            Package/Identity/Name
#   PINGTABLE_STORE_PUBLISHER       Package/Identity/Publisher, starting CN=
#   PINGTABLE_STORE_PUBLISHER_NAME  Package/Properties/PublisherDisplayName
# Without them the script still packs, as a preview with a made-up identity,
# to prove the package builds. A preview cannot be submitted.
#
# Needs makeappx.exe from the Windows SDK. Run packing on native Windows in
# Git Bash with the files from the Linux cross-build. MAKEAPPX names the program if it is not in
# the usual place. How to submit the package: docs/store.md.
set -euo pipefail
cd "$(dirname "$0")/.."

die() { echo "package-msix: $*" >&2; exit 1; }

if [ "${1:-}" != "--no-build" ]; then
    scripts/build-windows.sh
fi

if [ -f store.env ]; then
    while IFS='=' read -r key value; do
        case "$key" in
            PINGTABLE_STORE_NAME | PINGTABLE_STORE_PUBLISHER | PINGTABLE_STORE_PUBLISHER_NAME | MAKEAPPX)
                value=${value%$'\r'}
                [ -n "${!key:-}" ] || export "$key=$value"
                ;;
        esac
    done < <(grep -v '^[[:space:]]*#' store.env || true)
fi

# makeappx.exe: the newest one under the Windows SDK, unless MAKEAPPX names it.
makeappx=${MAKEAPPX:-}
if [ -z "$makeappx" ]; then
    for kits in "/mnt/c/Program Files (x86)/Windows Kits/10/bin" "/c/Program Files (x86)/Windows Kits/10/bin"; do
        if [ -d "$kits" ]; then
            makeappx=$(find "$kits" -maxdepth 3 -path '*/x64/makeappx.exe' 2>/dev/null | sort -V | tail -1)
            [ -n "$makeappx" ] && break
        fi
    done
fi
[ -n "$makeappx" ] && [ -f "$makeappx" ] || die "makeappx.exe was not found. Install the Windows SDK, or set MAKEAPPX."

# A path the Windows program understands.
winpath() {
    if command -v wslpath >/dev/null 2>&1; then wslpath -w "$1"
    elif command -v cygpath >/dev/null 2>&1; then cygpath -w "$1"
    else echo "$1"; fi
}

stage=dist/msix/stage
rm -rf dist/msix
mkdir -p "$stage/Assets"
for program in pingtable.exe WebView2Loader.dll pingtable-cli.exe; do
    test -f "dist/windows/$program" || die "dist/windows/$program is missing; run scripts/build-windows.sh"
    cp "dist/windows/$program" "$stage/"
done
cp LICENSE "$stage/LICENSE.txt"
cp DISCLAIMER.md "$stage/DISCLAIMER.txt"
cp NOTICE "$stage/NOTICE.txt"
export LC_ALL=C
sha256sum --binary dist/windows/THIRD-PARTY-NOTICES.md dist/windows/WebView2Loader.dll dist/windows/third-party/* \
    | cmp -s - dist/windows/NOTICES.sha256 || die "The cross-build notices or loader differ. Copy dist/windows from the matching build."
cp dist/windows/THIRD-PARTY-NOTICES.md "$stage/"
cp -R dist/windows/third-party "$stage/"

kind=$(python3 -I - "$stage" <<'PY'
import importlib.util, os, pathlib, re, sys

stage = pathlib.Path(sys.argv[1])
root = pathlib.Path('.')
version = re.search(r'^version = "([^"]+)"', (root / 'Cargo.toml').read_text(), re.M).group(1)

for exe in ('pingtable.exe', 'pingtable-cli.exe'):
    if f'ProductVersion\0{version}\0'.encode('utf-16-le') not in (stage / exe).read_bytes():
        sys.exit(f'package-msix: {exe} does not have ProductVersion {version}. Rebuild first.')

name = os.environ.get('PINGTABLE_STORE_NAME', '')
publisher = os.environ.get('PINGTABLE_STORE_PUBLISHER', '')
shown = os.environ.get('PINGTABLE_STORE_PUBLISHER_NAME', '')
real = bool(name and publisher and shown)
if not real:
    name, publisher, shown = 'LorinczLLC.pingtable.preview', 'CN=pingtable-preview', 'Lorincz LLC'
if not publisher.startswith('CN='):
    sys.exit('package-msix: PINGTABLE_STORE_PUBLISHER must start with CN=')

def xml(text):
    return text.replace('&', '&amp;').replace('"', '&quot;').replace('<', '&lt;').replace('>', '&gt;')

manifest = (root / 'packaging' / 'msix' / 'AppxManifest.xml').read_text()
for mark, value in (('@IDENTITY_NAME@', name), ('@IDENTITY_PUBLISHER@', publisher), ('@PUBLISHER_NAME@', shown), ('@VERSION@', version)):
    manifest = manifest.replace(mark, xml(value))
(stage / 'AppxManifest.xml').write_text(manifest)

# The tiles: the app's square, drawn at each size by the icon script.
spec = importlib.util.spec_from_file_location('make_icon', root / 'scripts' / 'make-icon.py')
icon = importlib.util.module_from_spec(spec)
sys.dont_write_bytecode = True
spec.loader.exec_module(icon)
for file, size in (('StoreLogo.png', 50), ('Square44x44Logo.png', 44), ('Square150x150Logo.png', 150)):
    (stage / 'Assets' / file).write_bytes(icon.png(icon.draw(size)))

print(f'{version} {"store" if real else "preview"}')
PY
)
version=${kind% *}
out="dist/pingtable-$version-x64.msix"
[ "${kind#* }" = store ] || out="dist/pingtable-$version-x64-preview.msix"
rm -f "$out"

"$makeappx" pack /o /d "$(winpath "$PWD/$stage")" /p "$(winpath "$PWD/$out")" | tr -d '\r' | tail -3
test -f "$out" || die "makeappx did not produce $out"
ls -l "$out"
if [ "${kind#* }" = store ]; then
    echo "package-msix: ready to upload in Partner Center (docs/store.md)"
else
    echo "package-msix: this is a preview with a made-up identity. Set the three PINGTABLE_STORE_ values to make the real one (docs/store.md)."
fi
