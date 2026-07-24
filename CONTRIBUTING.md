# Contributing to Xberg

Thank you for your interest in contributing to Xberg! Whether you're fixing a typo, adding a feature, or improving documentation, every contribution makes a difference.

## First time contributing?

Welcome! We're glad you're here. Start by choosing an issue that matches your experience level:

- [Good first issue](https://github.com/xberg-io/xberg/issues?q=is%3Aissue+is%3Aopen+label%3A%22good+first+issue%22) — small, well-scoped tasks ideal for newcomers
- [Help wanted](https://github.com/xberg-io/xberg/issues?q=is%3Aissue+is%3Aopen+label%3A%22help+wanted%22) — tasks where we'd especially appreciate community help

> **Tip:** Pick an issue you feel confident about. If you're unsure about scope or approach, leave a comment on the issue and we'll help you get started.

Want to work on something bigger or propose a new feature? [Open a discussion](https://github.com/xberg-io/xberg/issues) with maintainers first.

## Jump right in

**Quick fixes** (typos, small doc improvements):

1. Edit the file directly on GitHub
2. Submit a pull request — that's it!

**Larger contributions** (features, new bindings, bug fixes):

1. Read the full [Contributing Guide](https://docs.xberg.io/contributing/) on our docs site
2. Set up your development environment (see below)
3. Follow our workflow: branch → code → test → PR

## What can I contribute to?

Xberg is a polyglot project with many areas where you can help:

| Area                  | Description                                                    |
| --------------------- | -------------------------------------------------------------- |
| **Rust core**         | Parser implementations, extraction pipeline, performance       |
| **Language bindings** | Python, TypeScript, Ruby, Go, Java, C#, PHP, Elixir, WASM   |
| **Documentation**     | Guides, API references, examples, tutorials                    |
| **Testing**           | Unit tests, E2E test fixtures, cross-language coverage         |
| **Plugins**           | New extraction plugins, plugin system improvements             |
| **CI/CD**             | Build pipeline, cross-architecture support, release automation |

## Development setup

### System dependencies

**Required for all contributions:**

- [Git](https://git-scm.com/)
- [Task](https://taskfile.dev/installation/) — our task runner for all build and test workflows
- [Rust](https://rustup.rs/) stable (via `rustup`) — the `wasm32-unknown-unknown` target is configured automatically via `rust-toolchain.toml`
- [poly](https://github.com/goldziher/homebrew-tap) — our lint/format/pre-commit tool: `brew install goldziher/tap/poly` (`task setup` installs it automatically if missing)

**Required for WASM builds** (tree-sitter and tesseract compile C/C++ to wasm):

- [WASI SDK](https://github.com/WebAssembly/wasi-sdk/releases) — install to `$HOME/wasi-sdk` or set `WASI_SDK_PATH`

**Language-specific toolchains** (only install what you need):

| Language | Version | Tool                                     |
| -------- | ------- | ---------------------------------------- |
| Python   | 3.10+   | [`uv`](https://docs.astral.sh/uv/)       |
| Node.js  | 20+     | [`pnpm`](https://pnpm.io/)               |
| Ruby     | 3.2+    | `rbenv` or `rvm`                         |
| Go       | 1.26+   | [Official installer](https://go.dev/dl/) |
| Java     | 25+     | JDK (via [sdkman](https://sdkman.io/))   |
| .NET     | 10+     | `dotnet`                                 |
| PHP      | 8.1+    | `composer`                               |
| Elixir   | 1.14+   | `mix` (OTP 25+)                          |

### Getting started

```bash
task setup
```

This installs all toolchains and dependencies across every language. Safe to re-run anytime.

## Pre-commit hooks

Install the git hooks with `task setup` (or `poly hooks install` directly). On
every commit, poly runs lint, format, and file-safety checks plus `cargo clippy`;
the commit-msg hook validates the message. Run all hooks manually with
`poly hooks run pre-commit --all-files`.

## Quick reference

| Command       | What it does                                    |
| ------------- | ----------------------------------------------- |
| `task setup`  | Install all dependencies (idempotent)           |
| `task build`  | Build all language bindings                     |
| `task test`   | Run all test suites                             |
| `task lint`   | Run all linters (with auto-fix)                 |
| `task format` | Format all code                                 |
| `task check`  | Combined lint + format check (no modifications) |

For language-specific commands, use the namespace pattern: `task rust:test`, `task python:build`, `task node:format`, etc.

For the complete development workflow, build profiles, coding standards, and PR guidelines, see the full [Contributing Guide](https://docs.xberg.io/contributing/).

## Commit messages

We use [Conventional Commits](https://www.conventionalcommits.org/). Prefix your commit messages with a type:

- `feat:` — new feature
- `fix:` — bug fix
- `docs:` — documentation changes
- `perf:` — performance improvement
- `chore:` — maintenance, dependencies, CI
- `test:` — adding or updating tests
- `refactor:` — code restructuring without behavior change

## Community

- **Star the repo:** [Give us a star on GitHub](https://github.com/xberg-io/xberg) — it helps others discover Xberg!
- **Documentation:** [docs.xberg.io](https://docs.xberg.io)
- **Discord:** [Join our community](https://discord.gg/xt9WY3GnKR)
- **Issues:** [GitHub Issues](https://github.com/xberg-io/xberg/issues)
- **License:** [MIT License (MIT)](LICENSE)

Thank you for helping make Xberg better!
