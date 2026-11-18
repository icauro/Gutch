"""Build the README banner from the Gutch logo source.

The banner shows the logo and name, then one store serving many users: a
full install, and delta updates and repairs that send only the chunks a
machine is missing. Edit
this script or `_docs/assets/gutch_logo.svg`, then run it to rewrite
`_docs/assets/banner.svg`, plus `_docs/assets/social_preview.svg`: a simpler
card for GitHub's 1280×640 social preview with just the logo, name, slogan
and core technologies. To update the repository's social preview, export
that SVG to PNG at 1280×640 and upload it under Settings → General → Social
preview. `--check` fails when either SVG is stale.

The text uses Titillium Web. Only the letters the banner shows are embedded,
so the banner looks the same on every system and stays small.
"""

import argparse
import base64
import html
import io
from pathlib import Path
import re
import sys

from fontTools import subset
from fontTools.ttLib import TTFont

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from _tools.render_logo import SIZE, SOURCE, X, Y, placed_drawing  # noqa: E402


ROOT = Path(__file__).resolve().parents[1]
BANNER = ROOT / "_docs" / "assets" / "banner.svg"
SOCIAL_PREVIEW = ROOT / "_docs" / "assets" / "social_preview.svg"
FONTS = ROOT / "_docs" / "assets" / "fonts"
WIDTH, HEIGHT = 1280, 400
# GitHub's recommended social preview size.
SOCIAL_HEIGHT = 640
SLOGAN = "Store, update and repair by delta"
SUMMARY = "Fast deduplicating store and installer for folder trees"
FAMILY = "Titillium Web"
FONT_FILES = {400: "TitilliumWeb-Regular.ttf", 600: "TitilliumWeb-SemiBold.ttf", 700: "TitilliumWeb-Bold.ttf"}
FONT = f'font-family="&apos;{FAMILY}&apos;, &apos;Segoe UI&apos;, &apos;Helvetica Neue&apos;, Arial, sans-serif"'
# Logo colors, so the banner and the logo read as one brand.
TEAL, MINT, DEEP, INK = "#19a99d", "#89f4d6", "#057c7c", "#151826"
NEW = "#fbbf24"
# Core technologies, shown as tags along the bottom of the banner.
STORE_LOCATIONS = ("Local", "HTTP", "S3")
TECHNOLOGIES = ("Rust", "FastCDC", "BLAKE3", "Zstandard", "LZ4", "AES-256-GCM")
# Each user: label, detail, chunks on the wire and whether they travel as a delta.
USERS = (
    ("New user", "gets all of v2.0", 6, False),
    ("Update", "v1.1 to v2.0: changes only", 2, True),
    ("Repair", "fixes only broken files", 1, True),
    ("Rollback", "back to v1.0: changes only", 3, True),
)


def text(x, y, size, fill, content, weight=400, extra=""):
    return f'<text x="{x}" y="{y}" {FONT} font-size="{size}" font-weight="{weight}" fill="{fill}"{extra}>{content}</text>'


def text_width(content: str, size: float, weight: int) -> float:
    """Measure text set in the embedded font from its advance widths."""
    font = TTFont(FONTS / FONT_FILES[weight], lazy=True)
    cmap, metrics = font.getBestCmap(), font["hmtx"].metrics
    return sum(metrics[cmap[ord(char)]][0] for char in content) * size / font["head"].unitsPerEm


def tags(x, y, labels, size):
    """A row of rounded tags, each sized to its label; returns the markup and the row's right edge."""
    parts, height = [], size + 12
    for label in labels:
        width = text_width(label, size, 600) + size + 6
        parts.append(f'<rect x="{x:.1f}" y="{y}" width="{width:.1f}" height="{height}" rx="{height / 2}" fill="#12263a" '
                     f'stroke="{DEEP}" stroke-width="1"/>'
                     + text(f"{x + (size + 6) / 2:.1f}", y + height / 2 + size * 0.36, size, MINT, label, 600))
        x += width + 6
    return "".join(parts), x - 6


