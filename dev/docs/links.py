"""Where a link in Solium's documentation points, once the book is built.

The documentation is written to be read on GitHub, where a link is a path in
the repository: `docs/ricing.md` links `../crates/solium/lua/config.lua`, and
the README links `docs/ricing.md`. The book is built from `docs/`, with a few
files from elsewhere copied in, so the same link has to become one of three
things:

* a page or picture in the book, relative to the page it is on -- a doc in
  `docs/`, a file copied in, or the reference page generated from a source
  file (`config.lua` becomes the configuration reference);
* a link to the file on GitHub, for anything else in the repository;
* a page of the book again, for a link to one of its pages on the published
  site -- which is how a doc on GitHub links a page only the book has, such as
  the bindings reference -- so the link check reads that one too;
* left alone, for any other URL, an anchor on the same page, or a path that
  goes nowhere -- which the link check then reports.

One module, used by `generate.py` for the pages it writes and by
`preprocess.py` for every chapter mdBook reads, so the two cannot disagree.
Standard library only.
"""

import os
import posixpath
import re

REPOSITORY = "https://github.com/Lilium-Linux/solium"
BRANCH = "stage"
# Where the book is published, from `stage`: book.toml's `site-url`.
SITE = "https://lilium-linux.github.io/solium/"

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

# Files from outside `docs/` that the book carries, copied verbatim to
# `docs/generated/repo/<the same path>`.
COPIED = [
    "CONTRIBUTING.md",
    "CODE_OF_CONDUCT.md",
    "CLA.md",
    "SECURITY.md",
    "THIRD_PARTY.md",
    "dev/README.md",
    "dev/wirecheck/README.md",
    "dev/qtprobe/README.md",
    "dev/host-window-rule.md",
    "crates/solium/qml/panes/README.md",
    "crates/solium/effects/README.md",
]
COPIES = "generated/repo"

# Source files whose page in the book is generated from them.
GENERATED = {
    "crates/solium/lua/config.lua": "generated/reference/configuration.md",
    "crates/solium/lua/meta/sol.lua": "generated/reference/lua-api.md",
    "crates/solium/environment.txt": "generated/reference/environment.md",
}

# The README is split in two: its install sections are the Getting started
# page, and the rest is the introduction.
README = "README.md"
INTRODUCTION = "introduction.md"
GETTING_STARTED = "generated/getting-started.md"
INSTALL_SECTIONS = ("Building", "Running it", "Install")

IMAGES = (".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp")


def anchor(heading):
    """The id mdBook gives a heading, which is also the one GitHub gives."""
    text = re.sub(r"<[^>]+>", "", heading).strip().lower()
    out = []
    for char in text:
        if char.isalnum() or char in "-_":
            out.append(char)
        elif char.isspace():
            out.append("-")
    return "".join(out)


def install_anchors():
    """The anchors of the README's install sections, which live elsewhere."""
    return {anchor(section) for section in INSTALL_SECTIONS}


def book_path(repo_path, fragment=""):
    """The book's own path for a file in the repository, or None."""
    if repo_path == README:
        if fragment and fragment in install_anchors():
            return GETTING_STARTED
        return INTRODUCTION
    if repo_path in GENERATED:
        return GENERATED[repo_path]
    if repo_path in COPIED:
        return posixpath.join(COPIES, repo_path)
    if repo_path.startswith("docs/"):
        return repo_path[len("docs/"):]
    return None


def github(repo_path, fragment=""):
    """A link to a file or folder in the repository, on GitHub."""
    full = os.path.join(ROOT, repo_path)
    if repo_path.lower().endswith(IMAGES):
        url = f"{REPOSITORY}/raw/{BRANCH}/{repo_path}"
    elif os.path.isdir(full):
        url = f"{REPOSITORY}/tree/{BRANCH}/{repo_path}"
    else:
        url = f"{REPOSITORY}/blob/{BRANCH}/{repo_path}"
    return url + (f"#{fragment}" if fragment else "")


