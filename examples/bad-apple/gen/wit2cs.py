#!/usr/bin/env python3
"""Write C# types for Solium's window manager protocol from its WIT.

    gen/wit2cs.py <wit directory> <output .cs>

The WIT files are the protocol's one definition, so a window manager in any
language can be built from them the way this one is: every record becomes a
class, every variant a class with one nested class a case, every enum an
enum, each with its `Encode` and `Decode` in the protocol's encoding
(docs/window-managers.md, "The wire format"). Nothing is written by hand
that the WIT already says.

Python 3 and its standard library only.
"""

import re
import sys

PRIMITIVES = {
    "bool": ("bool", "Bool"),
    "u8": ("byte", "U8"),
    "u16": ("ushort", "U16"),
    "u32": ("uint", "U32"),
    "u64": ("ulong", "U64"),
    "s8": ("sbyte", "S8"),
    "s16": ("short", "S16"),
    "s32": ("int", "S32"),
    "s64": ("long", "S64"),
    "f32": ("float", "F32"),
    "f64": ("double", "F64"),
    "string": ("string", "Text"),
}


def pascal(name):
    return "".join(part.capitalize() for part in name.lstrip("%").split("-"))


def split_top(text):
    """A comma-separated list, cut at the commas outside `<>` and `()`."""
    out, depth, start = [], 0, 0
    for index, char in enumerate(text):
        if char in "<(":
            depth += 1
        elif char in ">)":
            depth -= 1
        elif char == "," and depth == 0:
            out.append(text[start:index])
            start = index + 1
    out.append(text[start:])
    return [part.strip() for part in out if part.strip()]


def parse_type(text):
    """A WIT type as a tree: ("prim", name), ("named", name), ("list", T),
    ("option", T) or ("tuple", [T...])."""
    text = text.strip()
    for kind in ("list", "option", "tuple"):
        if text.startswith(kind + "<") and text.endswith(">"):
            inner = text[len(kind) + 1:-1]
            if kind == "tuple":
                return ("tuple", [parse_type(part) for part in split_top(inner)])
            return (kind, parse_type(inner))
    if text in PRIMITIVES:
        return ("prim", text)
    return ("named", text)


def parse(directory):
    """Every named type of the three files, in the order written."""
    types = {}
    for name in ("types.wit", "commands.wit", "messages.wit"):
        body, current = None, None
        with open(f"{directory}/{name}", encoding="utf-8") as file:
            lines = file.read().split("\n")
        for line in lines:
            text = line.split("//")[0].strip()
            if not text:
                continue
            if body is not None:
                for part in split_top(text.split("}")[0]):
                    match = re.match(r"%?([a-z0-9-]+)\s*(?::\s*(.+)|\((.+)\))?$", part)
                    payload = match.group(2) or match.group(3)
                    current["items"].append((match.group(1), parse_type(payload) if payload else None))
                if "}" in text:
                    body = None
                continue
            alias = re.match(r"type ([a-z0-9-]+) = (.+);$", text)
            opened = re.match(r"(record|variant|enum) ([a-z0-9-]+) \{(.*)$", text)
            if alias:
                types[alias.group(1)] = {"kind": "alias", "target": parse_type(alias.group(2))}
            elif opened:
                current = {"kind": opened.group(1), "items": []}
                types[opened.group(2)] = current
                rest = opened.group(3)
                for part in split_top(rest.split("}")[0]):
                    match = re.match(r"%?([a-z0-9-]+)\s*(?::\s*(.+)|\((.+)\))?$", part)
                    payload = match.group(2) or match.group(3)
                    current["items"].append((match.group(1), parse_type(payload) if payload else None))
                body = None if "}" in rest else True
    return types


