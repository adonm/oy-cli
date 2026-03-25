from __future__ import annotations

import asyncio
import time
from typing import cast

import msgspec
import tiktoken
import toons
from openai import AuthenticationError, BadRequestError, PermissionDeniedError, RateLimitError
from . import runtime as rt
from .providers import AssistantMessage, ChatMessage, CompletionClient, SystemMessage, ToolMessage, UserMessage
from .providers import _tool_output_text
from .runtime import active_tool_specs, session_text
from .tools import TodoItem, ToolRegistry, _format_todos, _positive_int


class AgentState(msgspec.Struct, omit_defaults=True):
    root: rt.Path
    tool_specs: ToolRegistry
    unattended_timeout_seconds: int
    unattended_deadline: float
    interactive: bool = False
    approve_all_mutating_tools: bool = False
    todos: list[TodoItem] = msgspec.field(default_factory=list)

    @classmethod
    def new(
        cls,
        *,
        root: rt.Path,
        tool_specs: ToolRegistry,
        unattended_timeout_seconds: int,
        interactive: bool = False,
    ) -> "AgentState":
        return cls(
            root=root,
            tool_specs=tool_specs,
            unattended_timeout_seconds=unattended_timeout_seconds,
            unattended_deadline=time.monotonic() + unattended_timeout_seconds,
            interactive=interactive,
        )

    def remaining_unattended_seconds(self) -> float:
        return self.unattended_deadline - time.monotonic()

    def note_progress(self) -> None:
        if self.remaining_unattended_seconds() <= 0:
            raise TimeoutError(
                "reached unattended timeout "
                f"({rt._format_duration(self.unattended_timeout_seconds)}) without a final response"
            )


def _message_text(message: ChatMessage) -> str:
    if isinstance(message, ToolMessage):
        return _tool_output_text(message.content)
    return message.content


def count_tokens(text: str) -> int:
    return rt.count_tokens(text)


def _message_tokens(message: ChatMessage) -> int:
    return 4 + count_tokens(_message_text(message))


def _truncate_message(message: ChatMessage, max_tokens: int) -> ChatMessage:
    if isinstance(message, ToolMessage) or not message.content:
        return message
    if (truncated := rt.truncate_str_to_tokens(message.content, max_tokens=max_tokens)) is message.content:
        return message
    match message:
        case SystemMessage():
            return SystemMessage(truncated)
        case UserMessage():
            return UserMessage(truncated)
        case AssistantMessage():
            return AssistantMessage(
                truncated,
                tool_calls=message.tool_calls,
                thought_signatures=message.thought_signatures,
            )
    return message


def _message_role(message: ChatMessage) -> str:
    if isinstance(message, SystemMessage):
        return "system"
    if isinstance(message, UserMessage):
        return "user"
    if isinstance(message, AssistantMessage):
        return "assistant"
    return "tool"


def _can_pack_message_with_toons(message: ChatMessage) -> bool:
    if isinstance(message, UserMessage):
        return True
    if isinstance(message, AssistantMessage):
        return not message.tool_calls and not message.thought_signatures
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
    return SystemMessage(session_text("transcript", "packed_history_note", packed=packed).strip())


def _pack_messages_with_toons(messages: list[ChatMessage]) -> list[ChatMessage]:
    systems = [message for message in messages if isinstance(message, SystemMessage)]
    other = [message for message in messages if not isinstance(message, SystemMessage)]
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
        if isinstance(message, AssistantMessage) and message.tool_calls:
            end = index + 1
            while end < len(messages) and isinstance(messages[end], ToolMessage):
                end += 1
            units.append(messages[index:end])
            index = end
            continue
        units.append([message])
        index += 1
    return units


class Transcript(msgspec.Struct, omit_defaults=True):
    messages: list[ChatMessage] = msgspec.field(default_factory=list)
    max_context_tokens: int = rt.MAX_CONTEXT_TOKENS
    max_message_tokens: int = rt.BUDGETS.message_tokens

    @classmethod
    def with_system_prompt(cls, system_prompt: str) -> "Transcript":
        transcript = cls()
        transcript.set_system_prompt(system_prompt)
        return transcript

    def set_system_prompt(self, system_prompt: str) -> None:
        if self.messages and isinstance(self.messages[0], SystemMessage):
            self.messages[0] = SystemMessage(system_prompt)
        else:
            self.messages[:0] = [SystemMessage(system_prompt)]

    def clear(self, system_prompt: str) -> None:
        self.messages.clear()
        self.set_system_prompt(system_prompt)

    def checkpoint(self) -> int:
        return len(self.messages)

    def rollback(self, checkpoint: int) -> None:
        del self.messages[checkpoint:]

    def undo_last_turn(self) -> bool:
        for index in range(len(self.messages) - 1, 0, -1):
            if isinstance(self.messages[index], UserMessage):
                del self.messages[index:]
                return True
        return False

    def add_user(self, prompt: str) -> None:
        self.messages.append(UserMessage(prompt))

    def add_assistant(self, message: AssistantMessage) -> None:
        self.messages.append(message)

    def add_tool_outputs(self, calls, results) -> None:
        self.messages.extend(
            ToolMessage(tool_call_id=call_id, name=name, content=result)
            for (call_id, name, _), (_, result) in zip(calls, results, strict=False)
        )

    def prepared_messages(
        self, model: str | None = None, todos: list[TodoItem] | None = None
    ) -> list[ChatMessage]:
        messages = [_truncate_message(message, self.max_message_tokens) for message in self.messages]
        if model:
            messages = _pack_messages_with_toons(messages)
        system_messages = [message for message in messages if isinstance(message, SystemMessage)]
        if todos:
            system_messages.append(SystemMessage(session_text("transcript", "todo_system", todos=_format_todos(todos)).strip()))
        other = [message for message in messages if not isinstance(message, SystemMessage)]
        budget = self.max_context_tokens - sum(map(_message_tokens, system_messages))
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
        return system_messages + (
            [UserMessage(session_text("transcript", "omitted_messages", omitted_messages=omitted_messages))]
            if omitted_messages > 0
            else []
        ) + kept

    def session_tokens(self) -> int:
        return sum(map(_message_tokens, self.messages))

    def prepared_tokens(
        self, model: str | None = None, todos: list[TodoItem] | None = None
    ) -> int:
        return sum(map(_message_tokens, self.prepared_messages(model=model, todos=todos)))


