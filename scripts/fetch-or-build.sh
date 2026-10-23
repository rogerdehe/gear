#!/bin/sh
# fetch-or-build.sh — the herdr [[build]] step for the GEAR plugin (T-27).
#
# Fast path: download the prebuilt `gear` for THIS source's declared version and
# platform from the GitHub release, verify its SHA-256, and install it at
# target/release/gear (where bin/gear looks for it). Users need no Rust
# toolchain when a matching release exists.
#
# Fallback: on ANY miss — no asset for this version, no network, no curl/wget,
# checksum mismatch, unmapped platform — say so and build from source with
# `cargo build --release`. Installing never gets harder than before.
#
# The paths, the release base URL and the build command can be overridden
# (GEAR_REPO_ROOT / GEAR_CARGO_TOML / GEAR_OUT / GEAR_BASE_URL / GEAR_CARGO) so
# the logic is tested hermetically with a local `file://` release.
#
# The asset names match .github/workflows/release.yml exactly:
#   gear-<version>-<target>.tar.gz        (contains the binary as `gear`)
#   gear-<version>-<target>.tar.gz.sha256 (<sha256>  <archive name>)
set -u

repo="rogerdehe/gear"

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_root="${GEAR_REPO_ROOT:-$script_dir/..}"
cargo_toml="${GEAR_CARGO_TOML:-$repo_root/herdr-plugin.toml}"
out="${GEAR_OUT:-$repo_root/target/release/gear}"
base_url="${GEAR_BASE_URL:-https://github.com/$repo/releases/download}"
cargo_cmd="${GEAR_CARGO:-cargo}"

have() { command -v "$1" >/dev/null 2>&1; }

# Build from source — what the plugin did before prebuilt releases. Source
# ~/.cargo/env so cargo is found even when Herdr was launched without
# ~/.cargo/bin on PATH; a missing env file is not an error.
build_from_source() {
  [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
  if ! have "$cargo_cmd" && [ ! -x "$cargo_cmd" ]; then
    echo "GEAR needs Rust to build, but '$cargo_cmd' was not found. Install Rust from https://rustup.rs, or install a release with a prebuilt gear for your platform." >&2
    exit 1
  fi
  exec "$cargo_cmd" build --release
}

fallback() {
  echo "gear: $1 — building from source instead." >&2
  [ -n "${tmpdir:-}" ] && rm -rf "$tmpdir"
  build_from_source
}

download() { # download <url> <dest>
  if have curl; then
    curl -fsSL -o "$2" "$1"
  elif have wget; then
    wget -q -O "$2" "$1"
  else
    return 127
  fi
}

sha256_of() { # the hex digest of file $1
  if have sha256sum; then
    sha256sum "$1" | awk '{print $1}'
  elif have shasum; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    return 127
  fi
}

# --- the target triple of this machine -----------------------------------------------------
os=$(uname -s 2>/dev/null || echo unknown)
arch=$(uname -m 2>/dev/null || echo unknown)
triple=""
case "$os" in
  Darwin)
    case "$arch" in
      arm64|aarch64) triple="aarch64-apple-darwin" ;;
      x86_64|amd64)  triple="x86_64-apple-darwin" ;;
    esac
    ;;
  Linux)
    case "$arch" in
      x86_64|amd64) triple="x86_64-unknown-linux-gnu" ;;
    esac
    ;;
esac
[ -n "$triple" ] || fallback "no prebuilt gear for $os/$arch"

# --- the version this source declares ------------------------------------------------------
version=$(sed -nE 's/^version *= *"([^"]+)".*/\1/p' "$cargo_toml" 2>/dev/null | head -n 1)
[ -n "$version" ] || fallback "could not read the version from $cargo_toml"

archive="gear-$version-$triple.tar.gz"
bin_url="$base_url/v$version/$archive"
sum_url="$base_url/v$version/$archive.sha256"

tmpdir=$(mktemp -d 2>/dev/null) || fallback "could not create a temp dir"
trap 'rm -rf "$tmpdir"' EXIT

download "$bin_url" "$tmpdir/$archive" || fallback "prebuilt gear is not available for v$version ($triple)"
download "$sum_url" "$tmpdir/$archive.sha256" || fallback "the checksum of $archive is not published"

# The checksum file is `<hex>  <archive name>` (one line).
expected=$(awk -v name="$archive" '$2 == name || $2 == "*"name { print $1 }' "$tmpdir/$archive.sha256" 2>/dev/null | head -n 1)
[ -n "$expected" ] || fallback "no checksum listed for $archive"

actual=$(sha256_of "$tmpdir/$archive") || fallback "no sha-256 tool (sha256sum/shasum) available"
if [ "$actual" != "$expected" ]; then
  fallback "checksum mismatch for $archive (expected $expected, got $actual)"
fi

# Verified: unpack the binary into place.
mkdir -p "$(dirname "$out")" || fallback "could not create $(dirname "$out")"
tar -C "$tmpdir" -xzf "$tmpdir/$archive" gear 2>/dev/null || fallback "could not unpack $archive"
mv -f "$tmpdir/gear" "$out" || fallback "could not install the verified binary to $out"
chmod +x "$out"
echo "gear: installed prebuilt v$version ($triple), verified SHA-256."
exit 0
