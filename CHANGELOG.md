# Changelog

All notable changes to `dibs` are recorded in this document.

## 1.0.0

First public stable release of `dibs`, the expiring claim lease backlog protocol for parallel AI agent swarms, published as `dibs-ai` on crates.io and PyPI and as `ghcr.io/david-engelmann/dibs` on GitHub Container Registry.

### Added

- Atomic contention-free next-task claiming with `claim-next` on the command line, `claim_next` and `claim_next_with_tags` on `Registry` in Rust, and `dibs.claim_next` in Python.
- All-or-nothing batch ingestion through `offer-batch`, `offer_many` on `Registry`, and `dibs.offer_many`, validating intra-batch forward and backward dependency edges atomically under one exclusive lock.
- Single-item inspection via `show`, ready-queue inspection via `ready`, and deterministic single-line `--json` output across all data subcommands with zero bearer token hash leakage.
- Safe credential and path delivery through `--token -` bounded standard input, `DIBS_TOKEN`, flexible `--registry` placement before or after subcommands, `DIBS_REGISTRY`, `--version`, and shell completions for `bash`, `zsh`, and `fish`.
- Optional default UTC wall-clock instants (`now=None`) across all lease-evaluating operations in the Python binding while keeping explicit `Instant` inputs in Rust and wall-clock honesty on the command line.
- Upward-compatible `dibs-registry/2` wire format preserving byte-for-byte `dibs-registry/1` output whenever `dibs-registry/2` features stand unused.
- Directed acyclic task dependency graphs (`depends_on`) verified by iterative heap-stack three-colour cycle detection, automatic `blocked` holding until prerequisites complete, four priority tiers from `0` critical to `3` low, and repeatable capability `tags` routing.
- Bounded `max_takeovers` dead-lettering into `exhausted` status with explicit operator recovery via `reset` and recorded `reset` history events.
- Crash-idempotent history compaction (`compact --through-seq --archive`) to external canonical JSON Lines archives with SHA-256 digest verification, baseline `takeover_count` cross-validation, and torn-line recovery.
- PEP 561 `py.typed` marker and complete `python/dibs/__init__.pyi` static type annotations verified under `mypy --strict` and `pyright`.
- Multi-stage non-root `Dockerfile`, `cargo-binstall` metadata, and five-target multi-platform release automation building precompiled archives, LF-terminated `SHA256SUMS.txt`, Python `abi3` wheels, source distributions, and container images across Linux (`x86_64` and `aarch64`), macOS (`arm64` and `x86_64`), and Windows (`x86_64`).

### Changed

- Migrated file locking to Rust 1.89 standard library `File` `lock` and `lock_shared`, splitting lock-free-fallback shared read locks on `board`, `ready`, `show`, and `history` from umask-respecting exclusive locks on mutations and protecting open lock files against deletion on Windows.
- Hardened registry and archive path validation before touching disk, rejecting empty, dot, root, trailing-separator, and aliased paths at exit `2` while preserving named-directory read failures at exit `1`.
- Isolated the internal `codepoint_scan` helper into `crates/codepoint_scan` with `publish = false` so `cargo install dibs-ai` installs the `dibs` binary alone without binary collisions.
