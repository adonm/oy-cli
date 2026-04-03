from __future__ import annotations

import time
from typing import Any

import tiktoken
import toons
from . import runtime as rt
from .providers import (
    AuthenticationError,
    AssistantMessage,
    BadRequestError,
    ChatMessage,
    PermissionDeniedError,
    RateLimitError,
    SystemMessage,
    ToolMessage,
    UserMessage,
    _tool_output_text,
)
from .runtime import active_tool_registry, session_text
from .tools import _format_todos, _positive_int, invoke_tool, tool_specs


type AgentState = dict[str, Any]
type Transcript = dict[str, Any]
type Wait = dict[str, Any]


def agent_state(
    *,
    root: rt.Path,
    tool_registry: dict[str, dict[str, Any]],
    unattended_limit_seconds: int,
    unattended_deadline: float,
    interactive: bool = False,
    approve_all_mutating_tools: bool = False,
    yolo: bool = False,
    todos: list[dict[str, str]] | None = None,
) -> AgentState:
    return {
        "root": root,
        "tool_registry": tool_registry,
        "unattended_limit_seconds": unattended_limit_seconds,
        "unattended_deadline": unattended_deadline,
        "interactive": interactive,
        "approve_all_mutating_tools": approve_all_mutating_tools,
        "yolo": yolo,
        "todos": list(todos or []),
    }


def new_agent_state(
    *,
    root: rt.Path,
    tool_registry: dict[str, dict[str, Any]],
    unattended_limit_seconds: int,
    interactive: bool = False,
    yolo: bool = False,
) -> AgentState:
    return agent_state(
        root=root,
        tool_registry=tool_registry,
        unattended_limit_seconds=unattended_limit_seconds,
        unattended_deadline=time.monotonic() + unattended_limit_seconds,
        interactive=interactive,
        yolo=yolo,
        approve_all_mutating_tools=yolo,
    )


def remaining_unattended_seconds(state: AgentState) -> float:
    return state["unattended_deadline"] - time.monotonic()


def note_progress(state: AgentState) -> None:
    if remaining_unattended_seconds(state) <= 0:
        raise TimeoutError(
            "reached unattended timeout "
            f"({rt._format_duration(state['unattended_limit_seconds'])}) without a final response"
        )


def _message_text(message: ChatMessage) -> str:
    if message.get("role") == "tool":
        return _tool_output_text(message["content"])
    return message["content"]


def count_tokens(text: str) -> int:
    return rt.count_tokens(text)


def _message_tokens(message: ChatMessage) -> int:
    return 4 + count_tokens(_message_text(message))


def _truncate_message(message: ChatMessage, max_tokens: int) -> ChatMessage:
    if message.get("role") == "tool" or not message["content"]:
        return message
    if (
        truncated := rt.truncate_str_to_tokens(
            message["content"], max_tokens=max_tokens
        )
    ) is message["content"]:
        return message
    role = message.get("role")
    if role == "system":
        return SystemMessage(truncated)
    if role == "user":
        return UserMessage(truncated)
    if role == "assistant":
        return AssistantMessage(
            truncated,
            tool_calls=message["tool_calls"],
            thought_signatures=message["thought_signatures"],
        )
    return message


def _message_role(message: ChatMessage) -> str:
    return message.get("role", "tool")


def _can_pack_message_with_toons(message: ChatMessage) -> bool:
    if message.get("role") == "user":
        return True
    if message.get("role") == "assistant":
        return not message["tool_calls"] and not message["thought_signatures"]
    return False


def _packed_history_note(messages: list[ChatMessage]) -> SystemMessage:
    packed = toons.dumps(
        {
            "messages": [
                {"role": _message_role(message), "content": _message_text(message)}
                for message in messages
            ]
        }
    )
    return SystemMessage(
        session_text("transcript", "packed_history_note", packed=packed).strip()
    )


def _pack_messages_with_toons(messages: list[ChatMessage]) -> list[ChatMessage]:
    systems = [message for message in messages if message.get("role") == "system"]
    other = [message for message in messages if message.get("role") != "system"]
    max_prefix = len(other) - 2
    if max_prefix < 2:
        return messages

    packable_prefix: list[ChatMessage] = []
    for message in other[:max_prefix]:
        if not _can_pack_message_with_toons(message):
            break
        packable_prefix.append(message)
    if len(packable_prefix) < 2:
        return messages

    best_count = 0
    best_note: SystemMessage | None = None
    raw_tokens = 0
    try:
        for count, message in enumerate(packable_prefix, start=1):
            raw_tokens += _message_tokens(message)
            if count < 2:
                continue
            note = _packed_history_note(packable_prefix[:count])
            savings = raw_tokens - _message_tokens(note)
            if savings > 0:
                best_count = count
                best_note = note
    except Exception:
        return messages

    if best_note is None:
        return messages
    return [*systems, best_note, *other[best_count:]]


