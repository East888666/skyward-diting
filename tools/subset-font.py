#!/usr/bin/env python3
"""Subset the bundled variable font for small recovery partitions.

Requires fonttools==4.60.1. All original supported characters appearing in
Rust sources and device configs are retained, along with GB2312 and common text ranges.
Arbitrary file names may need --extra-text; the full font remains the default.
"""
import argparse
import hashlib
import json
import re
from pathlib import Path

from fontTools import subset
from fontTools.ttLib import TTFont


def codepoints(root, extra_text):
    chars = set()
    for folder, pattern in (("crates", "*.rs"), ("devices", "*.toml")):
        for path in sorted((root / folder).rglob(pattern)):
            text = path.read_text(encoding="utf-8")
            chars.update(map(ord, text))
            # Rust strings may spell a character as a Unicode escape.
            chars.update(int(value, 16) for value in re.findall(r"\\u\{([0-9a-fA-F_]+)\}", text.replace("_", "")))
    # Preserve the full GB2312 repertoire for common Chinese file names.
    # Traditional characters used by the UI are retained by the source scan.
    for lead in range(0xA1, 0xF8):
        for trail in range(0xA1, 0xFF):
            chars.update(map(ord, bytes((lead, trail)).decode("gb2312", errors="ignore")))
    for path in extra_text:
        chars.update(map(ord, path.read_text(encoding="utf-8")))
    # Latin, common punctuation, currency, arrows, terminal drawing,
    # symbols, CJK punctuation and full-width Latin characters.
    for first, last in (
        (0x20, 0x24F), (0x2000, 0x206F), (0x20A0, 0x20CF),
        (0x2190, 0x21FF), (0x2500, 0x26FF), (0x3000, 0x303F),
        (0xFF00, 0xFFEF), (0xFFFD, 0xFFFD),
    ):
        chars.update(range(first, last + 1))
    return chars


def main():
    root = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--font", type=Path, default=root / "resources/fonts/MiSansVF.ttf")
    parser.add_argument("--out", type=Path, default=root / "target/fonts/MiSansVF-subset.ttf")
    parser.add_argument("--extra-text", type=Path, action="append", default=[],
                        help="UTF-8 text containing extra file-name or other characters to keep")
    args = parser.parse_args()
    if args.font.resolve() == args.out.resolve():
        parser.error("--out must differ from --font")
    needed = codepoints(root, args.extra_text)
    font = TTFont(args.font, recalcTimestamp=False)
    supported = set(font.getBestCmap())
    wanted = needed & supported
    options = subset.Options()
    # Keep the font's names, copyright and variation names, and its weight
    # axis. Only unused glyphs and their variation data are removed.
    options.name_IDs = ["*"]
    options.name_legacy = True
    options.name_languages = ["*"]
    options.layout_features = ["*"]
    sub = subset.Subsetter(options=options)
    sub.populate(unicodes=wanted)
    sub.subset(font)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    font.save(args.out)
    font.close()
    with TTFont(args.out, recalcTimestamp=False) as result:
        missing = wanted - set(result.getBestCmap())
        if missing:
            raise RuntimeError("Subset lost required characters: " + repr(sorted(missing)))
        if "fvar" not in result:
            raise RuntimeError("Subset lost the font's variable weight axis")
    print(json.dumps({
        "font": str(args.out), "bytes": args.out.stat().st_size,
        "source_bytes": args.font.stat().st_size,
        "required_supported_characters": len(wanted),
        "sha256": hashlib.sha256(args.out.read_bytes()).hexdigest(),
        "scope": "UI source and device config text, GB2312 and common ranges; use extra-text for other names",
    }, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
