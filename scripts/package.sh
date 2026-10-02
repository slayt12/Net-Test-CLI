#!/usr/bin/env bash
# Assemble the downloadable release artifacts for one version into dist/.
#
# Why: GitHub Releases should offer ready-to-run packages (archive per platform with the manual
# and licence inside, plus bare binaries for people who want a single file) and a checksum list.
# This script is the single place that defines those names, so the CI workflow
# (.github/workflows/release.yml) and a manual release produce identical assets.
#
# Usage:  scripts/package.sh <version>          e.g. scripts/package.sh 1.0.0  (no leading "v")
# Expects both release builds to exist already:
#   cargo build --release
#   cargo build --release --target x86_64-pc-windows-gnu
#
# NETTEST_ROOT overrides the source tree to package (default: the repo this script lives in). The
# release workflow uses it to build an older tag with the current script.
#
# Invariants: the built client must report exactly <version>, otherwise the tag and the binaries
# would disagree and the script refuses. dist/ is recreated from scratch on every run.
#
# Exit codes: 0 ok, 1 missing binary or version mismatch, 2 required tool missing.
set -euo pipefail

VER="${1:?usage: scripts/package.sh <version>}"
VER="${VER#v}"
ROOT="${NETTEST_ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
DIST="$ROOT/dist"
LIN="$ROOT/target/release"
WIN="$ROOT/target/x86_64-pc-windows-gnu/release"

for f in "$LIN/nettest-client" "$LIN/nettest-server" "$WIN/nettest-client.exe" "$WIN/nettest-server.exe"; do
    [[ -f "$f" ]] || { echo "missing $f: build both release targets first" >&2; exit 1; }
done
for tool in zip tar sha256sum; do
    command -v "$tool" >/dev/null || { echo "$tool is required" >&2; exit 2; }
done

got="$("$LIN/nettest-client" --version | awk '{print $2}')"
if [[ "$got" != "$VER" ]]; then
    echo "built binary reports version $got but $VER was requested; bump [workspace.package] version in Cargo.toml" >&2
    exit 1
fi

LINUX_PKG="nettest-$VER-linux-x86_64"
WIN_PKG="nettest-$VER-windows-x86_64"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT

rm -rf "$DIST"
mkdir -p "$DIST" "$stage/$LINUX_PKG" "$stage/$WIN_PKG"
cp "$LIN/nettest-client" "$LIN/nettest-server" "$stage/$LINUX_PKG/"
cp "$WIN/nettest-client.exe" "$WIN/nettest-server.exe" "$stage/$WIN_PKG/"
for d in "$stage/$LINUX_PKG" "$stage/$WIN_PKG"; do
    cp "$ROOT/README.md" "$ROOT/LICENSE" "$ROOT/nettest-admin-guide.pdf" "$d/"
done
tar -C "$stage" -czf "$DIST/$LINUX_PKG.tar.gz" "$LINUX_PKG"
(cd "$stage" && zip -qr "$DIST/$WIN_PKG.zip" "$WIN_PKG")

cp "$LIN/nettest-client" "$DIST/nettest-client-linux-x86_64"
cp "$LIN/nettest-server" "$DIST/nettest-server-linux-x86_64"
cp "$WIN/nettest-client.exe" "$DIST/nettest-client-windows-x86_64.exe"
cp "$WIN/nettest-server.exe" "$DIST/nettest-server-windows-x86_64.exe"
(cd "$DIST" && sha256sum -- * > SHA256SUMS)

# Lowest glibc the Linux binaries need; depends on the build host, so it is measured, not assumed.
glibc="unknown"
if command -v objdump >/dev/null; then
    glibc="$(objdump -T "$LIN/nettest-client" "$LIN/nettest-server" 2>/dev/null \
        | grep -o 'GLIBC_[0-9.]*' | sed 's/GLIBC_//' | sort -V | tail -1 || true)"
    glibc="${glibc:-unknown}"
fi

cat > "$DIST/RELEASE_NOTES.md" <<NOTES
nettest $VER: dependency-free network troubleshooting client and server for Windows 11 and Linux.

| Download | Contents |
|---|---|
| \`$WIN_PKG.zip\` | nettest-client.exe, nettest-server.exe, admin guide (PDF), README, LICENSE |
| \`$LINUX_PKG.tar.gz\` | nettest-client, nettest-server, admin guide (PDF), README, LICENSE |
| \`nettest-client-windows-x86_64.exe\`, \`nettest-server-windows-x86_64.exe\` | single executables, no installer |
| \`nettest-client-linux-x86_64\`, \`nettest-server-linux-x86_64\` | single executables (\`chmod +x\` after download) |
| \`SHA256SUMS\` | checksums of every file above |

- Windows: x86-64, imports only system DLLs. The files are not code-signed, so SmartScreen may
  warn on first run (More info, then Run anyway). Defender Firewall prompts once when the server
  first listens.
- Linux: x86-64, needs glibc $glibc or newer. Build from source with the musl target for older
  systems (see README, Building).
- Quick start, every flag and the troubleshooting guide: README.md and nettest-admin-guide.pdf
  inside each archive, or https://github.com/slayt12/Net-Test-CLI#readme.
NOTES

ls -la "$DIST"