def _history_units(messages: list[ChatMessage]) -> list[list[ChatMessage]]:
    units: list[list[ChatMessage]] = []
    index = 0
    while index < len(messages):
        message = messages[index]
        if message.get("role") == "assistant" and message["tool_calls"]:
            end = index + 1
            while end < len(messages) and messages[end].get("role") == "tool":
                end += 1
            units.append(messages[index:end])
            index = end
            continue
        units.append([message])
        index += 1
    return units


def transcript(
    messages: list[ChatMessage] | None = None,
    *,
    max_context_tokens: int = rt.MAX_CONTEXT_TOKENS,
    max_message_tokens: int = rt.BUDGETS["message_tokens"],
) -> Transcript:
    return {
        "messages": list(messages or []),
        "max_context_tokens": max_context_tokens,
        "max_message_tokens": max_message_tokens,
    }


def transcript_with_system_prompt(system_prompt: str) -> Transcript:
    tx = transcript()
    set_system_prompt(tx, system_prompt)
    return tx


def set_system_prompt(tx: Transcript, system_prompt: str) -> None:
    if tx["messages"] and tx["messages"][0].get("role") == "system":
        tx["messages"][0] = SystemMessage(system_prompt)
    else:
        tx["messages"][:0] = [SystemMessage(system_prompt)]


def clear_transcript(tx: Transcript, system_prompt: str) -> None:
    tx["messages"].clear()
    set_system_prompt(tx, system_prompt)


def checkpoint(tx: Transcript) -> int:
    return len(tx["messages"])


def rollback(tx: Transcript, point: int) -> None:
    del tx["messages"][point:]


def undo_last_turn(tx: Transcript) -> bool:
    for index in range(len(tx["messages"]) - 1, 0, -1):
        if tx["messages"][index].get("role") == "user":
            del tx["messages"][index:]
            return True
    return False


def add_user(tx: Transcript, prompt: str) -> None:
    tx["messages"].append(UserMessage(prompt))


def add_assistant(tx: Transcript, message: AssistantMessage) -> None:
    tx["messages"].append(message)


def add_tool_results(tx: Transcript, results: list[dict[str, Any]]) -> None:
    tx["messages"].extend(
        ToolMessage(
            tool_call_id=result["call_id"],
            name=result["name"],
            content=result["result"],
        )
        for result in results
    )


def prepared_messages(
    tx: Transcript,
    model: str | None = None,
    todos: list[dict[str, str]] | None = None,
) -> list[ChatMessage]:
    messages = [
        _truncate_message(message, tx["max_message_tokens"])
        for message in tx["messages"]
    ]
    if model:
        messages = _pack_messages_with_toons(messages)
    system_messages = [
        message for message in messages if message.get("role") == "system"
    ]
    if todos:
        system_messages.append(
            SystemMessage(
                session_text(
                    "transcript", "todo_system", todos=_format_todos(todos)
                ).strip()
            )
        )
    other = [message for message in messages if message.get("role") != "system"]
    budget = tx["max_context_tokens"] - sum(map(_message_tokens, system_messages))
    if budget <= 0:
        return system_messages
    kept_units: list[list[ChatMessage]] = []
    used = 0
    for unit in reversed(_history_units(other)):
        cost = sum(_message_tokens(message) for message in unit)
        if cost + used <= budget:
            kept_units.append(unit)
            used += cost
    kept = [message for unit in reversed(kept_units) for message in unit]
    omitted_messages = len(other) - len(kept)
    return (
        system_messages
        + (
            [
                UserMessage(
                    session_text(
                        "transcript",
                        "omitted_messages",
                        omitted_messages=omitted_messages,
                    )
                )
            ]
            if omitted_messages > 0
            else []
        )
        + kept
    )


def session_tokens(tx: Transcript) -> int:
    return sum(map(_message_tokens, tx["messages"]))


def prepared_tokens(
    tx: Transcript,
    model: str | None = None,
    todos: list[dict[str, str]] | None = None,
) -> int:
    return sum(map(_message_tokens, prepared_messages(tx, model=model, todos=todos)))


_tokenizer: tiktoken.Encoding | None = None


def wait(label: str) -> Wait:
    return {"label": label, "active": False}


def start_wait(item: Wait) -> None:
    item["active"] = True
    rt._note(item["label"], tag="wait")


def stop_wait(item: Wait) -> None:
    item["active"] = False


def update_wait(item: Wait, label: str) -> None:
    item["label"] = label
    if item["active"]:
        rt._note(item["label"], tag="wait")


def log_wait(item: Wait, message: str) -> None:
    _ = item
    rt._note(message, tag="wait")


def _normalized_vote_text(text: str) -> str:
    return " ".join(text.split()).strip().lower()



