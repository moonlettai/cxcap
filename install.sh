#!/bin/sh
# CXCAP installer: binary + cxcap-development skill + Claude symlink + PATH.
# Non-interactive. Idempotent. Never overwrites unrelated user content.
# Usage: install.sh [--prefix DIR] [--binary PATH] [--from-source] [--no-skill] [--no-path] [-h|--help]
#
# Binary source, first match wins:
#   1. --binary PATH          explicit binary file
#   2. --from-source          build with cargo (requires Rust stable)
#   3. target/release/cxcap   source-checkout layout next to this script
#   4. ./cxcap                extracted-release-tarball layout next to this script
#   5. download               latest release tarball for your platform
#                             (or $CXCAP_VERSION), checksum-verified against
#                             the release SHA256SUMS.txt before use
#
# Env overrides (mirrors, staging, tests):
#   CXCAP_VERSION        pinned version, e.g. 1.0.1 (skips latest lookup)
#   CXCAP_RELEASE_BASE   release download base URL
#                        (default https://github.com/moonlettai/cxcap/releases/download)
#   CXCAP_LATEST_URL     latest-release API URL
#                        (default https://api.github.com/repos/moonlettai/cxcap/releases/latest)
set -eu

PREFIX="${HOME}/.local"
BIN_SRC=""
FROM_SOURCE=0
WITH_SKILL=1
WITH_PATH=1

for arg in "$@"; do
    case "$arg" in
        --prefix=*) PREFIX="${arg#--prefix=}";;
        --prefix) echo "install.sh: --prefix needs DIR via --prefix=DIR" >&2; exit 2;;
        --binary=*) BIN_SRC="${arg#--binary=}";;
        --binary) echo "install.sh: --binary needs PATH via --binary=PATH" >&2; exit 2;;
        --from-source) FROM_SOURCE=1;;
        --no-skill) WITH_SKILL=0;;
        --no-path) WITH_PATH=0;;
        -h|--help)
            echo "usage: install.sh [--prefix=DIR] [--binary=PATH] [--from-source] [--no-skill] [--no-path]"
            echo "  --prefix DIR   install binary under DIR/bin (default ~/.local)"
            echo "  --binary PATH  install this binary file instead of detecting one"
            echo "  --from-source  build with cargo instead of detecting a binary"
            echo "  --no-skill     skip the cxcap-development agent skill"
            echo "  --no-path      do not touch shell rc files even if PREFIX/bin is missing from PATH"
            echo "env: CXCAP_VERSION, CXCAP_RELEASE_BASE, CXCAP_LATEST_URL"
            exit 0;;
        *) echo "install.sh: unknown flag $arg" >&2; exit 2;;
    esac
done

die() {
    echo "install.sh: $1" >&2
    exit 1
}

need_cmd() {
    command -v "$1" >/dev/null 2>&1 || die "needs '$1' on PATH"
}

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
SKILL_SRC="$SCRIPT_DIR/skill/cxcap-development/SKILL.md"
SKILL_DEST="$HOME/.agents/skills/cxcap-development"
CLAUDE_LINK="$HOME/.claude/skills/cxcap-development"

# --- binary resolution ---
TMPD=""
cleanup() {
    if [ "$TMPD" != "" ]; then
        rm -rf "$TMPD"
    fi
}
trap cleanup EXIT INT TERM

