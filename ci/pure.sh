#!/usr/bin/env bash
# Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
# licensed under the terms of the MIT license which can be found in the
# root directory of this project.

# Fails if Tokio is anywhere in the dependency graph of a crate that must
# not have it: the pure crates and the media threads, with their tests,
# and the clients of the control API, which keep the blocking pipe client.
# docs/RUNTIME.md says where Tokio may be and why it may not be here.

set -u
cd "$(dirname "$0")/.."

# The pure crates, and `media`, `telephony` and `sippeer`, which use
# sockets and threads but no runtime. Their tests are looked at too.
pure="g711 dspkit tones chansim engine at serial sdp rtp sip msg config ctlproto pool media telephony sippeer glyph model"
# The manager and its wrappers over Win32.
clients="win32 manager"
# rustmodemctl itself. Its tests run the commands against the real control
# server, which is on Tokio, so only what the program is built from is
# looked at.
programs="ctl"

bad=0
check() {
    local crate=$1 edges=$2
    local found
    # Every target, so that what only Windows pulls in is seen on Linux.
    found=$(cargo tree -p "$crate" --edges "$edges" --target all --prefix none 2>/dev/null \
        | grep -E '^tokio' | sort -u)
    if [ -n "$found" ]; then
        echo "$crate depends on Tokio:"
        echo "$found" | sed 's/^/    /'
        bad=1
    fi
}

for crate in $pure $clients; do
    check "$crate" normal,build,dev
done
for crate in $programs; do
    check "$crate" normal,build
done

if [ "$bad" -ne 0 ]; then
    exit 1
fi
echo "no Tokio in: $pure $clients $programs"
