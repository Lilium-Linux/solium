#!/usr/bin/env python3
"""Fail when a link inside the built book goes nowhere.

    dev/docs/linkcheck.py target/book

Reads the HTML mdBook wrote, not the Markdown, so it checks what a reader
actually gets: every relative `href` and `src` has to name a file in the book,
and every `#fragment` an id in that file. Links out of the book are not
fetched -- a build should not depend on the network -- and the Rust API under
`api/` is checked for its files only, not for rustdoc's own anchors.

`print.html` repeats every chapter and `404.html` is written for the site's
root rather than its own place, so both are skipped.

mdbook-linkcheck would do this from inside mdBook, but it has not been
released since 2022 and does not read the book mdBook 0.5 hands it.
"""

import html.parser
import os
import re
import sys
import urllib.parse

SKIPPED = {"print.html", "404.html"}


class Collector(html.parser.HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.links = []
        self.ids = set()

    def handle_starttag(self, tag, attrs):
        for name, value in attrs:
            if value is None:
                continue
            if name in ("id", "name"):
                self.ids.add(value)
            elif name in ("href", "src") and tag in ("a", "img", "link", "script", "source"):
                self.links.append(value)


def parse(path, cache):
    if path not in cache:
        collector = Collector()
        with open(path, encoding="utf-8", errors="replace") as file:
            collector.feed(file.read())
        cache[path] = collector
    return cache[path]


def main():
    if len(sys.argv) != 2:
        sys.exit("usage: linkcheck.py <book directory>")
    book = os.path.abspath(sys.argv[1])
    api = os.path.join(book, "api") + os.sep
    cache = {}
    broken = []
    checked = 0
    for directory, _, files in os.walk(book):
        if (directory + os.sep).startswith(api):
            continue
        for name in sorted(files):
            if not name.endswith(".html"):
                continue
            page = os.path.join(directory, name)
            if os.path.relpath(page, book) in SKIPPED:
                continue
            for link in parse(page, cache).links:
                if re.match(r"^[a-zA-Z][a-zA-Z0-9+.-]*:", link) or link.startswith("//"):
                    continue
                checked += 1
                target, _, fragment = link.partition("#")
                target = urllib.parse.unquote(target.split("?", 1)[0])
                where = os.path.normpath(os.path.join(directory, target)) if target else page
                if os.path.isdir(where):
                    where = os.path.join(where, "index.html")
                shown = os.path.relpath(page, book)
                if not where.startswith(book + os.sep) and where != book:
                    broken.append(f"{shown}: {link} leaves the book")
                    continue
                if not os.path.isfile(where):
                    broken.append(f"{shown}: {link} (no such file)")
                    continue
                if fragment and where.endswith(".html") and not where.startswith(api):
                    if urllib.parse.unquote(fragment) not in parse(where, cache).ids:
                        broken.append(f"{shown}: {link} (no #{fragment} there)")
    if broken:
        print(f"linkcheck: {len(broken)} broken link(s) of {checked}:", file=sys.stderr)
        for line in broken:
            print(f"  {line}", file=sys.stderr)
        sys.exit(1)
    print(f"linkcheck: {checked} links inside the book, none broken")


if __name__ == "__main__":
    main()
