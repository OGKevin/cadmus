#!/usr/bin/env python3
"""
mdBook preprocessor for converting Mermaid diagrams to static PNG for EPUB output.

This preprocessor follows the mdBook preprocessor protocol:
1. Reads book JSON from stdin
2. Processes mermaid code blocks → static PNG images
3. Outputs modified book JSON to stdout

PNG is used instead of SVG because mermaid-cli generates SVG with foreignObject
elements containing HTML, which EPUB readers don't support well. PNG ensures
text is properly rendered as raster graphics.

Source files remain untouched - all processing happens in memory.
"""

import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path


def render_mermaid_to_png(mermaid_code: str, output_path: Path) -> bool:
    """
    Render a mermaid diagram to PNG using mermaid-cli (mmdc).

    Args:
        mermaid_code: The mermaid diagram code
        output_path: Path where PNG should be saved

    Returns:
        True if rendering succeeded, False otherwise
    """
    try:
        with tempfile.NamedTemporaryFile(
            mode="w", suffix=".mmd", delete=False
        ) as temp_mmd:
            temp_mmd.write(mermaid_code)
            temp_mmd_path = temp_mmd.name

        mmdc_cmd = "../node_modules/.bin/mmdc"
        home = Path.home()
        puppeteer_cache = home / ".cache" / "puppeteer" / "chrome-headless-shell"

        chrome_path = None
        if puppeteer_cache.exists():
            for version_dir in puppeteer_cache.iterdir():
                if version_dir.is_dir():
                    for platform in [
                        "chrome-headless-shell-mac-arm64",
                        "chrome-headless-shell-linux-x64",
                        "chrome-headless-shell",
                    ]:
                        chrome_exe = version_dir / platform / "chrome-headless-shell"
                        if chrome_exe.exists():
                            chrome_path = str(chrome_exe)
                            break
                    if chrome_path:
                        break

        env = os.environ.copy()
        if chrome_path:
            env["PUPPETEER_EXECUTABLE_PATH"] = chrome_path

        cmd = [mmdc_cmd] + [
            "-i",
            temp_mmd_path,
            "-o",
            str(output_path),
            "-b",
            "transparent",
        ]

        is_ci = os.environ.get("CI") or os.environ.get("GITHUB_ACTIONS")
        temp_puppeteer_config = None
        if is_ci:
            with tempfile.NamedTemporaryFile(
                mode="w", suffix=".json", delete=False
            ) as config_file:
                json.dump({"args": ["--no-sandbox"]}, config_file)
                temp_puppeteer_config = config_file.name
            cmd.extend(["--puppeteerConfigFile", temp_puppeteer_config])

        subprocess.run(cmd, capture_output=True, text=True, check=True, env=env)
        
        if temp_puppeteer_config:
            Path(temp_puppeteer_config).unlink()

        Path(temp_mmd_path).unlink()
        return True

    except subprocess.CalledProcessError as e:
        print(
            f"Warning: Failed to render mermaid diagram: stdout={e.stdout}, stderr={e.stderr}",
            file=sys.stderr,
        )
        return False
    except FileNotFoundError as e:
        print(f"Warning: mmdc command not found: {e}", file=sys.stderr)
        return False
    except Exception as e:
        print(f"Warning: Unexpected error rendering diagram: {e}", file=sys.stderr)
        return False


def process_chapter_content(content: str, chapter_name: str, png_dir: Path) -> str:
    """
    Process chapter content, converting mermaid code blocks to PNG image references.

    Args:
        content: The markdown content of the chapter
        chapter_name: Name of the chapter (for PNG filenames)
        png_dir: Directory where PNG files should be saved

    Returns:
        Modified content with mermaid blocks replaced by image references
    """
    pattern = r"^```mermaid\s*\n(.*?)^```$"
    diagram_count = 0

    def replace_mermaid_block(match):
        nonlocal diagram_count
        diagram_count += 1

        mermaid_code = match.group(1).strip()
        img_filename = f"{chapter_name}-diagram-{diagram_count}.png"
        img_path = png_dir / img_filename
        rel_path = f"../mermaid-images/{img_filename}"

        if render_mermaid_to_png(mermaid_code, img_path):
            return f"![Mermaid Diagram]({rel_path})"
        else:
            return match.group(0)

    return re.sub(
        pattern, replace_mermaid_block, content, flags=re.MULTILINE | re.DOTALL
    )


def process_book(book_data: dict) -> dict:
    """
    Process the entire book, converting mermaid diagrams in all chapters.

    Args:
        book_data: The book data from mdBook (JSON)

    Returns:
        Modified book data with processed chapters
    """
    root = book_data.get("root", ".")
    book_dir = Path(root)
    src_dir = book_dir / "src"
    png_dir = src_dir / "mermaid-images"
    png_dir.mkdir(exist_ok=True)

    items = book_data.get("items", [])

    def process_section(section):
        if "Chapter" in section:
            chapter = section["Chapter"]
            chapter_name = chapter.get("name", "unnamed")
            chapter_name = re.sub(r"[^\w\-]", "-", chapter_name.lower())

            content = chapter.get("content", "")
            if "```mermaid" in content:
                chapter["content"] = process_chapter_content(
                    content, chapter_name, png_dir
                )

            sub_items = chapter.get("sub_items", [])
            for sub_item in sub_items:
                process_section(sub_item)

    for item in items:
        process_section(item)

    return book_data


def main():
    """Main entry point for the preprocessor."""
    if len(sys.argv) > 1:
        if sys.argv[1] == "supports":
            renderer = sys.argv[2] if len(sys.argv) > 2 else ""
            sys.exit(0 if renderer == "epub" else 1)
        else:
            print(f"Unknown argument: {sys.argv[1]}", file=sys.stderr)
            sys.exit(1)

    context_and_book = json.load(sys.stdin)

    if isinstance(context_and_book, list):
        book = context_and_book[1]
    else:
        print("Error: Expected book data as a tuple [context, book]", file=sys.stderr)
        sys.exit(1)

    processed_book = process_book(book)

    json.dump(processed_book, sys.stdout)


if __name__ == "__main__":
    main()
