# Roadmap

Dibs owns the expiring claim lease backlog protocol for parallel AI agent swarms. One single registry file holds the backlog items, their active leases, their monotonic fencing epochs, and their verifiable claim history, with zero background daemons, zero timers, and zero network services.

## Fleet boundary

Mayfly makes the box, switchyard furnishes each parallel agent with a branch and port range and database and environment file, wayside checkpoints and restores session working state, and hookdoctor audits harness hook configurations and session traces. Dibs answers who holds which task right now, under what expiring lease and monotonic fencing epoch, and which tasks stand ready to claim. Integration with sibling tools happens strictly across clean data boundaries by passing claimed item identifiers and fencing epochs into downstream environments and checkpoints.

## Path to v1.0.0

### Slice DB-S1, lock hardening, path validation, and portable gate parity

Retired the unmaintained fs2 crate in favour of Rust 1.89 standard library file locks across Linux, macOS, and Windows. Enforced registry path validation before touching the filesystem while preserving operating system read failures on existing named directories. Split read only shared locking from umask respecting exclusive locking so board and history never create or modify lock files on disk and succeed inside read only directories. Protected open lock files against deletion on Windows via share mode flags, added bounded rename retries for Windows sharing contention, guarded APFS non UTF-8 directory staging in tests, added a portable Unix closed pipe stdout pin, reconciled README test counts, and added macOS and Windows continuous integration parity alongside the required Linux test and Python binding checks.

### Slice DB-S2, swarm ergonomics on dibs-registry/1

Kept the on disk format byte identical to dibs-registry/1 while landing high contention swarm ergonomics across the Rust library, command line, and Python binding. Added contention free atomic claim-next selection, all or nothing offer-batch ingestion, single item show inspection, deterministic JSON output across every data subcommand with zero token hash leakage, safe credential delivery via bounded standard input and environment variables, flexible registry path placement before or after subcommands with environment fallback, optional default UTC wall clock instants on the Python binding by explicit contract amendment, version reporting, and shell completion generation.

### Slice DB-S3, dibs-registry/2 task graphs, priorities, dead lettering, and compaction

Will introduce dibs-registry/2 with strict upward compatibility so registries using only v1 features continue to write dibs-registry/1 byte for byte. Will add directed acyclic dependency edges with iterative cycle detection, blocked and ready task filtering, four priority tiers, capability tags on offers and claims, takeover caps with dead letter exhaustion and explicit operator reset events, and crash idempotent history compaction to external JSON Lines archives verified by SHA-256 digests and baseline chain cross validation.

### Slice DB-S4, packaging isolation, typed Python distribution, and v1.0.0 release

Will isolate internal development helpers into a non published workspace crate so cargo install and crates.io packaging ship the dibs library and binary cleanly under the dibs-ai package name. Will consolidate root Python packaging metadata with maturin, ship PEP 561 py.typed markers and strict type stubs verified by mypy and pyright, add multi platform release automation for crates.io, PyPI wheels, prebuilt archives, and container images, complete final worktree and archive gates, and tag v1.0.0.
