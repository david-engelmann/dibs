"""Static typing verification harness for PEP 561 stubs under mypy --strict and pyright."""

from __future__ import annotations

import pathlib

import dibs
from dibs import (
    BoardRowDict,
    ClaimGrantedDict,
    CompactSummaryDict,
    CorruptError,
    DibsError,
    EventDict,
    FailureError,
    InvalidNameError,
    InvalidRequestError,
    InvalidTtlError,
    ItemDetailDict,
    NotFoundError,
    OfferSpecDict,
    RefusalError,
    RenewedDict,
    StrPath,
)


def verify_all_signatures(reg_str: str, reg_path: pathlib.Path, archive_path: pathlib.Path) -> None:
    version_text: str = dibs.__version__
    assert isinstance(version_text, str)

    p1: StrPath = reg_str
    p2: StrPath = reg_path

    dibs.offer(p1, "task-a")
    dibs.offer(
        p2,
        "task-b",
        "2031-05-04T12:00:00+00:00",
        "note text",
        priority=0,
        tags=["linux", "gpu"],
        depends_on=("task-a",),
        max_takeovers=3,
    )

    spec_min: OfferSpecDict = {"id": "task-c"}
    spec_full: OfferSpecDict = {
        "id": "task-d",
        "note": None,
        "priority": 1,
        "tags": ["linux"],
        "depends_on": ["task-c"],
        "max_takeovers": None,
    }
    offered_ids: list[str] = dibs.offer_many(reg_path, ["task-e", spec_min, spec_full], None)
    assert offered_ids

    granted: ClaimGrantedDict = dibs.claim(reg_path, "task-a", "worker-1", 60, None)
    g_item: str = granted["item"]
    g_holder: str = granted["holder"]
    g_token: str = granted["token"]
    g_epoch: int = granted["epoch"]
    g_exp: str = granted["expires_at"]
    assert g_item and g_holder and g_token and g_epoch >= 1 and g_exp

    next_granted: ClaimGrantedDict = dibs.claim_next(reg_str, "worker-2", 120, tags=("linux",))
    renewed: RenewedDict = dibs.renew(
        reg_path,
        next_granted["item"],
        next_granted["holder"],
        next_granted["token"],
        next_granted["epoch"],
        30,
    )
    r_exp: str = renewed["expires_at"]
    assert r_exp

    dibs.release(
        reg_path,
        next_granted["item"],
        next_granted["holder"],
        next_granted["token"],
        next_granted["epoch"],
        "2031-05-04T12:00:05+00:00",
    )
    dibs.complete(reg_str, g_item, g_holder, g_token, g_epoch)
    dibs.reset(reg_path, "task-b", None)

    rows: list[BoardRowDict] = dibs.board(reg_path)
    for row in rows:
        r_id: str = row["id"]
        r_note: str = row["note"]
        r_status: str = row["status"]
        r_holder: str | None = row["holder"]
        r_expires: str | None = row["expires_at"]
        r_epoch: int = row["epoch"]
        _ = (r_id, r_note, r_status, r_holder, r_expires, r_epoch)

    ready_items: list[ItemDetailDict] = dibs.ready(reg_str, None, tags=["linux"])
    for det in ready_items:
        d_id: str = det["id"]
        d_note: str = det["note"]
        d_status: str = det["status"]
        d_eff: str = det["effective_status"]
        d_prio: int = det["priority"]
        d_tags: list[str] = det["tags"]
        d_deps: list[str] = det["depends_on"]
        d_holder: str | None = det["holder"]
        d_expires: str | None = det["expires_at"]
        d_epoch: int = det["epoch"]
        d_tc: int = det["takeover_count"]
        d_mt: int | None = det["max_takeovers"]
        _ = (
            d_id,
            d_note,
            d_status,
            d_eff,
            d_prio,
            d_tags,
            d_deps,
            d_holder,
            d_expires,
            d_epoch,
            d_tc,
            d_mt,
        )

    single: ItemDetailDict = dibs.show(reg_path, "task-a")
    assert single["id"] == "task-a"

    events_all: list[EventDict] = dibs.history(reg_str)
    events_item: list[EventDict] = dibs.history(reg_path, "task-a")
    for ev in events_all + events_item:
        e_seq: int = ev["seq"]
        e_kind: str = ev["kind"]
        e_item: str = ev["item"]
        e_holder: str | None = ev["holder"]
        e_at: str = ev["at"]
        e_epoch: int = ev["epoch"]
        e_prev: str | None = ev["previous_holder"]
        _ = (e_seq, e_kind, e_item, e_holder, e_at, e_epoch, e_prev)

    comp: CompactSummaryDict = dibs.compact(reg_path, 2, archive_path, None)
    c_seq: int = comp["through_seq"]
    c_arch: int = comp["archived_count"]
    c_rem: int = comp["remaining_count"]
    c_sha: str = comp["archived_events_sha256"]
    c_path: str = comp["archive_path"]
    _ = (c_seq, c_arch, c_rem, c_sha, c_path)

    err_classes: tuple[type[DibsError], ...] = (
        DibsError,
        InvalidNameError,
        InvalidTtlError,
        InvalidRequestError,
        NotFoundError,
        RefusalError,
        CorruptError,
        FailureError,
    )
    for cls in err_classes:
        assert issubclass(cls, Exception)
