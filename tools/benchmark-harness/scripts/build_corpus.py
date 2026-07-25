#!/usr/bin/env python3
"""Single reproducible builder for the xberg PDF→Markdown ground-truth corpus.

One command reproduces the entire corpus from pinned upstream sources, records every modification
applied to every document, gates each doc against an independent oracle, and emits the fixtures, GT,
manifest, and README under `test_documents/`. Re-running with the same pins is deterministic.

    python build_corpus.py --stage all                 # acquire → normalize → gate → assemble → manifest
    python build_corpus.py --stage normalize           # run one stage
    python build_corpus.py --stage all --dry-run       # plan only, no writes to test_documents

Design principles:
- **Pinned + recorded provenance.** Each source has a repo id + revision; the resolved commit sha,
  license, and URL are recorded per doc in the manifest.
- **Every modification logged.** Normalization transforms are declared once in `normalize_gt.py`
  (`READOC_TRANSFORMS`/`COMMON_TRANSFORMS`); this script applies them via `normalize_with_report` and
  records the exact per-doc transform counts in the build ledger. The README "How the data was
  modified" section is generated FROM that ledger — documentation cannot drift from code.
- **Gated.** No doc enters the corpus without an oracle verdict (via the calibrated gate in
  `gt_eval.py`); rejects are recorded with a reason, never silently dropped.
- **Idempotent + resumable.** Stages skip completed work; the build ledger at
  `<staging>/build_ledger.json` is the single source of truth across runs.

Stages: acquire | normalize | gate | assemble | manifest  (all = in order).
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
from dataclasses import asdict, dataclass, field
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from normalize_gt import COMMON_TRANSFORMS, READOC_TRANSFORMS, html_to_gfm, normalize_with_report

REPO_ROOT = Path(__file__).resolve().parents[3]
STAGING = Path("/tmp/corpus_src")  # noqa: S108  dataset staging area (large, non-sensitive)
TEST_DOCS = REPO_ROOT / "test_documents"
GT_DIR = TEST_DOCS / "ground_truth" / "pdf"
PDF_DIR = TEST_DOCS / "pdf"
FIXTURE_DIR = REPO_ROOT / "tools/benchmark-harness/fixtures/pdf"
MANIFEST = TEST_DOCS / "ground_truth" / "corpus_manifest.json"
README = TEST_DOCS / "README.md"
ATTRIBUTIONS = TEST_DOCS / "ATTRIBUTIONS.md"
LICENSES = TEST_DOCS / "LICENSES.md"
# Gitignored on-demand cache for `reference`-class docs (source PDFs + GT we may NOT redistribute).
# Materialized by `build_corpus.py --stage materialize`; committed fixtures for reference docs point
# here, so the public repo never contains the third-party bytes. ~keep
CACHE = TEST_DOCS / ".corpus-cache"
CACHE_PDF = CACHE / "pdf"
CACHE_GT = CACHE / "ground_truth" / "pdf"
LEDGER = STAGING / "build_ledger.json"
PARSEBENCH = Path("/tmp/parsebench")  # noqa: S108  ParseBench staging (table.jsonl + docs/table/*.pdf)
PB_MIN_TABLE_COVERAGE = 0.60
#                               table-dominant — its non-table prose would distort whole-doc scoring. ~keep

# Pinned upstream sources. revision=None ⇒ resolve latest at acquire time and record the sha. ~keep
SOURCES = {
    "readoc": {
        "repo": "lazyc/READoc",
        "revision": None,
        "license": "MIT",
        # PER-DOC (see doc_redistribute): GitHub docs (author-owned README content) are vendored;
        # arXiv docs are reference-only — the MIT tag can't license the bundled arXiv PDFs
        # (nonexclusive-distrib), and their LaTeX-source GT is version-drift-prone. ~keep
        "redistribute": "per-doc",
        "url": "https://huggingface.co/datasets/lazyc/READoc",
        "citation": "READoc: A Unified Benchmark for Realistic Document Structured Extraction (arXiv:2409.05137)",
        "granularity": "document",
        "gt_dirs": ["arxiv_ground_truth", "github_ground_truth"],
        "pdf_zips": ["arxiv.zip", "github.zip"],
        "note": "arXiv GT = author LaTeX→pandoc (no tables); GitHub GT = author README rendered to PDF",
    },
    "parsebench": {
        "repo": "llamaindex/ParseBench",
        "revision": None,
        "license": "Apache-2.0",
        # Apache-2.0 covers ParseBench's ANNOTATIONS, not the bundled third-party enterprise PDFs
        # (which carry a takedown clause). So the source PDFs are reference-only, not vendored. ~keep
        "redistribute": "reference",
        "url": "https://huggingface.co/datasets/llamaindex/ParseBench",
        "citation": "ParseBench: A Document Parsing Benchmark for AI Agents, Zhang et al. (arXiv:2604.08538)",
        "granularity": "page",
        "note": "only table.jsonl ships expected_markdown (HTML tables); human-verified",
    },
    "fintabnet": {
        "repo": "bsmock/FinTabNet.c",
        "revision": None,  # resolved from HF at acquire (real dataset repo); staged copy re-gated here ~keep
        "license": "CDLA-Permissive-2.0",
        # CDLA-Permissive-2.0 is a permissive, redistribution-friendly data license → vendored. ~keep
        "redistribute": "vendor",
        "url": "https://huggingface.co/datasets/bsmock/FinTabNet.c",
        "citation": (
            'Smock et al., "Aligning benchmark datasets for table structure recognition", ICDAR 2023 (arXiv:2303.00716)'
        ),
        "granularity": "page",
        "note": "financial-statement table crops; GT = canonicalized cell structure rendered to GFM",
    },
    "federal_register": {
        "repo": "federalregister.gov",
        # Not an HF dataset → pin the staging snapshot instead of resolving a HF sha at acquire. ~keep
        "revision": "2026-07-20",
        "license": "US-PD (17 U.S.C. §105)",
        # U.S. Government work, public domain → freely redistributable → vendored. ~keep
        "redistribute": "vendor",
        "url": "https://www.federalregister.gov",
        "citation": (
            "U.S. Federal Register (OFR/GPO), federalregister.gov — U.S. Government work, "
            "public domain (17 U.S.C. §105)"
        ),
        "granularity": "document",
        "note": "OFR/GPO full-text XML → GFM (headings, label blocks, GPOTABLE pipe tables)",
    },
}

# Vendored datasets pre-staged on disk as (pdf, gt.md) pairs under <base>/{pdf,gt}/<id>.{pdf,md}
# (already triaged upstream; this builder re-gates them with its own oracle). Normalized GT is written
# to <base>/normalized/<id>.md, mirroring the readoc/parsebench normalize→gate→assemble flow. ~keep
STAGED_SOURCES = {
    "fintabnet": STAGING / "fintabnet" / "staged",
    "federal_register": STAGING / "federal_register" / "staged",
}

# Redistribution policy. xberg (this repo) is MIT-licensed, public, non-commercial open-source.
# `vendor`    = permissive (MIT/Apache/CC-BY/CC0/public-domain) → source PDFs + GT committed here.
# `reference` = non-commercial / ShareAlike / research-only ToU → NOT committed; fetched to local
#               staging by build_corpus and used for scoring only (non-commercial use), never
#               redistributed. Recorded in the manifest with license + source URL for provenance. ~keep


def sha256_bytes(b: bytes) -> str:
    return hashlib.sha256(b).hexdigest()


def sha256_file(p: Path) -> str:
    return sha256_bytes(p.read_bytes())


@dataclass
class DocRecord:
    """Per-document build record — the authoritative log of how one doc was produced + modified."""

    id: str
    source_dataset: str
    upstream_id: str
    source_url: str
    license: str
    source_revision: str = ""
    src_gt_sha256: str = ""  # sha of the raw upstream GT, before any modification ~keep
    out_gt_md_sha256: str = ""  # sha of the normalized GT actually committed ~keep
    transforms: dict = field(default_factory=dict)  # {transform_name: count} — the modifications ~keep
    gate_verdict: str = "PENDING"
    exclusion_reason: str = ""
    cohorts: list = field(default_factory=list)
    role: str = ""
    size_tier: str = ""


def load_ledger() -> dict[str, dict]:
    if LEDGER.exists():
        return {r["id"]: r for r in json.loads(LEDGER.read_text())}
    return {}


def save_ledger(records: dict[str, dict]) -> None:
    STAGING.mkdir(parents=True, exist_ok=True)
    LEDGER.write_text(json.dumps(sorted(records.values(), key=lambda r: r["id"]), indent=2) + "\n")


def resolve_revision(repo: str) -> str:
    """Record the exact upstream commit so a rebuild is reproducible."""
    out = subprocess.run(
        ["curl", "-sL", f"https://huggingface.co/api/datasets/{repo}"],
        capture_output=True,
        timeout=30,
    ).stdout
    return json.loads(out).get("sha", "") if out else ""


def stage_acquire(records: dict, dry: bool) -> None:
    """Download pinned sources into staging (idempotent). GT dirs first, PDFs second."""
    for name, cfg in SOURCES.items():
        cfg["revision"] = cfg["revision"] or resolve_revision(cfg["repo"])
        print(f"[acquire] {name} {cfg['repo']}@{cfg['revision'][:8]} ({cfg['license']})")
        dest = STAGING / name
        if dry:
            continue
        dest.mkdir(parents=True, exist_ok=True)
        if name == "readoc" and not (dest / "arxiv_ground_truth").exists():
            _hf(cfg["repo"], dest, cfg["revision"], include="*_ground_truth/*")
        # ParseBench GT + table PDFs handled by its assemble handler (already staged at /tmp/parsebench). ~keep


def stage_normalize(records: dict, dry: bool) -> None:
    """Normalize each source GT to canonical GFM, recording exact per-doc transform counts."""
    cfg = SOURCES["readoc"]
    for gt_dir in cfg["gt_dirs"]:
        for md in sorted((STAGING / "readoc" / gt_dir).glob("*.md")):
            raw = md.read_text("utf-8", "replace")
            out, report = normalize_with_report(raw, source="readoc")
            rec = records.get(md.stem) or asdict(
                DocRecord(
                    id=md.stem,
                    source_dataset="readoc",
                    upstream_id=md.stem,
                    source_url=cfg["url"],
                    license=cfg["license"],
                    source_revision=cfg["revision"],
                )
            )
            rec["src_gt_sha256"] = sha256_bytes(raw.encode())
            rec["out_gt_md_sha256"] = sha256_bytes(out.encode())
            rec["transforms"] = report
            records[md.stem] = rec
            if not dry:
                normd = STAGING / "readoc" / "normalized"
                normd.mkdir(exist_ok=True)
                (normd / f"{md.stem}.md").write_text(out)
    n = sum(1 for r in records.values() if r["source_dataset"] == "readoc")
    print(f"[normalize] readoc: {n} GT files normalized")

    # ParseBench — each record's expected_markdown is an HTML table; convert to GFM with our engine. ~keep
    pb = SOURCES["parsebench"]
    count = 0
    for pid, _pdf, html in _parsebench_records():
        out = html_to_gfm(html)
        rec = records.get(pid) or asdict(
            DocRecord(
                id=pid,
                source_dataset="parsebench",
                upstream_id=pid[3:],
                source_url=pb["url"],
                license=pb["license"],
                source_revision=pb["revision"],
            )
        )
        rec["src_gt_sha256"] = sha256_bytes(html.encode())
        rec["out_gt_md_sha256"] = sha256_bytes(out.encode())
        rec["transforms"] = {"html_to_gfm": 1}
        records[pid] = rec
        count += 1
        if not dry:
            normd = PARSEBENCH / "normalized"
            normd.mkdir(exist_ok=True)
            (normd / f"{pid}.md").write_text(out)
    print(f"[normalize] parsebench: {count} HTML tables → GFM")

    # Pre-staged vendored datasets (fintabnet, federal_register): GT is already GFM on disk; canonicalize
    # it (COMMON_TRANSFORMS only for these non-readoc sources) and record the exact per-doc transforms. ~keep
    for name, base in STAGED_SOURCES.items():
        _normalize_staged(records, name, base, dry)


def _normalize_staged(records: dict, name: str, base: Path, dry: bool) -> None:
    """Normalize a pre-staged (pdf, gt.md)-pair dataset into <base>/normalized/<id>.md + DocRecords."""
    cfg = SOURCES[name]
    normd = base / "normalized"
    count = 0
    for gt_md in sorted((base / "gt").glob("*.md")):
        sid = gt_md.stem
        if not (base / "pdf" / f"{sid}.pdf").exists():
            continue  # orphan GT with no source PDF — skip (gate would only mark it NO_PDF) ~keep
        raw = gt_md.read_text("utf-8", "replace")
        out, report = normalize_with_report(raw, source=name)
        rec = records.get(sid) or asdict(
            DocRecord(
                id=sid,
                source_dataset=name,
                upstream_id=sid[3:] if name == "fintabnet" else sid,  # strip the `ft_` id prefix ~keep
                source_url=cfg["url"],
                license=cfg["license"],
                source_revision=cfg.get("revision") or "",
            )
        )
        rec["src_gt_sha256"] = sha256_bytes(raw.encode())
        rec["out_gt_md_sha256"] = sha256_bytes(out.encode())
        rec["transforms"] = report
        records[sid] = rec
        count += 1
        if not dry:
            normd.mkdir(parents=True, exist_ok=True)
            (normd / f"{sid}.md").write_text(out)
    print(f"[normalize] {name}: {count} GT files normalized")


def _parsebench_records() -> list[tuple[str, Path, str]]:
    """(id, pdf_path, html) per ParseBench table record. id is namespaced `pb_<pdf-stem>`."""
    out = []
    tj = PARSEBENCH / "table.jsonl"
    if not tj.exists():
        return out
    for line in tj.open():
        r = json.loads(line)
        html = r.get("expected_markdown")
        if not html:
            continue
        stem = re.sub(r"[^A-Za-z0-9._-]+", "_", Path(r["pdf"]).stem)  # filename-safe id (names have spaces) ~keep
        out.append((f"pb_{stem}", PARSEBENCH / r["pdf"], html))
    return out


def _readoc_pdf(stem: str) -> Path | None:
    for sub in ("arxiv", "github"):
        p = STAGING / "readoc" / "unpacked" / sub / "pdf" / f"{stem}.pdf"
        if p.exists():
            return p
    return None


def _apply_gate(rec: dict, ev, extra_cohorts: list[str]) -> None:
    """Record an oracle verdict + scores + cohorts onto a doc record."""
    rec["gate_verdict"] = ev.verdict
    rec["cohorts"] = list(dict.fromkeys([*rec.get("cohorts", []), *ev.cohorts, *extra_cohorts]))
    rec["exclusion_reason"] = "; ".join(ev.reasons) if ev.verdict in ("REJECT", "NO_ORACLE", "NO_PDF") else ""
    rec["oracle_scores"] = {
        "coverage": round(ev.coverage, 3),
        "precision": round(ev.precision, 3),
        "head_recall": round(ev.head_recall, 3),
        "tail_recall": round(ev.tail_recall, 3),
        "table_label_support": round(ev.worst_table_label_support, 3),
        "pages": ev.pages,
        "empty_pages": ev.empty_pages,
    }


def stage_gate(records: dict, dry: bool) -> None:
    """Gate every normalized GT against its PDF via the calibrated oracle (gt_eval). Resumable:
    already-gated records are skipped. Records verdict, cohorts, oracle scores, and — for REJECT/
    NO_ORACLE — an exclusion reason. No doc enters the corpus without a verdict.
    """
    from gt_eval import evaluate  # noqa: PLC0415

    def done(rec):
        return rec.get("gate_verdict") not in (None, "", "PENDING")

    norm = STAGING / "readoc" / "normalized"
    rd: dict[str, int] = {}
    for rid, rec in records.items():
        if rec["source_dataset"] != "readoc":
            continue
        if done(rec):
            rd[rec["gate_verdict"]] = rd.get(rec["gate_verdict"], 0) + 1
            continue
        pdf, gt = _readoc_pdf(rid), norm / f"{rid}.md"
        if pdf is None or not gt.exists():
            rec["gate_verdict"], rec["exclusion_reason"] = "NO_PDF", "missing source PDF or normalized GT"
            continue
        ev = evaluate(pdf, gt.read_text("utf-8", "replace"), source="readoc")
        _apply_gate(rec, ev, [])
        rd[ev.verdict] = rd.get(ev.verdict, 0) + 1
        if not dry and sum(rd.values()) % 200 == 0:
            save_ledger(records)
    print(f"[gate] readoc: {rd}")

    pb_norm = PARSEBENCH / "normalized"
    pdf_map = {pid: pdf for pid, pdf, _ in _parsebench_records()}
    pb: dict[str, int] = {}
    for rid, rec in records.items():
        if rec["source_dataset"] != "parsebench" or done(rec):
            continue
        pdf, gt = pdf_map.get(rid), pb_norm / f"{rid}.md"
        if pdf is None or not pdf.exists() or not gt.exists():
            rec["gate_verdict"], rec["exclusion_reason"] = "NO_PDF", "missing table PDF or normalized GT"
            continue
        ev = evaluate(pdf, gt.read_text("utf-8", "replace"), source="parsebench")
        _apply_gate(rec, ev, ["tables"])
        # ParseBench GT is human-verified, so a "fabricated table" flag (header tokens absent from the ~keep
        # text layer) means the table is an IMAGE the oracle can't read, not fabrication → REVIEW+OCR.
        if ev.verdict == "REJECT" and "fabricated table" in rec["exclusion_reason"]:
            rec["gate_verdict"] = "REVIEW"
            rec["exclusion_reason"] = "table absent from text layer (image table?) — needs 2nd OCR oracle"
            rec["cohorts"] = list(dict.fromkeys([*rec["cohorts"], "figures"]))
        # A table-only GT on a prose-heavy page distorts whole-doc scoring → hold for review. ~keep
        elif ev.verdict == "ACCEPT" and ev.coverage < PB_MIN_TABLE_COVERAGE:
            rec["gate_verdict"] = "REVIEW"
            rec["exclusion_reason"] = f"table-only GT but page is {ev.coverage:.2f} table (non-table prose)"
        pb[rec["gate_verdict"]] = pb.get(rec["gate_verdict"], 0) + 1
        if not dry and sum(pb.values()) % 100 == 0:
            save_ledger(records)
    print(f"[gate] parsebench: {pb}")

    # Pre-staged vendored datasets — same calibrated oracle, source-agnostic (fintabnet crops are
    # table-dominant → tagged `tables`; federal_register is whole-doc prose+tables). ~keep
    for name, base in STAGED_SOURCES.items():
        norm = base / "normalized"
        extra = ["tables"] if name == "fintabnet" else []
        tally: dict[str, int] = {}
        for rid, rec in records.items():
            if rec["source_dataset"] != name:
                continue
            if done(rec):
                tally[rec["gate_verdict"]] = tally.get(rec["gate_verdict"], 0) + 1
                continue
            pdf, gt = _source_pdf(rec), norm / f"{rid}.md"
            if pdf is None or not gt.exists():
                rec["gate_verdict"], rec["exclusion_reason"] = "NO_PDF", "missing source PDF or normalized GT"
                tally["NO_PDF"] = tally.get("NO_PDF", 0) + 1
                continue
            ev = evaluate(pdf, gt.read_text("utf-8", "replace"), source=name)
            _apply_gate(rec, ev, extra)
            tally[ev.verdict] = tally.get(ev.verdict, 0) + 1
            if not dry and sum(tally.values()) % 100 == 0:
                save_ledger(records)
        print(f"[gate] {name}: {tally}")


CORE_SIZE = 160  # active fast-iteration tier (runtime-governed; calibrate to codex's wall-clock) ~keep
CORE_PER_STRATUM = 24  # ensure each diagnostic stratum is represented in core ~keep
SMOKE_PER_STRATUM = 2  # smoke covers every stratum minimally (~24 docs) ~keep
TUNE_FRACTION = 0.70  # deterministic 70/30 tune/eval split (overfitting guard) ~keep

_H = re.compile
_MATH = _H(r"\$[^$\n]+\$|\$\$")
_HEADING = _H(r"^(#{1,6})\s", re.MULTILINE)
_NESTED_LIST = _H(r"^(?:\s{2,}|\t)(?:[-*+]|\d+\.)\s", re.MULTILINE)
_IMAGE = _H(r"!\[[^\]]*\]\(")
_CAPTION = _H(r"^\s*(?:Figure|Fig\.|Table)\s*\d", re.MULTILINE)


def _hash01(s: str) -> float:
    """Deterministic, seedless [0,1) hash for reproducible tier/role assignment."""
    return int(hashlib.sha256(s.encode()).hexdigest(), 16) / (16**64)


def _normalized_gt(rec: dict) -> Path:
    ds = rec["source_dataset"]
    if ds == "parsebench":
        return PARSEBENCH / "normalized" / f"{rec['id']}.md"
    if ds in STAGED_SOURCES:
        return STAGED_SOURCES[ds] / "normalized" / f"{rec['id']}.md"
    return STAGING / "readoc" / "normalized" / f"{rec['id']}.md"


def diagnostic_strata(md: str, pages: int | None, pb_html: str | None) -> list[str]:
    """Orthogonal diagnostic-stratum tags derived from the GT (and, for tables, the source HTML)."""
    tags = []
    if _MATH.search(md):
        tags.append("formula")
    if "```" in md:
        tags.append("code")
    levels = [len(m.group(1)) for m in _HEADING.finditer(md)]
    if levels and max(levels) >= 3:
        tags.append("nested-heading")
    if _NESTED_LIST.search(md):
        tags.append("nested-list")
    if _IMAGE.search(md):
        tags.append("figures")
    if _CAPTION.search(md):
        tags.append("caption/footnote")
    if pages and pages > 1:
        tags.append("multipage-reading-order")
    if pb_html is not None:
        cols = max((row.count("<td") + row.count("<th") for row in pb_html.split("</tr>")), default=0)
        if cols >= 6:
            tags.append("wide-table")
        if "colspan" in pb_html or "rowspan" in pb_html:
            tags.append("complex-span-table")
    return tags


def stage_curate(records: dict, dry: bool) -> None:
    """Tag diagnostic strata, then assign deterministic size tiers (smoke ⊂ core ⊂ extended) and a
    70/30 tune/eval role to every accepted doc. Reproducible: all assignment is by hash(id).
    """
    from collections import defaultdict

    pb_html = {pid: html for pid, _pdf, html in _parsebench_records()}
    accept = [r for r in records.values() if r["gate_verdict"] == "ACCEPT"]

    for r in accept:
        gt = _normalized_gt(r)
        md = gt.read_text("utf-8", "replace") if gt.exists() else ""
        pages = (r.get("oracle_scores") or {}).get("pages")
        strata = diagnostic_strata(md, pages, pb_html.get(r["id"]))
        r["cohorts"] = list(dict.fromkeys([*r.get("cohorts", []), *strata]))
        r["size_tier"] = "extended"
        r["role"] = "tune" if _hash01(r["id"]) < TUNE_FRACTION else "eval"
        r["redistribute"] = doc_redistribute(r)

    by_stratum: dict[str, list] = defaultdict(list)
    for r in accept:
        for c in r["cohorts"]:
            by_stratum[c].append(r)
    core, smoke = set(), set()
    for rs in by_stratum.values():
        srt = sorted(rs, key=lambda r: _hash01(r["id"]))
        core.update(r["id"] for r in srt[:CORE_PER_STRATUM])
        smoke.update(r["id"] for r in srt[:SMOKE_PER_STRATUM])
    for r in sorted(accept, key=lambda r: _hash01(r["id"])):
        if len(core) >= CORE_SIZE:
            break
        core.add(r["id"])
    for r in accept:
        if r["id"] in core:
            r["size_tier"] = "core"
        if r["id"] in smoke:
            r["size_tier"] = "smoke"

    from collections import Counter

    tiers = Counter(r["size_tier"] for r in accept)
    roles = Counter(r["role"] for r in accept)
    strata_counts = Counter(c for r in accept for c in r["cohorts"])
    print(f"[curate] tiers={dict(tiers)} roles={dict(roles)}")
    print(f"[curate] strata={dict(sorted(strata_counts.items(), key=lambda x: -x[1]))}")


def _source_pdf(rec: dict) -> Path | None:
    ds = rec["source_dataset"]
    if ds == "parsebench":
        return {pid: pdf for pid, pdf, _ in _parsebench_records()}.get(rec["id"])
    if ds in STAGED_SOURCES:
        p = STAGED_SOURCES[ds] / "pdf" / f"{rec['id']}.pdf"
        return p if p.exists() else None
    return _readoc_pdf(rec["id"])


_STRIP = [
    (re.compile(r"```.*?```", re.DOTALL), " "),
    (re.compile(r"`[^`]*`"), " "),
    (re.compile(r"!?\[([^\]]*)\]\([^)]*\)"), r"\1"),
    (re.compile(r"\$\$?[^$]*\$\$?"), " "),
    (re.compile(r"^[#>\s]*\|?|[|>#]"), " "),
    (re.compile(r"[*_~`]+"), ""),
    (re.compile(r"[ \t]+"), " "),
]


def strip_to_text(md: str) -> str:
    """Derive plaintext GT (for text-F1) from GFM markdown by removing structural syntax."""
    t = md
    for pat, repl in _STRIP:
        t = pat.sub(repl, t)
    return "\n".join(line.strip() for line in t.splitlines() if line.strip()) + "\n"


def _size_category(nbytes: int) -> str:
    return "small" if nbytes < 500_000 else "medium" if nbytes < 5_000_000 else "large"


def doc_redistribute(rec: dict) -> str:
    """Vendor (committed) vs reference (gitignored, fetched on demand), per document. ReaDoc is split:
    GitHub docs (pure-int ids = author-owned README content) vendor; arXiv docs reference — the wrapper
    MIT tag cannot license the bundled arXiv PDFs, and their source-derived GT is version-drift-prone.
    """
    ds = rec["source_dataset"]
    if ds == "readoc":
        return "vendor" if rec["id"].isdigit() else "reference"
    return SOURCES.get(ds, {}).get("redistribute", "reference")


def _dest(redistribute: str) -> tuple[Path, Path, str]:
    """(pdf_dir, gt_dir, fixture-path prefix) for a redistribution class."""
    if redistribute == "vendor":
        return PDF_DIR, GT_DIR, "../../../../test_documents"
    return CACHE_PDF, CACHE_GT, "../../../../test_documents/.corpus-cache"


def _write_doc(r: dict) -> tuple[int, str] | None:
    """Copy the source PDF + write GFM/.txt GT to the doc's redistribution destination (committed for
    vendor, gitignored cache for reference). Records redistribute + pdf/txt hashes. Returns (bytes, prefix).
    """
    import shutil

    src_pdf, gt = _source_pdf(r), _normalized_gt(r)
    if src_pdf is None or not src_pdf.exists() or not gt.exists():
        r["exclusion_reason"] = "missing source PDF or normalized GT"
        return None
    md = gt.read_text("utf-8", "replace")
    txt = strip_to_text(md)
    r["redistribute"] = doc_redistribute(r)
    pdf_dir, gt_dir, prefix = _dest(r["redistribute"])
    for d in (pdf_dir, gt_dir):
        d.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(src_pdf, pdf_dir / f"{r['id']}.pdf")
    (gt_dir / f"{r['id']}.md").write_text(md)
    (gt_dir / f"{r['id']}.txt").write_text(txt)
    r["pdf_sha256"] = sha256_file(pdf_dir / f"{r['id']}.pdf")
    r["gt_txt_sha256"] = sha256_bytes(txt.encode())
    return (pdf_dir / f"{r['id']}.pdf").stat().st_size, prefix


def _fixture(r: dict, nbytes: int, prefix: str) -> dict:
    return {
        "document": f"{prefix}/pdf/{r['id']}.pdf",
        "file_type": "pdf",
        "file_size": nbytes,
        "expected_frameworks": ["xberg", "liteparse", "docling"],
        "metadata": {
            "description": f"{r['source_dataset']} {r['upstream_id']}",
            "source": r["source_dataset"],
            "redistribute": r["redistribute"],
            "size_category": _size_category(nbytes),
            "cohorts": r["cohorts"],
            "size_tier": r["size_tier"],
            "role": r["role"],
        },
        "ground_truth": {
            "text_file": f"{prefix}/ground_truth/pdf/{r['id']}.txt",
            "markdown_file": f"{prefix}/ground_truth/pdf/{r['id']}.md",
            "source": r["source_dataset"],
        },
    }


def _prune_managed() -> None:
    """Assemble owns the corpus files: clear prior readoc/parsebench outputs — committed AND cached —
    so tier/redistribute changes never leave stale files. Non-managed (e.g. manual) is kept.
    """
    for fx in FIXTURE_DIR.glob("*.json"):
        try:
            src = json.loads(fx.read_text()).get("ground_truth", {}).get("source")
        except (json.JSONDecodeError, OSError):
            continue
        if src in ("readoc", "parsebench", *STAGED_SOURCES):
            stem = fx.stem
            fx.unlink(missing_ok=True)
            (PDF_DIR / f"{stem}.pdf").unlink(missing_ok=True)
            (CACHE_PDF / f"{stem}.pdf").unlink(missing_ok=True)
            for base in (GT_DIR, CACHE_GT):
                (base / f"{stem}.md").unlink(missing_ok=True)
                (base / f"{stem}.txt").unlink(missing_ok=True)


def _write_gitignore() -> None:
    gi = TEST_DOCS / ".gitignore"
    existing = gi.read_text() if gi.exists() else ""
    if ".corpus-cache" not in existing:
        gi.write_text(existing + ".corpus-cache/\n")


def stage_assemble(records: dict, dry: bool) -> None:
    """Materialize the active tiers (smoke ⊂ core) + write fixtures. VENDOR docs → committed
    test_documents; REFERENCE docs → the gitignored .corpus-cache (never redistributed). Fixtures are
    always committed (pointers); reference-doc fixtures point into the cache and require --stage
    materialize before a run.
    """
    active = [r for r in records.values() if r["gate_verdict"] == "ACCEPT" and r.get("size_tier") in ("smoke", "core")]
    if not dry:
        for d in (PDF_DIR, GT_DIR, FIXTURE_DIR, CACHE_PDF, CACHE_GT):
            d.mkdir(parents=True, exist_ok=True)
        _prune_managed()
        _write_gitignore()
    written = {"vendor": 0, "reference": 0}
    for r in active:
        if dry:
            if _source_pdf(r) and _normalized_gt(r).exists():
                written[doc_redistribute(r)] += 1
            continue
        res = _write_doc(r)
        if res is None:
            continue
        nbytes, prefix = res
        (FIXTURE_DIR / f"{r['id']}.json").write_text(json.dumps(_fixture(r, nbytes, prefix), indent=2) + "\n")
        written[r["redistribute"]] += 1
    print(f"[assemble] vendor→committed {written['vendor']}, reference→gitignored-cache {written['reference']}")


def stage_materialize(records: dict, dry: bool) -> None:
    """On-demand fetch: populate the gitignored .corpus-cache with source PDF + GT for every committed
    REFERENCE fixture. This is what the harness runs before a benchmark so reference docs exist locally
    without ever being redistributed. Idempotent (skips docs already cached). Requires acquire+normalize
    to have run (ledger + staged sources present).
    """
    ref_fixtures = []
    for fx in FIXTURE_DIR.glob("*.json"):
        try:
            d = json.loads(fx.read_text())
        except (json.JSONDecodeError, OSError):
            continue
        if d.get("metadata", {}).get("redistribute") == "reference":
            ref_fixtures.append(fx.stem)
    have = built = missing = 0
    for stem in ref_fixtures:
        if (CACHE_PDF / f"{stem}.pdf").exists():
            have += 1
            continue
        r = records.get(stem)
        if dry or r is None or _write_doc(r) is None:
            missing += 1
        else:
            built += 1
    print(f"[materialize] {len(ref_fixtures)} reference fixtures — cached {have}, built {built}, missing {missing}")


def stage_manifest(records: dict, dry: bool) -> None:
    """Write the immutable corpus manifest (every gated doc, with a frozen top-level hash) and the
    generated README.
    """
    docs = sorted(records.values(), key=lambda r: r["id"])
    payload = json.dumps(docs, sort_keys=True).encode()
    manifest = {
        "schema": 1,
        "sources": {k: {kk: v[kk] for kk in ("repo", "revision", "license", "url")} for k, v in SOURCES.items()},
        "manifest_sha256": sha256_bytes(payload),
        "counts": _verdict_counts(records),
        "documents": docs,
    }
    if dry:
        print(f"[manifest] dry-run: {manifest['counts']} hash={manifest['manifest_sha256'][:12]}")
        return
    MANIFEST.parent.mkdir(parents=True, exist_ok=True)
    MANIFEST.write_text(json.dumps(manifest, indent=2) + "\n")
    generate_readme(records)
    generate_attributions(records)
    generate_licenses(records)
    print(f"[manifest] wrote {MANIFEST} (hash {manifest['manifest_sha256'][:12]}) + README + ATTRIBUTIONS + LICENSES")


def _source_doc_counts(records: dict) -> dict:
    from collections import Counter

    return dict(Counter(r["source_dataset"] for r in records.values() if r["gate_verdict"] == "ACCEPT"))


def generate_attributions(records: dict) -> None:
    """Per-source attribution (satisfies CC-BY / dataset citation requirements)."""
    counts = _source_doc_counts(records)
    lines = [
        "# Attributions",
        "",
        "This benchmark corpus is derived from third-party datasets. Each is credited below as required",
        "by its license. Per-document provenance (upstream id, source revision, license) is in",
        "`ground_truth/corpus_manifest.json`.",
        "",
    ]
    for name, cfg in SOURCES.items():
        n = counts.get(name, 0)
        lines += [
            f"## {cfg['repo']}",
            "",
            f"- **Citation:** {cfg.get('citation', cfg['repo'])}",
            f"- **Source:** {cfg['url']}",
            f"- **License:** {cfg['license']}",
            f"- **Used here:** {n} accepted documents ({cfg['redistribute']}).",
            (
                "- **Modifications:** ground truth normalized to canonical GFM (see README → How the data "
                "was modified). Derived GT is a derivative work under the upstream license."
            ),
            "",
        ]
    ATTRIBUTIONS.write_text("\n".join(lines))


def generate_licenses(records: dict) -> None:
    """License clarification: what the repo's MIT covers vs. what upstream data retains."""
    vend = [c for c in SOURCES.values() if c.get("redistribute") == "vendor"]
    ref = [c for c in SOURCES.values() if c.get("redistribute") == "reference"]
    rows = ["| dataset | license | policy |", "|---|---|---|"]
    for cfg in SOURCES.values():
        rows.append(f"| [{cfg['repo']}]({cfg['url']}) | {cfg['license']} | {cfg['redistribute']} |")
    body = f"""# Licensing

`test_documents` is part of **xberg** (https://github.com/xberg-io/xberg), which is **MIT-licensed,
public, non-commercial open-source**. Kreuzberg, Inc.'s commercial product lives in a separate repo.

## What the MIT license covers

The repository's MIT `LICENSE` covers **our own work**: the corpus tooling
(`tools/benchmark-harness/scripts/*`), the `corpus_manifest.json`, this file, `ATTRIBUTIONS.md`, and
`README.md`. It does **not** relicense third-party dataset content. Each source document and its
upstream ground truth **retain their own upstream license** (see the table below and
`ATTRIBUTIONS.md`). Our GFM-normalized ground truth is a derivative work carried under its source's
license.

## Redistribution policy

Datasets are handled by license class:

- **vendor** — permissively licensed (MIT / Apache-2.0 / CC-BY / CC0 / US public-domain). Source PDFs
  and ground truth are **committed** into this repo, with attribution in `ATTRIBUTIONS.md`.
- **reference** — non-commercial (CC-BY-NC), ShareAlike (CC-BY-SA / -NC-SA), or research-only terms.
  These are **not committed**. `build_corpus.py` fetches them to local staging on demand; they are
  used only for **non-commercial benchmarking** of this open-source library and are **never
  redistributed** here. Their manifest entries carry the source URL + license for provenance.

This keeps the public MIT repo free of content it cannot redistribute, while still letting the
non-commercial benchmark use non-commercial data.

## Sources

{chr(10).join(rows)}

Vendored sources: {len(vend)}. Reference-only sources: {len(ref)}.
"""
    LICENSES.write_text(body)


