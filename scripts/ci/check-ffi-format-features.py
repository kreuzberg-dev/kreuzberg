#!/usr/bin/env python3
"""Guard the native FFI build against silently losing document formats.

GH#1387 / task #339. `crates/xberg-ffi/Cargo.toml` selects the core crate's
features by hand rather than via `full`, because `cf7fa0533d` had to drop
`full` to keep `libheif-sys`/candle out of Swift's cargo-zigbuild Linux
targets. That hand-written list silently lost eight formats, and every
non-Windows .NET / Go / Java-native / C-tarball artifact shipped without them.

`a0583843a3` put the formats back in the manifest -- but that manifest is
alef-GENERATED, and alef matches the LITERAL feature strings in `alef.toml`.
Because the fix was applied only to the generated output, the very next regen
rewrote the manifest without them and reverted the fix, invisibly. Nothing in
CI could have caught it: `alef verify` compares an inputs hash, not content,
so it is green on a manifest that is faithfully generated from a wrong source.

This script closes that loop by asserting BOTH ends agree:

  1. `alef.toml`'s ffi feature list names every required format leaf, so a
     regen cannot drop them (the source-side fix), and
  2. the generated `crates/xberg-ffi/Cargo.toml` actually carries them right
     now (the output-side fix), so a stale or hand-reverted manifest is caught
     even before a regen runs.

Checking only (1) would pass on a tree whose committed manifest is stale;
checking only (2) would pass on a tree that is one regen away from breaking.
The bug this script exists to prevent lived exactly in the gap between them.

`full` must stay ABSENT from the ffi list -- see `cf7fa0533d`. If a future
change adds it back, the formats are covered transitively and this check is
satisfied, but the Swift Linux cross-build breaks instead; the assertion below
reports that case distinctly rather than silently passing.

Exit codes: 0 = all good, 1 = a required format is missing, 2 = malformed input.
"""

from __future__ import annotations

import sys
from pathlib import Path

import tomllib

# The eight formats `cf7fa0533d` dropped when it replaced `features = ["full", ...]`
# with a hand-maintained list. These are the ones GH#1387 reported as missing from
# the shipped native artifacts; keep this list in sync with that issue, not with
# whatever happens to be in the manifest today. ~keep
REQUIRED_FORMAT_FEATURES = (
    "excel",
    "hwp",
    "hwpx",
    "iwork",
    "wordperfect",
    "mdx",
    "xml",
    "qr-codes",
)

FFI_MANIFEST = Path("crates/xberg-ffi/Cargo.toml")


def _fail(message: str) -> None:
    print(f"FAIL: {message}", file=sys.stderr)


def alef_crate_features(alef_toml: Path, table: str) -> list[str]:
    """Return the feature list alef will generate `[crates.<table>]`'s manifest from."""
    with alef_toml.open("rb") as handle:
        config = tomllib.load(handle)

    crates = config.get("crates")
    if not isinstance(crates, list) or not crates:
        raise ValueError(f"{alef_toml}: expected a non-empty [[crates]] array")

    entry = crates[0].get(table)
    if not isinstance(entry, dict):
        raise ValueError(f"{alef_toml}: crates[0] has no [crates.{table}] table")

    features = entry.get("features")
    if not isinstance(features, list):
        raise ValueError(f"{alef_toml}: crates[0].{table} has no `features` list")

    return [f for f in features if isinstance(f, str)]


