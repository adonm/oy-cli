from __future__ import annotations

import bz2
from collections import Counter
import concurrent.futures
from dataclasses import dataclass
from functools import partial
import gzip
import lzma
import os
from pathlib import Path
import tarfile
from typing import Any, BinaryIO, Iterable
import zipfile

import pathspec
from pygount import DuplicatePool, ProjectSummary, SourceAnalysis
import regex
import zstandard

from .. import runtime as rt
from .core import (
    ListArgs,
    ReadArgs,
    ReplaceArgs,
    SearchArgs,
    SlocArgs,
    _positive_int,
    tool,
)
from .output import _shown_line_limit

_STREAM_OPENERS = {
    ".gz": gzip.open,
    ".bz2": bz2.open,
    ".xz": lzma.open,
    ".zst": zstandard.open,
}
_ARCHIVE_SUFFIXES = frozenset({".zip", ".tar"})
_ARCHIVE_NAMES = (".tar", ".tgz", ".tar.gz")
_DEFAULT_THREADS = min(32, (os.cpu_count() or 4))

@dataclass(frozen=True, slots=True)
class SearchMatch:
    source: str
    line_number: int | None = None
    text: str = ""
    error: str | None = None

@dataclass(frozen=True, slots=True)
class ReplaceResult:
    source: str
    replacements: int = 0
    skipped: str | None = None
    error: str | None = None

@dataclass(frozen=True, slots=True)
class SlocLanguage:
    language: str
    file_count: int
    code_count: int
    documentation_count: int
    empty_count: int
    string_count: int

@dataclass(frozen=True, slots=True)
class SlocStateCount:
    state: str
    file_count: int

@dataclass(frozen=True, slots=True)
class SlocError:
    path: str
    message: str

@dataclass(frozen=True, slots=True)
class SlocReport:
    total_file_count: int
    total_code_count: int
    total_documentation_count: int
    total_empty_count: int
    total_string_count: int
    total_line_count: int
    languages: tuple[SlocLanguage, ...]
    state_counts: tuple[SlocStateCount, ...]
    errors: tuple[SlocError, ...]

def _ignore_spec(root: Path) -> pathspec.PathSpec:
    patterns = [".git/"]
    gitignore = root / ".gitignore"
    if gitignore.is_file():
        patterns.extend(gitignore.read_text(encoding="utf-8", errors="replace").splitlines())
    return pathspec.GitIgnoreSpec.from_lines(patterns)

def _resolve_ignore_root(target: Path, ignore_root: str | Path | None) -> Path:
    root = (
        Path(ignore_root).resolve()
        if ignore_root is not None
        else (target if target.is_dir() else target.parent).resolve()
    )
    try:
        target.relative_to(root)
    except ValueError:
        return (target if target.is_dir() else target.parent).resolve()
    return root

def _is_ignored(path: Path, spec: pathspec.PathSpec, root: Path) -> bool:
    rel_path = path.relative_to(root).as_posix()
    if path.is_dir():
        rel_path += "/"
    return spec.match_file(rel_path)

def _iter_files(target: Path, *, ignore_root: str | Path | None = None) -> list[Path]:
    root = _resolve_ignore_root(target, ignore_root)
    spec = _ignore_spec(root)
    if target.is_file():
        return [] if _is_ignored(target, spec, root) else [target]
    files: list[Path] = []
    for current_dir, dirs, names in os.walk(target):
        current = Path(current_dir)
        dirs[:] = [name for name in dirs if not _is_ignored(current / name, spec, root)]
        for name in names:
            file_path = current / name
            if not _is_ignored(file_path, spec, root):
                files.append(file_path)
    files.sort()
    return files

def _streams(path: Path) -> Iterable[tuple[str, BinaryIO]]:
    if path.suffix == ".zip":
        with zipfile.ZipFile(path, "r") as archive:
            for name in sorted(archive.namelist()):
                if not name.endswith("/"):
                    yield f"{path}::{name}", archive.open(name, "r")
        return
    if path.name.endswith(_ARCHIVE_NAMES):
        with tarfile.open(path, "r:*") as archive:
            for member in archive.getmembers():
                if member.isfile() and (stream := archive.extractfile(member)) is not None:
                    yield f"{path}::{member.name}", stream
        return
    opener = _STREAM_OPENERS.get(path.suffix, open)
    yield str(path), opener(path, "rb")