def monitor(x, y):
    """A small screen standing for one user's machine, 54 wide and 46 tall."""
    return (f'<rect x="{x}" y="{y}" width="54" height="36" rx="5" fill="#13213f" stroke="{TEAL}" stroke-width="2"/>'
            f'<rect x="{x + 22}" y="{y + 36}" width="10" height="6" fill="{TEAL}"/>'
            f'<rect x="{x + 14}" y="{y + 42}" width="26" height="4" rx="2" fill="{TEAL}"/>')


def font_faces(body: str) -> str:
    """Return @font-face rules embedding, per weight, only the letters the body draws."""
    letters = {weight: set() for weight in FONT_FILES}
    for weight, content in re.findall(r'<text [^>]*font-weight="(\d+)"[^>]*>([^<]*)</text>', body):
        letters[int(weight)].update(html.unescape(content))
    rules = []
    for weight, name in FONT_FILES.items():
        if not letters[weight]:
            continue
        font = TTFont(FONTS / name, recalcTimestamp=False)
        missing = sorted(char for char in letters[weight] if ord(char) not in font.getBestCmap())
        if missing:
            raise ValueError(f"{name} has no glyph for {''.join(missing)!r}")
        options = subset.Options()
        options.layout_features = ["kern", "liga"]
        options.name_IDs = [0, 1, 2]
        options.drop_tables += ["FFTM"]
        subsetter = subset.Subsetter(options)
        subsetter.populate(text="".join(sorted(letters[weight])))
        subsetter.subset(font)
        font.flavor = "woff"
        data = io.BytesIO()
        font.save(data, reorderTables=True)
        encoded = base64.b64encode(data.getvalue()).decode("ascii")
        rules.append(f"@font-face {{ font-family: '{FAMILY}'; font-weight: {weight}; "
                     f"src: url(data:font/woff;base64,{encoded}) format('woff'); }}")
    return "  <style>\n" + "\n".join(f"    {rule}" for rule in rules) + "\n  </style>\n"


def social_preview(source: str) -> str:
    """The logo beside the name, slogan, summary and technologies, centered on the social preview canvas."""
    logo, gap = 330, 24
    title_size, slogan_size, summary_size, tag_size = 150, 38, 25, 18
    _, tags_end = tags(0, 0, TECHNOLOGIES, tag_size)
    column = max(text_width("Gutch", title_size, 700), text_width(SLOGAN, slogan_size, 600),
                 text_width(SUMMARY, summary_size, 400), tags_end)
    left = (WIDTH - logo - gap - column) / 2
    right = left + logo + gap
    top = (SOCIAL_HEIGHT - logo) / 2
    out = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{WIDTH}" height="{SOCIAL_HEIGHT}" '
        f'viewBox="0 0 {WIDTH} {SOCIAL_HEIGHT}">',
        "<title>Gutch: store, update and repair by delta</title>",
        "<!-- Generated by _tools/render_banner.py; edit that script or gutch_logo.svg instead. -->",
        f"""<defs>
  <linearGradient id="bg" x1="0" y1="0" x2="1" y2="1">
    <stop offset="0" stop-color="#0b0e18"/><stop offset="1" stop-color="{INK}"/>
  </linearGradient>
  <radialGradient id="halo" gradientUnits="userSpaceOnUse" cx="{left + logo / 2:.1f}" cy="{SOCIAL_HEIGHT / 2}" r="360">
    <stop offset="0" stop-color="{TEAL}" stop-opacity="0.14"/><stop offset="1" stop-color="{TEAL}" stop-opacity="0"/>
  </radialGradient>
  <linearGradient id="title" x1="0" y1="0" x2="1" y2="0">
    <stop offset="0" stop-color="#ecfeff"/><stop offset="1" stop-color="{MINT}"/>
  </linearGradient>
</defs>""",
        f'<rect width="{WIDTH}" height="{SOCIAL_HEIGHT}" fill="url(#bg)"/>',
        f'<rect width="{WIDTH}" height="{SOCIAL_HEIGHT}" fill="url(#halo)"/>',
        f'<svg x="{left:.1f}" y="{top:.1f}" width="{logo}" height="{logo}" viewBox="{X} {Y} {SIZE} {SIZE}">\n'
        f'{placed_drawing(source)}</svg>',
        text(f"{right - 4:.1f}", 290, title_size, "url(#title)", "Gutch", 700, ' letter-spacing="-2"'),
        text(f"{right:.1f}", 350, slogan_size, "#f1f5f9", SLOGAN, 600),
        text(f"{right:.1f}", 392, summary_size, "#a3b0c2", SUMMARY),
        tags(right, 424, TECHNOLOGIES, tag_size)[0],
        "</svg>",
    ]
    body = "\n".join(out) + "\n"
    return body.replace("<defs>\n", "<defs>\n" + font_faces(body), 1)


