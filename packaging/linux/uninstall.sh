#!/usr/bin/env bash
#
# Log Lens — Linux uninstaller (user scope, no root).
#
#     ./uninstall.sh            # interactive
#     ./uninstall.sh --quiet    # print nothing but errors
#
# install.sh copies this script to ~/.local/share/loglens/uninstall.sh, so it
# is still findable long after the downloaded archive has been deleted.
#
# It removes exactly what install.sh installed:
#
#     ~/.local/bin/loglens
#     ~/.local/share/applications/io.github.dennisdms.LogLens.desktop
#     ~/.local/share/icons/hicolor/256x256/apps/io.github.dennisdms.LogLens.png
#     ~/.local/share/loglens/install-manifest.json
#     ~/.local/share/loglens/uninstall.sh   (this file)
#
# and nothing else. Notably it keeps:
#
#   - ~/.config/loglens/ — Connections, Saved Searches and settings. Reinstall
#     -to-fix is the most common reason anyone uninstalls; wiping their
#     configured Connections for that is hostile. The path is printed on the
#     way out so someone who genuinely means it can remove it by hand.
#   - Every keyring entry (stored Connection secrets), for the same reason.
#   - ~/.local/share/loglens/loglens.log — the crash log, if one was written.
#     The directory itself is removed only when it ends up empty.

set -euo pipefail

APP_ID="io.github.dennisdms.LogLens"
APP_NAME="Log Lens"
BIN_NAME="loglens"

DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
CONFIG_HOME="${XDG_CONFIG_HOME:-$HOME/.config}"

BIN_DIR="$HOME/.local/bin"
APPLICATIONS_DIR="$DATA_HOME/applications"
ICON_DIR="$DATA_HOME/icons/hicolor/256x256/apps"
APP_DATA_DIR="$DATA_HOME/$BIN_NAME"

QUIET=0

usage() {
    cat <<EOF
Usage: uninstall.sh [--quiet] [--help]

Removes the per-user $APP_NAME installation. Keeps your Connections, Saved
Searches, settings and stored secrets.

  -q, --quiet   Print nothing but errors.
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
            printf 'uninstall.sh: unknown option: %s\n\n' "$arg" >&2
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

remove() {
    if [ -e "$1" ] || [ -L "$1" ]; then
        rm -f -- "$1"
        say "  removed $1"
    fi
}

say "Removing $APP_NAME…"

remove "$BIN_DIR/$BIN_NAME"
remove "$APPLICATIONS_DIR/$APP_ID.desktop"
remove "$ICON_DIR/$APP_ID.png"
remove "$APP_DATA_DIR/install-manifest.json"

if command -v update-desktop-database > /dev/null 2>&1; then
    update-desktop-database "$APPLICATIONS_DIR" > /dev/null 2>&1 || true
fi
if command -v gtk-update-icon-cache > /dev/null 2>&1; then
    gtk-update-icon-cache --ignore-theme-index --quiet "$DATA_HOME/icons/hicolor" > /dev/null 2>&1 || true
fi

say ""
say "Your Connections and settings were kept in $CONFIG_HOME/$BIN_NAME/,"
say "and your stored Connection secrets were left in your keyring."
say "Delete that directory by hand if you want them gone too."

# Last, because this is the running script. Unlinking it is safe on Linux —
# the shell holds an open descriptor and reads the rest from the unlinked
# inode — but there is no reason to do it any earlier.
remove "$APP_DATA_DIR/uninstall.sh"
# Only if install.sh left nothing else there — a crash log keeps it alive.
rmdir -- "$APP_DATA_DIR" 2> /dev/null || true
