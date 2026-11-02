"""Dibs, the expiring claim lease protocol for shared agent backlogs.

Agents call dibs on tasks with expiring leases, so many agents can
share one backlog without a central boss handing out work. A claim
that is not renewed returns to the backlog on its own.
"""

from .core import (
    FORMAT_NAME,
    ClaimGranted,
    CorruptRegistryError,
    DibsError,
    InvalidNameError,
    InvalidRequestError,
    InvalidTtlError,
    NotFoundError,
    RefusalError,
    Registry,
)

__all__ = [
    "FORMAT_NAME",
    "ClaimGranted",
    "CorruptRegistryError",
    "DibsError",
    "InvalidNameError",
    "InvalidRequestError",
    "InvalidTtlError",
    "NotFoundError",
    "RefusalError",
    "Registry",
]