def target(link, origin, dest):
    """Where `link`, written in the file `origin`, points from the page `dest`.

    `origin` is a path in the repository, `dest` a path in the book. Returns
    the link unchanged when it is not one to rewrite.
    """
    if link and link.startswith(SITE):
        path, _, fragment = link[len(SITE):].partition("#")
        if not path.endswith(".html"):
            return link
        # mdBook writes `x.md` as `x.html`, and points a link at `x.md` there.
        page = path[: -len(".html")] + ".md"
        relative = posixpath.relpath(page, posixpath.dirname(dest) or ".")
        return relative + (f"#{fragment}" if fragment else "")
    if not link or re.match(r"^[a-zA-Z][a-zA-Z0-9+.-]*:", link) or link.startswith("//"):
        return link
    path, _, fragment = link.partition("#")
    if not path:
        # An anchor on the page it was written on. Unchanged when that page is
        # this one; otherwise it now points at a page of its own.
        if book_path(origin, fragment) == dest:
            return link
        path = posixpath.basename(origin)
    resolved = posixpath.normpath(posixpath.join(posixpath.dirname(origin), path))
    if resolved.startswith("../") or resolved == "..":
        return link
    page = book_path(resolved, fragment)
    if page is None:
        if not os.path.exists(os.path.join(ROOT, resolved)):
            return link
        return github(resolved, fragment)
    relative = posixpath.relpath(page, posixpath.dirname(dest) or ".")
    return relative + (f"#{fragment}" if fragment else "")


# `[text](target "title")`, and `![alt](target)`. The target cannot contain a
# space or a closing parenthesis, which is true of every link in these docs.
INLINE = re.compile(r"(\]\()([^)\s]+)((?:\s+\"[^\"]*\")?\))")
# `[label]: target`
REFERENCE = re.compile(r"^(\s*\[[^\]]+\]:\s*)(\S+)(.*)$")
# `src="..."` and `href="..."` in raw HTML.
ATTRIBUTE = re.compile(r"((?:src|href)=\")([^\"]+)(\")")
FENCE = re.compile(r"^\s*(```|~~~)")


def rewrite(markdown, origin, dest):
    """Every link in `markdown`, pointed at where it goes from `dest`.

    Code is left alone: a fenced block, an indented block after a blank line,
    and an inline code span are all text, whatever is in them.
    """
    out = []
    fenced = None
    previous_blank = True
    in_indented = False
    for line in markdown.split("\n"):
        stripped = line.strip()
        fence = FENCE.match(line)
        if fenced:
            if fence and fence.group(1) == fenced:
                fenced = None
            out.append(line)
            continue
        if fence:
            fenced = fence.group(1)
            out.append(line)
            continue
        indented = line.startswith("    ") or line.startswith("\t")
        if indented and (previous_blank or in_indented) and not re.match(r"^\s*([-*+]|\d+\.)\s", line):
            in_indented = True
            out.append(line)
            previous_blank = False
            continue
        in_indented = in_indented and (not stripped)
        previous_blank = not stripped

        reference = REFERENCE.match(line)
        if reference:
            out.append(reference.group(1) + target(reference.group(2), origin, dest) + reference.group(3))
            continue
        # Split on inline code spans and rewrite only what is outside them.
        parts = re.split(r"(`+[^`]*`+)", line)
        for index, part in enumerate(parts):
            if index % 2:
                continue
            part = INLINE.sub(lambda m: m.group(1) + target(m.group(2), origin, dest) + m.group(3), part)
            part = ATTRIBUTE.sub(lambda m: m.group(1) + target(m.group(2), origin, dest) + m.group(3), part)
            parts[index] = part
        out.append("".join(parts))
    return "\n".join(out)


def origin_of(chapter_path):
    """The repository file a chapter of the book was written as."""
    if chapter_path.startswith(COPIES + "/"):
        return chapter_path[len(COPIES) + 1:]
    return posixpath.join("docs", chapter_path)