_tokenizer: tiktoken.Encoding | None = None


class _WaitIndicator:
    def __init__(self, label: str):
        self._label = label
        self._active = False

    def start(self):
        self._active = True
        rt._note(self._label, tag="wait")

    def stop(self):
        self._active = False

    def update(self, label: str):
        self._label = label
        if self._active:
            rt._note(self._label, tag="wait")

    def log(self, message: str):
        rt._note(message, tag="wait")


async def run_turn(
    client,
    transcript: Transcript,
    state: AgentState,
    model_spec,
    tool_defs,
):
    _, model = rt.split_model_spec(model_spec)
    step = 0
    while True:
        state.note_progress()
        prepared = transcript.prepared_messages(model=model, todos=state.todos)
        rt._debug_log(
            "request",
            model=model_spec,
            step=step,
            messages=[rt._msg_to_dict(message) for message in prepared],
            tool_count=len(tool_defs),
        )
        size_str = rt.format_tokens(sum(map(_message_tokens, prepared)))
        spinner = _WaitIndicator(f"Waiting for {model_spec} | {size_str}")
        spinner.start()

        def on_retry(attempt, max_attempts, error_ctx=None):
            excerpt = ""
            if error_ctx:
                excerpt = " | ".join(line.strip() for line in error_ctx.strip().splitlines()[:3] if line.strip())
            spinner.log(f"retry {attempt}/{max_attempts}{': ' + excerpt if excerpt else ''}")
            spinner.update(f"Retrying {model_spec} (attempt {attempt}/{max_attempts}) | {size_str}")

        try:
            message = await asyncio.wait_for(
                cast(CompletionClient, client).chat_completion(
                    model=model,
                    messages=prepared,
                    tools=tool_defs,
                    tool_choice="auto",
                    on_retry=on_retry,
                ),
                timeout=state.remaining_unattended_seconds(),
            )
        except asyncio.TimeoutError as exc:
            raise TimeoutError(
                "reached unattended timeout "
                f"({rt._format_duration(state.unattended_timeout_seconds)}) without a final response"
            ) from exc
        finally:
            spinner.stop()
        calls = [(call.id, call.name, call.arguments) for call in message.tool_calls]
        rt._debug_log("response", model=model_spec, step=step, assistant=rt._msg_to_dict(message))
        if calls:
            transcript.add_assistant(message)
            results = [
                (call_id, state.tool_specs.invoke(state, name, args))
                for call_id, name, args in calls
            ]
            rt._debug_log(
                "tool_results",
                model=model_spec,
                step=step,
                results=[
                    {"call_id": call_id, "name": name, "ok": result.ok}
                    for (call_id, name, _), (_, result) in zip(calls, results, strict=False)
                ],
            )
            transcript.add_tool_outputs(calls, results)
            step += 1
            continue
        rt._print(value=message.content)
        return 0, message.content


async def run_agent(
    prompt,
    model,
    root,
    system_prompt,
    unattended_timeout_seconds,
    interactive,
    transcript: Transcript | None = None,
):
    tool_specs = active_tool_specs(interactive)
    unattended_timeout_seconds = _positive_int(
        unattended_timeout_seconds, "unattended_timeout_seconds"
    )
    state = AgentState.new(
        root=root,
        tool_specs=tool_specs,
        unattended_timeout_seconds=unattended_timeout_seconds,
        interactive=interactive,
    )
    if transcript is None:
        transcript = Transcript.with_system_prompt(system_prompt)
    else:
        transcript.set_system_prompt(system_prompt)
    transcript.add_user(prompt)

    async def runner(client):
        return await run_turn(client, transcript, state, model, tool_specs.specs())

    try:
        return await runner(rt.get_client(model))
    except (AuthenticationError, PermissionDeniedError) as exc:
        if not rt.ensure_api_env(root):
            return rt.fail(
                f"API {'authentication' if isinstance(exc, AuthenticationError) else 'permission'} error: {exc}"
            ), ""
        rt._print("warning", "Credentials expired. Refreshing.", err=True)
        try:
            return await runner(rt.get_client(model))
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
    "_message_tokens",
    "_pack_messages_with_toons",
    "_packed_history_note",
    "run_agent",
    "run_turn",
]
