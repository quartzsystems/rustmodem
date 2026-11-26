#!/usr/bin/env bash
# Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
# licensed under the terms of the MIT license which can be found in the
# root directory of this project.

# Fails if any Rust source file, tracked or untracked-but-not-ignored, does
# not start with the three licence header lines. Carriage returns are
# stripped before comparing.

set -u
cd "$(dirname "$0")/.."

want='// Copyright (C), 2026 Quartz Systems. Some rights reserved. This work is
// licensed under the terms of the MIT license which can be found in the
// root directory of this project.'

bad=0
count=0
while IFS= read -r -d '' file; do
    [ -f "$file" ] || continue
    count=$((count + 1))
    got=$(head -n 3 "$file" | tr -d '\r')
    if [ "$got" != "$want" ]; then
        echo "missing licence header: $file"
        bad=1
    fi
done < <(git ls-files -z --cached --others --exclude-standard -- '*.rs')

if [ "$bad" -ne 0 ]; then
    exit 1
fi
echo "licence headers present in $count Rust files"
