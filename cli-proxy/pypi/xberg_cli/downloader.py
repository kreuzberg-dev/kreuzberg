"""Resolve, download, verify, and run the native xberg binary.

Self-healing asset discovery: query the GitHub releases API and pick the asset
whose name contains the platform target triple, instead of hardcoding one exact
asset name. This keeps working as release asset naming standardizes over time.
All diagnostics go to stderr.
"""

from __future__ import annotations

import hashlib
import json
import os
import platform
import shutil
import subprocess
import sys
import tarfile
import tempfile
import zipfile
from pathlib import Path, PurePosixPath
from urllib.error import URLError
from urllib.parse import quote, urlsplit
from urllib.request import HTTPRedirectHandler, Request, build_opener

REPO = "xberg-io/xberg"
BIN_NAME = "xberg"
PKG_NAME = "xberg-cli"
VERSION_ENV = "XBERG_CLI_VERSION"
_USER_AGENT = "xberg-cli-python-proxy"


def _target_triple() -> str:
    system = platform.system().lower()
    machine = platform.machine().lower()

    if system == "windows":
        if machine in {"amd64", "x86_64"}:
            return "x86_64-pc-windows-msvc"
        raise RuntimeError(f"unsupported Windows arch: {machine}")
    if system == "linux":
        if machine in {"amd64", "x86_64"}:
            return "x86_64-unknown-linux-gnu"
        if machine in {"aarch64", "arm64"}:
            return "aarch64-unknown-linux-gnu"
        raise RuntimeError(f"unsupported Linux arch: {machine}")
    if system == "darwin":
        if machine in {"aarch64", "arm64"}:
            return "aarch64-apple-darwin"
        if machine in {"amd64", "x86_64"}:
            return "x86_64-apple-darwin"
        raise RuntimeError(f"unsupported macOS arch: {machine}")
    raise RuntimeError(f"unsupported platform: {system} {machine}")


def _binary_name() -> str:
    return f"{BIN_NAME}.exe" if platform.system().lower() == "windows" else BIN_NAME


class _HttpsOnlyRedirectHandler(HTTPRedirectHandler):
    """Reject any redirect whose target is not https (downgrade/SSRF guard)."""

    def redirect_request(  # noqa: PLR0917 -- overrides HTTPRedirectHandler's fixed positional signature
        self, req, fp, code, msg, headers, newurl
    ):  # type: ignore[no-untyped-def]
        if urlsplit(newurl).scheme.lower() != "https":
            raise URLError(f"refusing non-https redirect to: {newurl}")
        return super().redirect_request(req, fp, code, msg, headers, newurl)


_opener = build_opener(_HttpsOnlyRedirectHandler())


def _http_get(url: str, accept: str | None = None) -> bytes:
    if urlsplit(url).scheme.lower() != "https":
        raise RuntimeError(f"refusing non-https URL: {url}")
    headers = {"User-Agent": _USER_AGENT}
    if accept:
        headers["Accept"] = accept
    request = Request(url, headers=headers)
    try:
        with _opener.open(request, timeout=60) as response:
            if response.status != 200:
                raise RuntimeError(f"HTTP {response.status} for {url}")
            return response.read()
    except URLError as exc:
        raise RuntimeError(f"failed to fetch {url}: {exc}") from exc


def _asset_score(name: str) -> int:
    lowered = name.lower()
    score = 0
    if BIN_NAME.lower() in lowered:
        score += 2
    if "cli" in lowered:
        score += 1
    return score


def _resolve_release() -> tuple[str, dict, dict | None]:
    """Return (tag, archive_asset, checksums_asset_or_none) for this platform."""
    triple = _target_triple()
    pinned = os.getenv(VERSION_ENV)
    if pinned:
        api_url = f"https://api.github.com/repos/{REPO}/releases/tags/{quote(pinned, safe='')}"
    else:
        api_url = f"https://api.github.com/repos/{REPO}/releases/latest"

    release = json.loads(_http_get(api_url, accept="application/vnd.github+json"))
    assets = release.get("assets") or []
    tag = release.get("tag_name") or pinned or "latest"

    archives = [
        a
        for a in assets
        if triple in (a.get("name") or "").lower()
        and ((a.get("name") or "").lower().endswith(".tar.gz") or (a.get("name") or "").lower().endswith(".zip"))
    ]
    if not archives:
        raise RuntimeError(f'no release asset matching target triple "{triple}" in {REPO} release {tag}')
    archives.sort(key=lambda a: _asset_score(a.get("name") or ""), reverse=True)
    archive = archives[0]

    checksums = next((a for a in assets if "SHA256SUMS" in (a.get("name") or "").upper()), None)
    return tag, archive, checksums


def _expected_digest(text: str, asset_name: str) -> str | None:
    for line in text.splitlines():
        stripped = line.strip()
        if not stripped:
            continue
        parts = stripped.split()
        if len(parts) < 2:
            continue
        name = parts[-1].lstrip("*")
        if name == asset_name:
            return parts[0].lower()
    return None


