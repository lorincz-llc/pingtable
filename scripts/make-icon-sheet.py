#!/usr/bin/env python3
"""Write brand/icons/preview.html: icon candidates side by side, at real sizes.

Every candidate is a few rectangles on a 64 x 64 grid, so the same definition
can later be drawn into the .ico by scripts/make-icon.py. Each rectangle has a
role: p (primary), a (accent) or r (alert); a treatment maps roles to colours.

Run from the repository root:  python3 scripts/make-icon-sheet.py
"""
import json
from pathlib import Path

CANDIDATES = [
    ("A", "Bar", "The Petrol bar that already sits beside the wordmark. Nothing else.",
     [("a", 22, 6, 20, 52)]),
    ("B", "Square", "One status square, the shape every row starts with.",
     [("a", 12, 12, 40, 40)]),
    ("C", "Rows", "Three table rows; the last one is short.",
     [("p", 8, 12, 48, 10), ("p", 8, 27, 48, 10), ("a", 8, 42, 28, 10)]),
    ("D", "Three squares", "Up, up, down: the status column turned on its side.",
     [("a", 4, 24, 16, 16), ("a", 24, 24, 16, 16), ("r", 44, 24, 16, 16)]),
    ("E", "Echo", "A square and its echo: a ping going out.",
     [("p", 8, 8, 48, 6), ("p", 8, 50, 48, 6), ("p", 8, 14, 6, 36), ("p", 50, 14, 6, 36), ("a", 24, 24, 16, 16)]),
    ("F", "P", "A block letter P.",
     [("a", 14, 8, 12, 48), ("a", 14, 8, 36, 12), ("a", 38, 8, 12, 30), ("a", 14, 26, 36, 12)]),
    ("G", "Bar and rows", "The current icon, for comparison.",
     [("a", 10, 12, 10, 40), ("p", 26, 12, 28, 8), ("p", 26, 28, 28, 8), ("r", 26, 44, 16, 8)]),
]

# name, tile colour (None = no tile), colours for p / a / r, and whether single-colour marks use p
TREATMENTS = [
    ("Petrol tile", "#0b5f6a", {"p": "#ffffff", "a": "#a6e8ef", "r": "#ff9a80"}, True),
    ("Black tile", "#000000", {"p": "#ffffff", "a": "#4cc3cf", "r": "#f0563c"}, False),
    ("White tile", "#ffffff", {"p": "#000000", "a": "#0b5f6a", "r": "#c8371c"}, False),
    ("No tile", None, {"p": "#7b8b8e", "a": "#1596a6", "r": "#f0563c"}, False),
]


def svg(shapes, treatment, size):
    _, tile, colours, mono_uses_primary = treatment
    roles = {role for role, *_ in shapes}
    parts = [f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64" width="{size}" height="{size}" shape-rendering="crispEdges" role="img">']
    if tile:
        parts.append(f'<rect width="64" height="64" fill="{tile}"/>')
    for role, x, y, w, h in shapes:
        # On a coloured tile a one-colour mark reads best in the tile's strongest contrast.
        colour = colours["p"] if (mono_uses_primary and roles == {"a"}) else colours[role]
        parts.append(f'<rect x="{x}" y="{y}" width="{w}" height="{h}" fill="{colour}"/>')
    parts.append("</svg>")
    return "".join(parts)


def main():
    rows = []
    for letter, name, note, shapes in CANDIDATES:
        big = "".join(
            f'<figure class="tile{" bare" if t[1] is None else ""}{" edge" if t[1] == "#ffffff" else ""}">{svg(shapes, t, 96)}<figcaption>{letter}{i + 1} · {t[0]}</figcaption></figure>'
            for i, t in enumerate(TREATMENTS)
        )
        small = "".join(
            f'<div class="bar {mode}"><span class="lab">{mode} taskbar</span>'
            + "".join(f'<span class="pair">{svg(shapes, t, 32)}{svg(shapes, t, 16)}</span>' for t in TREATMENTS)
            + "</div>"
            for mode in ("light", "dark")
        )
        rows.append(f'<section><header><b>{letter}</b><h2>{name}</h2><p>{note}</p></header><div class="big">{big}</div><div class="small">{small}</div></section>')

    html = f"""<!doctype html>
<html lang="en">
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>pingtable icon options</title>
<style>
  body {{ margin: 0; padding: 28px 32px 48px; background: #fff; color: #000; font: 13px/1.45 "Cascadia Mono", ui-monospace, Consolas, monospace; }}
  h1 {{ margin: 0 0 4px; font: 900 34px/1 "Segoe UI Variable Display", "Segoe UI", Arial, sans-serif; letter-spacing: .01em; }}
  .lead {{ margin: 0 0 22px; color: #555; max-width: 78ch; }}
  section {{ display: grid; grid-template-columns: 210px auto 1fr; gap: 24px; align-items: center; padding: 18px 0; border-top: 2px solid #000; }}
  header b {{ display: inline-block; font: 900 30px/1 "Segoe UI Variable Display", "Segoe UI", Arial, sans-serif; margin-right: 8px; }}
  header h2 {{ display: inline; margin: 0; font: 900 18px/1 "Segoe UI Variable Display", "Segoe UI", Arial, sans-serif; text-transform: uppercase; letter-spacing: .04em; }}
  header p {{ margin: 6px 0 0; color: #555; }}
  .big {{ display: flex; gap: 14px; }}
  figure {{ margin: 0; text-align: center; }}
  figure svg {{ display: block; }}
  figure.edge svg {{ outline: 1px solid #d6d6d0; }}
  figure.bare svg {{ background: repeating-conic-gradient(#eee 0 25%, #fff 0 50%) 0 0 / 16px 16px; }}
  figcaption {{ margin-top: 5px; font-size: 11px; color: #555; }}
  .small {{ display: grid; gap: 6px; }}
  .bar {{ display: flex; align-items: center; gap: 18px; padding: 7px 12px; }}
  .bar.light {{ background: #f3f3f3; }}
  .bar.dark {{ background: #202020; color: #bbb; }}
  .lab {{ width: 100px; font-size: 11px; }}
  .pair {{ display: inline-flex; align-items: center; gap: 8px; }}
  @media (max-width: 900px) {{ section {{ grid-template-columns: 1fr; }} }}
</style>
<h1>PINGTABLE ICON OPTIONS</h1>
<p class="lead">Seven marks, each shown four ways. Large tiles are 96 px; the strips show each one at 32 px and 16 px, the sizes Windows uses in the taskbar and title bar. Tell me a code such as <b>B1</b> (Square on the Petrol tile) and I will build the real icon from it.</p>
{"".join(rows)}
</html>
"""
    out = Path(__file__).resolve().parent.parent / "brand" / "icons"
    out.mkdir(parents=True, exist_ok=True)
    (out / "preview.html").write_text(html, encoding="utf-8")
    (out / "candidates.json").write_text(
        json.dumps({"grid": 64, "candidates": [{"code": c, "name": n, "shapes": [{"role": r, "x": x, "y": y, "w": w, "h": h} for r, x, y, w, h in s]} for c, n, _, s in CANDIDATES],
                    "treatments": [{"index": i + 1, "name": t[0], "tile": t[1], "colours": t[2]} for i, t in enumerate(TREATMENTS)]}, indent=2) + "\n",
        encoding="utf-8",
    )
    print(f"wrote {out / 'preview.html'} and candidates.json")


if __name__ == "__main__":
    main()