def manifest_core_features(manifest: Path) -> tuple[str, list[str]]:
    """Return the (cfg, features) of the ffi manifest's desktop `xberg` dependency.

    The manifest declares `xberg` once per platform under `[target.'cfg(..)'.dependencies]`.
    Only the desktop entry -- the `cfg(not(any(android, ios, windows, macos-x86_64)))`
    catch-all -- carries the format features; the mobile and Windows entries deliberately
    request a single narrow target feature instead. Picking the wrong one silently makes
    this check vacuous, so select the entry with the largest feature list and report which
    cfg it came from.
    """
    with manifest.open("rb") as handle:
        config = tomllib.load(handle)

    candidates: list[tuple[str, list[str]]] = []
    for cfg, table in config.get("target", {}).items():
        core = table.get("dependencies", {}).get("xberg")
        if isinstance(core, dict) and isinstance(core.get("features"), list):
            candidates.append((cfg, [f for f in core["features"] if isinstance(f, str)]))

    core = config.get("dependencies", {}).get("xberg")
    if isinstance(core, dict) and isinstance(core.get("features"), list):
        candidates.append(("[dependencies]", [f for f in core["features"] if isinstance(f, str)]))

    if not candidates:
        raise ValueError(f"{manifest}: found no `xberg` dependency with a `features` list")

    return max(candidates, key=lambda pair: len(pair[1]))


def check(alef_toml: Path) -> int:
    try:
        source_features = alef_crate_features(alef_toml, "ffi")
        # The swift crate keeps its own hand-maintained copy of the same list. It has no
        # output-side counterpart to check: the generated packages/swift/rust/Cargo.toml reaches
        # the formats through `ffi_features = ["full-no-heic"]` on the xberg-ffi edge, not through
        # a granular list. Guarding the source side is what stops the two drifting -- swift was
        # missing `wordperfect` while ffi had all eight, and this script did not catch it because
        # it only ever looked at ffi. ~keep
        swift_features = alef_crate_features(alef_toml, "swift")
        desktop_cfg, generated_features = manifest_core_features(FFI_MANIFEST)
    except (OSError, ValueError, tomllib.TOMLDecodeError) as error:
        _fail(str(error))
        return 2

    failed = False

    ffi_consequence = (
        "Every native artifact built from this crate (Go, the C tarball, Java natives, "
        ".NET natives) ships without those formats -- this is GH#1387."
    )
    swift_consequence = (
        "The swift crate keeps its own copy of this list. A missing leaf is inert for codegen "
        "today (nothing in the parsed sources is gated on these literals) and the formats still "
        'reach the build through `ffi_features = ["full-no-heic"]` -- but that makes the dep '
        "array correct only by accident of what the ffi edge carries. Narrow that edge the way "
        "cf7fa0533d narrowed `full` and swift silently loses the format while the format table "
        "keeps advertising it."
    )

    for label, features, path, consequence in (
        ("alef.toml crates[0].ffi.features", source_features, alef_toml, ffi_consequence),
        ("alef.toml crates[0].swift.features", swift_features, alef_toml, swift_consequence),
        (
            f"{FFI_MANIFEST} xberg.features under target.'{desktop_cfg}'",
            generated_features,
            FFI_MANIFEST,
            ffi_consequence,
        ),
    ):
        if "full" in features:
            _fail(
                f"{label} lists `full`. That covers the formats transitively, but "
                f"cf7fa0533d removed it deliberately: `full` pulls in libheif-sys and "
                f"candle, which break Swift's cargo-zigbuild Linux targets. Restore the "
                f"granular list instead ({path})."
            )
            failed = True
            continue

        missing = [f for f in REQUIRED_FORMAT_FEATURES if f not in features]
        if missing:
            _fail(
                f"{label} is missing {len(missing)} format feature(s): "
                f"{', '.join(missing)}. {consequence} Add them to {path}."
            )
            failed = True

    if failed:
        print(
            "\nNote: both ends must carry the formats. alef.toml is the source a regen "
            "reads; crates/xberg-ffi/Cargo.toml is what actually builds today. Fixing "
            "only the generated manifest is reverted by the next regen.",
            file=sys.stderr,
        )
        return 1

    print(
        f"OK: all {len(REQUIRED_FORMAT_FEATURES)} required format features present in "
        f"alef.toml's ffi and swift lists and in {FFI_MANIFEST}"
    )
    return 0


def main() -> int:
    alef_toml = Path(sys.argv[1]) if len(sys.argv) > 1 else Path("alef.toml")
    if not alef_toml.is_file():
        _fail(f"{alef_toml}: not found (run from the repository root)")
        return 2
    return check(alef_toml)


if __name__ == "__main__":
    sys.exit(main())
