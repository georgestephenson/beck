#!/bin/sh
# The one place the release version is read from.
#
# `docs/28-releases-and-deployment.md` §28.2 item 4 numbers releases `0.x` per phase-sized
# increment, and the number lives in `compiler/Cargo.toml`'s `[workspace.package]` — every crate
# inherits it with `version.workspace = true`, and so does `beck --version`. A tag that disagrees
# with it would publish an artefact whose own version string contradicts the release it is in, so
# `build.sh --expect-version` compares the two and refuses.
#
# Usage: release/version.sh          # prints e.g. 0.3.0
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
manifest="$root/compiler/Cargo.toml"

[ -f "$manifest" ] || {
    echo "release/version.sh: $manifest is missing" >&2
    exit 1
}

# The first `version = "…"` after the `[workspace.package]` header, and nothing else in the file:
# `[workspace.dependencies]` below it carries a version per crate, and picking one of those up
# would be a number that happens to agree today.
version=$(awk '
    /^\[workspace\.package\]/ { in_pkg = 1; next }
    /^\[/                     { in_pkg = 0 }
    in_pkg && /^version[ \t]*=/ {
        gsub(/^version[ \t]*=[ \t]*"/, "")
        gsub(/".*$/, "")
        print
        exit
    }
' "$manifest")

[ -n "$version" ] || {
    echo "release/version.sh: no version under [workspace.package] in $manifest" >&2
    exit 1
}

printf '%s\n' "$version"