def _verdict_counts(records: dict) -> dict:
    from collections import Counter

    return dict(Counter(r["gate_verdict"] for r in records.values()))


def _transform_catalog() -> str:
    rows = ["| transform | applies to | what it changes |", "|---|---|---|"]
    for name, desc, *_ in READOC_TRANSFORMS:
        rows.append(f"| `{name}` | ReaDoc arXiv | {desc} |")
    for name, desc, *_ in COMMON_TRANSFORMS:
        rows.append(f"| `{name}` | all sources | {desc} |")
    return "\n".join(rows)


def _modification_summary(records: dict) -> str:
    agg: dict[str, int] = {}
    docs_touched = 0
    for r in records.values():
        if r.get("transforms"):
            docs_touched += 1
            for k, v in r["transforms"].items():
                agg[k] = agg.get(k, 0) + v
    lines = [f"- **{docs_touched}** documents had at least one normalization applied."]
    for k, v in sorted(agg.items(), key=lambda x: -x[1]):
        lines.append(f"- `{k}`: {v} substitutions across the corpus.")
    return "\n".join(lines)


def generate_readme(records: dict) -> None:
    src_rows = ["| dataset | license | GT provenance | role |", "|---|---|---|---|"]
    for cfg in SOURCES.values():
        # revision is None for sources resolved from HF at acquire time (e.g. fintabnet); show HEAD. ~keep
        rev = (cfg.get("revision") or "HEAD")[:8]
        src_rows.append(
            f"| [{cfg['repo']}]({cfg['url']}) @`{rev}` | {cfg['license']} | {cfg['note']} | {cfg['granularity']} |"
        )
    body = f"""# xberg PDF→Markdown benchmark corpus

Ground truth for the xberg PDF→Markdown benchmark. **Every document here is reproduced from a pinned
upstream source and gated against an independent text oracle** — see `ground_truth/corpus_manifest.json`
for per-document provenance and verdicts. Do not hand-edit GT; change the builder and re-run.

## Reproduce

```
python tools/benchmark-harness/scripts/build_corpus.py --stage all
```

This is the ONLY sanctioned way to modify the corpus. It acquires the pinned sources, normalizes GT to
canonical GFM, gates each doc, and writes `pdf/`, `ground_truth/pdf/<stem>.{{md,txt}}`, the fixtures, and
this file. Re-running with the same pins is deterministic.

## Sources

{chr(10).join(src_rows)}

Excluded on purpose: **OmniDocBench** (research-only / non-commercial — incompatible with this MIT repo)
and **Nougat** (weights CC-BY-NC; corpus not distributed).

## How the data was modified

Upstream GT is not committed verbatim — it is normalized to canonical GFM so it can be scored
consistently. The transforms are declared once in `scripts/normalize_gt.py` and applied by
`build_corpus.py`; this section and the per-doc `transforms` field in the manifest are generated from
the build ledger, so they always match what actually ran.

{_transform_catalog()}

Applied this build:

{_modification_summary(records)}

## Layout

- `pdf/<stem>.pdf` — source document.
- `ground_truth/pdf/<stem>.md` — normalized canonical-GFM GT (scored by the harness).
- `ground_truth/pdf/<stem>.txt` — plaintext GT (text-F1).
- `ground_truth/corpus_manifest.json` — immutable manifest: per-doc hashes, source+license+revision,
  transforms, oracle verdict+scores, cohorts, size tier, tune/eval role; one frozen top-level hash.

## Manifest, cohorts, tiers, roles

See the plan and `corpus_manifest.json`. Cohorts tag execution mode (native-clean / native-corrupt-font
/ selective-OCR / forced-OCR) and diagnostic strata (tables, multicolumn, formulas, …). Size tiers
`smoke ⊂ core ⊂ extended` and a `tune`/`eval` role per doc support fast iteration without overfitting.
"""
    README.parent.mkdir(parents=True, exist_ok=True)
    README.write_text(body)


def _hf(repo: str, dest: Path, revision: str, include: str) -> None:
    cmd = [
        "hf",
        "download",
        repo,
        "--repo-type",
        "dataset",
        "--local-dir",
        str(dest),
        "--revision",
        revision,
        "--include",
        include,
    ]
    subprocess.run(cmd, check=True)


STAGES = {
    "acquire": stage_acquire,
    "normalize": stage_normalize,
    "gate": stage_gate,
    "curate": stage_curate,
    "assemble": stage_assemble,
    "manifest": stage_manifest,
    "materialize": stage_materialize,
}
ORDER = ["acquire", "normalize", "gate", "curate", "assemble", "manifest"]
# `materialize` is on-demand (harness pre-run step), not part of `all`. ~keep


def main() -> int:
    ap = argparse.ArgumentParser(description="Reproducible xberg GT corpus builder")
    ap.add_argument("--stage", choices=[*ORDER, "all", "materialize"], default="all")
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()

    records = load_ledger()
    stages = ORDER if args.stage == "all" else [args.stage]
    for s in stages:
        STAGES[s](records, args.dry_run)
        if not args.dry_run:
            save_ledger(records)
    return 0


if __name__ == "__main__":
    sys.exit(main())