def _search_file(path: Path, compiled: regex.Pattern) -> list[SearchMatch]:
    matches: list[SearchMatch] = []
    for source, stream in _streams(path):
        try:
            with stream as handle:
                for line_number, raw_line in enumerate(handle, 1):
                    if compiled.search(raw_line):
                        text = raw_line.decode("utf-8", errors="replace").rstrip("\r\n")
                        matches.append(
                            SearchMatch(
                                source=source,
                                line_number=line_number,
                                text=text,
                            )
                        )
        except Exception as exc:
            matches.append(SearchMatch(source=source, error=str(exc)))
    return matches

def search(
    target: str | Path,
    pattern: str,
    *,
    threads: int | None = None,
    ignore_root: str | Path | None = None,
) -> list[SearchMatch]:
    path = Path(target).resolve()
    if not path.exists():
        raise ValueError(f"Path not found: {target}")
    if not (path.is_dir() or path.is_file()):
        raise ValueError(f"Unsupported search target: {target}")
    try:
        compiled = regex.compile(pattern.encode("utf-8"))
    except regex.error as exc:
        raise ValueError(f"Invalid search pattern: {exc}") from exc
    files = _iter_files(path, ignore_root=ignore_root)
    if not files:
        return []
    worker_count = min(len(files), max(1, threads or _DEFAULT_THREADS))
    if worker_count == 1:
        results: list[SearchMatch] = []
        for file_path in files:
            results.extend(_search_file(file_path, compiled))
        return results
    results: list[SearchMatch] = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=worker_count) as pool:
        for batch in pool.map(partial(_search_file, compiled=compiled), files):
            results.extend(batch)
    return results

def _is_archive(path: Path) -> bool:
    return (
        path.suffix in _STREAM_OPENERS
        or path.suffix in _ARCHIVE_SUFFIXES
        or path.name.endswith(_ARCHIVE_NAMES)
    )

def _replace_file(
    path: Path,
    compiled: regex.Pattern,
    replacement: str,
) -> ReplaceResult:
    if path.is_symlink():
        return ReplaceResult(source=str(path), skipped="symlink")
    if _is_archive(path):
        return ReplaceResult(source=str(path), skipped="archive")
    try:
        raw = path.read_bytes()
    except OSError as exc:
        return ReplaceResult(source=str(path), error=str(exc))
    if b"\x00" in raw:
        return ReplaceResult(source=str(path), skipped="binary file")
    try:
        original_text = raw.decode("utf-8")
    except UnicodeDecodeError as exc:
        return ReplaceResult(source=str(path), error=f"cannot decode utf-8: {exc}")
    updated_text, replacements = compiled.subn(replacement, original_text)
    if replacements == 0:
        return ReplaceResult(source=str(path))
    try:
        with path.open("w", encoding="utf-8", newline="") as handle:
            handle.write(updated_text)
    except OSError as exc:
        return ReplaceResult(source=str(path), error=str(exc))
    return ReplaceResult(source=str(path), replacements=replacements)

def replace(
    target: str | Path,
    pattern: str,
    replacement: str,
    *,
    threads: int | None = None,
    ignore_root: str | Path | None = None,
) -> list[ReplaceResult]:
    path = Path(target).resolve()
    if not path.exists():
        raise ValueError(f"Path not found: {target}")
    if not (path.is_dir() or path.is_file()):
        raise ValueError(f"Unsupported replace target: {target}")
    try:
        compiled = regex.compile(pattern)
    except regex.error as exc:
        raise ValueError(f"Invalid replace pattern: {exc}") from exc
    files = _iter_files(path, ignore_root=ignore_root)
    if not files:
        return []
    worker_count = min(len(files), max(1, threads or _DEFAULT_THREADS))
    if worker_count == 1:
        return [_replace_file(file_path, compiled, replacement) for file_path in files]
    with concurrent.futures.ThreadPoolExecutor(max_workers=worker_count) as pool:
        return list(
            pool.map(
                partial(_replace_file, compiled=compiled, replacement=replacement),
                files,
            )
        )

