# dibs

Agents call dibs on tasks with expiring leases, so a thousand agents can share one backlog without a central boss handing out work.

## What this is

The default shared checklist breaks under real concurrency in named ways, double claims, dead agents holding work forever, lost updates, and claim history buried in diffs. Dibs provides atomic claims with expiry as a standalone protocol that any fleet can adopt. A claim that is not renewed returns to the backlog on its own, so a dead agent's work frees itself. The live board of claims flickering, expiring, and being taken over is the operator's view of who holds what.

## Status

Seeded in October 2026. The build has not started. Part of the impatient programming portfolio, in the Fleet Operations arc.

## Licence

MIT. See LICENSE.
