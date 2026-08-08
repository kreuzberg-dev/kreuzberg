#!/usr/bin/env python3
"""Verify every disk-referenced path in alef.toml's README/docs pipeline resolves.

Task #391: during a recent campaign, 12 alef.toml readme snippet paths pointed at
files that did not exist. Each one was discovered one at a time, and each
discovery cost a full ~10-minute `alef all` regen cycle. This script statically
resolves every path Alef's README/docs generation reads from disk and reports
ALL unresolved paths in a single pass, in well under a second.

Key shapes covered (see module docstring in the repo task notes for the full
rationale):
  - workspace.docs.cli.sources           (files)
  - workspace.docs.mcp.sources           (files)
  - workspace.docs.skills.template_dir   (dir)
  - workspace.docs.snippets.dirs         (dirs)
  - crates.readme.template_dir           (dir)
  - crates.readme.snippets_dir           (dir)
  - crates.readme.targets.*.template     (file, resolved under template_dir)
  - crates.readme.languages.*.template   (file, resolved under template_dir)
  - crates.readme.languages.*.snippets.* (file, resolved under
    snippets_dir/<snippet_language or language key>/<path>)

`snippet_language` (alef >= 0.54.1) redirects `include_snippet(language)` at a
different snippets subdirectory while leaving the `languages.<key>` name intact
for template branching (e.g. `kotlin_android` sets `snippet_language = "kotlin"`
because snippets live under `docs-site/src/snippets/kotlin/`, not
`.../kotlin_android/`). This script MUST resolve snippet paths against
`snippet_language` when present, not the table key, or it reports false
failures for every crate that uses the override.

Deliberately NOT covered (see the report for the full reasoning):
  - `[[crates]].sources` / `[[crates.source_crates]].sources` -- Rust API
    extraction inputs, not readme/docs inputs. `source_crates` points at sibling
    repos (`../crawlberg`, `../html-to-markdown`) that do not exist in this
    repo's own CI checkout; checking them would produce failures unrelated to
    the bug this script targets. `cargo check` / `alef extract` already fail
    loudly on a bad Rust source path.
  - `workspace.sync.extra_paths` -- version-sync manifest targets, validated by
    the version-sync task machinery itself.
  - `workspace.poly.exclude` -- glob *exclude* patterns; legitimately may match
    nothing.
  - `crates.readme.*.output_path` / `output_pattern` -- generation OUTPUTS, not
    inputs. They need not exist before a run.
  - `crates.e2e.packages.*.path` -- e2e package roots resolved relative to the
    generated `e2e/<lang>/` tree (a different base and a different freshness
    contract owned by the e2e generator's own verify step), and `e2e/` /
    `packages/` are out of scope for this script.

Usage:
    python3 scripts/ci/check-alef-readme-paths.py [path/to/alef.toml] [repo_root]

`repo_root` defaults to the alef.toml's own parent directory (the normal case:
alef.toml lives at the repo root). It is only needed when validating a scratch
copy of alef.toml against the real repo tree (e.g. to test this script itself).

Exits 0 when every referenced path resolves. Exits 1 and lists every unresolved
path (with its alef.toml line number) otherwise -- never fails fast on the
first one, since that is exactly the failure mode this check exists to avoid.
"""

from __future__ import annotations

import re
import sys
from dataclasses import dataclass
from pathlib import Path

import tomllib


@dataclass(frozen=True)
class PathRef:
    """A single path referenced from alef.toml that must resolve on disk."""

    description: str
    relative_path: str
    line_number: int
    must_be_dir: bool = False


def find_line(lines: list[str], section: str, key: str) -> int:
    """Return the 1-indexed line number of `key = ...` inside `[section]`.

    Only matches single-bracket table headers (`[section]`), never
    array-of-tables headers (`[[section]]`), and anchors the key match so a
    prefix collision (e.g. `template` vs `template_dir`) cannot cross-match.
    """
    key_pattern = re.compile(rf"^{re.escape(key)}\s*=")
    in_section = False
    for index, raw_line in enumerate(lines, start=1):
        stripped = raw_line.strip()
        if stripped.startswith("[[") and stripped.endswith("]]"):
            in_section = False
            continue
        if stripped.startswith("[") and stripped.endswith("]"):
            in_section = stripped[1:-1] == section
            continue
        if in_section and key_pattern.match(stripped):
            return index
    raise LookupError(f"could not locate `{key}` under [{section}] in alef.toml")