def sloc(
    target: str | Path,
    *,
    ignore_root: str | Path | None = None,
) -> SlocReport:
    path = Path(target).resolve()
    if not path.exists():
        raise ValueError(f"Path not found: {target}")
    if not (path.is_dir() or path.is_file()):
        raise ValueError(f"Unsupported sloc target: {target}")

    summary = ProjectSummary()
    duplicate_pool = DuplicatePool()
    state_counts: Counter[str] = Counter()
    errors: list[SlocError] = []
    group = path.name if path.is_dir() else (path.parent.name or ".")

    for file_path in _iter_files(path, ignore_root=ignore_root):
        if file_path.is_symlink():
            state_counts["symlink"] += 1
            continue
        analysis = SourceAnalysis.from_file(
            str(file_path),
            group=group,
            encoding="utf-8",
            fallback_encoding="utf-8",
            duplicate_pool=duplicate_pool,
        )
        summary.add(analysis)
        if analysis.is_countable:
            continue
        state_counts[analysis.state.name] += 1
        if analysis.state.name == "error":
            errors.append(
                SlocError(
                    path=str(file_path),
                    message=analysis.state_info or "unknown pygount error",
                )
            )

    languages = sorted(
        (
            SlocLanguage(
                language=language_summary.language,
                file_count=language_summary.file_count,
                code_count=language_summary.code_count,
                documentation_count=language_summary.documentation_count,
                empty_count=language_summary.empty_count,
                string_count=language_summary.string_count,
            )
            for language_summary in summary.language_to_language_summary_map.values()
            if not language_summary.is_pseudo_language
        ),
        key=lambda item: (-item.code_count, -item.file_count, item.language.lower()),
    )
    ordered_states = tuple(
        SlocStateCount(state=state, file_count=file_count)
        for state, file_count in sorted(
            state_counts.items(), key=lambda item: (-item[1], item[0])
        )
    )

    return SlocReport(
        total_file_count=summary.total_file_count,
        total_code_count=summary.total_code_count,
        total_documentation_count=summary.total_documentation_count,
        total_empty_count=summary.total_empty_count,
        total_string_count=summary.total_string_count,
        total_line_count=summary.total_line_count,
        languages=tuple(languages),
        state_counts=ordered_states,
        errors=tuple(errors),
    )

def _join_paths(paths: list[Path], root: Path, empty: str = "<no matches>") -> str:
    return (
        "\n".join(
            rt._rel(root, path) + ("/" if path.is_dir() else "")
            for path in paths
        )
        or empty
    )

def _path_listing(
    paths: list[Path], root: Path, *, limit: int, empty: str = "<no matches>"
) -> str:
    return _join_paths(paths[: _shown_line_limit(limit)], root, empty)

def _glob_paths(root: Path, pattern: str) -> list[Path]:
    if Path(pattern).is_absolute() or ".." in Path(pattern).parts:
        raise ValueError(f"Path traversal denied: '{pattern}'")
    matches: list[Path] = []
    for candidate in root.glob(pattern):
        try:
            resolved = candidate.resolve()
        except OSError:
            continue
        if resolved == root or root in resolved.parents:
            matches.append(resolved)
    return sorted(set(matches), key=lambda item: item.as_posix())

@tool(ListArgs)
def tool_list(state: Any, path: str = "*", limit: int = rt.BUDGETS.default_line_limit):
    rt.note_tool(
        state,
        "list",
        _defaults={"path": "*", "limit": rt.BUDGETS.default_line_limit},
        path=path,
        limit=limit,
    )
    text = _path_listing(_glob_paths(state.root, path), state.root, limit=limit)
    return rt._show_and_clip(text)

@tool(ReadArgs)
def tool_read(
    state: Any, path: str, offset: int = 1, limit: int = rt.BUDGETS.default_line_limit
):
    rt.note_tool(
        state,
        "read",
        _defaults={"offset": 1, "limit": rt.BUDGETS.default_line_limit},
        path=path,
        offset=offset,
        limit=limit,
    )
    target = rt.resolve_path(state.root, path)
    if not target.exists():
        raise ValueError(f"read path does not exist: {rt._rel(state.root, target)}")
    if target.is_dir():
        text = _path_listing(
            sorted(target.iterdir(), key=lambda item: item.as_posix()),
            state.root,
            limit=limit,
            empty="<empty directory>",
        )
        return rt._show_and_clip(text)
    start = max(_positive_int(offset, "offset"), 1) - 1
    lines = target.read_text(encoding="utf-8", errors="replace").splitlines()
    shown = lines[start : start + _shown_line_limit(limit)]
    text = "\n".join(
        f"{lineno}: {line}" for lineno, line in enumerate(shown, start + 1)
    )
    return rt._show_and_clip(text or "<empty file>")

