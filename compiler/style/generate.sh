#!/bin/sh
# Ask Tailwind which of `candidates.txt` are utilities, and write down its answer.
#
# `docs/104` §104.4: "the oracle is Tailwind itself, not a table somebody typed in". This is how
# that oracle is consulted, and it is deliberately *not* a test: it needs a network and an npm
# registry, and a gate that installs a package is a gate that fails when somebody else's server
# does. The answer is committed under `expected/` instead, which is `clbg/`'s pattern — hold
# somebody else's published artefact so a wrong constant fails even against a matching wrong
# expectation.
#
# Run it by hand when the pinned version moves, and commit what changes:
#
#     cd compiler/style && ./generate.sh
#
# `style.rs::the_utility_table_agrees_with_tailwind` is what reads the result.
set -eu

VERSION=4.3.3
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

cd "$work"
npm install --silent "@tailwindcss/cli@$VERSION" "tailwindcss@$VERSION" >/dev/null

printf '@import "tailwindcss";\n' > in.css
# One element carrying every candidate. Tailwind's scanner reads any string that looks like a
# class, so the shape of the file matters less than that each candidate appears in it once.
{
    printf '<div class="'
    tr '\n' ' ' < "$here/candidates.txt"
    printf '"></div>\n'
} > page.html

./node_modules/.bin/tailwindcss -i in.css -o out.css --content page.html >/dev/null 2>&1

# The selectors the sheet actually defines, as a set — **not** a substring search over the file.
# The first version of this script asked `grep -F ".rounded-ful"` and got a hit from
# `.rounded-full`, so a misspelling came back as a utility and the gate below would have been
# green about a name Tailwind refuses. A generator that reads its oracle wrongly is worse than no
# oracle, because the answer looks authoritative.
python3 - "$here" "$VERSION" <<'PY'
import re, sys
here, version = sys.argv[1], sys.argv[2]
css = open("out.css", encoding="utf-8").read()
# Every `.name` at the head of a selector, with CSS's escaping undone.
defined = {re.sub(r"\\(.)", r"\1", m) for m in re.findall(r"\.((?:[^\s,{}:\\]|\\.)+)", css)}
lines = [f"# Tailwind {version}, asked which of ../candidates.txt it emits a rule for.",
         "# Written by ./generate.sh — do not edit by hand."]
for name in open(f"{here}/candidates.txt", encoding="utf-8").read().split():
    # A variant's rule is `.hover\:bg-emerald-700:hover`, so the class part is what is compared and
    # the pseudo-class Tailwind appends is not part of the name.
    lines.append(("rule\t" if name in defined else "none\t") + name)
open(f"{here}/expected/tailwind-{version}.txt", "w", encoding="utf-8").write("\n".join(lines) + "\n")
print(f"wrote expected/tailwind-{version}.txt")
PY
