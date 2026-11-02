# dibs

Agents call dibs on tasks with expiring leases, so a thousand agents can share one backlog without a central boss handing out work.

## What this is

The default shared checklist breaks under real concurrency in named ways, double claims, dead agents holding work forever, lost updates, and claim history buried in diffs. Dibs provides atomic claims with expiry as a standalone protocol that any fleet can adopt. A claim that is not renewed returns to the backlog on its own, so a dead agent's work frees itself. The live board of claims flickering, expiring, and being taken over is the operator's view of who holds what.

## Status

The remote seed was verified on 2026-11-02 against the staged seed and its commit is c1533bbc5fcfb803c1511d677d36c3c3925f075d, a parentless commit holding exactly README.md and LICENSE and .gitignore. Setup arrives through a pull request and no work lands on main directly.

The contract lives in CONTRACT.md. Setup obligations S1 to S5 are the contract state until the full contract lands as the first build act of this repo, and this setup slice claims no feature behaviour for dibs.

Closed perfection rounds are 0 of 100. A round counts only when a full cycle of work and audit and review and red team and approval has closed, and seeding and setup are not rounds.

The local gate is python -m pytest -q plus python scripts/codepoint_scan.py, run on the exact final head before any push, with the continuous integration workflow running the same commands as a backstop.

## Licence

MIT. See LICENSE.