def _search_summary(matches: int, shown: int, errors: int = 0) -> str:
    if not matches and not errors:
        return "(no matches)"
    parts = []
    if matches:
        extra = f"; showing {shown} of {matches}" if shown < matches else ""
        plural = "es" if matches != 1 else ""
        parts.append(f"{matches} match{plural}{extra}")
    if errors:
        plural = "s" if errors != 1 else ""
        parts.append(f"{errors} error{plural}")
    return "(" + "; ".join(parts) + ")"

def _search_display_path(root: Path, source: str) -> str:
    if "::" in source:
        container, member = source.split("::", 1)
        return f"{rt._rel(root, Path(container))}::{member}"
    return rt._rel(root, Path(source))

def _search_preview_line(root: Path, match: SearchMatch) -> str:
    path_text = _search_display_path(root, match.source)
    if match.error:
        return f"[!] {path_text}: {match.error}"
    text = rt._truncate_long_lines(match.text)
    return f"{path_text}:{match.line_number}:1:{text}"

def _optional_counts(**counts: int) -> dict[str, int]:
    return {
        f"{name}_count": value
        for name, value in counts.items()
        if value
    }

def _search_payload(
    root: Path,
    pattern: str,
    path: str,
    results: list[SearchMatch],
    *,
    limit: int,
) -> tuple[dict[str, Any], str, int, int, int]:
    matches = [match for match in results if not match.error]
    errors = [match for match in results if match.error]
    shown_limit = _shown_line_limit(limit)
    shown_matches = matches[:shown_limit]
    payload: dict[str, Any] = {
        "pattern": pattern,
        "path": path,
        "match_count": len(matches),
        "matches": [
            {
                "path": _search_display_path(root, match.source),
                "line_number": match.line_number,
                "column": 1,
                "text": rt._truncate_long_lines(match.text),
            }
            for match in shown_matches
        ],
        "truncated": len(matches) > len(shown_matches),
    }
    payload.update(_optional_counts(error=len(errors)))
    if errors:
        payload["errors"] = [
            {"path": _search_display_path(root, match.source), "message": match.error}
            for match in errors[:shown_limit]
        ]
    preview_lines = [_search_preview_line(root, match) for match in shown_matches]
    if errors:
        preview_lines.extend(
            _search_preview_line(root, match) for match in errors[:shown_limit]
        )
    preview = "\n".join(preview_lines) if preview_lines else "<no matches>"
    if len(matches) > len(shown_matches):
        preview += f"\n... [{len(matches) - len(shown_matches)} more matches omitted]"
    return payload, preview, len(matches), len(shown_matches), len(errors)

def _search_contents(root: Path, pattern: str, path: str, *, limit: int):
    target = rt.resolve_path(root, path)
    if not target.exists():
        raise ValueError(f"search path does not exist: {rt._rel(root, target)}")
    results = search(target, pattern, ignore_root=root)
    return _search_payload(root, pattern, path, results, limit=limit)

@tool(SearchArgs)
def tool_search(
    state: Any, pattern: str, path: str = ".", limit: int = rt.BUDGETS.default_line_limit
):
    defaults = {"path": ".", "limit": rt.BUDGETS.default_line_limit}
    payload, preview, matches, shown, errors = _search_contents(
        state.root, pattern, path, limit=limit
    )
    rt.note_tool(
        state,
        "search",
        _defaults=defaults,
        _suffix=_search_summary(matches, shown, errors),
        pattern=pattern,
        path=path,
        limit=limit,
    )
    rt.show(preview)
    return payload

