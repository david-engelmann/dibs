# dibs

Agents call dibs on tasks with expiring leases, so a thousand agents can share one backlog without a central boss handing out work.

## What this is

The default shared checklist breaks under real concurrency in named ways, double claims, dead agents holding work forever, lost updates, and claim history buried in diffs. Dibs provides atomic claims with expiry as a standalone protocol that any fleet can adopt. A claim that is not renewed returns to the backlog on its own, so a dead agent's work frees itself. The live board of claims flickering, expiring, and being taken over is the operator's view of who holds what.

## Status

The remote seed was verified on 2026-11-02 against the staged seed and its commit is c1533bbc5fcfb803c1511d677d36c3c3925f075d, a parentless commit holding exactly README.md and LICENSE and .gitignore. Setup arrives through a pull request and no work lands on main directly.

The contract lives in CONTRACT.md. The setup obligations S1 to S5 stand at its head, and the Round 1 amendment below them states the protocol obligations DB-1 to DB-13 together with the probe findings they rest on. The Rust port that opens Round 2 replaces the Python package with a Rust crate that carries the same claim lease protocol over one registry file in the dibs-registry/1 format, with the operations offer and claim and renew and release and complete and board and history through the dibs library and the dibs binary. Claims are atomic under one exclusive flock, expiry is lazy and evaluated at an instant every operation is given, possession is proven by a token whose hash alone is stored, leases are fenced by epoch, and the full claim history is recorded inside the same document as the state.

Closed perfection rounds are 1 of 100. A round counts only when a full cycle of work and audit and review and red team and approval has closed, and seeding and setup are not rounds.

The local gate is cargo test plus the codepoint scan run through cargo, run on the exact final head before any push, with the continuous integration workflow running the same commands as a backstop.

## Python binding

The crate carries a thin Python binding in the bindings directory, for the agent frameworks that drive a shared backlog directly from Python. It exposes the same seven operations as the command line, offer and claim and renew and release and complete and board and history, returning plain dictionaries of the lease and board and history facts, and a refusal raises a subclass of DibsError carrying the same message text the command line prints. Every operation that judges a lease takes the current instant as ISO 8601 text the library parses, exactly as the library takes it, and history takes no instant because it judges nothing, and no call reads a clock of its own. The binding holds no logic of its own and every rule stays in the Rust library, whose suite remains the gate. Build it with maturin develop --manifest-path bindings/Cargo.toml inside a virtual environment, and the smoke test in tests/test_binding_smoke.py drives the full claim lifecycle through it.

## Licence

MIT. See LICENSE.
