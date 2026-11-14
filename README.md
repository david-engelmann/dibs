# dibs

Agents call dibs on tasks with expiring leases, so parallel agents can share one backlog without a central boss handing out work, measured on one shared registry of ten thousand tasks, with every claim accounted for.

Dibs is a claim lease protocol for shared agent backlogs, carried by a Rust library with a command line and a thin Python binding. Work is offered to a backlog that lives in one registry file. An agent claims an item under a lease that expires unless the holder renews it, and the claim prints a token once. The token proves possession for as long as the lease stands, and the registry keeps the standing state and the full claim history in the same document.

[![CI](https://github.com/david-engelmann/dibs/actions/workflows/ci.yml/badge.svg)](https://github.com/david-engelmann/dibs/actions/workflows/ci.yml)

```
token MISnF1ZYy9dX7wp2qkbYofnvxVDtoLjGFufy-cjMLSI
epoch 1
expires_at 2026-11-07T15:19:21.298104+00:00
```

## Install

Dibs builds from source with Cargo. Clone the repo and install the binary onto your path.

```
git clone https://github.com/david-engelmann/dibs.git
cd dibs
cargo install --path . --bin dibs
```

The install puts the dibs binary in the Cargo bin directory. On a default install that directory is ~/.cargo/bin, and it must stand on your PATH for the dibs command to resolve. The command names the dibs binary on purpose. The repo also builds a codepoint_scan helper, and a machine that already carries that helper from a sibling repo refuses a plain install, so the named form installs dibs alone.

## Quickstart

Offer one task to a fresh backlog and claim it. The registry file is created by the first command that changes it.

```
dibs --registry backlog.json offer write-docs --note "draft the user guide"
```

```
offered write-docs
```

```
dibs --registry backlog.json claim --holder alice --ttl 3600 write-docs
```

```
token MISnF1ZYy9dX7wp2qkbYofnvxVDtoLjGFufy-cjMLSI
epoch 1
expires_at 2026-11-07T15:19:21.298104+00:00
```

The token prints once, at the claim that issues it. Nothing else in dibs ever prints it, and the registry stores only its hash. The board shows the standing lease.

```
dibs --registry backlog.json board
```

```
write-docs claimed holder=alice expires_at=2026-11-07T15:19:21.298104+00:00 epoch=1
```

## Usage

The examples below run in a new directory with a fresh registry, so every line of output follows from the commands as written.

Two items are offered and one is claimed, and the board shows the claim standing next to the open item.

```
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

```
dibs --registry backlog.json board
```

```
write-docs claimed holder=alice expires_at=2026-11-07T15:17:32.193891+00:00 epoch=1
fix-parser open epoch=0
```

The holder renews the lease and then releases it. A renew extends the standing lease from its current expiry by the time to live, never from the instant of the renew. Presenting the token and the epoch is what proves the lease is the holder's to move, and the epoch shown is the one the claim printed.

```
dibs --registry backlog.json renew --holder alice --token GLzH-0Q7SzjISfdBeJcjGa0LXhyVF9DG-Av3B-w6gDA --epoch 1 --ttl 1800 write-docs
dibs --registry backlog.json release --holder alice --token GLzH-0Q7SzjISfdBeJcjGa0LXhyVF9DG-Av3B-w6gDA --epoch 1 write-docs
```

```
expires_at 2026-11-07T15:47:32.193891+00:00
released write-docs
```

```
dibs --registry backlog.json board
```

```
write-docs open epoch=1
fix-parser open epoch=0
```

History returns the recorded events in order, one line per event, and an item name narrows the read to that item alone.

```
dibs --registry backlog.json history
```

```
1 offered item=write-docs holder=- at=2026-11-07T14:17:32.014944+00:00 epoch=0
2 offered item=fix-parser holder=- at=2026-11-07T14:17:32.102729+00:00 epoch=0
3 claimed item=write-docs holder=alice at=2026-11-07T14:17:32.193891+00:00 epoch=1
4 renewed item=write-docs holder=alice at=2026-11-07T14:17:32.312995+00:00 epoch=1
5 released item=write-docs holder=alice at=2026-11-07T14:17:32.371482+00:00 epoch=1
```

A lease that lapses frees its item on its own. The next claim takes the item over, the epoch rises, and the history records the takeover and names the previous holder. This run claims with a two second lease and waits three seconds before the second claim.

```
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

```
dibs --registry backlog.json history water-plants
```

```
6 offered item=water-plants holder=- at=2026-11-07T14:17:32.486318+00:00 epoch=0
7 claimed item=water-plants holder=bob at=2026-11-07T14:17:32.575013+00:00 epoch=1
8 taken_over item=water-plants holder=carol at=2026-11-07T14:17:35.682217+00:00 epoch=2 previous_holder=bob
```

The registry is read strictly. A document whose format name is wrong refuses every operation as corrupt, reads included, and the file is left exactly as it stands. Here the registry is written to a copy with the copy's format name changed to dibs-registry/2, and the board read against the copy prints one line on stderr and exits 1.

```
sed 's/dibs-registry\/1/dibs-registry\/2/' backlog.json > corrupt.json
dibs --registry corrupt.json board
```

```
error: the registry is corrupt, the format name is 'dibs-registry/2', not 'dibs-registry/1'
```

The Python binding drives the same lifecycle from Python. Build it inside a virtual environment with maturin, and every operation takes the registry path and the current instant as ISO 8601 text, so a Python caller names its own clock and no call reads one on its own. History takes no instant.

```
python3 -m venv .venv
.venv/bin/pip install maturin
.venv/bin/maturin develop --manifest-path bindings/Cargo.toml
```

```python
import dibs

registry = "binding-backlog.json"
dibs.offer(registry, "write-docs", "2031-05-04T12:00:00+00:00", note="draft the user guide")
granted = dibs.claim(registry, "write-docs", "alice", 3600, "2031-05-04T12:00:00+00:00")
print(granted["epoch"])
print(granted["expires_at"])
for row in dibs.board(registry, "2031-05-04T12:00:00+00:00"):
    print(row["id"], row["status"], row["holder"], row["epoch"])
```

```
1
2031-05-04T13:00:00+00:00
write-docs claimed alice 1
```

A refusal raises a subclass of DibsError carrying the same message text the command line prints, and the smoke test in tests/test_binding_smoke.py drives the full lifecycle through the binding, including a refusal it must raise.

## Status

What works. Every operation in the contract runs against the registry file. Claims are atomic, so two processes racing one item produce exactly one winner, and a lost race changes nothing on disk. A lease is judged lazily at the instant the caller names, so no daemon, timer, or background process ever runs. A token prints once at the claim and only its SHA-256 hash is stored, so the registry itself never carries a usable credential. Every claim and every takeover raises the item's epoch. A completed item is terminal for all time, and its id can never be offered again. The registry is read strictly, so a damaged document refuses every operation as corrupt and its bytes stay exactly as they stand. Updates publish by rename, so an interrupted run leaves the previous complete document or the new one, never a blend.

What does not. The command line reads the wall clock on every call and takes no clock override, so a caller that needs a fixed or virtual time drives the library or the binding, never the command line. The binding is thin and covers the same seven operations, and a refusal there raises instead of printing, which is the one shape difference between the two surfaces.

What is not built. Expiry is access-only, with no notifications and no watchers. There are no priorities and no queues, and the board lists items in offer order. One registry lives in one file on one machine, serving callers that share its filesystem, and there is no network service and no federation between registries. A lost token has no recovery path. The history is unbounded, with no compaction, pruning, or export, and it grows with every event for as long as the registry lives.

The registry format, dibs-registry/1, is a protocol surface that other programs pin, so its document shape and error text are load-bearing beyond this repo. The Rust test suite carries 135 root tests across every operation and the Python binding smoke test carries 4, and both suites pass on the current head with the binding built the way CI builds it.

Closed perfection rounds are 59 of 100. The program began as a seed commit whose state is independently verifiable against c1533bbc5fcfb803c1511d677d36c3c3925f075d.

## License

MIT. See [LICENSE](LICENSE).
