"""Tests for the wheel build hook that repackages the released CLI tarballs.

These cover GH#1407: the hook used to force-include only ``*.dylib`` siblings, so
every Linux shared library staged beside the binary was silently dropped and the
installed wheel resolved ``libheif.so.1`` against whatever the host had. The musl
wheel was worse — it shipped the launcher shell script alone, with neither
``xberg.bin`` nor the ``lib/`` tree the launcher execs.
"""

from __future__ import annotations

import io
import tarfile
from pathlib import Path

import pytest
from hatch_build import CustomBuildHook

_GNU_TARGET = "x86_64-unknown-linux-gnu"
_MUSL_TARGET = "x86_64-unknown-linux-musl"
_DARWIN_TARGET = "aarch64-apple-darwin"


def _write_archive(root: Path, target: str, members: dict[str, bytes]) -> None:
    """Stage a ``xberg-cli-<target>.tar.gz`` where the hook will look for it."""
    dist = root / "dist"
    dist.mkdir(parents=True, exist_ok=True)
    with tarfile.open(dist / f"xberg-cli-{target}.tar.gz", "w:gz") as archive:
        for name, payload in members.items():
            info = tarfile.TarInfo(name)
            info.size = len(payload)
            archive.addfile(info, io.BytesIO(payload))


def _build(root: Path, target: str, members: dict[str, bytes], monkeypatch) -> dict:
    _write_archive(root, target, members)
    monkeypatch.setenv("XBERG_CLI_TARGET", target)
    build_data: dict = {}
    hook = CustomBuildHook(
        str(root),
        config={},
        build_config={},
        metadata=None,
        directory=str(root / "dist"),
        target_name="wheel",
    )
    hook.initialize("1.0.0", build_data)
    return build_data


def _included(build_data: dict) -> set[str]:
    return set(build_data["force_include"].values())


def test_should_include_linux_shared_libraries_beside_the_binary(tmp_path: Path, monkeypatch) -> None:
    build_data = _build(
        tmp_path,
        _GNU_TARGET,
        {
            f"stage/{name}": data
            for name, data in (
                ("xberg", b"\x7fELF"),
                ("libheif.so.1", b"lib"),
                ("libde265.so.0", b"lib"),
            )
        },
        monkeypatch,
    )

    assert _included(build_data) == {
        f"xberg_cli/bin/{_GNU_TARGET}/xberg",
        f"xberg_cli/bin/{_GNU_TARGET}/libheif.so.1",
        f"xberg_cli/bin/{_GNU_TARGET}/libde265.so.0",
    }


def test_should_preserve_relative_paths_of_the_musl_lib_tree(tmp_path: Path, monkeypatch) -> None:
    build_data = _build(
        tmp_path,
        _MUSL_TARGET,
        {
            "stage/xberg": b"#!/bin/sh\n",
            "stage/xberg.bin": b"\x7fELF",
            "stage/lib/ld-musl-x86_64.so.1": b"ld",
            "stage/lib/libheif.so.1": b"lib",
        },
        monkeypatch,
    )

    # The launcher execs "$SCRIPT_DIR/lib/ld-musl-*.so.1", so a flattened
    # payload would install cleanly and still fail to start.
    assert f"xberg_cli/bin/{_MUSL_TARGET}/lib/ld-musl-x86_64.so.1" in _included(build_data)
    assert f"xberg_cli/bin/{_MUSL_TARGET}/xberg.bin" in _included(build_data)


def test_should_still_include_dylibs_on_macos(tmp_path: Path, monkeypatch) -> None:
    build_data = _build(
        tmp_path,
        _DARWIN_TARGET,
        {"stage/xberg": b"\xcf\xfa\xed\xfe", "stage/libheif.1.dylib": b"lib"},
        monkeypatch,
    )

    assert f"xberg_cli/bin/{_DARWIN_TARGET}/libheif.1.dylib" in _included(build_data)


def test_should_reject_a_gnu_wheel_with_no_libheif_staged(tmp_path: Path, monkeypatch) -> None:
    with pytest.raises(RuntimeError, match=r"no libheif\.so"):
        _build(tmp_path, _GNU_TARGET, {"stage/xberg": b"\x7fELF"}, monkeypatch)


def test_should_reject_a_musl_wheel_holding_only_the_launcher_script(tmp_path: Path, monkeypatch) -> None:
    # Exactly what xberg_cli-1.0.14 musllinux shipped: a 257-byte shell script
    # and nothing for it to exec.
    with pytest.raises(RuntimeError, match="musl payload incomplete"):
        _build(tmp_path, _MUSL_TARGET, {"stage/xberg": b"#!/bin/sh\nexec ...\n"}, monkeypatch)
