"""Custom hatchling build hook that bundles the native xberg binary into a wheel.

When building a platform-specific wheel in CI, the target triple is supplied via
the ``XBERG_CLI_TARGET`` env var (the build host is always linux/amd64, so the
triple cannot be inferred from ``platform.*``). The matching
``xberg-cli-<target>.tar.gz`` / ``.zip`` is located (repo root or ``dist/``),
the binary is extracted into ``xberg_cli/bin/<target>/``, force-included in the
wheel, and the wheel is tagged for that platform so PyPI serves the right artifact.

If no target/binary is found (e.g. the sdist build, or an unknown platform), the
hook is a no-op and the package falls back to the runtime downloader in
``xberg_cli/downloader.py`` (see ``cli.py``).
"""

from __future__ import annotations

import os
import shutil
import tarfile
import zipfile
from pathlib import Path

from hatchling.builders.hooks.plugin.interface import BuildHookInterface

_TAG_MAP = {
    "x86_64-pc-windows-msvc": "win_amd64",
    "x86_64-unknown-linux-gnu": "manylinux_2_28_x86_64",
    "aarch64-unknown-linux-gnu": "manylinux_2_28_aarch64",
    "x86_64-unknown-linux-musl": "musllinux_1_2_x86_64",
    "aarch64-unknown-linux-musl": "musllinux_1_2_aarch64",
    "aarch64-apple-darwin": "macosx_11_0_arm64",
    "x86_64-apple-darwin": "macosx_11_0_x86_64",
}


def _safe_destination(root: Path, member_name: str) -> Path:
    """Resolve an archive member path and require it to stay under root."""
    root = root.resolve()
    target = (root / member_name.replace("\\", "/")).resolve(strict=False)
    if target != root and not target.is_relative_to(root):
        raise RuntimeError(f"archive member escapes extraction directory: {member_name}")
    return target


def _extract_zip_bounded(archive: Path, extract_dir: Path) -> None:
    """Extract zip entries after bounding each destination path."""
    with zipfile.ZipFile(archive) as zf:
        for member in zf.infolist():
            target = _safe_destination(extract_dir, member.filename)
            if member.is_dir():
                target.mkdir(parents=True, exist_ok=True)
                continue

            target.parent.mkdir(parents=True, exist_ok=True)
            with zf.open(member) as source, target.open("wb") as destination:
                shutil.copyfileobj(source, destination)


def _extract_tar_bounded(archive: Path, extract_dir: Path) -> None:
    """Extract regular tar entries after bounding each destination path."""
    with tarfile.open(archive, "r:gz") as tf:
        for member in tf.getmembers():
            target = _safe_destination(extract_dir, member.name)
            if member.isdir():
                target.mkdir(parents=True, exist_ok=True)
                continue
            if not member.isfile():
                raise RuntimeError(f"unsupported archive member type: {member.name}")

            target.parent.mkdir(parents=True, exist_ok=True)
            source = tf.extractfile(member)
            if source is None:
                raise RuntimeError(f"could not read archive member: {member.name}")
            with source, target.open("wb") as destination:
                shutil.copyfileobj(source, destination)


