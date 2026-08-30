#!/usr/bin/env bash
#
# Log Lens — Linux installer (user scope, no root).
#
# Run from the unpacked Artifact directory:
#
#     ./install.sh            # interactive
#     ./install.sh --quiet    # non-interactive; used by the in-app Update
#
# It writes only under $HOME. Nothing here needs sudo, and nothing here edits
# your shell configuration.
#
# ---------------------------------------------------------------------------
# CONTRACT 1 — Artifact naming (docs/plans/d1-distribution-pipeline.md, 4.4)
# ---------------------------------------------------------------------------
# The Update check matches Release assets by name, so these names are a
# compatibility contract, not a formatting choice. Renaming one breaks
# self-update for every already-installed copy.
#
#     LogLens-<version>-windows-x86_64-setup.exe
#     LogLens-<version>-windows-x86_64-portable.zip
#     LogLens-<version>-linux-x86_64.tar.gz
#     SHA256SUMS
#
# The Linux tarball unpacks into a single directory — no tarbombs:
#
#     LogLens-<version>-linux-x86_64/
#     ├── loglens                                  the binary
#     ├── install.sh                               this script (mode 755)
#     ├── uninstall.sh                             (mode 755)
#     ├── io.github.dennisdms.LogLens.desktop      Exec= is rewritten below
#     └── io.github.dennisdms.LogLens.png          256x256 app icon
#
# ---------------------------------------------------------------------------
# CONTRACT 2 — the Install flavour marker (plan 4.3)
# ---------------------------------------------------------------------------
# Written to $XDG_DATA_HOME/loglens/install-manifest.json (default
# ~/.local/share/loglens/install-manifest.json), which is Rust's
# `dirs::data_dir()/loglens/`:
#
#     {
#       "flavour": "installer",
#       "install_dir": "/home/you/.local/bin",
#       "version": "0.2.0"
#     }
#
# `install_dir` is the directory holding the binary — the parent of
# `std::env::current_exe()` for an installed copy. The app must compare the two
# and treat a mismatch as Portable; a marker that merely exists is not enough,
# or a Portable copy run on a machine that also has Log Lens installed would
# find this marker and believe it may update itself in place.
#
# ---------------------------------------------------------------------------
# Installed paths (plan 4.2)
# ---------------------------------------------------------------------------
#     binary          ~/.local/bin/loglens
#     desktop entry   ~/.local/share/applications/io.github.dennisdms.LogLens.desktop
#     icon            ~/.local/share/icons/hicolor/256x256/apps/io.github.dennisdms.LogLens.png
#     marker + uninstaller
#                     ~/.local/share/loglens/
#
# Settings and secrets are never touched: ~/.config/loglens/ and the keyring
# sit outside every one of these paths, deliberately.

set -euo pipefail

APP_ID="io.github.dennisdms.LogLens"
APP_NAME="Log Lens"
BIN_NAME="loglens"

# XDG_DATA_HOME/XDG_CONFIG_HOME are honoured because that is what Rust's
# `dirs` crate does; with them unset these are exactly the paths in the plan.
DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
CONFIG_HOME="${XDG_CONFIG_HOME:-$HOME/.config}"

BIN_DIR="$HOME/.local/bin"
APPLICATIONS_DIR="$DATA_HOME/applications"
ICON_DIR="$DATA_HOME/icons/hicolor/256x256/apps"
APP_DATA_DIR="$DATA_HOME/$BIN_NAME"

MANIFEST="$APP_DATA_DIR/install-manifest.json"

SRC_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"

QUIET=0

usage() {
    cat <<EOF
Usage: install.sh [--quiet] [--help]

Installs $APP_NAME for the current user. Writes only under \$HOME; never needs
root and never edits your shell configuration.

  -q, --quiet   Print nothing but errors. Used by the in-app Update.
  -h, --help    Show this message.
EOF
}

for arg in "$@"; do
    case "$arg" in
        -q | --quiet) QUIET=1 ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            printf 'install.sh: unknown option: %s\n\n' "$arg" >&2
            usage >&2
            exit 2
            ;;
    esac
done

say() {
    if [ "$QUIET" -eq 0 ]; then
        printf '%s\n' "$*"
    fi
}

die() {
    printf 'install.sh: %s\n' "$*" >&2
    exit 1
}

# Escape a string for embedding in a JSON string literal. Home directories can
# legally contain a backslash or a double quote.
json_escape() {
    printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g'
}