def _replace_summary(
    changed_files: int, replacements: int, skipped: int = 0, errors: int = 0
) -> str:
    if not changed_files and not skipped and not errors:
        return "(no changes)"
    parts = []
    if changed_files:
        plural = "s" if changed_files != 1 else ""
        parts.append(f"{changed_files} file{plural} changed")
    if replacements:
        plural = "s" if replacements != 1 else ""
        parts.append(f"{replacements} replacement{plural}")
    if skipped:
        plural = "s" if skipped != 1 else ""
        parts.append(f"{skipped} skipped")
    if errors:
        plural = "s" if errors != 1 else ""
        parts.append(f"{errors} error{plural}")
    return "(" + "; ".join(parts) + ")"

def _replace_preview_line(root: Path, result: ReplaceResult) -> str:
    path_text = rt._rel(root, Path(result.source))
    if result.error:
        return f"[!] {path_text}: {result.error}"
    if result.skipped:
        return f"[-] {path_text}: skipped ({result.skipped})"
    return f"{path_text}: {result.replacements} replacement(s)"

def _replace_payload(
    root: Path,
    pattern: str,
    replacement: str,
    path: str,
    results: list[ReplaceResult],
    *,
    limit: int,
) -> tuple[dict[str, Any], str, int, int, int, int]:
    changed = [result for result in results if result.replacements]
    skipped = [result for result in results if result.skipped]
    errors = [result for result in results if result.error]
    shown_limit = _shown_line_limit(limit)
    shown_changed = changed[:shown_limit]
    replacement_count = sum(result.replacements for result in changed)
    payload: dict[str, Any] = {
        "pattern": pattern,
        "replacement": replacement,
        "path": path,
        "changed_file_count": len(changed),
        "replacement_count": replacement_count,
        "changed_files": [
            {
                "path": rt._rel(root, Path(result.source)),
                "replacements": result.replacements,
            }
            for result in shown_changed
        ],
        "truncated": len(changed) > len(shown_changed),
    }
    payload.update(_optional_counts(skipped=len(skipped), error=len(errors)))
    if skipped:
        payload["skipped"] = [
            {"path": rt._rel(root, Path(result.source)), "reason": result.skipped}
            for result in skipped[:shown_limit]
        ]
    if errors:
        payload["errors"] = [
            {"path": rt._rel(root, Path(result.source)), "message": result.error}
            for result in errors[:shown_limit]
        ]
    preview_lines = [_replace_preview_line(root, result) for result in shown_changed]
    if skipped:
        preview_lines.extend(
            _replace_preview_line(root, result) for result in skipped[:shown_limit]
        )
    if errors:
        preview_lines.extend(
            _replace_preview_line(root, result) for result in errors[:shown_limit]
        )
    preview = "\n".join(preview_lines) if preview_lines else "<no changes>"
    if len(changed) > len(shown_changed):
        preview += (
            f"\n... [{len(changed) - len(shown_changed)} more changed files omitted]"
        )
    return (
        payload,
        preview,
        len(changed),
        replacement_count,
        len(skipped),
        len(errors),
    )

def _replace_contents(
    root: Path, pattern: str, replacement: str, path: str, *, limit: int
):
    target = rt.resolve_path(root, path)
    if not target.exists():
        raise ValueError(f"replace path does not exist: {rt._rel(root, target)}")
    results = replace(target, pattern, replacement, ignore_root=root)
    return _replace_payload(root, pattern, replacement, path, results, limit=limit)

@tool(ReplaceArgs)
def tool_replace(
    state: Any,
    pattern: str,
    replacement: str,
    path: str = ".",
    limit: int = rt.BUDGETS.default_line_limit,
):
    defaults = {"path": ".", "limit": rt.BUDGETS.default_line_limit}
    payload, preview, changed, replacements, skipped, errors = _replace_contents(
        state.root, pattern, replacement, path, limit=limit
    )
    rt.note_tool(
        state,
        "replace",
        _defaults=defaults,
        _suffix=_replace_summary(changed, replacements, skipped, errors),
        pattern=pattern,
        replacement=replacement,
        path=path,
        limit=limit,
    )
    rt.show(preview)
    return payload

def _sloc_summary(report: SlocReport) -> str:
    parts = []
    if report.total_file_count:
        plural = "s" if report.total_file_count != 1 else ""
        parts.append(f"{report.total_file_count} file{plural}")
    if report.total_code_count:
        parts.append(f"{report.total_code_count} code lines")
    non_countable = sum(item.file_count for item in report.state_counts)
    if non_countable:
        parts.append(f"{non_countable} non-countable")
    if report.errors:
        plural = "s" if len(report.errors) != 1 else ""
        parts.append(f"{len(report.errors)} error{plural}")
    return "(" + "; ".join(parts) + ")" if parts else "(no source files)"