def _verify_or_warn(archive_path: Path, asset_name: str, checksums: dict | None) -> None:
    if not checksums:
        print(
            f"WARNING: no SHA256SUMS asset found for {asset_name}; "
            "installing over HTTPS without checksum verification.",
            file=sys.stderr,
        )
        return
    sums_text = _http_get(checksums["browser_download_url"]).decode("utf-8")
    expected = _expected_digest(sums_text, asset_name)
    if not expected:
        raise RuntimeError(
            f"no checksum entry for {asset_name} in {checksums['name']} — refusing to install unverified binary"
        )
    digest = hashlib.sha256(archive_path.read_bytes()).hexdigest().lower()
    if digest != expected:
        raise RuntimeError(f"checksum mismatch for {asset_name} (expected {expected}, got {digest})")
    print(f"Checksum verified for {asset_name}.", file=sys.stderr)


def _is_within(base: Path, target: Path) -> bool:
    """True if `target`'s resolved path stays inside `base`."""
    base_resolved = base.resolve()
    try:
        target.resolve().relative_to(base_resolved)
    except ValueError:
        return False
    return True


def _reject_unsafe_member(name: str) -> None:
    """Reject absolute paths and any `..` component (zip-slip / tar-slip)."""
    normalized = name.replace("\\", "/")
    pure = PurePosixPath(normalized)
    if pure.is_absolute() or normalized.startswith("/"):
        raise RuntimeError(f"refusing absolute path in archive: {name}")
    if len(normalized) >= 2 and normalized[1] == ":":
        raise RuntimeError(f"refusing absolute path in archive: {name}")
    if any(part == ".." for part in pure.parts):
        raise RuntimeError(f"refusing parent-directory escape in archive: {name}")


def _safe_extract(archive_path: Path, asset_name: str, dest: Path) -> None:
    """Extract a tar/zip, validating every member stays within `dest`.

    Does not rely on extractall() defaults or the 3.12-only ``filter='data'``:
    each member name is rejected up front if it is absolute or contains ``..``,
    and the resolved destination path is re-checked against ``dest``.
    """
    dest = dest.resolve()
    dest.mkdir(parents=True, exist_ok=True)

    if asset_name.lower().endswith(".zip"):
        with zipfile.ZipFile(archive_path) as zf:
            for member in zf.namelist():
                _reject_unsafe_member(member)
                if not _is_within(dest, dest / member):
                    raise RuntimeError(f"refusing archive entry escaping dest: {member}")
            for member in zf.namelist():
                zf.extract(member, dest)
    else:
        with tarfile.open(archive_path, "r:gz") as tf:
            for member in tf.getmembers():
                _reject_unsafe_member(member.name)
                if not _is_within(dest, dest / member.name):
                    raise RuntimeError(f"refusing archive entry escaping dest: {member.name}")
                if member.islnk() or member.issym():
                    link_target = dest / member.name
                    if not _is_within(dest, link_target.parent / member.linkname):
                        raise RuntimeError(f"refusing link escaping dest: {member.name}")
            tf.extractall(dest)  # noqa: S202 (members validated above)


def _find_binary(root: Path, name: str) -> Path | None:
    for candidate in root.rglob(name):
        if candidate.is_file():
            return candidate
    return None


def _cache_dir(tag: str) -> Path:
    cache = Path.home() / ".cache" / PKG_NAME / tag
    cache.mkdir(parents=True, exist_ok=True, mode=0o700)
    return cache


def ensure_binary() -> str:
    """Ensure the native binary exists locally, downloading if necessary."""
    override = os.getenv("XBERG_BINARY")
    if override:
        return override

    tag, archive, checksums = _resolve_release()
    cache = _cache_dir(tag)
    binary_path = cache / _binary_name()
    if binary_path.exists() and os.access(binary_path, os.X_OK):
        return str(binary_path)

    print(f"Downloading {BIN_NAME} {tag} asset {archive['name']}...", file=sys.stderr)
    with tempfile.TemporaryDirectory() as tmpdir:
        tmp = Path(tmpdir)
        archive_path = tmp / Path(archive["name"]).name
        archive_path.write_bytes(_http_get(archive["browser_download_url"]))
        _verify_or_warn(archive_path, archive["name"], checksums)
        extract_dir = tmp / "extract"
        extract_dir.mkdir()
        _safe_extract(archive_path, archive["name"], extract_dir)
        found = _find_binary(extract_dir, _binary_name())
        if not found:
            raise RuntimeError(f"binary {_binary_name()} not found after extracting {archive['name']}")
        shutil.move(str(found), str(binary_path))

    if platform.system().lower() != "windows":
        binary_path.chmod(0o755)
    print(f"{BIN_NAME} installed.", file=sys.stderr)
    return str(binary_path)


def run(args: list[str]) -> int:
    """Run the native binary with the given args, returning its exit code."""
    binary_path = ensure_binary()
    completed = subprocess.run([binary_path, *args], check=False)
    return completed.returncode
