#!/usr/bin/env python3
"""Render docs/benchmark.svg from benchmark numbers (seconds, median of 5 cold runs).

usage: bench/chart.py  (edit DATA below after running bench/compare.sh)
"""
import os

DATA = [  # (label, files, knip seconds, tidy seconds)
    ("micro\n24 workspaces, 8.1k files", 8142, 13.48, 0.30),
    ("TanStack/query\n102 workspaces, 1.4k files", 1442, 2.43, 0.20),
    ("chessbook\n488 files", 488, 2.12, 0.06),
]
OUT = os.path.join(os.path.dirname(__file__), "..", "docs", "benchmark.svg")

W, H = 760, 300
LEFT, TOP, ROW = 230, 40, 80
BAR_MAX = W - LEFT - 110
scale = BAR_MAX / max(k for _, _, k, _ in DATA)
font = "font-family='-apple-system,BlinkMacSystemFont,Segoe UI,Helvetica,Arial,sans-serif'"
svg = [f"<svg xmlns='http://www.w3.org/2000/svg' width='{W}' height='{H}' viewBox='0 0 {W} {H}'>",
       f"<rect width='{W}' height='{H}' fill='white'/>",
       f"<text x='{LEFT}' y='22' font-size='14' fill='#555' {font}>wall-clock seconds, cold, median of 5 runs (lower is better)</text>"]
for i, (label, _, knip, tidy) in enumerate(DATA):
    y = TOP + i * ROW
    lines = label.split("\n")
    svg.append(f"<text x='{LEFT - 14}' y='{y + 22}' text-anchor='end' font-size='15' font-weight='600' fill='#222' {font}>{lines[0]}</text>")
    svg.append(f"<text x='{LEFT - 14}' y='{y + 42}' text-anchor='end' font-size='12' fill='#777' {font}>{lines[1]}</text>")
    for j, (name, val, color) in enumerate([("knip", knip, "#c9ccd1"), ("tidy", tidy, "#2f81f7")]):
        by = y + 8 + j * 26
        w = max(3, val * scale)
        svg.append(f"<rect x='{LEFT}' y='{by}' width='{w:.1f}' height='20' rx='3' fill='{color}'/>")
        svg.append(f"<text x='{LEFT + w + 8:.1f}' y='{by + 15}' font-size='13' fill='#222' {font}>{name} {val:.2f}s"
                   + (f"  ({knip / tidy:.0f}x)" if name == "tidy" else "") + "</text>")
svg.append("</svg>")
open(OUT, "w").write("\n".join(svg) + "\n")
print("wrote", os.path.relpath(OUT))
