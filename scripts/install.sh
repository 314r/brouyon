#!/bin/sh
# Builds Brouyon and installs it for the current user. Run it again to update.
set -eu

cd "$(dirname "$0")/.."
data="${XDG_DATA_HOME:-$HOME/.local/share}"

cargo build --release
install -Dm755 target/release/brouyon "$HOME/.local/bin/brouyon"
install -Dm644 assets/brouyon.desktop "$data/applications/brouyon.desktop"
install -Dm644 assets/brouyon.svg "$data/icons/hicolor/scalable/apps/brouyon.svg"
if command -v update-desktop-database >/dev/null; then
    update-desktop-database "$data/applications"
fi

echo "Installed. Brouyon is in the Omarchy launcher, or run: brouyon"