def _sloc_totals_line(report: SlocReport) -> str:
    return (
        "totals: "
        f"{report.total_file_count} files, "
        f"{report.total_code_count} code, "
        f"{report.total_documentation_count} comments, "
        f"{report.total_empty_count} empty, "
        f"{report.total_string_count} strings, "
        f"{report.total_line_count} lines"
    )

def _sloc_language_preview_line(language: Any) -> str:
    file_plural = "s" if language.file_count != 1 else ""
    return (
        f"{language.language}: "
        f"{language.code_count} code, "
        f"{language.documentation_count} comments, "
        f"{language.empty_count} empty, "
        f"{language.string_count} strings "
        f"({language.file_count} file{file_plural})"
    )

def _sloc_state_preview_line(state_count: Any) -> str:
    file_plural = "s" if state_count.file_count != 1 else ""
    return f"other/{state_count.state}: {state_count.file_count} file{file_plural}"

def _sloc_payload(
    root: Path, path: str, report: SlocReport, *, limit: int
) -> tuple[dict[str, Any], str]:
    shown_limit = _shown_line_limit(limit)
    shown_languages = list(report.languages[:shown_limit])
    payload: dict[str, Any] = {
        "path": path,
        "total_file_count": report.total_file_count,
        "total_code_count": report.total_code_count,
        "total_documentation_count": report.total_documentation_count,
        "total_empty_count": report.total_empty_count,
        "total_string_count": report.total_string_count,
        "total_line_count": report.total_line_count,
        "language_count": len(report.languages),
        "languages": [
            {
                "language": language.language,
                "file_count": language.file_count,
                "code_count": language.code_count,
                "documentation_count": language.documentation_count,
                "empty_count": language.empty_count,
                "string_count": language.string_count,
            }
            for language in shown_languages
        ],
        "truncated": len(report.languages) > len(shown_languages),
    }
    if report.state_counts:
        payload["state_counts"] = [
            {"state": state_count.state, "file_count": state_count.file_count}
            for state_count in report.state_counts
        ]
    payload.update(_optional_counts(error=len(report.errors)))
    if report.errors:
        payload["errors"] = [
            {"path": rt._rel(root, Path(error.path)), "message": error.message}
            for error in report.errors[:shown_limit]
        ]
    if not report.total_file_count and not report.state_counts:
        return payload, "<no source files>"
    preview_lines = [_sloc_totals_line(report)]
    preview_lines.extend(
        _sloc_language_preview_line(language) for language in shown_languages
    )
    if len(report.languages) > len(shown_languages):
        preview_lines.append(
            f"... [{len(report.languages) - len(shown_languages)} more languages omitted]"
        )
    preview_lines.extend(
        _sloc_state_preview_line(state_count) for state_count in report.state_counts
    )
    preview_lines.extend(
        f"[!] {rt._rel(root, Path(error.path))}: {error.message}"
        for error in report.errors[:shown_limit]
    )
    return payload, "\n".join(preview_lines)

def _sloc_contents(root: Path, path: str, *, limit: int):
    target = rt.resolve_path(root, path)
    if not target.exists():
        raise ValueError(f"sloc path does not exist: {rt._rel(root, target)}")
    report = sloc(target, ignore_root=root)
    return _sloc_payload(root, path, report, limit=limit), report

@tool(SlocArgs)
def tool_sloc(state: Any, path: str = ".", limit: int = rt.BUDGETS.default_line_limit):
    defaults = {"path": ".", "limit": rt.BUDGETS.default_line_limit}
    (payload, preview), report = _sloc_contents(state.root, path, limit=limit)
    rt.note_tool(
        state,
        "sloc",
        _defaults=defaults,
        _suffix=_sloc_summary(report),
        path=path,
        limit=limit,
    )
    rt.show(preview)
    return payload

__all__ = [
    "ReplaceResult",
    "SearchMatch",
    "SlocReport",
    "_iter_files",
    "replace",
    "search",
    "sloc",
    "tool_list",
    "tool_read",
    "tool_replace",
    "tool_search",
    "tool_sloc",
]