def add_file_refs(refs: list[PathRef], lines: list[str], section: str, key: str, paths: list[str]) -> None:
    if not paths:
        return
    line_number = find_line(lines, section, key)
    for path in paths:
        refs.append(
            PathRef(
                description=f"{section}.{key}",
                relative_path=path,
                line_number=line_number,
            )
        )


def add_dir_ref(refs: list[PathRef], lines: list[str], section: str, key: str, path: str | None) -> None:
    if not path:
        return
    refs.append(
        PathRef(
            description=f"{section}.{key}",
            relative_path=path,
            line_number=find_line(lines, section, key),
            must_be_dir=True,
        )
    )


def build_refs(config: dict, lines: list[str]) -> list[PathRef]:
    refs: list[PathRef] = []

    docs = config.get("workspace", {}).get("docs", {})
    add_file_refs(refs, lines, "workspace.docs.cli", "sources", docs.get("cli", {}).get("sources", []))
    add_file_refs(refs, lines, "workspace.docs.mcp", "sources", docs.get("mcp", {}).get("sources", []))
    add_dir_ref(
        refs,
        lines,
        "workspace.docs.skills",
        "template_dir",
        docs.get("skills", {}).get("template_dir"),
    )
    for path in docs.get("snippets", {}).get("dirs", []):
        add_dir_ref(refs, lines, "workspace.docs.snippets", "dirs", path)

    crates = config.get("crates", [])
    if isinstance(crates, dict):
        crates = [crates]

    for crate in crates:
        readme = crate.get("readme")
        if not readme:
            continue

        template_dir = readme.get("template_dir")
        snippets_dir = readme.get("snippets_dir")
        add_dir_ref(refs, lines, "crates.readme", "template_dir", template_dir)
        add_dir_ref(refs, lines, "crates.readme", "snippets_dir", snippets_dir)

        for target_name, target in readme.get("targets", {}).items():
            section = f"crates.readme.targets.{target_name}"
            template = target.get("template")
            if template and template_dir:
                refs.append(
                    PathRef(
                        description=f"{section}.template",
                        relative_path=str(Path(template_dir) / template),
                        line_number=find_line(lines, section, "template"),
                    )
                )

        for lang_name, lang in readme.get("languages", {}).items():
            section = f"crates.readme.languages.{lang_name}"

            template = lang.get("template")
            if template and template_dir:
                refs.append(
                    PathRef(
                        description=f"{section}.template",
                        relative_path=str(Path(template_dir) / template),
                        line_number=find_line(lines, section, "template"),
                    )
                )

            snippet_lang = lang.get("snippet_language", lang_name)
            snippets = lang.get("snippets", {})
            if snippets and snippets_dir:
                line_number = find_line(lines, section, "snippets")
                for snippet_key, snippet_path in snippets.items():
                    refs.append(
                        PathRef(
                            description=f"{section}.snippets.{snippet_key}",
                            relative_path=str(Path(snippets_dir) / snippet_lang / snippet_path),
                            line_number=line_number,
                        )
                    )

    return refs


def main() -> int:
    toml_path = Path(sys.argv[1]) if len(sys.argv) > 1 else Path("alef.toml")
    if not toml_path.is_file():
        print(f"error: {toml_path} does not exist or is not a file", file=sys.stderr)
        return 2

    repo_root = Path(sys.argv[2]).resolve() if len(sys.argv) > 2 else toml_path.parent.resolve()
    raw_text = toml_path.read_text(encoding="utf-8")
    lines = raw_text.splitlines()
    config = tomllib.loads(raw_text)

    refs = build_refs(config, lines)

    failures: list[PathRef] = []
    for ref in refs:
        resolved = repo_root / ref.relative_path
        ok = resolved.is_dir() if ref.must_be_dir else resolved.is_file()
        if not ok:
            failures.append(ref)

    if not failures:
        print(f"OK: {len(refs)} alef.toml path reference(s) resolved ({toml_path})")
        return 0

    print(
        f"FAIL: {len(failures)}/{len(refs)} alef.toml path reference(s) did not resolve:",
        file=sys.stderr,
    )
    for ref in sorted(failures, key=lambda r: r.line_number):
        kind = "directory" if ref.must_be_dir else "file"
        print(
            f"  alef.toml:{ref.line_number}: {ref.description} -> {ref.relative_path} (missing {kind})",
            file=sys.stderr,
        )
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
