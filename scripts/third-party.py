#!/usr/bin/env python3
"""Generate the locked dependency notices or stage release-toolchain notices."""

import argparse
import hashlib
import json
import re
import shutil
import subprocess
from collections import defaultdict
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TARGET = "x86_64-pc-windows-gnu"
SDK = "1.0.3800.47"
LOADER_SHA256 = "86545b66cdb0603bc26b626fb9ad610cb6e71f28d468f5ea66df23b03dda96d5"
# These crates omit their workspace-level license from their crates.io archive.
# The pinned revision is taken from each published package's .cargo_vcs_info.json.
WEBVIEW_CRATES = {
    ("webview2-com", "0.39.1"): "edc2caf886175ccaebe86078c9cfe1ae2a187328",
    ("webview2-com-sys", "0.39.1"): "edc2caf886175ccaebe86078c9cfe1ae2a187328",
    ("webview2-com-macros", "0.8.1"): "dffa41a8a46d3f5565eefbff2de57d38d399f158",
}
NOTICE_NAME = re.compile(r"^(?:licen[cs]e|notice|copying|copyright|unlicense)(?:[._-]|$)", re.I)


def command(*args):
    return subprocess.check_output(args, cwd=ROOT, text=True).strip()


def content(path):
    # Preserve wording and indentation. Remove trailing whitespace and normalize line endings.
    return "\n".join(line.rstrip() for line in path.read_text(encoding="utf-8-sig").splitlines()).strip()


def crate_documents(package):
    root = Path(package["manifest_path"]).parent
    files = sorted(p for p in root.rglob("*") if NOTICE_NAME.match(p.name) and p.is_file())
    if package.get("license_file"):
        files = sorted(set(files) | {root / package["license_file"]})
    if files:
        return [(str(p.relative_to(root)), content(p)) for p in files]
    revision = WEBVIEW_CRATES.get((package["name"], package["version"]))
    if revision and json.loads((root / ".cargo_vcs_info.json").read_text())["git"]["sha1"] == revision:
        return [(f"upstream LICENSE @ {revision}", content(ROOT / "licenses/webview2-rs-MIT.txt"))]
    raise SystemExit(
        f"{package['name']} {package['version']} has no packaged license file. Review its licenses before release."
    )


def render(browser):
    metadata = json.loads(
        command("cargo", "metadata", "--locked", "--format-version", "1", "--filter-platform", TARGET)
    )
    selected = {node["id"] for node in metadata["resolve"]["nodes"]}
    crates = sorted(
        (p for p in metadata["packages"] if p["id"] in selected and p["source"]),
        key=lambda p: (p["name"], p["version"]),
    )
    lines = [
        "# Third-party notices",
        "",
        "Lorincz LLC licenses its original pingtable code under Apache License 2.0.",
        "The components below retain their own copyright notices and license terms.",
        "",
        "## Scope",
        "",
        "The source tree includes Latin subsets of JetBrains Mono 2.304 (Regular, Medium and",
        "SemiBold), Copyright 2020 The JetBrains Mono Project Authors, under SIL OFL 1.1.",
        "The interface embeds these fonts. The source distribution also includes the font",
        "license beside the files in ui/fonts and site/demo/fonts.",
        "",
        "Cargo fetches the Rust dependencies listed below when building for 64-bit Windows.",
        "The inventory includes build tools and dependencies used only by the native window.",
        "Binary packages also carry the Rust standard library and MinGW toolchain notices",
        "from the release build in the third-party directory.",
        "",
    ]
    documents = defaultdict(list)
    documents[content(ROOT / "ui/fonts/OFL.txt")].append("JetBrains Mono 2.304 / OFL.txt")
    if not browser:
        lines += [
            "The native-window package ships Microsoft's unmodified WebView2Loader.dll.",
            f"The x64 DLL matches the [Microsoft.Web.WebView2 {SDK} package](https://www.nuget.org/packages/Microsoft.Web.WebView2/{SDK}).",
            f"Its SHA-256 is `{LOADER_SHA256}`. The package's LICENSE.txt and NOTICE.txt are",
            "reproduced below. The browser-only download omits the loader. The separately",
            "installed WebView2 Runtime follows Microsoft's runtime terms.",
            "",
        ]
        for name in ("LICENSE.txt", "NOTICE.txt"):
            documents[content(ROOT / f"licenses/webview2-sdk-{SDK}" / name)].append(
                f"Microsoft.Web.WebView2 {SDK} / {name}"
            )
    else:
        lines += ["This is the browser edition. Its executable opens the table in the system browser.", ""]
    lines += [
        f"## Windows Cargo dependencies ({len(crates)})",
        "",
        "| Package | Version | Declared license |",
        "|---|---|---|",
    ]
    for p in crates:
        name, version = p["name"], p["version"]
        url = f"https://crates.io/crates/{name}/{version}"
        lines.append(f"| [{name}]({url}) | {version} | {p.get('license') or p.get('license_file')} |")
        for filename, doc in crate_documents(p):
            documents[doc].append(f"{name} {version} / {filename}")
    lines += [
        "",
        "## License and attribution texts",
        "",
        "Identical texts are grouped with every package and file to which they apply.",
        "",
    ]
    for i, (doc, sources) in enumerate(documents.items(), 1):
        if not doc:
            raise SystemExit(f"Empty license text for {sources}")
        lines += [f"### Document {i}", "", *[f"- {s}" for s in sources], "", "~~~~text", doc, "~~~~", ""]
    return "\n".join(lines)