def banner(source: str) -> str:
    out = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{WIDTH}" height="{HEIGHT}" viewBox="0 0 {WIDTH} {HEIGHT}">',
        "<title>Gutch: one store, every version, every user</title>",
        "<!-- Generated by _tools/render_banner.py; edit that script or gutch_logo.svg instead. -->",
        f"""<defs>
  <linearGradient id="bg" x1="0" y1="0" x2="1" y2="1">
    <stop offset="0" stop-color="#0b0e18"/><stop offset="1" stop-color="{INK}"/>
  </linearGradient>
  <radialGradient id="halo" cx="0.116" cy="0.43" r="0.26">
    <stop offset="0" stop-color="{TEAL}" stop-opacity="0.09"/><stop offset="1" stop-color="{TEAL}" stop-opacity="0"/>
  </radialGradient>
  <radialGradient id="halo2" cx="0.70" cy="0.43" r="0.4">
    <stop offset="0" stop-color="{DEEP}" stop-opacity="0.18"/><stop offset="1" stop-color="{DEEP}" stop-opacity="0"/>
  </radialGradient>
  <linearGradient id="title" x1="0" y1="0" x2="1" y2="0">
    <stop offset="0" stop-color="#ecfeff"/><stop offset="1" stop-color="{MINT}"/>
  </linearGradient>
  <filter id="glow" x="-80%" y="-80%" width="260%" height="260%">
    <feGaussianBlur stdDeviation="4" result="b"/><feMerge><feMergeNode in="b"/><feMergeNode in="SourceGraphic"/></feMerge>
  </filter>
</defs>""",
        f'<rect width="{WIDTH}" height="{HEIGHT}" rx="24" fill="url(#bg)"/>',
        f'<rect width="{WIDTH}" height="{HEIGHT}" rx="24" fill="url(#halo)"/>',
        f'<rect width="{WIDTH}" height="{HEIGHT}" rx="24" fill="url(#halo2)"/>',
        # Logo, scaled from its source canvas into a 272-unit square.
        f'<svg x="12" y="36" width="272" height="272" viewBox="{X} {Y} {SIZE} {SIZE}">\n{placed_drawing(source)}</svg>',
        text(302, 136, 100, "url(#title)", "Gutch", 700, ' letter-spacing="-1.5"'),
        text(308, 178, 27, "#f1f5f9", "Store, update and repair by delta", 600),
        *(f'<circle cx="315" cy="{y - 5}" r="3.5" fill="{MINT}"/>' + text(328, y, 18, "#cbd5e1", line)
          for y, line in ((218, "One store serves every user and version"), (245, "Downloads and installs at the same time"),
                          (272, "Checks every file by hash"), (299, "Rust library and command-line tool"))),
    ]

    # Bottom strip: the core technologies, centered under a thin divider.
    caption = "BUILT WITH"
    caption_width = text_width(caption, 14, 600) + 2.5 * len(caption)
    _, row_end = tags(0, 0, TECHNOLOGIES, 15)
    left = (WIDTH - caption_width - 16 - row_end) / 2
    out.append(f'<line x1="40" y1="340" x2="{WIDTH - 40}" y2="340" stroke="#1f2a3a" stroke-width="1"/>')
    out.append(text(f"{left:.1f}", 376, 14, "#7d8ba1", caption, 600, ' letter-spacing="2.5"'))
    out.append(tags(left + caption_width + 16, 357, TECHNOLOGIES, 15)[0])

    # Store: one repository holding every version once.
    sx, sy, sw, sh = 700, 74, 150, 180
    ux, top, step = 1000, 52, 70
    mid = sy + sh / 2
    label = "ONE STORE"
    out.append(text(sx, sy - 18, 14, "#7d8ba1", label, 600, ' letter-spacing="2.5"'))
    # Where the store can live: a local folder, plain HTTP(S) or an S3-compatible bucket.
    out.append(tags(sx + text_width(label, 14, 600) + 2.5 * len(label) + 10, sy - 35, STORE_LOCATIONS, 14)[0])
    out.append(f'<rect x="{sx}" y="{sy}" width="{sw}" height="{sh}" rx="16" fill="#0f1a2b" stroke="{DEEP}" stroke-width="2"/>')
    bend = (ux - sx - sw) * 0.45
    for row, label in enumerate(("v1.0", "v1.1", "v2.0")):
        ry = sy + 18 + row * 52
        out.append(f'<rect x="{sx + 14}" y="{ry}" width="{sw - 28}" height="40" rx="8" fill="#12263a"/>')
        out.append(text(sx + 22, ry + 26, 15, "#94a3b8", label, 600))
        for chunk in range(3):
            new_here = row > 0 and chunk == 2
            fill, glow = (MINT, ' filter="url(#glow)"') if new_here else (TEAL, "")
            out.append(f'<rect x="{sx + 62 + chunk * 26}" y="{ry + 10}" width="20" height="20" rx="4" fill="{fill}" '
                       f'opacity="{1 if row == 0 or new_here else 0.3}"{glow}/>')
    for line, (color, caption) in enumerate(((MINT, "stores only what's new"), (NEW, "sends only what's missing"))):
        y = sy + sh + 30 + line * 26
        out.append(f'<rect x="{sx}" y="{y - 11}" width="11" height="11" rx="2" fill="{color}" filter="url(#glow)"/>'
                   + text(sx + 19, y, 16, color, caption))

    # Users: one line per machine, carrying only the chunks that machine needs.
    for index, (label, detail, chunks, new) in enumerate(USERS):
        uy = top + index * step
        cy = uy + 18
        start, end = (sx + sw + 4, mid), (ux - 8, cy)
        out.append(f'<path d="M{start[0]} {start[1]} C {start[0] + bend:.1f} {start[1]}, {end[0] - bend:.1f} {end[1]}, {end[0]} {end[1]}" '
                   f'fill="none" stroke="{TEAL}" stroke-opacity="0.45" stroke-width="2"/>')
        for chunk in range(chunks):
            u = 0.3 + 0.55 * chunk / (chunks - 1) if chunks > 1 else 0.6
            px = (1 - u) ** 3 * start[0] + 3 * (1 - u) ** 2 * u * (start[0] + bend) + 3 * (1 - u) * u * u * (end[0] - bend) + u ** 3 * end[0]
            py = (1 - u) ** 3 * start[1] + 3 * (1 - u) ** 2 * u * start[1] + 3 * (1 - u) * u * u * end[1] + u ** 3 * end[1]
            color, glow = (NEW, ' filter="url(#glow)"') if new else (TEAL, "")
            out.append(f'<rect x="{px - 5:.1f}" y="{py - 5:.1f}" width="10" height="10" rx="2" fill="{color}"{glow}/>')
        out.append(monitor(ux, uy))
        out.append(text(ux + 68, uy + 16, 19, "#f1f5f9", label, 600))
        out.append(text(ux + 68, uy + 39, 16, NEW if new else "#a3b0c2", detail))
    out.append("</svg>")
    body = "\n".join(out) + "\n"
    return body.replace("<defs>\n", "<defs>\n" + font_faces(body), 1)


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--check", action="store_true", help="fail if the banner differs from its sources")
    args = parser.parse_args(argv)
    source = SOURCE.read_text(encoding="utf-8")
    outputs = {BANNER: banner(source), SOCIAL_PREVIEW: social_preview(source)}
    if args.check:
        stale = [path.name for path, expected in outputs.items()
                 if not path.exists() or path.read_text(encoding="utf-8") != expected]
        if stale:
            print(f"Stale {', '.join(stale)}. Run python _tools/render_banner.py.", file=sys.stderr)
            return 1
        return 0
    for path, expected in outputs.items():
        path.write_text(expected, encoding="utf-8", newline="\n")
        print(f"Wrote {path.relative_to(ROOT).as_posix()}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
