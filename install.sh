#!/bin/sh
# CXCAP installer: binary + cxcap-development skill + Claude symlink.
# Non-interactive. Idempotent. Never overwrites unrelated user content.
# Usage: install.sh [--prefix DIR] [--binary PATH] [--from-source] [--no-skill]
set -eu

PREFIX="${HOME}/.local"
BIN_SRC=""
FROM_SOURCE=0
WITH_SKILL=1

for arg in "$@"; do
    case "$arg" in
        --prefix=*) PREFIX="${arg#--prefix=}";;
        --prefix) echo "install.sh: --prefix needs DIR via --prefix=DIR" >&2; exit 2;;
        --binary=*) BIN_SRC="${arg#--binary=}";;
        --binary) echo "install.sh: --binary needs PATH via --binary=PATH" >&2; exit 2;;
        --from-source) FROM_SOURCE=1;;
        --no-skill) WITH_SKILL=0;;
        -h|--help)
            echo "usage: install.sh [--prefix=DIR] [--binary=PATH] [--from-source] [--no-skill]"
            exit 0;;
        *) echo "install.sh: unknown flag $arg" >&2; exit 2;;
    esac
done

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
SKILL_SRC="$SCRIPT_DIR/skill/cxcap-development/SKILL.md"
SKILL_DEST="$HOME/.agents/skills/cxcap-development"
CLAUDE_LINK="$HOME/.claude/skills/cxcap-development"

if [ "$BIN_SRC" = "" ]; then
    if [ "$FROM_SOURCE" -eq 1 ] || [ ! -x "$SCRIPT_DIR/target/release/cxcap" ]; then
        if [ "$FROM_SOURCE" -eq 1 ] || [ ! -f "$SCRIPT_DIR/target/release/cxcap" ]; then
            echo "install.sh: building release binary (cargo build --release)..." >&2
            (cd "$SCRIPT_DIR" && cargo build --release)
        fi
    fi
    BIN_SRC="$SCRIPT_DIR/target/release/cxcap"
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

case ":$PATH:" in
    *":$PREFIX/bin:"*) ;;
    *) echo "install.sh: note: $PREFIX/bin is not on PATH" >&2;;
esac
echo "install.sh: done" >&2
