#!/bin/sh
# Builds Coppersmith and adds it to ~/Applications, the app launcher and the
# terminal (`coppersmith`). `coppersmith uninstall` takes it out again.
set -e
cd "$(dirname "$0")/../.."
cargo build --release
./target/release/coppersmith install
