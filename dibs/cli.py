"""Command line for dibs, a thin shell over dibs.core.

Every command names the registry file first. The clock the command
line uses is the wall clock in UTC, and no flag overrides it, so the
expiry a caller sees is the expiry the machine clock gives. Errors
the protocol raises are printed as one line on stderr with the exit
code the error carries, and no traceback is ever printed for them.
"""

import argparse
import sys
from datetime import datetime, timezone

from . import core
from .core import DibsError, Registry


def _now():
    return datetime.now(timezone.utc)


def build_parser():
    parser = argparse.ArgumentParser(
        prog="dibs",
        description=(
            "Claim tasks from a shared backlog under leases that "
            "expire unless they are renewed."
        ),
    )
    parser.add_argument(
        "--registry",
        required=True,
        help="the registry file holding the backlog, created by the first mutation",
    )
    sub = parser.add_subparsers(dest="command", required=True)

    p_offer = sub.add_parser("offer", help="add an item to the backlog")
    p_offer.add_argument("item")
    p_offer.add_argument(
        "--note", default="", help="a short payload note for the item"
    )

    p_claim = sub.add_parser("claim", help="lease an item under a new claim")
    p_claim.add_argument("item")
    p_claim.add_argument("--holder", required=True)
    p_claim.add_argument(
        "--ttl", required=True, type=int, help="the lease length in seconds"
    )

    for name, help_text in (
        ("renew", "extend the standing lease from its current expiry"),
        ("release", "end the standing lease early"),
        ("complete", "mark the item done under its standing lease"),
    ):
        p_lease = sub.add_parser(name, help=help_text)
        p_lease.add_argument("item")
        p_lease.add_argument("--holder", required=True)
        p_lease.add_argument("--token", required=True)
        p_lease.add_argument("--epoch", required=True, type=int)
        if name == "renew":
            p_lease.add_argument(
                "--ttl", required=True, type=int,
                help="the added lease length in seconds",
            )

    sub.add_parser("board", help="show the open items and their standing leases")
    p_history = sub.add_parser("history", help="show the recorded claim history")
    p_history.add_argument("item", nargs="?")
    return parser


def _board_line(row):
    if row["status"] == "claimed":
        return (
            f"{row['id']} claimed holder={row['holder']} "
            f"expires_at={row['expires_at']} epoch={row['epoch']}"
        )
    return f"{row['id']} open epoch={row['epoch']}"


def _history_line(event):
    line = (
        f"{event['seq']} {event['kind']} item={event['item']} "
        f"holder={event['holder'] or '-'} at={event['at']} epoch={event['epoch']}"
    )
    if event["previous_holder"] is not None:
        line += f" previous_holder={event['previous_holder']}"
    return line


def main(argv=None):
    args = build_parser().parse_args(argv)
    registry = Registry(args.registry)
    try:
        if args.command == "offer":
            registry.offer(args.item, args.note, _now())
            print(f"offered {args.item}")
        elif args.command == "claim":
            granted = registry.claim(args.item, args.holder, args.ttl, _now())
            print(f"token {granted.token}")
            print(f"epoch {granted.epoch}")
            print(f"expires_at {core.format_instant(granted.expires_at)}")
        elif args.command == "renew":
            expires = registry.renew(
                args.item, args.holder, args.token, args.epoch, args.ttl, _now()
            )
            print(f"expires_at {core.format_instant(expires)}")
        elif args.command == "release":
            registry.release(args.item, args.holder, args.token, args.epoch, _now())
            print(f"released {args.item}")
        elif args.command == "complete":
            registry.complete(args.item, args.holder, args.token, args.epoch, _now())
            print(f"completed {args.item}")
        elif args.command == "board":
            for row in registry.board(_now()):
                print(_board_line(row))
        elif args.command == "history":
            for event in registry.history(args.item):
                print(_history_line(event))
    except DibsError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return exc.exit_code
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
