#!/usr/bin/env python3

import argparse
import ctypes
import hashlib
import json
import os
import re
import shutil
import sys
import tarfile
from pathlib import Path, PurePosixPath

ACTIVE_SIZE_TIERS = frozenset({"smoke", "core"})
AT_FDCWD = -100
HASH_CHUNK_SIZE = 1024 * 1024
HASH_PATTERN = re.compile(r"[0-9a-f]{64}")
RENAME_EXCHANGE = 2
REQUIRED_HASH_FIELDS = ("pdf_sha256", "out_gt_md_sha256", "gt_txt_sha256")


def _reference_documents(manifest_path: Path) -> list[dict[str, str]]:
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    if not isinstance(manifest, dict):
        raise ValueError("corpus manifest must be an object")
    documents = manifest.get("documents")
    if not isinstance(documents, list):
        raise ValueError("corpus manifest must contain a documents array")

    selected: list[dict[str, str]] = []
    seen_ids: set[str] = set()
    for document in documents:
        if not isinstance(document, dict):
            raise ValueError("corpus manifest documents must be objects")
        if (
            document.get("redistribute") != "reference"
            or document.get("gate_verdict") != "ACCEPT"
            or document.get("size_tier") not in ACTIVE_SIZE_TIERS
        ):
            continue

        document_id = document.get("id")
        if (
            not isinstance(document_id, str)
            or not document_id
            or Path(document_id).name != document_id
            or document_id in {".", ".."}
        ):
            raise ValueError(f"invalid reference document id: {document_id!r}")
        if document_id in seen_ids:
            raise ValueError(f"duplicate reference document id: {document_id}")
        seen_ids.add(document_id)

        record = {"id": document_id}
        for field in REQUIRED_HASH_FIELDS:
            value = document.get(field)
            if not isinstance(value, str) or HASH_PATTERN.fullmatch(value) is None:
                raise ValueError(f"{document_id}: invalid or missing {field}")
            record[field] = value
        selected.append(record)

    if not selected:
        raise ValueError("corpus manifest contains no active reference documents")
    return sorted(selected, key=lambda record: record["id"])


def manifest_digest(manifest_path: Path) -> str:
    payload = json.dumps(
        _reference_documents(manifest_path),
        sort_keys=True,
        separators=(",", ":"),
    ).encode()
    return hashlib.sha256(payload).hexdigest()


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as file:
        for chunk in iter(lambda: file.read(HASH_CHUNK_SIZE), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _expected_files(manifest_path: Path) -> dict[Path, str]:
    expected: dict[Path, str] = {}
    for document in _reference_documents(manifest_path):
        document_id = document["id"]
        expected[Path("pdf") / f"{document_id}.pdf"] = document["pdf_sha256"]
        expected[Path("ground_truth/pdf") / f"{document_id}.md"] = document["out_gt_md_sha256"]
        expected[Path("ground_truth/pdf") / f"{document_id}.txt"] = document["gt_txt_sha256"]
    return expected


def verify_cache(manifest_path: Path, cache_root: Path) -> int:
    expected = _expected_files(manifest_path)
    actual: set[Path] = set()
    for relative_root in (Path("pdf"), Path("ground_truth/pdf")):
        target_root = cache_root / relative_root
        if target_root.is_symlink():
            raise ValueError(f"cache contains a symbolic link: {relative_root}")
        for path in target_root.rglob("*"):
            if path.is_symlink():
                raise ValueError(f"cache contains a symbolic link: {path.relative_to(cache_root)}")
            if path.is_file():
                actual.add(path.relative_to(cache_root))

    missing = sorted(expected.keys() - actual)
    extra = sorted(actual - expected.keys())
    if missing:
        raise ValueError(f"cache is missing expected file: {missing[0]}")
    if extra:
        raise ValueError(f"cache contains unexpected file: {extra[0]}")

    for relative_path, expected_digest in expected.items():
        actual_digest = _sha256_file(cache_root / relative_path)
        if actual_digest != expected_digest:
            raise ValueError(
                f"cache digest mismatch for {relative_path}: expected {expected_digest}, got {actual_digest}"
            )
    return len(expected) // 3


def _is_legacy_appledouble(path: PurePosixPath) -> bool:
    return len(path.parts) > 1 and path.parts[0] == ".corpus-cache" and path.name.startswith("._")


def _expected_archive_files(manifest_path: Path) -> dict[PurePosixPath, str]:
    return {
        PurePosixPath(".corpus-cache") / PurePosixPath(path.as_posix()): digest
        for path, digest in _expected_files(manifest_path).items()
    }


def verify_archive(
    manifest_path: Path,
    archive_path: Path,
    allow_legacy_appledouble: bool = False,
) -> int:
    expected_files = _expected_archive_files(manifest_path)
    expected_directories = {
        parent for path in expected_files for parent in path.parents if parent != PurePosixPath(".")
    }
    seen: set[PurePosixPath] = set()

    with tarfile.open(archive_path, mode="r:") as archive:
        for member in archive:
            path = PurePosixPath(member.name.removeprefix("./"))
            if path.is_absolute() or ".." in path.parts or path in seen:
                raise ValueError(f"unsafe or duplicate archive member: {member.name}")
            seen.add(path)
            if member.isdir():
                if path not in expected_directories:
                    raise ValueError(f"unexpected archive directory: {member.name}")
            elif member.isfile():
                if path not in expected_files:
                    if allow_legacy_appledouble and _is_legacy_appledouble(path):
                        continue
                    raise ValueError(f"unexpected archive file: {member.name}")
                source = archive.extractfile(member)
                if source is None:
                    raise ValueError(f"could not read archive member: {member.name}")
                digest = hashlib.sha256()
                with source:
                    while chunk := source.read(HASH_CHUNK_SIZE):
                        digest.update(chunk)
                if digest.hexdigest() != expected_files[path]:
                    raise ValueError(f"archive digest mismatch for {member.name}")
            else:
                raise ValueError(f"unsupported archive member type: {member.name}")

    missing = sorted(expected_files.keys() - seen)
    if missing:
        raise ValueError(f"archive is missing expected file: {missing[0]}")
    return len(expected_files) // 3


def atomic_swap(current: Path, replacement: Path) -> None:
    if not replacement.is_dir() or replacement.is_symlink():
        raise ValueError(f"replacement cache is not a real directory: {replacement}")
    if not current.exists():
        replacement.rename(current)
        return
    if not current.is_dir() or current.is_symlink():
        raise ValueError(f"current cache is not a real directory: {current}")
    if current.parent != replacement.parent:
        raise ValueError("cache directories must be siblings for an atomic swap")
    if current.stat().st_dev != replacement.stat().st_dev:
        raise ValueError("cache directories must be on the same filesystem")

    library = ctypes.CDLL(None, use_errno=True)
    current_path = os.fsencode(current)
    replacement_path = os.fsencode(replacement)
    try:
        if sys.platform.startswith("linux"):
            rename = library.renameat2
            rename.argtypes = [
                ctypes.c_int,
                ctypes.c_char_p,
                ctypes.c_int,
                ctypes.c_char_p,
                ctypes.c_uint,
            ]
            result = rename(
                AT_FDCWD,
                current_path,
                AT_FDCWD,
                replacement_path,
                RENAME_EXCHANGE,
            )
        elif sys.platform == "darwin":
            rename = library.renamex_np
            rename.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_uint]
            result = rename(current_path, replacement_path, RENAME_EXCHANGE)
        else:
            raise ValueError(f"atomic directory exchange is unsupported on {sys.platform}")
    except AttributeError as error:
        raise ValueError(f"atomic directory exchange is unavailable on {sys.platform}") from error
    if result != 0:
        error_number = ctypes.get_errno()
        raise OSError(error_number, os.strerror(error_number))


