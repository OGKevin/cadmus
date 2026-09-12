#!/usr/bin/env python3
"""Verify that the documentation EPUB embeds Mermaid diagrams as PNG images.

The mdbook-mermaid-png preprocessor should replace every ```mermaid block with an
<img> reference to a PNG under mermaid-images/. This script fails CI when that
pipeline regresses (for example after a puppeteer or mermaid-cli upgrade).
"""

from __future__ import annotations

import argparse
import re
import sys
import zipfile
from pathlib import Path

MERMAID_FENCE = re.compile(r"^```mermaid\s*$", re.MULTILINE)
MERMAID_IMG = re.compile(
    r'<img[^>]+src="[^"]*mermaid-images/[^"]+\.png"[^>]*/?>',
    re.IGNORECASE,
)
MIN_PNG_BYTES = 500


def count_mermaid_blocks(src_dir: Path) -> int:
    total = 0
    for md_path in sorted(src_dir.rglob("*.md")):
        content = md_path.read_text(encoding="utf-8")
        total += len(MERMAID_FENCE.findall(content))
    return total


def chapter_html_path(md_path: Path, src_dir: Path) -> str:
    rel = md_path.relative_to(src_dir).with_suffix(".html")
    return f"OEBPS/{rel.as_posix()}"


def chapters_with_mermaid(src_dir: Path) -> list[tuple[Path, int]]:
    chapters: list[tuple[Path, int]] = []
    for md_path in sorted(src_dir.rglob("*.md")):
        count = len(MERMAID_FENCE.findall(md_path.read_text(encoding="utf-8")))
        if count:
            chapters.append((md_path, count))
    return chapters


def verify_epub(epub_path: Path, src_dir: Path) -> list[str]:
    errors: list[str] = []
    expected_blocks = count_mermaid_blocks(src_dir)

    if not epub_path.is_file():
        return [f"EPUB not found: {epub_path}"]

    with zipfile.ZipFile(epub_path) as archive:
        names = archive.namelist()
        png_paths = sorted(
            name for name in names if "/mermaid-images/" in name and name.endswith(".png")
        )

        if len(png_paths) < expected_blocks:
            errors.append(
                f"expected at least {expected_blocks} mermaid PNG(s) in EPUB, "
                f"found {len(png_paths)}"
            )

        for png_path in png_paths:
            data = archive.read(png_path)
            if len(data) < MIN_PNG_BYTES:
                errors.append(
                    f"{png_path} is only {len(data)} bytes "
                    f"(minimum {MIN_PNG_BYTES}); diagram may have failed to render"
                )

        html_paths = {name for name in names if name.endswith(".html")}
        for md_path, block_count in chapters_with_mermaid(src_dir):
            html_path = chapter_html_path(md_path, src_dir)
            if html_path not in html_paths:
                errors.append(f"missing EPUB chapter for {md_path}: {html_path}")
                continue

            html = archive.read(html_path).decode("utf-8")
            if "```mermaid" in html:
                errors.append(f"{html_path} still contains raw ```mermaid markup")

            img_count = len(MERMAID_IMG.findall(html))
            if img_count < block_count:
                errors.append(
                    f"{html_path} expected {block_count} mermaid image(s), found {img_count}"
                )

    return errors


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--epub",
        type=Path,
        default=Path("docs/book/epub/Cadmus Documentation.epub"),
        help="Path to the generated documentation EPUB",
    )
    parser.add_argument(
        "--src",
        type=Path,
        default=Path("docs/src"),
        help="Path to mdBook source markdown",
    )
    args = parser.parse_args()

    errors = verify_epub(args.epub, args.src)
    if errors:
        print("EPUB mermaid verification failed:", file=sys.stderr)
        for error in errors:
            print(f"  - {error}", file=sys.stderr)
        return 1

    block_count = count_mermaid_blocks(args.src)
    print(
        f"EPUB mermaid verification passed "
        f"({block_count} diagram(s) embedded as PNG images)"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
