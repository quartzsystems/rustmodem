#!/usr/bin/env bash
# Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
# licensed under the terms of the MIT license which can be found in the
# root directory of this project.

# With no arguments: checks that the workspace version in Cargo.toml equals
# the VERSION file. With `prepare <version>`: writes that version to both.

set -eu
cd "$(dirname "$0")/.."

cargo_version() {
    # The first version line after the [workspace.package] header.
    awk '
        /^\[workspace\.package\]/ { inside = 1; next }
        /^\[/ { inside = 0 }
        inside && /^version[ \t]*=/ {
            gsub(/^version[ \t]*=[ \t]*"|"[ \t\r]*$/, "")
            print
            exit
        }
    ' Cargo.toml
}

case "${1:-check}" in
check)
    want=$(tr -d ' \r\n' < VERSION)
    got=$(cargo_version)
    if [ "$want" != "$got" ]; then
        echo "version mismatch: VERSION has $want, Cargo.toml has $got"
        exit 1
    fi
    echo "version $want"
    ;;
prepare)
    new="${2:-}"
    if ! printf '%s' "$new" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'; then
        echo "usage: ci/version.sh prepare <major.minor.patch>"
        exit 2
    fi
    printf '%s\n' "$new" > VERSION
    awk -v new="$new" '
        /^\[workspace\.package\]/ { inside = 1; print; next }
        /^\[/ { inside = 0 }
        inside && /^version[ \t]*=/ && !done {
            print "version = \"" new "\""
            done = 1
            next
        }
        { print }
    ' Cargo.toml > Cargo.toml.new
    mv Cargo.toml.new Cargo.toml
    echo "version set to $new; run cargo check to refresh Cargo.lock"
    ;;
*)
    echo "usage: ci/version.sh [check | prepare <version>]"
    exit 2
    ;;
esac
