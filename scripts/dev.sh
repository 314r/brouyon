#!/bin/sh
# Builds Brouyon and runs it on throwaway notes and settings, which stay in
# /tmp/brouyon-dev between runs. It can't touch your real ones, because Settings
# writes to whichever config folder it is given. Rust has to be built before the
# folders change: mise keeps it under XDG_DATA_HOME.
set -eu

cd "$(dirname "$0")/.."
dev=/tmp/brouyon-dev

cargo build

# The font comes from fontconfig, which looks in these same folders.
mkdir -p "$dev/config" "$dev/data"
ln -sfn "${XDG_CONFIG_HOME:-$HOME/.config}/fontconfig" "$dev/config/fontconfig"
ln -sfn "${XDG_DATA_HOME:-$HOME/.local/share}/fonts" "$dev/data/fonts"

XDG_CONFIG_HOME="$dev/config" XDG_DATA_HOME="$dev/data" exec target/debug/brouyon
