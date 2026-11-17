#!/usr/bin/env python3
"""Write THIRD-PARTY-NOTICES.md: every third-party component in the Windows build.

Run from the repository root after changing dependencies:
    python3 scripts/third-party.py
"""
import json
import subprocess
from pathlib import Path

TARGET = "x86_64-pc-windows-gnu"


def main():
    meta = json.loads(subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--filter-platform", TARGET],
        check=True, capture_output=True, text=True).stdout)
    linked = {node["id"] for node in meta["resolve"]["nodes"]}
    crates = sorted((p for p in meta["packages"] if p["id"] in linked and p["source"]), key=lambda p: (p["name"], p["version"]))

    lines = [
        "# Third-party notices",
        "",
        "pingtable is built from the open-source components listed here. Each is used",
        "under the license shown; where a component offers a choice, pingtable uses it",
        "under the Apache License 2.0 or the MIT License. The full license texts are in",
        "each component's source, linked below.",
        "",
        "## Microsoft Edge WebView2 loader",
        "",
        "The Windows package includes `WebView2Loader.dll`, part of the Microsoft Edge",
        "WebView2 SDK, Copyright (C) Microsoft Corporation. It is redistributed unmodified",
        "under the license of that SDK: <https://www.nuget.org/packages/Microsoft.Web.WebView2>",
        "",
        f"## Rust crates ({len(crates)}, for the `{TARGET}` build)",
        "",
        "| Crate | Version | License | Source |",
        "|---|---|---|---|",
    ]
    for p in crates:
        source = p.get("repository") or f"https://crates.io/crates/{p['name']}"
        lines.append(f"| {p['name']} | {p['version']} | {p.get('license') or 'see source'} | <{source}> |")
    lines.append("")
    Path("THIRD-PARTY-NOTICES.md").write_text("\n".join(lines), encoding="utf-8")
    print(f"wrote THIRD-PARTY-NOTICES.md ({len(crates)} crates)")


if __name__ == "__main__":
    main()
