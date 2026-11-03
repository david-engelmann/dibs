# Fixture manifest

These registry files are frozen ground truth for the Rust port that opens Round 2. Each one was written by the Python implementation of dibs on main, driven through the public Registry API with injected instants, so the bytes are the bytes that implementation writes for the same operations. The base instant of every production run is 2031-05-04T12:00:00+00:00, the same base instant the Round 1 test suite uses. The port suite loads each file under Rust, checks the recorded board and history truths against it, and continues the standing claims with the recorded tokens. A token appears in this manifest only, never in a registry file, and every token here is a synthetic value drawn during production.

The empty registry was published through the module's own publish path on the module's own empty document, because no sequence of public operations writes an empty document. Every other fixture is the product of public operations only. The lock files the production runs created beside the registries were production byproducts and are not fixtures, so they were removed.

## Inventory

- `empty.json` stands at 66 bytes with SHA-256 `2cf0628996bdfcc24bdf5cff1133712dbde1798d5087893b8fa22a9574378b97`. It holds no items and no events.
- `offers.json` stands at 825 bytes with SHA-256 `9bd88441ba5b04b7e2c039aa7bbf9530bab3c78d728509f2a52523f977dac025`. It holds two offered events and a board of task-alpha open at epoch 0 and task-beta open at epoch 0.
- `claimed.json` stands at 739 bytes with SHA-256 `4af3d1169347fe15ab3570b12c1250c466af76cca75842ff955dc01eeebd31e9`. It holds an offered event and a claimed event, and task-alpha stands claimed by agent-a at epoch 1 with expiry 2031-05-04T12:10:10+00:00. The live token for the claim is `J-55nqLzVcGYFXUo0PG0Y-jSixW_9ub2u_FI3GTSNhw`.
- `renewed.json` stands at 937 bytes with SHA-256 `3db3e42b1f1394f83ee6d0bcd82dee72adf4fc883d619433963f389e571328f7`. It holds offered and claimed and renewed events, and task-alpha stands claimed by agent-a at epoch 1 with expiry 2031-05-04T12:15:00+00:00 after one renewal. The live token for the claim is `gZrIqNVGgaTWd0eRH702NHHBNEKssae7a23IaAp0T8o`.
- `released.json` stands at 845 bytes with SHA-256 `c72e35709b9ace2a79aa28ba1e96851c8eb57ec91cf0dabe6b842c95d246a01b`. It holds offered and claimed and released events, and task-alpha stands open at epoch 1.
- `completed.json` stands at 851 bytes with SHA-256 `5d0b8dcee187a888c539d6566b5b51b26f5688a403327aa81c3341b3c9ad93fc`. It holds offered and claimed and completed events, and its board is empty while its history stays readable.
- `taken-over.json` stands at 945 bytes with SHA-256 `a94e08110bfe5a0580a97cbfd792ac9e2eeb678f65a27936366add7f15de51b1`. It holds offered and claimed and taken_over events, the taken_over event names agent-a as the previous holder, and task-alpha stands claimed by agent-b at epoch 2 with expiry 2031-05-04T12:12:00+00:00. The live token for the standing claim is `g8lm6gLOTd3ggd2Zon0D0hpNApNe9H6c3mfcEC9NWOY`, and the fenced token of the first claim is `eIE6nDz4GwTmg1Xfbi_MClZ53yGokB0d5tJ58defsSA`.
- `mixed.json` stands at 2563 bytes with SHA-256 `90acad5aaee8df5aa46b554a2f874b645eed9a46703af39c2b02848d3f470c6f`. It holds four offered events, three claimed events, and one completed event. At twenty seconds past the base instant its board reads task-alpha open at epoch 0, task-beta claimed by agent-a at epoch 1 with expiry 2031-05-04T12:10:04+00:00, and task-gamma claimed by agent-c at epoch 1 with expiry 2031-05-04T12:00:35+00:00, and task-delta stands completed and off the board. The live token for task-beta is `yTkpBTNBr0ATZAswyoeJDRP3-nkRTxTPOmfsyIBn7Ug`, and the token for the short task-gamma claim is `FY_jsxhU50NePKEoHuk9bzNVvivgBAfLdf11qU6Yu6o`.

