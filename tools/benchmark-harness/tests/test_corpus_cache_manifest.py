import hashlib
import importlib.util
import io
import json
import tarfile
import tempfile
import unittest
from pathlib import Path
from types import ModuleType

# ruff: noqa: PT027


def _load_module() -> ModuleType:
    script = Path(__file__).parents[1] / "scripts" / "corpus_cache_manifest.py"
    spec = importlib.util.spec_from_file_location("corpus_cache_manifest_under_test", script)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


CORPUS_CACHE_MANIFEST = _load_module()


def _sha256(content: bytes) -> str:
    return hashlib.sha256(content).hexdigest()


def _reference_document(document_id: str, pdf: bytes, markdown: bytes, text: bytes) -> dict:
    return {
        "id": document_id,
        "redistribute": "reference",
        "gate_verdict": "ACCEPT",
        "size_tier": "core",
        "pdf_sha256": _sha256(pdf),
        "out_gt_md_sha256": _sha256(markdown),
        "gt_txt_sha256": _sha256(text),
    }


class CorpusCacheManifestTest(unittest.TestCase):
    """Validate content-addressed reference corpus cache manifests."""

    def _write_manifest(self, root: Path, documents: list[dict]) -> Path:
        manifest = root / "corpus_manifest.json"
        manifest.write_text(json.dumps({"documents": documents}), encoding="utf-8")
        return manifest

    def _write_archive(self, path: Path, members: list[tuple[str, bytes]]) -> None:
        with tarfile.open(path, "w") as archive:
            for name, content in members:
                member = tarfile.TarInfo(name)
                member.size = len(content)
                archive.addfile(member, io.BytesIO(content))

    def test_digest_ignores_non_reference_documents(self) -> None:
        """Public corpus changes must not invalidate the private cache."""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            reference = _reference_document("reference", b"pdf", b"md", b"text")
            manifest = self._write_manifest(root, [reference])
            initial_digest = CORPUS_CACHE_MANIFEST.manifest_digest(manifest)

            manifest = self._write_manifest(
                root,
                [
                    reference,
                    {
                        "id": "public",
                        "redistribute": "vendor",
                        "gate_verdict": "ACCEPT",
                        "size_tier": "core",
                    },
                ],
            )
            assert CORPUS_CACHE_MANIFEST.manifest_digest(manifest) == initial_digest

    def test_digest_changes_with_reference_content(self) -> None:
        """Reference byte changes must produce a different cache key."""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            original = _reference_document("reference", b"pdf", b"md", b"text")
            manifest = self._write_manifest(root, [original])
            initial_digest = CORPUS_CACHE_MANIFEST.manifest_digest(manifest)

            changed = _reference_document("reference", b"changed", b"md", b"text")
            manifest = self._write_manifest(root, [changed])
            assert CORPUS_CACHE_MANIFEST.manifest_digest(manifest) != initial_digest

    def test_verify_cache_rejects_extra_and_mismatched_files(self) -> None:
        """Verification must fail closed on extra or corrupt cache files."""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            cache = root / "cache"
            pdf = b"pdf"
            markdown = b"md"
            text = b"text"
            manifest = self._write_manifest(
                root,
                [_reference_document("reference", pdf, markdown, text)],
            )
            (cache / "pdf").mkdir(parents=True)
            (cache / "ground_truth/pdf").mkdir(parents=True)
            (cache / "pdf/reference.pdf").write_bytes(pdf)
            (cache / "ground_truth/pdf/reference.md").write_bytes(markdown)
            (cache / "ground_truth/pdf/reference.txt").write_bytes(text)

            assert CORPUS_CACHE_MANIFEST.verify_cache(manifest, cache) == 1

            (cache / "pdf/unexpected").write_text("extra", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "unexpected file"):
                CORPUS_CACHE_MANIFEST.verify_cache(manifest, cache)
            (cache / "pdf/unexpected").unlink()

            (cache / "pdf/reference.pdf").write_bytes(b"corrupt")
            with self.assertRaisesRegex(ValueError, "digest mismatch"):
                CORPUS_CACHE_MANIFEST.verify_cache(manifest, cache)

    def test_manifest_rejects_duplicate_reference_ids(self) -> None:
        """Duplicate IDs must not collapse into one expected file set."""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            document = _reference_document("duplicate", b"pdf", b"md", b"text")
            manifest = self._write_manifest(root, [document, document])
            with self.assertRaisesRegex(ValueError, "duplicate reference document id"):
                CORPUS_CACHE_MANIFEST.manifest_digest(manifest)

    def test_verify_archive_rejects_extra_and_link_members(self) -> None:
        """Archive validation must reject members outside the exact file set."""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            manifest = self._write_manifest(
                root,
                [_reference_document("reference", b"pdf", b"md", b"text")],
            )
            source = root / "source"
            (source / ".corpus-cache/pdf").mkdir(parents=True)
            (source / ".corpus-cache/ground_truth/pdf").mkdir(parents=True)
            (source / ".corpus-cache/pdf/reference.pdf").write_bytes(b"pdf")
            (source / ".corpus-cache/ground_truth/pdf/reference.md").write_bytes(b"md")
            (source / ".corpus-cache/ground_truth/pdf/reference.txt").write_bytes(b"text")

            archive_path = root / "cache.tar"
            with tarfile.open(archive_path, "w") as archive:
                archive.add(source / ".corpus-cache", arcname=".corpus-cache")
            assert CORPUS_CACHE_MANIFEST.verify_archive(manifest, archive_path) == 1
            destination = root / "extracted"
            assert CORPUS_CACHE_MANIFEST.extract_archive(manifest, archive_path, destination) == 1
            assert (destination / ".corpus-cache/pdf/reference.pdf").read_bytes() == b"pdf"

            (source / ".corpus-cache/pdf/extra.pdf").write_bytes(b"extra")
            with tarfile.open(archive_path, "w") as archive:
                archive.add(source / ".corpus-cache", arcname=".corpus-cache")
            with self.assertRaisesRegex(ValueError, "unexpected archive file"):
                CORPUS_CACHE_MANIFEST.verify_archive(manifest, archive_path)

            (source / ".corpus-cache/pdf/extra.pdf").unlink()
            link = tarfile.TarInfo(".corpus-cache/pdf/link.pdf")
            link.type = tarfile.SYMTYPE
            link.linkname = "../../outside"
            with tarfile.open(archive_path, "w") as archive:
                archive.add(source / ".corpus-cache", arcname=".corpus-cache")
                archive.addfile(link)
            with self.assertRaisesRegex(ValueError, "unsupported archive member type"):
                CORPUS_CACHE_MANIFEST.verify_archive(manifest, archive_path)

    def test_verify_archive_rejects_unsafe_incomplete_or_corrupt_payloads(self) -> None:
        """Archive validation must reject structural and content corruption."""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            manifest = self._write_manifest(
                root,
                [_reference_document("reference", b"pdf", b"md", b"text")],
            )
            archive_path = root / "cache.tar"
            valid_members = [
                (".corpus-cache/pdf/reference.pdf", b"pdf"),
                (".corpus-cache/ground_truth/pdf/reference.md", b"md"),
                (".corpus-cache/ground_truth/pdf/reference.txt", b"text"),
            ]

            cases = [
                (
                    "missing",
                    valid_members[:-1],
                    "archive is missing expected file",
                ),
                (
                    "traversal",
                    [*valid_members, ("../escape", b"unsafe")],
                    "unsafe or duplicate archive member",
                ),
                (
                    "absolute",
                    [*valid_members, ("/absolute", b"unsafe")],
                    "unsafe or duplicate archive member",
                ),
                (
                    "duplicate",
                    [*valid_members, valid_members[0]],
                    "unsafe or duplicate archive member",
                ),
                (
                    "hash mismatch",
                    [(valid_members[0][0], b"corrupt"), *valid_members[1:]],
                    "archive digest mismatch",
                ),
            ]
            for name, members, expected_error in cases:
                with self.subTest(name=name):
                    self._write_archive(archive_path, members)
                    with self.assertRaisesRegex(ValueError, expected_error):
                        CORPUS_CACHE_MANIFEST.verify_archive(manifest, archive_path)

    def test_legacy_appledouble_compatibility_is_narrow_and_does_not_extract(self) -> None:
        """Only legacy archives may ignore regular AppleDouble sidecars."""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            manifest = self._write_manifest(
                root,
                [_reference_document("reference", b"pdf", b"md", b"text")],
            )
            archive_path = root / "cache.tar"
            valid_members = [
                (".corpus-cache/pdf/reference.pdf", b"pdf"),
                (".corpus-cache/ground_truth/pdf/reference.md", b"md"),
                (".corpus-cache/ground_truth/pdf/reference.txt", b"text"),
            ]
            appledouble_member = (".corpus-cache/._pdf", b"metadata")
            self._write_archive(archive_path, [*valid_members, appledouble_member])

            with self.assertRaisesRegex(ValueError, "unexpected archive file"):
                CORPUS_CACHE_MANIFEST.verify_archive(manifest, archive_path)
            assert (
                CORPUS_CACHE_MANIFEST.verify_archive(
                    manifest,
                    archive_path,
                    allow_legacy_appledouble=True,
                )
                == 1
            )

            destination = root / "extracted"
            assert (
                CORPUS_CACHE_MANIFEST.extract_archive(
                    manifest,
                    archive_path,
                    destination,
                    allow_legacy_appledouble=True,
                )
                == 1
            )
            assert not (destination / ".corpus-cache/._pdf").exists()

            rejected_members = [
                (".corpus-cache/pdf/unexpected.bin", b"extra"),
                ("../._escape", b"unsafe"),
            ]
            for name, content in rejected_members:
                with self.subTest(name=name):
                    self._write_archive(archive_path, [*valid_members, (name, content)])
                    with self.assertRaisesRegex(
                        ValueError,
                        "unexpected archive file|unsafe or duplicate archive member",
                    ):
                        CORPUS_CACHE_MANIFEST.verify_archive(
                            manifest,
                            archive_path,
                            allow_legacy_appledouble=True,
                        )

            special_members = [
                (tarfile.DIRTYPE, "unexpected archive directory"),
                (tarfile.SYMTYPE, "unsupported archive member type"),
                (tarfile.LNKTYPE, "unsupported archive member type"),
                (tarfile.FIFOTYPE, "unsupported archive member type"),
            ]
            for member_type, expected_error in special_members:
                with self.subTest(member_type=member_type):
                    self._write_archive(archive_path, valid_members)
                    member = tarfile.TarInfo(".corpus-cache/._metadata")
                    member.type = member_type
                    member.linkname = ".corpus-cache/pdf/reference.pdf"
                    with tarfile.open(archive_path, "a") as archive:
                        archive.addfile(member)
                    with self.assertRaisesRegex(ValueError, expected_error):
                        CORPUS_CACHE_MANIFEST.verify_archive(
                            manifest,
                            archive_path,
                            allow_legacy_appledouble=True,
                        )

            expected_id = "._reference"
            expected_manifest = self._write_manifest(
                root,
                [_reference_document(expected_id, b"pdf", b"md", b"text")],
            )
            expected_members = [
                (f".corpus-cache/pdf/{expected_id}.pdf", b"pdf"),
                (f".corpus-cache/ground_truth/pdf/{expected_id}.md", b"md"),
                (f".corpus-cache/ground_truth/pdf/{expected_id}.txt", b"text"),
            ]
            self._write_archive(archive_path, expected_members)
            expected_destination = root / "expected-extracted"
            assert (
                CORPUS_CACHE_MANIFEST.extract_archive(
                    expected_manifest,
                    archive_path,
                    expected_destination,
                    allow_legacy_appledouble=True,
                )
                == 1
            )
            for relative_path, expected_content in expected_members:
                assert (expected_destination / relative_path).read_bytes() == expected_content

    def test_atomic_swap_exchanges_complete_cache_trees(self) -> None:
        """An installed tree must replace the old tree in one exchange."""
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            current = root / "current"
            replacement = root / "replacement"
            current.mkdir()
            replacement.mkdir()
            (current / "generation").write_text("old", encoding="utf-8")
            (replacement / "generation").write_text("new", encoding="utf-8")

            CORPUS_CACHE_MANIFEST.atomic_swap(current, replacement)

            assert (current / "generation").read_text(encoding="utf-8") == "new"
            assert (replacement / "generation").read_text(encoding="utf-8") == "old"


if __name__ == "__main__":
    unittest.main()