class Generator:
    def __init__(self, types):
        self.types = types
        self.out = []
        self.fresh = 0

    def temp(self):
        self.fresh += 1
        return f"v{self.fresh}"

    def resolve(self, tree):
        """A type with every alias replaced by what it stands for."""
        while tree[0] == "named" and self.types[tree[1]]["kind"] == "alias":
            tree = self.types[tree[1]]["target"]
        return tree

    def cs(self, tree):
        tree = self.resolve(tree)
        kind = tree[0]
        if kind == "prim":
            return PRIMITIVES[tree[1]][0]
        if kind == "named":
            # Qualified: a variant's case is a nested class that may share
            # the name of the type it carries.
            return "global::Solium.Wm." + pascal(tree[1])
        if kind == "list":
            return f"List<{self.cs(tree[1])}>"
        if kind == "option":
            return self.cs(tree[1]) + "?"
        return "(" + ", ".join(self.cs(part) for part in tree[1]) + ")"

    def initial(self, tree):
        """What a field of this type starts as, where it must start as
        something: text empty, a list empty, a class to be filled in."""
        tree = self.resolve(tree)
        if tree == ("prim", "string"):
            return ' = ""'
        if tree[0] == "list":
            return " = new()"
        if tree[0] == "named" and self.types[tree[1]]["kind"] != "enum":
            return " = default!"
        return ""

    def write(self, tree, expr, pad):
        tree = self.resolve(tree)
        kind = tree[0]
        if kind == "prim":
            self.out.append(f"{pad}w.{PRIMITIVES[tree[1]][1]}({expr});")
        elif kind == "named":
            if self.types[tree[1]]["kind"] == "enum":
                self.out.append(f"{pad}w.U32((uint){expr});")
            else:
                self.out.append(f"{pad}{expr}.Encode(w);")
        elif kind == "list":
            item = self.temp()
            self.out.append(f"{pad}w.U32((uint){expr}.Count);")
            self.out.append(f"{pad}foreach (var {item} in {expr})")
            self.out.append(f"{pad}{{")
            self.write(tree[1], item, pad + "    ")
            self.out.append(f"{pad}}}")
        elif kind == "option":
            value = self.temp()
            self.out.append(f"{pad}if ({expr} is {{ }} {value})")
            self.out.append(f"{pad}{{")
            self.out.append(f"{pad}    w.U8(1);")
            self.write(tree[1], value, pad + "    ")
            self.out.append(f"{pad}}}")
            self.out.append(f"{pad}else")
            self.out.append(f"{pad}{{")
            self.out.append(f"{pad}    w.U8(0);")
            self.out.append(f"{pad}}}")
        else:
            for index, part in enumerate(tree[1], 1):
                self.write(part, f"{expr}.Item{index}", pad)

    def read(self, tree, name, pad):
        """Declare `name`, read from `r`."""
        tree = self.resolve(tree)
        kind = tree[0]
        if kind == "prim":
            self.out.append(f"{pad}var {name} = r.{PRIMITIVES[tree[1]][1]}();")
        elif kind == "named":
            if self.types[tree[1]]["kind"] == "enum":
                self.out.append(f"{pad}var {name} = ({self.cs(tree)})r.U32();")
            else:
                self.out.append(f"{pad}var {name} = {self.cs(tree)}.Decode(r);")
        elif kind == "list":
            count, index, item = self.temp(), self.temp(), self.temp()
            self.out.append(f"{pad}var {count} = r.Length();")
            self.out.append(f"{pad}var {name} = new {self.cs(tree)}({count});")
            self.out.append(f"{pad}for (var {index} = 0; {index} < {count}; {index}++)")
            self.out.append(f"{pad}{{")
            self.read(tree[1], item, pad + "    ")
            self.out.append(f"{pad}    {name}.Add({item});")
            self.out.append(f"{pad}}}")
        elif kind == "option":
            inner = self.temp()
            self.out.append(f"{pad}{self.cs(tree)} {name} = null;")
            self.out.append(f"{pad}if (r.Bool())")
            self.out.append(f"{pad}{{")
            self.read(tree[1], inner, pad + "    ")
            self.out.append(f"{pad}    {name} = {inner};")
            self.out.append(f"{pad}}}")
        else:
            parts = []
            for part in tree[1]:
                parts.append(self.temp())
                self.read(part, parts[-1], pad)
            self.out.append(f"{pad}var {name} = ({', '.join(parts)});")

    def record(self, name, items):
        cls = pascal(name)
        self.out.append(f"public sealed class {cls}")
        self.out.append("{")
        for field, tree in items:
            self.out.append(f"    public {self.cs(tree)} {pascal(field)}{self.initial(tree)};")
        self.out.append("")
        self.out.append("    public void Encode(Writer w)")
        self.out.append("    {")
        for field, tree in items:
            self.write(tree, pascal(field), "        ")
        self.out.append("    }")
        self.out.append("")
        self.out.append(f"    public static {cls} Decode(Reader r)")
        self.out.append("    {")
        self.out.append(f"        var it = new {cls}();")
        for field, tree in items:
            value = self.temp()
            self.read(tree, value, "        ")
            self.out.append(f"        it.{pascal(field)} = {value};")
        self.out.append("        return it;")
        self.out.append("    }")
        self.out.append("}")
        self.out.append("")

    def variant(self, name, items):
        cls = pascal(name)
        self.out.append(f"public abstract class {cls}")
        self.out.append("{")
        self.out.append("    public abstract void Encode(Writer w);")
        self.out.append("")
        self.out.append(f"    public static {cls} Decode(Reader r)")
        self.out.append("    {")
        self.out.append("        var index = r.U32();")
        self.out.append("        switch (index)")
        self.out.append("        {")
        for index, (case, tree) in enumerate(items):
            self.out.append(f"            case {index}:")
            self.out.append("            {")
            if tree is None:
                self.out.append(f"                return new {pascal(case)}();")
            else:
                value = self.temp()
                self.read(tree, value, "                ")
                self.out.append(f"                return new {pascal(case)} {{ Value = {value} }};")
            self.out.append("            }")
        self.out.append("            default:")
        self.out.append(f'                throw new InvalidDataException($"{name} has no case {{index}}");')
        self.out.append("        }")
        self.out.append("    }")
        for index, (case, tree) in enumerate(items):
            self.out.append("")
            self.out.append(f"    /// <summary>The WIT's <c>{name}</c> case <c>{case}</c>.</summary>")
            self.out.append(f"    public sealed class {pascal(case)} : {cls}")
            self.out.append("    {")
            if tree is not None:
                self.out.append(f"        public {self.cs(tree)} Value{self.initial(tree)};")
                self.out.append("")
            self.out.append("        public override void Encode(Writer w)")
            self.out.append("        {")
            self.out.append(f"            w.U32({index});")
            if tree is not None:
                self.write(tree, "Value", "            ")
            self.out.append("        }")
            self.out.append("    }")
        self.out.append("}")
        self.out.append("")

    def enum(self, name, items):
        self.out.append(f"public enum {pascal(name)}")
        self.out.append("{")
        for case, _ in items:
            self.out.append(f"    {pascal(case)},")
        self.out.append("}")
        self.out.append("")

    def run(self, version):
        self.out += [
            "// <auto-generated>",
            "// Written by gen/wit2cs.py from Solium's WIT. Change the WIT, not this.",
            "// </auto-generated>",
            "#nullable enable",
            "using System.Collections.Generic;",
            "using System.IO;",
            "",
            "namespace Solium.Wm;",
            "",
            "public static class Protocol",
            "{",
            "    /// <summary>The protocol's version: the WIT package's.</summary>",
            f'    public const string Version = "{version}";',
            "}",
            "",
        ]
        for name, each in self.types.items():
            if each["kind"] == "record":
                self.record(name, each["items"])
            elif each["kind"] == "variant":
                self.variant(name, each["items"])
            elif each["kind"] == "enum":
                self.enum(name, each["items"])
        return "\n".join(self.out)


def main():
    if len(sys.argv) != 3:
        sys.exit(__doc__.split("\n\n")[1].strip())
    directory, target = sys.argv[1], sys.argv[2]
    with open(f"{directory}/types.wit", encoding="utf-8") as file:
        version = re.search(r"package solium:wm@([0-9.]+);", file.read()).group(1)
    text = Generator(parse(directory)).run(version)
    with open(target, "w", encoding="utf-8") as file:
        file.write(text + "\n")


if __name__ == "__main__":
    main()
