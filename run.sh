#!/usr/bin/env sh
# Build (release) and launch Base Search on macOS or Linux.
#   ./run.sh        -> build and run the desktop app
#   ./run.sh cli ... -> build and run the command-line tool with arguments
#
# Requires the Rust toolchain (https://rustup.rs). On Linux you also need the
# GUI build dependencies listed in the README.
set -e
cd "$(dirname "$0")"

if ! command -v cargo >/dev/null 2>&1; then
    if [ -f "$HOME/.cargo/env" ]; then
        . "$HOME/.cargo/env"
    fi
fi
if ! command -v cargo >/dev/null 2>&1; then
    echo "Rust is not installed. Install it from https://rustup.rs and re-run." >&2
    exit 1
fi

cargo build --release

if [ "$1" = "cli" ]; then
    shift
    exec ./target/release/base-search-cli "$@"
else
    exec ./target/release/BaseSearch
fi