# Install SRC to DEST via a temporary file in the destination directory,
# followed by a rename. The rename matters: during an in-app Update the old
# binary is still running, and writing over a running executable in place
# fails with ETXTBSY ("Text file busy"). A rename unlinks the old inode, which
# the running process keeps using until it exits.
install_file() {
    local src="$1" dest="$2" mode="$3"
    local tmp="${dest}.new.$$"
    install -m "$mode" -- "$src" "$tmp"
    mv -f -- "$tmp" "$dest"
}

for required in "$BIN_NAME" "uninstall.sh" "$APP_ID.desktop" "$APP_ID.png"; do
    [ -f "$SRC_DIR/$required" ] || die "missing $required next to install.sh — is this a complete, unpacked $APP_NAME archive?"
done

# The version recorded in the marker. Ask the binary, which prints
# "Log Lens <version> (<sha>)"; fall back to the Artifact directory name
# (LogLens-<version>-linux-x86_64) if it cannot run here.
detect_version() {
    local out=""
    if out="$("$SRC_DIR/$BIN_NAME" --version 2>/dev/null)"; then
        out="$(printf '%s' "$out" | awk 'NR==1 {print $3}')"
        if [ -n "$out" ]; then
            printf '%s' "$out"
            return 0
        fi
    fi
    out="$(basename -- "$SRC_DIR")"
    case "$out" in
        LogLens-*-linux-x86_64)
            out="${out#LogLens-}"
            printf '%s' "${out%-linux-x86_64}"
            return 0
            ;;
    esac
    printf 'unknown'
}

VERSION="$(detect_version)"

say "Installing $APP_NAME $VERSION for $(id -un)…"

mkdir -p -- "$BIN_DIR" "$APPLICATIONS_DIR" "$ICON_DIR" "$APP_DATA_DIR"

install_file "$SRC_DIR/$BIN_NAME" "$BIN_DIR/$BIN_NAME" 755
install_file "$SRC_DIR/$APP_ID.png" "$ICON_DIR/$APP_ID.png" 644
install_file "$SRC_DIR/uninstall.sh" "$APP_DATA_DIR/uninstall.sh" 755

# The desktop entry ships with a bare `Exec=loglens`; rewrite it to the
# absolute path as it is copied. A fresh Debian does not have ~/.local/bin on
# PATH, and a launcher entry whose command is not on PATH fails silently.
desktop_tmp="$APPLICATIONS_DIR/$APP_ID.desktop.new.$$"
sed -e "s|^Exec=.*|Exec=$BIN_DIR/$BIN_NAME|" -- "$SRC_DIR/$APP_ID.desktop" > "$desktop_tmp"
chmod 644 -- "$desktop_tmp"
mv -f -- "$desktop_tmp" "$APPLICATIONS_DIR/$APP_ID.desktop"

manifest_tmp="$MANIFEST.new.$$"
cat > "$manifest_tmp" <<EOF
{
  "flavour": "installer",
  "install_dir": "$(json_escape "$BIN_DIR")",
  "version": "$(json_escape "$VERSION")"
}
EOF
chmod 644 -- "$manifest_tmp"
mv -f -- "$manifest_tmp" "$MANIFEST"

# Refresh the desktop and icon caches when the tools are there. Plenty of
# systems do not ship them, and plenty of desktops need no prompting; a
# missing cache tool must never fail an install.
if command -v update-desktop-database > /dev/null 2>&1; then
    update-desktop-database "$APPLICATIONS_DIR" > /dev/null 2>&1 || true
fi
if command -v gtk-update-icon-cache > /dev/null 2>&1; then
    gtk-update-icon-cache --ignore-theme-index --quiet "$DATA_HOME/icons/hicolor" > /dev/null 2>&1 || true
fi

say ""
say "Installed:"
say "  $BIN_DIR/$BIN_NAME"
say "  $APPLICATIONS_DIR/$APP_ID.desktop"
say "  $ICON_DIR/$APP_ID.png"
say "  $MANIFEST"
say "  $APP_DATA_DIR/uninstall.sh"
say ""
say "$APP_NAME should now appear in your launcher. Settings live in"
say "$CONFIG_HOME/$BIN_NAME/ and are never touched by install or uninstall."
say ""
say "To remove it later: $APP_DATA_DIR/uninstall.sh"

# PATH note only — deliberately no shell-configuration edit. An installer that
# silently rewrites .bashrc/.zshrc/.profile earns a bug report, and the
# launcher entry works without PATH because Exec= is absolute.
case ":${PATH-}:" in
    *":$BIN_DIR:"*) ;;
    *)
        say ""
        say "Note: $BIN_DIR is not on your PATH, so typing '$BIN_NAME' in a"
        say "terminal will not find it. The launcher entry works regardless."
        say "To fix it, add this line to your shell profile yourself:"
        say ""
        say "    export PATH=\"\$HOME/.local/bin:\$PATH\""
        ;;
esac
