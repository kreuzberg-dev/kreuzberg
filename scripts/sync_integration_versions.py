#!/usr/bin/env python3
"""Align xberg integration packages with the core version.

The integrations under ``integrations/`` are versioned and published in lockstep
with the xberg core. This script reads the single source of truth (the root
``Cargo.toml`` version, including any ``-rc.N`` pre-release suffix) and writes it
to every integration manifest:

* the package's own ``version`` field, and
* its ``xberg`` dependency pin (floor).

Python manifests use the PEP 440 form (``1.0.0-rc.32`` -> ``1.0.0rc32``); the
Maven pom and the npm ``package.json`` manifests use the native form
(``1.0.0-rc.32``, which is also valid semver). npm packages pin ``@xberg-io/xberg``
exactly (not a floor). Run via ``task version:sync`` after ``alef sync-versions``
has synced the alef-managed binding manifests.

Usage:
    python3 scripts/sync_integration_versions.py           # apply
    python3 scripts/sync_integration_versions.py --check    # verify (CI), exit 1 on drift
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# npm integration packages (package.json): version + exact `@xberg-io/xberg` pin,
# both in the native/semver form. The committed `package-lock.json` is intentionally NOT
# synced here: the core pin resolves to an rc that is published to npm only during the same
# release run, so its integrity/resolved cannot be generated ahead of time. The npm publish +
# CI jobs use `npm install` (not `npm ci`) to reconcile the core entry against the just-published
# version while preserving third-party pins. ~keep
NPM_MANIFESTS = [
    "integrations/node/n8n-nodes-xberg/package.json",
    "integrations/node/langchain-xberg/package.json",
    "integrations/node/llamaindex-xberg/package.json",
]

# Published manifests whose own `version` is aligned with core. ~keep
VERSION_TARGETS = [
    "integrations/python/crewai/pyproject.toml",
    "integrations/python/txtai/pyproject.toml",
    "integrations/python/surrealdb/pyproject.toml",
    "integrations/python/langchain/pyproject.toml",
    "integrations/python/llama-index/readers/llama-index-readers-xberg/pyproject.toml",
    "integrations/python/llama-index/node_parsers/llama-index-node-parser-xberg/pyproject.toml",
    "integrations/java/spring-ai/pom.xml",
    *NPM_MANIFESTS,
]

# All manifests that carry an `xberg` dependency pin to sync (superset of the
# above minus node-parser, plus the llama-index dev aggregator). ~keep
XBERG_DEP_MANIFESTS = [
    "integrations/python/crewai/pyproject.toml",
    "integrations/python/txtai/pyproject.toml",
    "integrations/python/surrealdb/pyproject.toml",
    "integrations/python/langchain/pyproject.toml",
    "integrations/python/llama-index/pyproject.toml",
    "integrations/python/llama-index/readers/llama-index-readers-xberg/pyproject.toml",
    "integrations/java/spring-ai/pom.xml",
    *NPM_MANIFESTS,
]


def core_version() -> str:
    text = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    match = re.search(r'(?m)^version = "([^"]+)"', text)
    if not match:
        sys.exit("could not read version from Cargo.toml")
    return match.group(1)


def to_pep440(version: str) -> str:
    """Convert a Cargo pre-release (1.0.0-rc.32) to PEP 440 (1.0.0rc32)."""
    labels = {"alpha": "a", "beta": "b", "a": "a", "b": "b", "rc": "rc"}
    return re.sub(
        r"-(rc|alpha|beta|a|b)\.?(\d+)",
        lambda m: f"{labels[m.group(1)]}{m.group(2)}",
        version,
    )


def transform(rel: str, sync_version: bool, sync_dep: bool, maven: str, pep440: str) -> tuple[str, str]:
    path = ROOT / rel
    original = path.read_text(encoding="utf-8")
    updated = original
    if rel.endswith(".xml"):
        if sync_version:
            updated = re.sub(r"<version>[^<]*</version>", f"<version>{maven}</version>", updated, count=1)
        if sync_dep:
            updated = re.sub(
                r"<xberg\.version>[^<]*</xberg\.version>",
                f"<xberg.version>{maven}</xberg.version>",
                updated,
            )
    elif rel.endswith(".json"):
        if sync_version:
            updated = re.sub(r'(?m)^(\s*"version":\s*)"[^"]*"', rf'\g<1>"{maven}"', updated, count=1)
        if sync_dep:
            updated = re.sub(r'("@xberg-io/xberg":\s*)"[^"]*"', rf'\g<1>"{maven}"', updated)
    else:
        if sync_version:
            updated = re.sub(r'(?m)^version = "[^"]*"', f'version = "{pep440}"', updated, count=1)
        if sync_dep:
            updated = re.sub(r'(?<![\w.-])xberg>=[^"\',\]]+', f"xberg>={pep440}", updated)
    return original, updated


def main() -> int:
    check = "--check" in sys.argv[1:]
    maven = core_version()
    pep440 = to_pep440(maven)

    targets: dict[str, tuple[bool, bool]] = {}
    for rel in VERSION_TARGETS:
        targets[rel] = (True, rel in XBERG_DEP_MANIFESTS)
    for rel in XBERG_DEP_MANIFESTS:
        sync_version, _ = targets.get(rel, (False, False))
        targets[rel] = (sync_version, True)

    drift = []
    for rel, (sync_version, sync_dep) in sorted(targets.items()):
        original, updated = transform(rel, sync_version, sync_dep, maven, pep440)
        if original == updated:
            continue
        drift.append(rel)
        if not check:
            (ROOT / rel).write_text(updated, encoding="utf-8")

    if check:
        if drift:
            print(f"integration versions out of sync with core {maven}:")
            for rel in drift:
                print(f"  - {rel}")
            print("run: task version:sync")
            return 1
        print(f"integration versions in sync with core {maven}")
        return 0

    if drift:
        print(f"aligned {len(drift)} integration manifest(s) to xberg {maven} (python: {pep440})")
        for rel in drift:
            print(f"  - {rel}")
    else:
        print(f"integration manifests already at xberg {maven}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