def run_turn(
    client,
    transcript: Transcript,
    state: AgentState,
    model_spec,
    tool_definitions,
):
    _, model = rt.split_model_spec(model_spec)
    step = 0
    while True:
        note_progress(state)
        prepared = prepared_messages(transcript, model=model, todos=state["todos"])
        rt._debug_log(
            "request",
            model=model_spec,
            step=step,
            messages=[rt._msg_to_dict(message) for message in prepared],
            tool_count=len(tool_definitions),
        )
        size_str = rt.format_tokens(sum(map(_message_tokens, prepared)))
        spinner = wait(f"Waiting for {model_spec} | {size_str}")
        start_wait(spinner)

        def on_retry(attempt, max_attempts, error_ctx=None):
            excerpt = ""
            if error_ctx:
                excerpt = " | ".join(
                    line.strip()
                    for line in error_ctx.strip().splitlines()[:3]
                    if line.strip()
                )
            log_wait(
                spinner,
                f"retry {attempt}/{max_attempts}{': ' + excerpt if excerpt else ''}",
            )
            update_wait(
                spinner,
                f"Retrying {model_spec} (attempt {attempt}/{max_attempts}) | {size_str}",
            )

        try:
            if remaining_unattended_seconds(state) <= 0:
                raise TimeoutError(
                    "reached unattended timeout "
                    f"({rt._format_duration(state['unattended_limit_seconds'])}) without a final response"
                )
            message = client["chat_completion"](
                model=model,
                messages=prepared,
                tools=tool_definitions,
                tool_choice="auto",
                on_retry=on_retry,
            )
        finally:
            stop_wait(spinner)
        rt._debug_log(
            "response",
            model=model_spec,
            step=step,
            assistant=rt._msg_to_dict(message),
        )
        calls = list(message["tool_calls"])
        if calls:
            add_assistant(transcript, message)
            results = [
                {
                    "call_id": call["id"],
                    "name": call["name"],
                    "result": invoke_tool(
                        state["tool_registry"], state, call["name"], call["arguments"]
                    ),
                }
                for call in calls
            ]
            rt._debug_log(
                "tool_results",
                model=model_spec,
                step=step,
                results=[
                    {
                        "call_id": result["call_id"],
                        "name": result["name"],
                        "ok": result["result"]["ok"],
                    }
                    for result in results
                ],
            )
            add_tool_results(transcript, results)
            step += 1
            continue
        rt._print(value=message["content"])
        return 0, message["content"]


def run_agent(
    prompt,
    model,
    root,
    system_prompt,
    unattended_limit_seconds,
    interactive,
    yolo: bool = False,
    transcript: Transcript | None = None,
):
    tool_registry = active_tool_registry(interactive)
    unattended_limit_seconds = _positive_int(
        unattended_limit_seconds, "unattended_limit_seconds"
    )
    state = new_agent_state(
        root=root,
        tool_registry=tool_registry,
        unattended_limit_seconds=unattended_limit_seconds,
        interactive=interactive,
        yolo=yolo,
    )
    if transcript is None:
        transcript = transcript_with_system_prompt(system_prompt)
    else:
        set_system_prompt(transcript, system_prompt)
    add_user(transcript, prompt)

    def runner(client):
        return run_turn(
            client,
            transcript,
            state,
            model,
            tool_specs(tool_registry),
        )

    try:
        return runner(rt.get_client(model))
    except (AuthenticationError, PermissionDeniedError) as exc:
        if not rt.ensure_api_env(root):
            return rt.fail(
                f"API {'authentication' if isinstance(exc, AuthenticationError) else 'permission'} error: {exc}"
            ), ""
        rt._warn("Credentials expired. Refreshing.")
        try:
            return runner(rt.get_client(model))
        except (AuthenticationError, PermissionDeniedError) as exc:
            return rt.fail(
                f"API {'authentication' if isinstance(exc, AuthenticationError) else 'permission'} error: {exc}"
            ), ""
        except Exception as exc:
            return rt.fail(str(exc)), ""
    except RateLimitError as exc:
        return rt.fail(f"API rate limit: {exc}"), ""
    except BadRequestError as exc:
        return rt.fail(f"API bad request: {exc}"), ""
    except Exception as exc:
        return rt.fail(str(exc)), ""


__all__ = [
    "AgentState",
    "Transcript",
    "agent_state",
    "new_agent_state",
    "remaining_unattended_seconds",
    "note_progress",
    "transcript",
    "transcript_with_system_prompt",
    "set_system_prompt",
    "clear_transcript",
    "checkpoint",
    "rollback",
    "undo_last_turn",
    "add_user",
    "add_assistant",
    "add_tool_results",
    "prepared_messages",
    "session_tokens",
    "prepared_tokens",
    "_message_tokens",
    "_pack_messages_with_toons",
    "_packed_history_note",
    "run_agent",
    "run_turn",
]