class CustomBuildHook(BuildHookInterface):
    """Inject the matching native binary into a platform-tagged wheel."""

    PLUGIN_NAME = "custom"

    def initialize(self, version: str, build_data: dict) -> None:  # noqa: ARG002
        """Bundle a staged native binary when building a targeted wheel."""
        target = os.environ.get("XBERG_CLI_TARGET", "").strip()
        if not target:
            return

        archive = self._find_archive(target)
        if archive is None:
            raise RuntimeError(
                f"XBERG_CLI_TARGET={target} but no xberg-cli-{target}.(tar.gz|zip) "
                f"found in repo root or dist/; refusing to build an empty platform wheel."
            )

        wheel_tag = _TAG_MAP.get(target)
        if wheel_tag is None:
            raise RuntimeError(f"no wheel platform tag mapped for target {target}")

        binary = self._extract_binary(archive, target)

        relative = f"xberg_cli/bin/{target}/{binary.name}"
        force_include = build_data.setdefault("force_include", {})
        force_include[str(binary)] = relative

        self._include_staged_payload(force_include, binary, target)

        build_data["pure_python"] = False
        build_data["infer_tag"] = False
        build_data["tag"] = f"py3-none-{wheel_tag}"

    def _include_staged_payload(self, force_include: dict[str, str], binary: Path, target: str) -> None:
        """Force-include every file the release archive staged beside the binary.

        The released tarballs already ship the binary's complete native dependency
        closure next to it: ``libheif.so*`` and friends on linux-gnu (found via an
        ``$ORIGIN`` rpath), ``*.dylib`` on macOS (``@loader_path``), and on musl an
        ``xberg.bin`` plus a ``lib/`` tree that the ``xberg`` launcher script execs
        by relative path. Copying the staged tree wholesale, rather than globbing a
        single suffix, is what stops a newly added dependency from being dropped
        from the wheel without anyone noticing.
        """
        stage_dir = binary.parent
        for path in sorted(stage_dir.rglob("*")):
            if not path.is_file() or path == binary:
                continue
            relative_path = path.relative_to(stage_dir).as_posix()
            force_include[str(path)] = f"xberg_cli/bin/{target}/{relative_path}"

        self._verify_staged_payload(stage_dir, target)

    def _verify_staged_payload(self, stage_dir: Path, target: str) -> None:
        """Refuse to build a wheel whose platform payload is incomplete.

        A wheel missing these files still builds, installs, and only then fails at
        run time by resolving against whatever the host happens to have installed —
        the failure reported in GH#1407. These checks mirror the tarball guards in
        ``.github/workflows/publish.yaml`` so the wheel cannot drift away from the
        artifact it repackages.
        """
        if target.endswith("-unknown-linux-gnu") and not list(stage_dir.glob("libheif.so*")):
            raise RuntimeError(
                f"no libheif.so* staged beside the binary for {target}; the wheel would "
                f"fall through to the system libheif at run time. Contents: "
                f"{sorted(path.name for path in stage_dir.iterdir())}"
            )

        if target.endswith("-unknown-linux-musl"):
            missing = [name for name in ("xberg.bin", "lib") if not (stage_dir / name).exists()]
            if missing:
                raise RuntimeError(
                    f"musl payload incomplete for {target}, missing {missing}; the staged "
                    f"'xberg' is only a launcher script that execs lib/ld-musl-*.so.1 and "
                    f"xberg.bin, so a wheel without them cannot run. Contents: "
                    f"{sorted(path.name for path in stage_dir.iterdir())}"
                )

    def _find_archive(self, target: str) -> Path | None:
        root = Path(self.root)
        repo_root = root.parent.parent
        for base in (repo_root, repo_root / "dist", root, root / "dist"):
            for ext in ("tar.gz", "zip"):
                candidate = base / f"xberg-cli-{target}.{ext}"
                if candidate.is_file():
                    return candidate
        return None

    def _extract_binary(self, archive: Path, target: str) -> Path:
        is_windows = target.endswith("windows-msvc")
        binary_name = "xberg.exe" if is_windows else "xberg"
        extract_dir = Path(self.root) / ".build-extract" / target
        if extract_dir.exists():
            shutil.rmtree(extract_dir, ignore_errors=True)
        extract_dir.mkdir(parents=True, exist_ok=True)

        if str(archive).lower().endswith(".zip"):
            _extract_zip_bounded(archive, extract_dir)
        else:
            _extract_tar_bounded(archive, extract_dir)

        for candidate in extract_dir.rglob(binary_name):
            if candidate.is_file():
                candidate.chmod(0o755)
                return candidate
        raise RuntimeError(f"binary {binary_name} not found inside {archive.name}")
