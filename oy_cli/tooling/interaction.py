from __future__ import annotations

import sys
from typing import Any

import msgspec

from .. import runtime as rt
from .core import AskArgs, TodoArgs, TodoItem, _TODO_STATUSES, tool

@tool(AskArgs)
def tool_ask(state: Any, question: str, choices: list[str] | None = None):
    rt.note_tool(state, "ask", question=question, choices=choices)
    if not sys.stdin.isatty():
        raise ValueError("Cannot ask question: stdin is not a TTY")
    rt._print("prompt", question, err=True)
    if not choices:
        return rt.Prompt.ask("Answer", console=rt.STDERR).strip()
    rt._print(
        value="## Options\n\n"
        + "\n".join(
            f"{i}. {rt._fmt('inline', choice)}" for i, choice in enumerate(choices, 1)
        ),
        err=True,
    )
    while True:
        response = rt.Prompt.ask("Selection", console=rt.STDERR).strip()
        if response.isdigit() and 0 < int(response) <= len(choices):
            return choices[int(response) - 1]
        if response in choices:
            return response
        rt._print(
            "warning",
            f"Enter a number 1-{len(choices)} or type the choice exactly.",
            err=True,
        )

def _format_todos(todos: list[TodoItem]) -> str:
    if not todos:
        return "<empty todo list>"
    status_icons = {"pending": "[ ]", "in_progress": "[~]", "done": "[x]"}
    return "\n".join(
        f"{status_icons.get(item.status, '[ ]')} {item.id}: {item.task}" for item in todos
    )

@tool(TodoArgs)
def tool_todo(state: Any, todos: list[TodoItem] | list[dict[str, Any]] | None = None):
    if todos is None:
        todos = []
    validated: list[TodoItem] = []
    for item in todos:
        if isinstance(item, dict):
            item = msgspec.convert(item, TodoItem)
        if item.status not in _TODO_STATUSES:
            raise ValueError(
                f"Invalid status {item.status!r} for todo {item.id!r}. "
                f"Use one of: {', '.join(sorted(_TODO_STATUSES))}"
            )
        validated.append(item)
    state.todos = validated
    result = _format_todos(state.todos)
    rt.note_tool(state, "todo", _suffix=f"({len(state.todos)} items)")
    rt.show(result)
    return result

__all__ = ["_format_todos", "tool_ask", "tool_todo"]
