#!/usr/bin/env bash
# Cross-build pingtable for 64-bit Windows and collect the result in dist/windows.
#
#   pingtable.exe       the window app (double-click it)
#   WebView2Loader.dll  Microsoft's loader for the window; must sit beside pingtable.exe
#   pingtable-cli.exe   the console program: `serve` and `once`
#
# Needs: sudo apt install -y mingw-w64   and   rustup target add x86_64-pc-windows-gnu
set -euo pipefail
cd "$(dirname "$0")/.."

target=x86_64-pc-windows-gnu
cargo build --release -p pingtable --target "$target"

out=dist/windows
mkdir -p "$out"
cp "target/$target/release/pingtable-window.exe" "$out/pingtable.exe"
cp "target/$target/release/pingtable.exe" "$out/pingtable-cli.exe"

# The MinGW build cannot link the WebView2 loader in, so ship the DLL that the
# webview2-com-sys crate carries (Microsoft's redistributable loader).
loader_dir=$(cargo metadata --format-version 1 --filter-platform "$target" |
  python3 -I -c 'import json, os, sys
for package in json.load(sys.stdin)["packages"]:
    if package["name"] == "webview2-com-sys":
        print(os.path.join(os.path.dirname(package["manifest_path"]), "x64"))')
cp "$loader_dir/WebView2Loader.dll" "$out/WebView2Loader.dll"
ls -l "$out"
