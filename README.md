# dibs

Agents call dibs on tasks with expiring leases, so parallel agents can share one backlog without a central boss handing out work, measured on one shared registry of ten thousand tasks, with every claim accounted for.

Dibs is a zero-daemon claim lease protocol for shared AI agent backlogs, carried by a Rust library (`dibs-ai`), the `dibs` command line, and a typed `abi3` Python package (`dibs-ai`, imported as `dibs`). Work is offered to a backlog that lives in one registry file on a shared filesystem, with optional directed acyclic task dependencies (`depends_on`), priorities (`0` critical to `3` low), capability `tags`, and takeover caps (`max_takeovers`). An agent claims a task under a lease that expires lazily unless the holder renews it, receiving a one-time cryptographic bearer token while the registry stores only its SHA-256 digest alongside monotonic fencing epochs and verifiable history.

[![CI](https://github.com/david-engelmann/dibs/actions/workflows/ci.yml/badge.svg)](https://github.com/david-engelmann/dibs/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/dibs-ai.svg)](https://crates.io/crates/dibs-ai)
[![PyPI](https://img.shields.io/pypi/v/dibs-ai.svg)](https://pypi.org/project/dibs-ai/)

```
token MISnF1ZYy9dX7wp2qkbYofnvxVDtoLjGFufy-cjMLSI
epoch 1
expires_at 2026-11-07T15:19:21.298104+00:00
```

## Install

Install the `dibs` command line from crates.io with Cargo or from prebuilt release archives with `cargo-binstall`.

```sh
cargo install dibs-ai
```

```sh
cargo binstall dibs-ai
```

Install the typed Python package (`import dibs`) from PyPI. Prebuilt `abi3` wheels cover Python 3.9 and newer across Linux, macOS, and Windows.

```sh
pip install dibs-ai
```

Run the non-root container image directly against a mounted working directory.

```sh
docker run --rm -v "$PWD:/work" ghcr.io/david-engelmann/dibs --help
```

To build and install from a local checkout, run Cargo from the repository root. Because the workspace isolates the internal `codepoint_scan` helper under `crates/codepoint_scan` with `publish = false`, installing the root crate places only `dibs` in `~/.cargo/bin`.

```sh
git clone https://github.com/david-engelmann/dibs.git
cd dibs
cargo install --path .
```

## Quickstart

Offer one task to a fresh backlog and claim it. The registry file is created by the first command that mutates it, and you can pass `--registry` before or after the subcommand or set `DIBS_REGISTRY`.

```sh
dibs --registry backlog.json offer write-docs --note "draft the user guide"
```

```
offered write-docs
```

```sh
dibs --registry backlog.json claim --holder alice --ttl 3600 write-docs
```

```
token MISnF1ZYy9dX7wp2qkbYofnvxVDtoLjGFufy-cjMLSI
epoch 1
expires_at 2026-11-07T15:19:21.298104+00:00
```

The bearer token prints once, at the claim that issues it. Nothing else in dibs ever prints it, and the registry stores only its lowercase hexadecimal SHA-256 digest. The board shows the standing lease.

```sh
dibs --registry backlog.json board
```

```
write-docs claimed holder=alice expires_at=2026-11-07T15:19:21.298104+00:00 epoch=1
```

## Usage

The examples below run in a new directory with a fresh registry, so every line of output follows from the commands as written.

### Atomic claims, renewals, releases, and lazy takeovers

Two items are offered and one is claimed, and the board shows the standing claim beside the open item. Whenever a registry uses only `dibs-registry/1` fields, dibs writes `dibs-registry/1` byte for byte.

```sh
dibs --registry backlog.json offer write-docs --note "draft the user guide"
dibs --registry backlog.json offer fix-parser --note "repair the date grammar"
dibs --registry backlog.json claim --holder alice --ttl 3600 write-docs
```

```
offered write-docs
offered fix-parser
token GLzH-0Q7SzjISfdBeJcjGa0LXhyVF9DG-Av3B-w6gDA
epoch 1
expires_at 2026-11-07T15:17:32.193891+00:00
```

The holder renews the lease and then releases it. A renew extends the standing lease from its current expiry by the time to live, never from the instant of the renew. Credentials can be passed with `--token`, piped safely over bounded standard input with `--token -`, or read from `DIBS_TOKEN`.

```sh
printf '%s\n' "GLzH-0Q7SzjISfdBeJcjGa0LXhyVF9DG-Av3B-w6gDA" | \
  dibs --registry backlog.json renew --holder alice --token - --epoch 1 --ttl 1800 write-docs
DIBS_TOKEN="GLzH-0Q7SzjISfdBeJcjGa0LXhyVF9DG-Av3B-w6gDA" \
  dibs --registry backlog.json release --holder alice --epoch 1 write-docs
```

```
expires_at 2026-11-07T15:47:32.193891+00:00
released write-docs
```

History returns the recorded events in order, one line per event, and an item name narrows the read to that item alone.

```sh
dibs --registry backlog.json history
```

```
1 offered item=write-docs holder=- at=2026-11-07T14:17:32.014944+00:00 epoch=0
2 offered item=fix-parser holder=- at=2026-11-07T14:17:32.102729+00:00 epoch=0
3 claimed item=write-docs holder=alice at=2026-11-07T14:17:32.193891+00:00 epoch=1
4 renewed item=write-docs holder=alice at=2026-11-07T14:17:32.312995+00:00 epoch=1
5 released item=write-docs holder=alice at=2026-11-07T14:17:32.371482+00:00 epoch=1
```

A lease that lapses frees its item on its own with zero background timers. The next claim takes the item over, increments the monotonic fencing `epoch`, and records `taken_over` naming `previous_holder`. This run claims with a two second lease and waits three seconds before the second claim.

```sh
dibs --registry backlog.json offer water-plants --note "twice a week"
dibs --registry backlog.json claim --holder bob --ttl 2 water-plants
sleep 3
dibs --registry backlog.json claim --holder carol --ttl 3600 water-plants
```

```
offered water-plants
token X0FAUYvyajsCgC9DAlBNCbQXUZJXltcw-9oSOTOBA6w
epoch 1
expires_at 2026-11-07T14:17:34.575013+00:00
token DooZVPBD5EkGn6-Jsz9sKrdYfdwDEEyQghoun61Pn0s
epoch 2
expires_at 2026-11-07T15:17:35.682217+00:00
```

```sh
dibs --registry backlog.json history water-plants
```

```
6 offered item=water-plants holder=- at=2026-11-07T14:17:32.486318+00:00 epoch=0
7 claimed item=water-plants holder=bob at=2026-11-07T14:17:32.575013+00:00 epoch=1
8 taken_over item=water-plants holder=carol at=2026-11-07T14:17:35.682217+00:00 epoch=2 previous_holder=bob
```

### Swarm DAGs, priorities, capability tags, dead-letter reset, and compaction

Under `dibs-registry/2`, `offer-batch` ingests JSON Lines batches all or nothing under one exclusive lock, allowing forward and backward intra-batch `depends_on` edges while rejecting cycles through iterative depth-first search. Downstream tasks remain `blocked` until every prerequisite completes, `ready` and `claim-next` route work in deterministic `(priority, offer_order)` order filtered by repeatable `--tag` flags, and `-j` or `--json` emits deterministic single-line JSON on every data subcommand.

```sh
cat <<'EOF' | dibs --registry swarm.json offer-batch --json
{"id": "deploy-api", "priority": 0, "tags": ["linux", "prod"], "depends_on": ["run-tests"], "max_takeovers": 2}
{"id": "run-tests", "priority": 1, "tags": ["linux", "gpu"], "depends_on": ["build-core"]}
{"id": "build-core", "priority": 0, "tags": ["linux", "gpu"]}
EOF
```

```json
{"count": 3, "offered": ["deploy-api", "run-tests", "build-core"]}
```

Parallel workers call `claim-next` to claim the highest-priority unblocked task matching their capability tags under one atomic lock cycle without thundering-herd retries, inspect items with `show`, and compact historical prefixes crash-idempotently to canonical JSON Lines archives with `compact`.

```sh
dibs --registry swarm.json claim-next --holder worker-1 --ttl 900 --tag linux --tag gpu --json
dibs --registry swarm.json ready --tag linux
dibs --registry swarm.json show deploy-api --json
dibs --registry swarm.json compact --through-seq 3 --archive swarm-archive.jsonl --json
dibs completions zsh > _dibs
```

When a task configured with `--max-takeovers` expires after reaching its takeover cap, its effective status becomes `exhausted` so workers skip it until an operator clears `takeover_count` back to `0` with `reset`.

```sh
dibs --registry swarm.json offer flaky-step --max-takeovers 1
dibs --registry swarm.json claim --holder bob --ttl 1 flaky-step
sleep 2
dibs --registry swarm.json claim --holder carol --ttl 1 flaky-step
sleep 2
dibs --registry swarm.json reset flaky-step --json
```

```json
{"item": "flaky-step", "status": "open", "epoch": 2, "takeover_count": 0}
```

### Strict corruption refusal

The registry is read strictly. Duplicate JSON keys, unknown or missing fields, history-and-state disagreements, or an unsupported format string refuse every operation as corrupt at exit `1` and leave the file untouched on disk.

```sh
sed 's/dibs-registry\/1/dibs-registry\/99/' backlog.json > corrupt.json
dibs --registry corrupt.json board
```

```
error: the registry is corrupt, the format name is 'dibs-registry/99', not 'dibs-registry/1' or 'dibs-registry/2'
```

### Typed Python API

The `dibs-ai` Python package ships PEP 561 `py.typed` markers and complete `python/dibs/__init__.pyi` stubs checked under `mypy --strict` and `pyright`. Every lease-evaluating call accepts an optional ISO 8601 `now` string for deterministic testing and defaults to the UTC wall clock when omitted or `None`.

```python
import dibs

registry = "swarm-python.json"
dibs.offer_many(
    registry,
    [
        {"id": "compile", "priority": 0, "tags": ["linux"]},
        {"id": "verify", "priority": 1, "tags": ["linux"], "depends_on": ["compile"]},
    ],
)

lease = dibs.claim_next(registry, "agent-1", 1800, tags=["linux"])
dibs.complete(registry, lease["item"], lease["holder"], lease["token"], lease["epoch"])

for task in dibs.ready(registry, tags=["linux"]):
    print(task["id"], task["effective_status"], task["priority"])
```

```
verify open 1
```

A refusal raises a typed subclass of `dibs.DibsError` (`InvalidNameError`, `InvalidTtlError`, `InvalidRequestError`, `NotFoundError`, `RefusalError`, `CorruptError`, or `FailureError`) carrying the exact message text the command line prints.

## Status

What works. Every operation in the contract runs against the registry file across Rust, the command line, and Python. Claims are atomic under standard library `File` exclusive `lock`, so concurrent processes racing one item produce one winner, read-only queries (`board`, `ready`, `show`, and `history`) coordinate through `File` `lock_shared` without creating lock files on disk, and parallel workers calling `claim-next` claim distinct ready tasks in one lock cycle without retries. Batch ingestion through `offer-batch` and `offer_many` validates and publishes task batches all or nothing. Directed acyclic dependency graphs (`depends_on`) refuse cycles via iterative three-colour traversal, hold dependents `blocked` until prerequisites complete, and unblock downstream tasks immediately upon prerequisite completion. Priorities from `0` critical to `3` low and capability `tags` order `ready` and `claim-next` deterministically, `max_takeovers` transitions repeatedly abandoned tasks into `exhausted` until an operator runs `reset`, and `compact` crash-idempotently archives prefix events to canonical JSON Lines with SHA-256 digest and baseline cross-validation under `dibs-registry/2` while preserving `dibs-registry/1` byte for byte whenever `dibs-registry/2` features stand unused. Every data subcommand supports deterministic single-line `--json` output with zero token hash leakage, credentials can arrive through bounded standard input or `DIBS_TOKEN`, and PEP 561 stubs cover the entire Python surface at 100 percent type completeness.

What does not. The command line reads the UTC wall clock on every call and takes no clock override flag, so callers needing fixed or virtual instants drive the Rust `Registry` API or pass an explicit instant string to Python `dibs` functions.

What is not built. Expiry is evaluated strictly on access, with no daemons, timers, notifications, or watchers. One registry lives in one file on one shared filesystem, with no network service and no cross-file federation. A lost live token has no recovery path before its lease expires.

The registry formats, `dibs-registry/1` and `dibs-registry/2`, are protocol surfaces that other programs pin, so their document shapes and error texts are load-bearing beyond this repo. The Rust workspace suite carries 204 tests across 189 root `dibs-ai` tests and 15 `codepoint_scan` tests, running 203 on Unix (188 root and 15 scanner) and 192 on Windows (180 root and 12 scanner), and the Python binding smoke test carries 12, and all suites alongside `mypy --strict` and `pyright` pass on the current head.

Closed perfection rounds are 100 of 100. The program began as a seed commit whose state is independently verifiable against c1533bbc5fcfb803c1511d677d36c3c3925f075d.

## License

MIT. See [LICENSE](LICENSE).