def extract_archive(
    manifest_path: Path,
    archive_path: Path,
    destination: Path,
    allow_legacy_appledouble: bool = False,
) -> int:
    count = verify_archive(manifest_path, archive_path, allow_legacy_appledouble)
    expected_files = _expected_archive_files(manifest_path)
    destination.mkdir(parents=True, exist_ok=True)
    if any(destination.iterdir()):
        raise ValueError(f"archive extraction destination is not empty: {destination}")

    with tarfile.open(archive_path, mode="r:") as archive:
        for member in archive:
            path = PurePosixPath(member.name.removeprefix("./"))
            target = destination.joinpath(*path.parts)
            if member.isdir():
                target.mkdir(parents=True, exist_ok=True)
                continue
            if allow_legacy_appledouble and path not in expected_files and _is_legacy_appledouble(path):
                continue
            source = archive.extractfile(member)
            if source is None:
                raise ValueError(f"could not read archive member: {member.name}")
            target.parent.mkdir(parents=True, exist_ok=True)
            with source, target.open("xb") as output:
                shutil.copyfileobj(source, output)
    return count


def _parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    subcommands = parser.add_subparsers(dest="command", required=True)

    digest_parser = subcommands.add_parser("digest")
    digest_parser.add_argument("--manifest", type=Path, required=True)

    verify_parser = subcommands.add_parser("verify")
    verify_parser.add_argument("--manifest", type=Path, required=True)
    verify_parser.add_argument("--cache-root", type=Path, required=True)

    archive_parser = subcommands.add_parser("verify-archive")
    archive_parser.add_argument("--manifest", type=Path, required=True)
    archive_parser.add_argument("--archive", type=Path, required=True)
    archive_parser.add_argument("--allow-legacy-appledouble", action="store_true")

    extract_parser = subcommands.add_parser("extract-archive")
    extract_parser.add_argument("--manifest", type=Path, required=True)
    extract_parser.add_argument("--archive", type=Path, required=True)
    extract_parser.add_argument("--destination", type=Path, required=True)
    extract_parser.add_argument("--allow-legacy-appledouble", action="store_true")

    swap_parser = subcommands.add_parser("atomic-swap")
    swap_parser.add_argument("--current", type=Path, required=True)
    swap_parser.add_argument("--replacement", type=Path, required=True)
    return parser.parse_args()


def main() -> int:
    args = _parse_args()
    try:
        if args.command == "digest":
            print(manifest_digest(args.manifest))
        elif args.command == "verify":
            count = verify_cache(args.manifest, args.cache_root)
            print(f"Verified {count} reference PDFs and their ground truth.")
        elif args.command == "verify-archive":
            count = verify_archive(
                args.manifest,
                args.archive,
                args.allow_legacy_appledouble,
            )
            print(f"Verified archive membership for {count} reference PDFs.")
        elif args.command == "extract-archive":
            count = extract_archive(
                args.manifest,
                args.archive,
                args.destination,
                args.allow_legacy_appledouble,
            )
            print(f"Safely extracted archive for {count} reference PDFs.")
        else:
            atomic_swap(args.current, args.replacement)
            print("Atomically installed verified corpus cache.")
    except (OSError, ValueError, json.JSONDecodeError, tarfile.TarError) as error:
        print(f"corpus cache validation failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
