#!/bin/sh
# Ask Tailwind what it emits for `candidates.txt`, and write down its answer.
#
# `docs/104` §104.4: "the oracle is Tailwind itself, not a table somebody typed in". This is how
# that oracle is consulted, and it is deliberately *not* a test: it needs a network and an npm
# registry, and a gate that installs a package is a gate that fails when somebody else's server
# does. The answer is committed under `expected/` instead, which is `clbg/`'s pattern — hold
# somebody else's published artefact so a wrong constant fails even against a matching wrong
# expectation.
#
# It records **what Tailwind emits**, not only whether it emits something, because the table stopped
# being a predicate: `beck_core::style::rule` turns a name into declarations, and a table that
# agreed about which names exist while disagreeing about what they mean would be a page styled
# wrongly with every gate green.
#
# Run it by hand when the pinned version moves, and commit what changes:
#
#     cd compiler/style && ./generate.sh
#
# `beck-cli/tests/style.rs` is what reads the result.
set -eu

VERSION=4.3.3
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

cd "$work"
npm install --silent "@tailwindcss/cli@$VERSION" "tailwindcss@$VERSION" >/dev/null

mkdir page
printf '@import "tailwindcss";\n' > page/in.css
# One element carrying every candidate. Tailwind's scanner reads any string that looks like a
# class, so the shape of the file matters less than that each candidate appears in it once — and
# the directory holds nothing else, because Tailwind 4 scans the tree it is pointed at and would
# otherwise find the candidates again inside its own output.
{
    printf '<div class="'
    tr '\n' ' ' < "$here/candidates.txt"
    printf '"></div>\n'
} > page/page.html

./node_modules/.bin/tailwindcss -i page/in.css -o out.css --content page/page.html >/dev/null 2>&1

# What the sheet actually defines, read as a **tree of blocks** rather than by searching the text.
# The first version of this script asked `grep -F ".rounded-ful"` and got a hit from
# `.rounded-full`, so a misspelling came back as a utility. The second read one level of at-rule
# and lost `dark:md:flex`, which is nested two deep, and read `.\32 xl\:flex` — CSS's escape for a
# leading digit, which is a hex code *and a space* — as a name that stops at the space. A generator
# that reads its oracle wrongly is worse than no oracle, because the answer looks authoritative.
python3 - "$here" "$VERSION" <<'PY'
import re, sys
here, version = sys.argv[1], sys.argv[2]
css = open("out.css", encoding="utf-8").read()


def blocks(text):
    """Top-level `header { body }` pairs."""
    out, i, n = [], 0, len(text)
    while i < n:
        j = text.find("{", i)
        if j < 0:
            break
        depth, k = 1, j + 1
        while k < n and depth:
            depth += {"{": 1, "}": -1}.get(text[k], 0)
            k += 1
        out.append((text[i:j].strip(), text[j + 1 : k - 1]))
        i = k
    return out


def layer(name):
    m = re.search(r"@layer %s\s*\{" % name, css)
    if not m:
        return ""
    depth, k = 1, m.end()
    while depth:
        depth += {"{": 1, "}": -1}.get(css[k], 0)
        k += 1
    return css[m.end() : k - 1]


def unescape(s):
    # `\32 ` is a hex escape with a trailing space; `\:` is the character itself.
    s = re.sub(r"\\([0-9a-fA-F]{1,6})\s?", lambda m: chr(int(m.group(1), 16)), s)
    return re.sub(r"\\(.)", r"\1", s)


def decls(body):
    return ";".join(" ".join(p.split()) for p in body.split(";") if p.strip())


def name_of(selector):
    m = re.search(r"\.((?:[^\s,{}:>()\[\]\\]|\\[0-9a-fA-F]{1,6}\s?|\\.)+)", selector)
    return unescape(m.group(1)) if m else None


found = {}


def walk(chain, text):
    for header, body in blocks(text):
        if header.startswith("@"):
            walk(chain + [" ".join(header.split())], body)
            continue
        name = name_of(header)
        if name is not None:
            found.setdefault(name, []).append(
                ("|".join(chain), " ".join(header.split()), decls(body))
            )


walk([], layer("utilities"))

theme = {}
for _, body in blocks(layer("theme")):
    for decl in body.split(";"):
        if ":" in decl and decl.strip().startswith("--"):
            k, v = decl.split(":", 1)
            theme[k.strip()] = " ".join(v.split())

props = {
    m.group(1): decls(m.group(2))
    for m in re.finditer(r"@property (--[a-z0-9-]+)\s*\{([^}]*)\}", css)
}

# The condition guarding the fallback Tailwind ships for browsers with no registered custom
# properties. Captured rather than transcribed: it is a browser-detection expression, which is
# exactly the kind of string nobody can check by reading it.
m = re.search(r"@supports (\([^{]*\))\s*\{", layer("properties"))
supports = " ".join(m.group(1).split()) if m else ""

lines = [
    f"# Tailwind {version}, asked what it emits for every name in ../candidates.txt.",
    "# Written by ./generate.sh — do not edit by hand.",
    "#",
    "# rule\tname\tat-rules (| separated, outermost first)\tselector\tdeclarations",
    "# none\tname",
    "# theme\ttoken\tvalue",
    "# property\ttoken\tdeclarations",
]
for name in open(f"{here}/candidates.txt", encoding="utf-8").read().split():
    for at, selector, body in found.get(name, []):
        lines.append("\t".join(["rule", name, at, selector, body]))
    if name not in found:
        lines.append("none\t" + name)
for token, value in sorted(theme.items()):
    lines.append("\t".join(["theme", token, value]))
for token, value in sorted(props.items()):
    lines.append("\t".join(["property", token, value]))
if supports:
    lines.append("\t".join(["supports", supports]))
open(f"{here}/expected/tailwind-{version}.txt", "w", encoding="utf-8").write("\n".join(lines) + "\n")
print(f"wrote expected/tailwind-{version}.txt: {len(found)} names with rules, "
      f"{len(theme)} theme tokens, {len(props)} properties")
PY
