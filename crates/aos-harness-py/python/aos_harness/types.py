from __future__ import annotations

from pathlib import Path
from typing import Any, Dict, List, Literal, TypedDict, Union

JsonPrimitive = Union[None, bool, int, float, str]
JsonObject = Dict[str, "JsonValue"]
JsonArray = List["JsonValue"]
JsonValue = Union[JsonPrimitive, JsonObject, JsonArray]
HashBytes = Union[bytes, bytearray, List[int]]
PathLike = Union[str, Path]
EffectModeName = Literal["scripted", "twin", "live"]
BuildProfileName = Literal["debug", "release"]
ReceiptStatusName = Literal["ok", "error", "timeout"]


class EffectObject(TypedDict, total=False):
    kind: str
    intent_hash: HashBytes
    params: JsonValue


class ReceiptObject(TypedDict, total=False):
    intent_hash: bytes
    status: ReceiptStatusName
    payload_cbor: bytes
    cost_cents: int
    signature: bytes


class HarnessArtifacts(TypedDict, total=False):
    evidence: JsonObject
    journal_entries: List[JsonObject]