def stage_toolchain(out):
    # This project builds Windows packages with the Linux Rust/MinGW cross toolchain.
    sysroot = Path(command("rustc", "--print", "sysroot"))
    rustdoc = sysroot / "share/doc/rust"
    files = {
        "Rust-standard-library.html": rustdoc / "COPYRIGHT-library.html",
        "Rust-MIT.txt": rustdoc / "licenses/MIT.txt",
        "Rust-Apache-2.0.txt": rustdoc / "licenses/Apache-2.0.txt",
        "MinGW-w64-copyright.txt": Path("/usr/share/doc/mingw-w64-common/copyright"),
        "GCC-MinGW-copyright.txt": Path("/usr/share/doc/gcc-mingw-w64-base/copyright"),
        "GCC-exception-3.1.txt": rustdoc / "licenses/GCC-exception-3.1.txt",
        "GPL-3.txt": rustdoc / "licenses/GPL-3.0-or-later.txt",
    }
    for source in files.values():
        if not source.is_file():
            raise SystemExit(
                f"Missing release toolchain notice {source}. Use the documented Rust/MinGW build environment."
            )
    out.mkdir(parents=True, exist_ok=True)
    extras = {p.name for p in out.iterdir()} - files.keys()
    if extras:
        raise SystemExit(f"Unexpected files in the notice staging directory {out}: {sorted(extras)}")
    for filename, source in files.items():
        shutil.copyfile(source, out / filename)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group()
    group.add_argument("--check", action="store_true", help="verify the checked-in notices")
    group.add_argument(
        "--bundle", type=Path, metavar="DIR", help="verify notices and packaged loader, stage toolchain files"
    )
    group.add_argument(
        "--browser-bundle",
        type=Path,
        metavar="DIR",
        help="stage the browser edition's notices and third-party directory",
    )
    opts = parser.parse_args()
    full = ROOT / "THIRD-PARTY-NOTICES.md"
    if opts.browser_bundle:
        stage_toolchain(opts.browser_bundle / "third-party")
        (opts.browser_bundle / full.name).write_text(render(True), encoding="utf-8")
        return
    notices = render(False)
    if opts.bundle or opts.check:
        if not full.exists() or full.read_text(encoding="utf-8") != notices:
            raise SystemExit(
                "THIRD-PARTY-NOTICES.md is out of date. Run python3 scripts/third-party.py and review the changes."
            )
    else:
        full.write_text(notices, encoding="utf-8")
    if opts.bundle:
        loader = ROOT / "dist/windows/WebView2Loader.dll"
        if hashlib.sha256(loader.read_bytes()).hexdigest() != LOADER_SHA256:
            raise SystemExit(
                "Loader differs from the verified SDK version. Review Microsoft's license and notices for the new DLL."
            )
        stage_toolchain(opts.bundle)


if __name__ == "__main__":
    main()
