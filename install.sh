#!/bin/sh
# Install Depot for the current user (no password needed), or remove it again.
#
#   ./install.sh               build (if possible) and install into ~/.local
#   ./install.sh --uninstall   remove Depot
#
# To install for every user instead, build as yourself first, then install as root:
#   ./install.sh && sudo PREFIX=/usr/local ./install.sh
set -eu

APP_ID=dev.kimzam.Depot
PREFIX="${PREFIX:-$HOME/.local}"
BIN="$PREFIX/bin/depot"
APPS="$PREFIX/share/applications"
ICONS="$PREFIX/share/icons/hicolor"

cd "$(dirname "$0")"

refresh_menus() {
    update-desktop-database -q "$APPS" 2>/dev/null || true
    gtk-update-icon-cache -q -t "$ICONS" 2>/dev/null || true
}

if [ "${1:-}" = "--uninstall" ]; then
    rm -f "$BIN" "$APPS/$APP_ID.desktop" "$ICONS/scalable/apps/$APP_ID.svg"
    rm -rf "${XDG_CACHE_HOME:-$HOME/.cache}/depot"
    refresh_menus
    echo "Depot has been removed."
    exit 0
fi

if [ "$(id -u)" = 0 ] && [ -x target/release/depot ]; then
    # never build as root: it would leave root-owned files in target/
    echo "Using the copy that's already built."
elif [ "$(id -u)" = 0 ]; then
    echo "Build Depot as yourself first (run ./install.sh without sudo), then run this again with sudo."
    exit 1
elif pkg-config --exists gtk4 libadwaita-1 2>/dev/null && command -v cargo >/dev/null 2>&1; then
    echo "Building Depot. The first build takes a minute or two…"
    cargo build --release --locked
elif [ -x target/release/depot ]; then
    echo "GTK development files aren't installed, so using the copy that's already built."
else
    echo "Depot has to be built first, and that needs a few tools:"
    echo
    echo "  sudo apt install libgtk-4-dev libadwaita-1-dev pkg-config build-essential"
    echo "  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh   # Rust, if 'cargo' is missing"
    echo
    echo "Then run ./install.sh again."
    exit 1
fi

install -Dm755 target/release/depot "$BIN"
install -Dm644 "data/icons/hicolor/scalable/apps/$APP_ID.svg" "$ICONS/scalable/apps/$APP_ID.svg"
mkdir -p "$APPS"
# point the launcher at the installed file so it works even if ~/.local/bin isn't on PATH
sed "s|^Exec=depot\$|Exec=$BIN|" "data/$APP_ID.desktop" > "$APPS/$APP_ID.desktop"
refresh_menus

echo
echo "Depot is installed. Open it from your app menu (search for \"Depot\"), or run: $BIN"
