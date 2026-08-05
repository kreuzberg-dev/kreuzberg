# xberg-ttf-parser

A safe, zero-allocation parser for TrueType, OpenType and AAT fonts, vendored
into Xberg from [ttf-parser](https://github.com/harfbuzz/ttf-parser) by Yevhenii
Reizner and contributors.

Vendored at upstream **v0.25.1** (dual MIT / Apache-2.0), plus the upstream
fixes listed below. The vendored copy preserves both upstream licenses; see
`LICENSE-MIT` and `LICENSE-APACHE`. See `ATTRIBUTIONS.md` at the repo root for
full attribution details.

This crate exists because upstream is currently unmaintained: at the time of
vendoring the last commit was 2025-11-22 and correctness fixes were sitting
unreviewed. Vendoring lets Xberg carry those fixes without waiting on an
upstream release.

## Carried upstream PRs

Fixes applied on top of v0.25.1. Each row is an upstream pull request
cherry-picked onto the vendored tree.

| Upstream PR               | Commit     | Fix                                                                        |
| ------------------------- | ---------- | -------------------------------------------------------------------------- |
| [harfbuzz/ttf-parser#203] | `b422ac0`  | Document that `Face::style()` falls back to the `head` mac style bits      |
| [harfbuzz/ttf-parser#207] | `52a9811`  | `Face::set_variation` returns `None` for an axis the face does not define  |
| [harfbuzz/ttf-parser#216] | `9b9e55f`  | Read the `fvar` `HIDDEN_AXIS` flag from bit 0, not reserved bit 3          |
| [harfbuzz/ttf-parser#222] | `3a585f1`  | Guard the CFF2 `blend` operator against an empty argument stack            |
| [harfbuzz/ttf-parser#223] | `32439d2`  | Promote `avar` axis value mapping to `i32` so it cannot silently wrap      |
| [harfbuzz/ttf-parser#224] | `dd2337b`  | Cap total component visits in `glyf` / `gvar` composite outlining          |
| [harfbuzz/ttf-parser#225] | `99aa5e3`  | Cap total paint-graph node visits in COLRv1 painting                       |
| [harfbuzz/ttf-parser#226] | `86daf57`  | Parse `loca` for a font carrying the maximum 65535 glyphs                  |
| [harfbuzz/ttf-parser#228] | `023f8163` | Ignore the deprecated `dotsection` operator in CFF charstrings             |

[harfbuzz/ttf-parser#203]: https://github.com/harfbuzz/ttf-parser/pull/203
[harfbuzz/ttf-parser#207]: https://github.com/harfbuzz/ttf-parser/pull/207
[harfbuzz/ttf-parser#216]: https://github.com/harfbuzz/ttf-parser/pull/216
[harfbuzz/ttf-parser#222]: https://github.com/harfbuzz/ttf-parser/pull/222
[harfbuzz/ttf-parser#223]: https://github.com/harfbuzz/ttf-parser/pull/223
[harfbuzz/ttf-parser#224]: https://github.com/harfbuzz/ttf-parser/pull/224
[harfbuzz/ttf-parser#225]: https://github.com/harfbuzz/ttf-parser/pull/225
[harfbuzz/ttf-parser#226]: https://github.com/harfbuzz/ttf-parser/pull/226
[harfbuzz/ttf-parser#228]: https://github.com/harfbuzz/ttf-parser/pull/228

Without #228, a CFF charstring containing `dotsection` (`12 0`) fails with
`UnsupportedOperator` and the whole glyph is dropped. Adobe's Type 1 to Type 2
conversion preserves the operator, so real-world fonts still carry it, and the
affected glyphs are exactly the dot-bearing ones: `i`, `j`, `!` and `.`. Pages
rendered from such a PDF silently lose those characters.

Two picks carry an xberg-authored follow-up commit rather than landing alone:
#216 and #207 ship no test upstream and are both silently reversible, so each
is followed by a regression test; #224's budget is charged per call, which does
not bound the work inside a call, so it is followed by a per-record charge.

The remaining open upstream PRs were evaluated and declined: #212 (skipping
null offsets in `LazyOffsetArray16`, which is indexed positionally, so skipping
would silently misalign glyph matching rather than fail closed), #214 (an
`alloc` feature with no consumer here, which also breaks
`--no-default-features --features gvar-alloc`), and #172 and #174 (public API
upstream has not accepted, with no caller in this workspace).

## How it reaches the rest of the workspace

Nothing in Xberg depends on `ttf-parser` directly. It arrives transitively
through `pdf_oxide` and `fontdb`, both of which request it by its crates.io
name. Those are the only two consumers in `Cargo.lock`; the shaper in tree is
`harfrust`, which does not use `ttf-parser` at all.

Cargo matches `[patch.crates-io]` entries on package name alone, so a crate
named `xberg-ttf-parser` cannot redirect them. The `compat/` subcrate carries
the upstream name, re-exports this crate, and is what the workspace patch entry
points at. One parser is compiled, so the types are identical across every
consumer.

    [patch.crates-io]
    ttf-parser = { path = "crates/xberg-ttf-parser/compat" }

The shim is never published; it exists only to satisfy the patch matcher.

## Modifications from upstream

The source is kept byte-identical to upstream wherever possible, so that future
upstream fixes cherry-pick without conflict. The deliberate differences are:

- Package renamed to `xberg-ttf-parser` with `[lib] name = "xberg_ttf_parser"`.
- Crate-level lint allows in `src/lib.rs` and `tests/tables/main.rs`, because the
  workspace lint set is stricter than upstream's CI. These are allowed rather
  than fixed: `div_ceil` and `is_multiple_of` postdate upstream's 1.63 MSRV, so
  rewriting them would break upstream's own build.
- A dev-dependency on `compat/`, so the vendored tests can keep referring to
  `ttf_parser` unchanged.
- The `tiny-skia-path` dev-dependency is `0.12` rather than upstream's `0.11`,
  matching the version already in the workspace so the lockfile does not carry
  two copies. The tests pass unchanged against it.
- `benches/`, `examples/`, `c-api/`, `testing-tools/` and `meson.build` are not
  vendored.
- The `glyf` / `gvar` component visit budget from upstream #224 is charged per
  component record rather than once per call. This is the only behavior-affecting
  xberg change inside the vendored `src/` tree; both sites carry a comment saying
  so. Upstream's version bounds how many times the outliner is entered but not
  how much work each entry does, since one glyph can carry 65535 components.
- Two regression tests live at the top level of `tests/`, named `xberg_*.rs`, so
  that `tests/tables/main.rs` stays byte-identical to upstream.

The public API is unchanged from upstream v0.25.1 except for `loca::Table`,
which #226 widens from `u16` to `u32` counters. No crate in this workspace's
dependency graph calls it.

## Updating

Cherry-pick the upstream commit onto `crates/xberg-ttf-parser/`, add a row to
the table above, and run:

    cargo test -p xberg-ttf-parser --all-features

## License

Dual-licensed under MIT or Apache-2.0, matching upstream.