## Producing script

The script below produced every fixture above when run against the Python implementation on main. It is recorded here as the provenance of the frozen bytes. The port ships no Python, so the script stands as a record and not as a runnable part of this repo.

```python
"""Produce the frozen registry fixtures for the dibs Rust port.

Every fixture is written by the Python implementation of dibs on
main, driven through the public Registry API with injected
instants, so the bytes are exactly the bytes the Round 1
implementation writes. The empty registry is published through
the module's own publish path on the module's own empty
document. Tokens granted during production are printed for the
fixture manifest, because a fixture that carries a live claim is
only useful to the Rust suite if the token that continues the
claim is recorded beside it. The tokens are synthetic values
drawn during this production run and they appear nowhere in the
registry bytes themselves.
"""

import hashlib
import json
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent / "pybase" / "tree"))

from dibs import core  # noqa: E402
from dibs.core import Registry  # noqa: E402

T0 = datetime(2031, 5, 4, 12, 0, 0, tzinfo=timezone.utc)
OUT = Path.home() / "workspace" / "chat3-dibs-port" / "tests" / "fixtures"


def at(seconds):
    return T0 + timedelta(seconds=seconds)


def produce():
    OUT.mkdir(parents=True, exist_ok=True)
    tokens = {}

    empty = Registry(OUT / "empty.json")
    empty._publish(core._empty_document())

    offers = Registry(OUT / "offers.json")
    offers.offer("task-alpha", "first payload", T0)
    offers.offer("task-beta", "second payload", at(5))

    claimed = Registry(OUT / "claimed.json")
    claimed.offer("task-alpha", "first payload", T0)
    grant = claimed.claim("task-alpha", "agent-a", 600, at(10))
    tokens["claimed.json"] = {"task-alpha": grant.token}

    renewed = Registry(OUT / "renewed.json")
    renewed.offer("task-alpha", "first payload", T0)
    grant = renewed.claim("task-alpha", "agent-a", 600, T0)
    renewed.renew("task-alpha", "agent-a", grant.token, 1, 300, at(100))
    tokens["renewed.json"] = {"task-alpha": grant.token}

    released = Registry(OUT / "released.json")
    released.offer("task-alpha", "first payload", T0)
    grant = released.claim("task-alpha", "agent-a", 600, T0)
    released.release("task-alpha", "agent-a", grant.token, 1, at(30))

    completed = Registry(OUT / "completed.json")
    completed.offer("task-alpha", "first payload", T0)
    grant = completed.claim("task-alpha", "agent-a", 600, T0)
    completed.complete("task-alpha", "agent-a", grant.token, 1, at(30))

    taken = Registry(OUT / "taken-over.json")
    taken.offer("task-alpha", "first payload", T0)
    first = taken.claim("task-alpha", "agent-a", 60, T0)
    second = taken.claim("task-alpha", "agent-b", 600, at(120))
    tokens["taken-over.json"] = {
        "task-alpha-first": first.token,
        "task-alpha": second.token,
    }

    mixed = Registry(OUT / "mixed.json")
    mixed.offer("task-alpha", "alpha payload", T0)
    mixed.offer("task-beta", "beta payload", at(1))
    mixed.offer("task-gamma", "gamma payload", at(2))
    mixed.offer("task-delta", "delta payload", at(3))
    beta = mixed.claim("task-beta", "agent-a", 600, at(4))
    gamma = mixed.claim("task-gamma", "agent-c", 30, at(5))
    delta = mixed.claim("task-delta", "agent-d", 600, at(6))
    mixed.complete("task-delta", "agent-d", delta.token, 1, at(10))
    tokens["mixed.json"] = {"task-beta": beta.token, "task-gamma": gamma.token}

    inventory = {}
    for path in sorted(OUT.glob("*.json")):
        raw = path.read_bytes()
        inventory[path.name] = {
            "sha256": hashlib.sha256(raw).hexdigest(),
            "bytes": len(raw),
        }
    print(json.dumps({"tokens": tokens, "inventory": inventory}, indent=2))


if __name__ == "__main__":
    produce()
```
