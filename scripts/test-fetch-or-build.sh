#!/bin/sh
# test-fetch-or-build.sh — the hermetic test of scripts/fetch-or-build.sh (T-27).
#
# Builds a fake `file://` release for this machine's triple, then runs the build
# step three ways:
#   1. a good archive + checksum  -> installs the binary, verifies the SHA-256;
#   2. a bad checksum             -> falls back to the build command;
#   3. no release at all          -> falls back to the build command.
#
# The build command is a stub (GEAR_CARGO) so the fallback is observable and
# nothing is compiled here. Run from anywhere: `sh scripts/test-fetch-or-build.sh`.
set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
root=$(dirname "$here")
script="$here/fetch-or-build.sh"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

version=$(sed -nE 's/^version *= *"([^"]+)".*/\1/p' "$root/herdr-plugin.toml" | head -n 1)
[ -n "$version" ] || { echo "no version in herdr-plugin.toml" >&2; exit 1; }

case "$(uname -s)" in
  Darwin)
    case "$(uname -m)" in
      arm64|aarch64) triple="aarch64-apple-darwin" ;;
      *) triple="x86_64-apple-darwin" ;;
    esac
    ;;
  *)
    triple="x86_64-unknown-linux-gnu"
    ;;
esac

# A stand-in for `cargo`: records that it ran and leaves a binary behind.
stub="$work/cargo-stub"
cat > "$stub" <<'STUB'
#!/bin/sh
echo "stub cargo: $*" >> "${GEAR_TEST_LOG:?}"
exit 0
STUB
chmod +x "$stub"

release="$work/release/v$version"
mkdir -p "$release"
binary="$work/gear"
printf '#!/bin/sh\necho "prebuilt gear %s"\n' "$version" > "$binary"
chmod +x "$binary"
archive="gear-$version-$triple.tar.gz"
tar -C "$work" -czf "$release/$archive" gear
( cd "$release" && sha256sum "$archive" > "$archive.sha256" )

run() { # run <name> <base_url> <out>
  log="$work/log-$1"
  : > "$log"
  GEAR_BASE_URL="$2" GEAR_OUT="$3" GEAR_CARGO="$stub" GEAR_TEST_LOG="$log" \
    sh "$script" >"$work/out-$1" 2>&1 || true
  echo "--- $1 ---"
  cat "$work/out-$1"
  echo "--- build stub ran: $(wc -l < "$log" | tr -d ' ') time(s)"
}

# 1. good release: installed and verified.
out1="$work/install/gear"
run good "file://$work/release" "$out1"
test -x "$out1" || { echo "FAIL: the verified binary was not installed" >&2; exit 1; }
"$out1" | grep -q "prebuilt gear $version" || { echo "FAIL: wrong binary installed" >&2; exit 1; }
grep -q "verified SHA-256" "$work/out-good" || { echo "FAIL: no verification message" >&2; exit 1; }
[ "$(wc -l < "$work/log-good")" -eq 0 ] || { echo "FAIL: built from source although the release was good" >&2; exit 1; }

# 2. bad checksum: must fall back, not install.
release2="$work/release-bad/v$version"
mkdir -p "$release2"
cp "$release/$archive" "$release2/$archive"
printf '%064d  %s\n' 0 "$archive" > "$release2/$archive.sha256"
out2="$work/install-bad/gear"
run bad "file://$work/release-bad" "$out2"
test ! -e "$out2" || { echo "FAIL: a bad checksum installed a binary" >&2; exit 1; }
grep -q "checksum mismatch" "$work/out-bad" || { echo "FAIL: no checksum message" >&2; exit 1; }
[ "$(wc -l < "$work/log-bad")" -eq 1 ] || { echo "FAIL: the checked bad archive did not fall back" >&2; exit 1; }

# 3. no release at all: must fall back.
run missing "file://$work/nowhere" "$work/install-none/gear"
grep -q "not available" "$work/out-missing" || { echo "FAIL: no 'not available' message" >&2; exit 1; }
[ "$(wc -l < "$work/log-missing")" -eq 1 ] || { echo "FAIL: a missing release did not fall back" >&2; exit 1; }

echo "OK: prebuilt installed and verified; bad checksum and missing release fall back to building."
