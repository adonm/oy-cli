from __future__ import annotations

from typing import Any, TypeAlias

import msgspec
import toons

JSONLike: TypeAlias = dict[str, Any] | list[Any] | str | int | float | bool | None


def normalize_jsonlike(value: Any) -> JSONLike:
    if value is None or isinstance(value, (str, int, float, bool)):
        return value
    if isinstance(value, dict):
        return {str(key): normalize_jsonlike(item) for key, item in value.items()}
    if isinstance(value, (list, tuple, set)):
        return [normalize_jsonlike(item) for item in value]
    return str(value)


def serialize_json(value: Any) -> str:
    normalized = normalize_jsonlike(value)
    return normalized if isinstance(normalized, str) else msgspec.json.encode(normalized).decode("utf-8")


def serialize_toon(value: Any) -> str:
    normalized = normalize_jsonlike(value)
    return normalized if isinstance(normalized, str) else toons.dumps(normalized)


__all__ = ["JSONLike", "normalize_jsonlike", "serialize_json", "serialize_toon"]