STAGE_DIR="$SCRIPT_DIR"
if [ "$BIN_SRC" = "" ]; then
    if [ "$FROM_SOURCE" -eq 1 ] || [ ! -x "$SCRIPT_DIR/target/release/cxcap" ]; then
        if [ "$FROM_SOURCE" -eq 1 ] || [ ! -f "$SCRIPT_DIR/target/release/cxcap" ]; then
            if [ -x "$SCRIPT_DIR/cxcap" ] && [ -f "$SCRIPT_DIR/cxcap" ] && [ "$FROM_SOURCE" -eq 0 ]; then
                BIN_SRC="$SCRIPT_DIR/cxcap"
            elif [ "$FROM_SOURCE" -eq 1 ]; then
                echo "install.sh: building release binary (cargo build --release)..." >&2
                (cd "$SCRIPT_DIR" && cargo build --release)
                BIN_SRC="$SCRIPT_DIR/target/release/cxcap"
            else
                # Download the release tarball, checksum-verified.
                # Platform + version resolve here only, so local binary
                # modes (--binary, --from-source, checkout, tarball layouts)
                # stay fully offline.
                OS="$(uname -s | tr '[:upper:]' '[:lower:]' | sed 's/darwin/apple-darwin/;s/linux/unknown-linux-gnu/')"
                ARCH="$(uname -m | sed 's/arm64/aarch64/')"
                case "$OS-$ARCH" in
                    apple-darwin-aarch64|apple-darwin-x86_64|unknown-linux-gnu-x86_64|unknown-linux-gnu-aarch64) ;;
                    *) die "unsupported platform: $(uname -s -m) (need macOS/Linux on arm64/x86_64)";;
                esac
                TARGET="$ARCH-$OS"
                RELEASE_BASE="${CXCAP_RELEASE_BASE:-https://github.com/moonlettai/cxcap/releases/download}"
                LATEST_URL="${CXCAP_LATEST_URL:-https://api.github.com/repos/moonlettai/cxcap/releases/latest}"
                if [ "${CXCAP_VERSION:-}" != "" ]; then
                    VER="$CXCAP_VERSION"
                else
                    VER=""
                fi
                case "$VER" in
                    "") need_cmd curl
                        TAG="$(curl -fsSL "$LATEST_URL" | grep -o '"tag_name": *"[^"]*"' | head -n 1 | cut -d'"' -f4)"
                        VER="$(printf '%s' "$TAG" | sed 's/^v//')"
                        [ "$VER" != "" ] || die "could not determine latest release (set CXCAP_VERSION=X.Y.Z to pin one)";;
                esac
                case "$VER" in
                    *[!0-9.]*|""|*..*|.*|*.) die "refusing suspicious version: $VER";;
                esac
                case "$VER" in
                    *.*.*) ;;
                    *) die "refusing suspicious version: $VER";;
                esac
                case "$VER" in
                    */*) die "refusing suspicious version: $VER";;
                esac
                need_cmd curl
                need_cmd tar
                if command -v sha256sum >/dev/null 2>&1; then
                    SHA256="sha256sum"
                elif command -v shasum >/dev/null 2>&1; then
                    SHA256="shasum -a 256"
                else
                    die "needs 'sha256sum' or 'shasum' on PATH to verify the download"
                fi
                TB="cxcap-$VER-$TARGET.tar.gz"
                TMPD="$(mktemp -d "${TMPDIR:-/tmp}/cxcap-install-XXXXXX")"
                echo "install.sh: downloading $TB (v$VER)..." >&2
                curl -fsSL "$RELEASE_BASE/v$VER/$TB" -o "$TMPD/$TB" \
                    || die "download failed (check platform/version or set CXCAP_VERSION)"
                curl -fsSL "$RELEASE_BASE/v$VER/SHA256SUMS.txt" -o "$TMPD/SHA256SUMS.txt" \
                    || die "checksum file download failed"
                EXPECT="$(grep -F "  $TB" "$TMPD/SHA256SUMS.txt" | awk '{print $1}')"
                [ "$EXPECT" != "" ] || die "no checksum entry for $TB, refusing to install"
                GOT="$($SHA256 "$TMPD/$TB" | awk '{print $1}')"
                [ "$EXPECT" = "$GOT" ] || die "checksum mismatch for $TB, refusing to install"
                tar -xzf "$TMPD/$TB" -C "$TMPD" || die "extract failed"
                [ -f "$TMPD/cxcap" ] || die "release archive has no cxcap binary"
                BIN_SRC="$TMPD/cxcap"
                STAGE_DIR="$TMPD"
                SKILL_SRC="$STAGE_DIR/skill/cxcap-development/SKILL.md"
            fi
        else
            BIN_SRC="$SCRIPT_DIR/target/release/cxcap"
        fi
    else
        BIN_SRC="$SCRIPT_DIR/target/release/cxcap"
    fi
fi

if [ ! -f "$BIN_SRC" ]; then
    echo "install.sh: binary not found: $BIN_SRC" >&2
    exit 1
fi

mkdir -p "$PREFIX/bin"
cp "$BIN_SRC" "$PREFIX/bin/cxcap"
chmod +x "$PREFIX/bin/cxcap"
echo "install.sh: installed cxcap to $PREFIX/bin/cxcap" >&2

if [ "$WITH_SKILL" -eq 1 ]; then
    if [ ! -f "$SKILL_SRC" ]; then
        echo "install.sh: skill source missing: $SKILL_SRC" >&2
        exit 1
    fi
    if [ -f "$SKILL_DEST/SKILL.md" ] && [ ! -f "$SKILL_DEST/.cxcap-managed" ]; then
        echo "install.sh: leaving user-owned $SKILL_DEST/SKILL.md untouched (no marker)" >&2
    else
        mkdir -p "$SKILL_DEST"
        cp "$SKILL_SRC" "$SKILL_DEST/SKILL.md"
        printf '1\n' > "$SKILL_DEST/.cxcap-managed"
        echo "install.sh: installed skill to $SKILL_DEST" >&2
    fi
    if [ -L "$CLAUDE_LINK" ]; then
        if [ "$(readlink "$CLAUDE_LINK")" = "$SKILL_DEST" ]; then
            : # already correct
        else
            echo "install.sh: leaving $CLAUDE_LINK untouched (points elsewhere)" >&2
        fi
    elif [ -e "$CLAUDE_LINK" ]; then
        echo "install.sh: leaving $CLAUDE_LINK untouched (not a symlink)" >&2
    else
        mkdir -p "$HOME/.claude/skills"
        ln -s "$SKILL_DEST" "$CLAUDE_LINK"
        echo "install.sh: linked $CLAUDE_LINK -> $SKILL_DEST" >&2
    fi
fi

# --- PATH wiring ---
case ":$PATH:" in
    *":$PREFIX/bin:"*)
        echo "install.sh: $PREFIX/bin is on PATH" >&2;;
    *)
        if [ "$WITH_PATH" -eq 0 ]; then
            echo "install.sh: note: $PREFIX/bin is not on PATH" >&2
        else
            SHELL_BASE="$(basename "${SHELL:-sh}")"
            case "$SHELL_BASE" in
                zsh) RC_CANDIDATES="$HOME/.zshrc";;
                bash) RC_CANDIDATES="$HOME/.bashrc $HOME/.bash_profile";;
                *) RC_CANDIDATES="$HOME/.profile";;
            esac
            ADDED=""
            for rc in $RC_CANDIDATES; do
                if [ -f "$rc" ] && grep -q "cxcap installer PATH" "$rc" 2>/dev/null; then
                    ADDED="$rc (already present)"
                    break
                fi
            done
            if [ "$ADDED" = "" ]; then
                RC="$(echo "$RC_CANDIDATES" | awk '{print $1}')"
                {
                    echo "# >>> cxcap installer PATH >>>"
                    echo "export PATH=\"$PREFIX/bin:\$PATH\""
                    echo "# <<< cxcap installer PATH <<<"
                } >> "$RC"
                ADDED="$RC"
            fi
            echo "install.sh: added $PREFIX/bin to PATH in $ADDED" >&2
            echo "install.sh: restart your shell or run: export PATH=\"$PREFIX/bin:\$PATH\"" >&2
        fi;;
esac
echo "install.sh: done" >&2
