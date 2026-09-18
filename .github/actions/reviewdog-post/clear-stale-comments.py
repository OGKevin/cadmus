#!/usr/bin/env python3
"""Delete reviewdog PR review comments for one tool when diagnostics are empty."""

from __future__ import annotations

import base64
import json
import re
import subprocess
import sys


def source_name_matches(meta_b64: str, tool_name: str) -> bool:
    try:
        raw = base64.b64decode(meta_b64, validate=True)
    except (ValueError, base64.binascii.Error):
        return False
    encoded = tool_name.encode("utf-8")
    if len(encoded) > 127:
        return False
    needle = bytes([0x12, len(encoded)]) + encoded
    return needle in raw


def main() -> None:
    if len(sys.argv) != 3:
        print("usage: clear-stale-comments.py <tool_name> <github_repository>", file=sys.stderr)
        sys.exit(2)

    tool_name = sys.argv[1]
    repo = sys.argv[2]
    comments = json.load(sys.stdin)

    meta_re = re.compile(r"<!-- __reviewdog__:([^>]+) -->")

    parents_with_replies: set[int] = set()
    for comment in comments:
        reply_to = comment.get("in_reply_to_id")
        if reply_to:
            parents_with_replies.add(int(reply_to))

    to_delete: list[int] = []
    for comment in comments:
        comment_id = int(comment["id"])
        if comment_id in parents_with_replies:
            continue
        body = comment.get("body") or ""
        for match in meta_re.finditer(body):
            if source_name_matches(match.group(1), tool_name):
                to_delete.append(comment_id)
                break

    for comment_id in to_delete:
        subprocess.run(
            [
                "gh",
                "api",
                "-X",
                "DELETE",
                f"repos/{repo}/pulls/comments/{comment_id}",
            ],
            check=True,
        )

    print(f"Cleared {len(to_delete)} stale reviewdog comment(s) for tool {tool_name}")


if __name__ == "__main__":
    main()
