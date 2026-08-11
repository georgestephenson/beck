#!/bin/sh
# Install a released `beck`.
#
#   curl -fsSL https://raw.githubusercontent.com/georgestephenson/beck/main/install.sh | sh
#
# What it does, in order: work out which platform this is, resolve the version, download that
# platform's tarball *and* the release's `SHA256SUMS`, refuse to go on unless the two agree, then
# unpack the binary into `~/.beck/bin` and run it once.
#
# The checksum step is the reason this file exists rather than a paragraph telling somebody to
# download a tarball. `compiler/crates/beck-cli/tests/release.rs` corrupts an archive and asserts
# that this script exits non-zero and installs nothing — the gate is about the gap, not about the
# code that closed it (`docs/84-a-quota-is-only-as-good-as-its-actor-report.md` §84.5).
#
# It is *not* a chain of trust. A checksum published beside the artefact it describes proves the
# download was not corrupted in transit; it proves nothing about the release page. What would is a
# signature and a transparency log, which `docs/99-supply-chain-report.md` §99.7 lists as unbuilt.
#
# Environment:
#   BECK_VERSION      version to install, e.g. 0.3.0 (default: the latest release)
#   BECK_INSTALL_DIR  where the binary goes (default: $HOME/.beck/bin)
#   BECK_TARGET       override the detected platform triple
#   BECK_BASE_URL     where the assets are (default: the GitHub release for that version)
#   BECK_REPO         owner/name (default: georgestephenson/beck)
set -eu

REPO=${BECK_REPO:-georgestephenson/beck}

say() { printf '%s\n' "$*"; }
die() {
    printf 'install.sh: %s\n' "$*" >&2
    exit 1
}

case "${1:-}" in
-h | --help)
    # Piped from `curl`, `$0` is not a readable file, so the comment header is a bonus rather than
    # the answer.
    say "install.sh — install a released beck."
    say ""
    say "  BECK_VERSION      version to install (default: the latest release)"
    say "  BECK_INSTALL_DIR  where the binary goes (default: \$HOME/.beck/bin)"
    say "  BECK_TARGET       override the detected platform triple"
    say "  BECK_BASE_URL     where the assets are (default: the GitHub release for that version)"
    say "  BECK_REPO         owner/name (default: georgestephenson/beck)"
    exit 0
    ;;
"") ;;
*) die "unknown argument: $1 (this script is configured by environment variables; --help lists them)" ;;
esac

# ---- the platform -----------------------------------------------------------------------------
# Every triple named here is one `.github/workflows/release.yml` builds, and `release.rs` asserts
# the two lists are the same set in both directions. An installer that offers a platform the
# pipeline does not build is a 404 in front of somebody's first five minutes; a pipeline that
# builds one the installer does not offer is an artefact nobody can reach.
SUPPORTED="x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu x86_64-apple-darwin aarch64-apple-darwin"

detect_target() {
    os=$(uname -s)
    arch=$(uname -m)
    case "$os" in
    Linux) os=unknown-linux-gnu ;;
    Darwin) os=apple-darwin ;;
    *) die "unsupported operating system: $os. Build from source: see docs/86-getting-started.md" ;;
    esac
    case "$arch" in
    x86_64 | amd64) arch=x86_64 ;;
    aarch64 | arm64) arch=aarch64 ;;
    *) die "unsupported architecture: $arch. Build from source: see docs/86-getting-started.md" ;;
    esac
    printf '%s-%s\n' "$arch" "$os"
}

target=${BECK_TARGET:-$(detect_target)}

# Checked even when `BECK_TARGET` supplied it: this script fetches from the project's own releases,
# so a triple no release contains can only produce a download error further down.
case " $SUPPORTED " in
*" $target "*) ;;
*) die "no release is built for $target. Built platforms: $SUPPORTED" ;;
esac

# ---- fetching ---------------------------------------------------------------------------------
if command -v curl >/dev/null 2>&1; then
    fetcher=curl
elif command -v wget >/dev/null 2>&1; then
    fetcher=wget
else
    die "neither curl nor wget is installed"
fi

fetch() { # url destination
    case "$fetcher" in
    curl) curl -fsSL "$1" -o "$2" || return 1 ;;
    wget) wget -q "$1" -O "$2" || return 1 ;;
    esac
}

latest_version() {
    # The `releases/latest` page redirects to the tag, so the tag is readable without parsing JSON
    # and without a token.
    [ "$fetcher" = curl ] || return 1
    url=$(curl -fsSLI -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest") || return 1
    tag=${url##*/}
    [ -n "$tag" ] && [ "$tag" != latest ] || return 1
    printf '%s\n' "${tag#v}"
}

version=${BECK_VERSION:-}
if [ -z "$version" ]; then
    if [ -n "${BECK_BASE_URL:-}" ]; then
        die "BECK_BASE_URL is set, so the version cannot be resolved from the release page — set BECK_VERSION too"
    fi
    version=$(latest_version) || die "could not resolve the latest release — set BECK_VERSION"
fi

base=${BECK_BASE_URL:-https://github.com/$REPO/releases/download/v$version}
asset="beck-$version-$target.tar.gz"
install_dir=${BECK_INSTALL_DIR:-$HOME/.beck/bin}

# ---- the checksum -----------------------------------------------------------------------------
# Resolved here rather than inside `sha256_of`, because that function is called in a command
# substitution and a `die` inside one exits the subshell rather than the script. Deliberately fatal:
# an installer that skips verification when the tool is missing has taught its users that
# verification is optional, which is worse than not verifying.
if command -v sha256sum >/dev/null 2>&1; then
    sha256_of() { sha256sum "$1" | cut -d' ' -f1; }
elif command -v shasum >/dev/null 2>&1; then
    sha256_of() { shasum -a 256 "$1" | cut -d' ' -f1; }
else
    die "no sha256sum and no shasum — cannot verify the download"
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

say "beck $version for $target"
say "  from $base"
fetch "$base/$asset" "$work/$asset" || die "could not download $base/$asset"
fetch "$base/SHA256SUMS" "$work/SHA256SUMS" || die "could not download $base/SHA256SUMS"

expected=$(awk -v want="$asset" '$2 == want || $2 == "*" want { print $1; exit }' "$work/SHA256SUMS")
[ -n "$expected" ] || die "SHA256SUMS does not mention $asset"
actual=$(sha256_of "$work/$asset")
[ "$expected" = "$actual" ] || die "checksum mismatch for $asset
  expected $expected
  got      $actual
Nothing was installed."
say "  sha256 $actual ✓"

# ---- installing -------------------------------------------------------------------------------
tar -xzf "$work/$asset" -C "$work" || die "could not unpack $asset"
binary="$work/beck-$version-$target/beck"
[ -f "$binary" ] || die "$asset does not contain beck-$version-$target/beck"

mkdir -p "$install_dir" || die "could not create $install_dir"
# Install through a temporary name in the same directory: a `beck` that is running elsewhere on
# this machine keeps the inode it started with, and a half-written binary is never on the path.
tmp="$install_dir/.beck.$$"
cp "$binary" "$tmp"
chmod 755 "$tmp"
mv -f "$tmp" "$install_dir/beck"

say "  installed $install_dir/beck"
"$install_dir/beck" --version || die "the installed binary does not run"

case ":${PATH}:" in
*":$install_dir:"*) ;;
*)
    say ""
    say "$install_dir is not on your PATH. Add it:"
    say "    export PATH=\"$install_dir:\$PATH\""
    ;;
esac
