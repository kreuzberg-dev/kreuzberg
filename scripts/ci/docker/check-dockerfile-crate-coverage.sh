#!/usr/bin/env bash
# Fails fast (no docker build required) when a Cargo workspace member under
# crates/ is neither COPYd into a docker/Dockerfile.* build stage nor
# explicitly stripped from that Dockerfile's copy of Cargo.toml via the
# `sed -i '/<crate>/d; ...' Cargo.toml` exclusion line.
#
# Regression guard for #325: `crates/ttf-parser-compat` was added as a
# workspace member but never added to any Dockerfile's COPY list, so cargo
# failed to load its manifest inside the Docker build context (which only
# contains the crates each Dockerfile explicitly COPYs) while every
# non-Docker CI leg — which checks out the full repo — stayed green.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

mapfile -t members < <(grep -oE '"crates/[a-zA-Z0-9_-]+"' Cargo.toml | tr -d '"' | sed 's#^crates/##')

status=0

for dockerfile in docker/Dockerfile.*; do
    [ -f "$dockerfile" ] || continue

    for crate in "${members[@]}"; do
        if grep -q "COPY crates/${crate}/ " "$dockerfile"; then
            continue
        fi
        if grep "sed -i" "$dockerfile" | grep -q -- "${crate}"; then
            continue
        fi
        echo "::error file=${dockerfile}::workspace member 'crates/${crate}' is neither COPYd nor sed-excluded"
        status=1
    done
done

if [ "$status" -eq 0 ]; then
    echo "All Cargo workspace members are covered by every docker/Dockerfile.*"
fi

exit "$status"
