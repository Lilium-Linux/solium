#!/usr/bin/env python3
"""mdBook preprocessor: point every chapter's links where they go in the book.

The docs link each other as paths in the repository, which is right on GitHub
and wrong in the book for anything outside `docs/`. This rewrites each
chapter's links as mdBook reads it, so the files themselves stay as they are.
What each link becomes is decided in `links.py`.

mdBook runs it as `preprocess.py supports <renderer>`, answered by the exit
status, and then with `[context, book]` as JSON on stdin, expecting the book
back on stdout.
"""

import json
import os
import sys

# No __pycache__ beside the scripts: the checkout is not the place for it.
sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import links  # noqa: E402


def walk(items):
    for item in items:
        chapter = item.get("Chapter") if isinstance(item, dict) else None
        if not chapter:
            continue
        path = chapter.get("path")
        if path:
            chapter["content"] = links.rewrite(chapter["content"], links.origin_of(path), path)
        walk(chapter.get("sub_items", []))


def main():
    if len(sys.argv) > 1 and sys.argv[1] == "supports":
        sys.exit(0)
    _context, book = json.load(sys.stdin)
    # `items` from mdBook 0.5, `sections` before it.
    walk(book.get("items", book.get("sections", [])))
    json.dump(book, sys.stdout)


if __name__ == "__main__":
    main()
