#!/bin/sh
# Build one release artefact: a `beck` binary for one target, its tarball, and its checksum.
#
# `docs/28-releases-and-deployment.md` §28.2 item 1: "no release-only build steps — the same
# `cargo build --release`, pinned toolchain and locked graph CI uses". That sentence is why this is
# a script and not a block of YAML. `.github/workflows/release.yml` runs it once per target; a
# person runs it with no arguments; both take the same path, so the pipeline is something that has
# been executed rather than something that has been written.
#
# Usage:
#   release/build.sh                                   # host target, into ./dist
#   release/build.sh --target aarch64-apple-darwin --out dist
#   release/build.sh --expect-version 0.3.0            # refuse if the workspace says otherwise
#   release/build.sh --check-only                      # validate the arguments, build nothing
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

target=""
out="$root/dist"
expect=""
check_only=0

die() {
    echo "release/build.sh: $*" >&2
    exit 1
}

while [ $# -gt 0 ]; do
    case "$1" in
    --target)
        [ $# -ge 2 ] || die "--target needs a triple"
        target=$2
        shift 2
        ;;
    --out)
        [ $# -ge 2 ] || die "--out needs a directory"
        out=$2
        shift 2
        ;;
    --expect-version)
        [ $# -ge 2 ] || die "--expect-version needs a version"
        expect=$2
        shift 2
        ;;
    --expect-tag)
        # What the workflow has: `v0.3.0`. The `v` is the tag's, not the version's.
        [ $# -ge 2 ] || die "--expect-tag needs a tag"
        expect=${2#v}
        shift 2
        ;;
    --check-only)
        check_only=1
        shift
        ;;
    *) die "unknown argument: $1" ;;
    esac
done

version=$("$root/release/version.sh")

# The tag names the version, or the release is a lie about itself. This is the check that makes
# `git tag v0.2.0` on a 0.3.0 workspace a failed build rather than a published artefact whose own
# `--version` contradicts the page it was downloaded from.
if [ -n "$expect" ] && [ "$expect" != "$version" ]; then
    die "the release says $expect and compiler/Cargo.toml says $version"
fi

if [ -z "$target" ]; then
    command -v rustc >/dev/null 2>&1 || die "rustc is not on the path"
    target=$(rustc -vV | awk '/^host: / { print $2 }')
    [ -n "$target" ] || die "rustc -vV printed no host triple"
fi

asset="beck-$version-$target.tar.gz"

if [ "$check_only" -eq 1 ]; then
    printf '%s\n' "$asset"
    exit 0
fi

command -v cargo >/dev/null 2>&1 || die "cargo is not on the path"
command -v tar >/dev/null 2>&1 || die "tar is not on the path"

# `--locked` rather than a plain build: a release that re-resolved the dependency graph would be a
# different program from the one the suite ran against (§7.9, "pinning — everything, always").
host=$(rustc -vV | awk '/^host: / { print $2 }')
if [ "$target" = "$host" ]; then
    (cd "$root/compiler" && cargo build --release --locked -p beck-cli)
    binary="$root/compiler/target/release/beck"
else
    (cd "$root/compiler" && cargo build --release --locked -p beck-cli --target "$target")
    binary="$root/compiler/target/$target/release/beck"
fi
[ -f "$binary" ] || die "the build produced no binary at $binary"

mkdir -p "$out"
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT

dir="beck-$version-$target"
mkdir -p "$stage/$dir"
cp "$binary" "$stage/$dir/beck"
chmod 755 "$stage/$dir/beck"
cp "$root/LICENSE" "$stage/$dir/LICENSE"

# Portable flags only: macOS `tar` is bsdtar and does not have GNU's `--sort` or `--owner`. This
# archive is therefore *not* byte-reproducible — gzip stamps an mtime — and nothing here claims it
# is. What the release publishes is a checksum of the artefact it built, which is a different and
# weaker property than the image half's (`docs/92-supply-chain-and-release-report.md` §99.4).
tar -czf "$out/$asset" -C "$stage" "$dir"

sum=$(
    cd "$out" || exit 1
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$asset"
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$asset"
    else
        echo "release/build.sh: no sha256sum and no shasum" >&2
        exit 1
    fi
)
printf '%s\n' "$sum" >"$out/$asset.sha256"

printf '%s\n' "$out/$asset"
printf '%s\n' "$sum"
