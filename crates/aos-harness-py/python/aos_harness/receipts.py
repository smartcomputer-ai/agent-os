from __future__ import annotations

from typing import TYPE_CHECKING, Any, Mapping, Optional, Union

from .types import JsonValue, ReceiptObject

if TYPE_CHECKING:
    from ._core import WorkflowHarness, WorldHarness

    HarnessLike = Union["WorkflowHarness", "WorldHarness"]
else:
    HarnessLike = Any


def _intent_hash(effect: Mapping[str, Any]) -> bytes:
    value = effect.get("intent_hash")
    if isinstance(value, (bytes, bytearray)):
        return bytes(value)
    if isinstance(value, list) and all(isinstance(item, int) for item in value):
        return bytes(value)
    raise TypeError("effect.intent_hash must be bytes or a list of byte values")


def _require_kind(effect: Mapping[str, Any], expected: str) -> None:
    kind = effect.get("kind")
    if kind != expected:
        raise ValueError(f"expected effect kind {expected!r}, got {kind!r}")


def timer_set_ok(
    harness: HarnessLike,
    effect: Mapping[str, Any],
    *,
    delivered_at_ns: int,
    key: Optional[str] = None,
) -> ReceiptObject:
    _require_kind(effect, "timer.set")
    return harness.receipt_timer_set_ok(_intent_hash(effect), delivered_at_ns, key)


def blob_put_ok(
    harness: HarnessLike,
    effect: Mapping[str, Any],
    *,
    blob_ref: str,
    edge_ref: str,
    size: int,
) -> ReceiptObject:
    _require_kind(effect, "blob.put")
    return harness.receipt_blob_put_ok(_intent_hash(effect), blob_ref, edge_ref, size)


def blob_get_ok(
    harness: HarnessLike,
    effect: Mapping[str, Any],
    *,
    blob_ref: str,
    data: bytes,
    size: Optional[int] = None,
) -> ReceiptObject:
    _require_kind(effect, "blob.get")
    return harness.receipt_blob_get_ok(_intent_hash(effect), blob_ref, data, size)


def http_request_ok(
    harness: HarnessLike,
    effect: Mapping[str, Any],
    *,
    status: int,
    headers: Optional[JsonValue] = None,
    body_ref: Optional[str] = None,
    start_ns: Optional[int] = None,
    end_ns: Optional[int] = None,
) -> ReceiptObject:
    _require_kind(effect, "http.request")
    return harness.receipt_http_request_ok(
        _intent_hash(effect),
        status,
        headers=headers,
        body_ref=body_ref,
        start_ns=start_ns,
        end_ns=end_ns,
    )


def llm_generate_ok(
    harness: HarnessLike,
    effect: Mapping[str, Any],
    *,
    output_ref: str,
    provider_id: str,
    finish_reason: str = "stop",
    prompt_tokens: int = 0,
    completion_tokens: int = 0,
    total_tokens: Optional[int] = None,
    raw_output_ref: Optional[str] = None,
    provider_response_id: Optional[str] = None,
    cost_cents: Optional[int] = None,
    warnings_ref: Optional[str] = None,
    rate_limit_ref: Optional[str] = None,
) -> ReceiptObject:
    _require_kind(effect, "llm.generate")
    return harness.receipt_llm_generate_ok(
        _intent_hash(effect),
        output_ref,
        provider_id,
        finish_reason=finish_reason,
        prompt_tokens=prompt_tokens,
        completion_tokens=completion_tokens,
        total_tokens=total_tokens,
        raw_output_ref=raw_output_ref,
        provider_response_id=provider_response_id,
        cost_cents=cost_cents,
        warnings_ref=warnings_ref,
        rate_limit_ref=rate_limit_ref,
    )
