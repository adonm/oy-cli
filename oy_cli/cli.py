from __future__ import annotations

import argparse
import os
import sys
import time
from pathlib import Path

import defopt
from prompt_toolkit.history import FileHistory

from . import runtime as rt
from .agent import (
    Transcript,
    add_user,
    checkpoint,
    clear_transcript,
    new_agent_state,
    prepared_tokens,
    rollback,
    run_agent,
    run_turn,
    set_system_prompt,
    session_tokens,
    transcript,
    transcript_with_system_prompt,
    undo_last_turn,
)
from .runtime import (
    AUDIT_SYSTEM_PROMPT,
    active_system_prompt,
    ask_system_prompt,
    read_only_tool_registry,
    session_text,
)
from .tools import tool_specs


_SESSIONS_DIR: Path | None = None

def _sessions_dir() -> Path:
    return _SESSIONS_DIR or (rt.CONFIG_PATH.parent / "sessions")


def _session_name(name: str) -> str:
    return "".join(
        char if char.isascii() and (char.isalnum() or char in "_-") else "_"
        for char in name
    )


def _session_file(name: str) -> Path:
    return _sessions_dir() / f"{_session_name(name)}.json"

def _transcript_data(transcript: Transcript) -> dict[str, object]:
    return {
        "messages": list(transcript["messages"]),
        "max_context_tokens": transcript["max_context_tokens"],
        "max_message_tokens": transcript["max_message_tokens"],
    }


def _load_transcript(data: object) -> Transcript:
    if not isinstance(data, dict):
        raise ValueError("Invalid transcript payload")
    messages = data.get("messages")
    if not isinstance(messages, list):
        raise ValueError("Invalid transcript messages")
    max_context_tokens = data.get("max_context_tokens", rt.MAX_CONTEXT_TOKENS)
    max_message_tokens = data.get("max_message_tokens", rt.BUDGETS["message_tokens"])
    if not isinstance(max_context_tokens, int) or not isinstance(max_message_tokens, int):
        raise ValueError("Invalid transcript token limits")
    return transcript(
        messages=list(messages),
        max_context_tokens=max_context_tokens,
        max_message_tokens=max_message_tokens,
    )

def load_system_prompt(system_file, interactive):
    base = active_system_prompt(interactive)
    if system_file is None:
        return base
    if not system_file.exists():
        rt.abort(f"System file does not exist: {rt._fmt('inline', system_file)}")
    if system_file.is_dir():
        rt.abort(f"System file is a directory: {rt._fmt('inline', system_file)}")
    try:
        return base + "\n\n" + system_file.read_text(encoding="utf-8")
    except OSError as exc:
        rt.abort(f"Could not read system file {rt._fmt('inline', system_file)}: {exc}")

def _set_terminal_title(title: str) -> None:
    if sys.stderr.isatty():
        sys.stderr.write(f"\033]0;{title}\007")
        sys.stderr.flush()

def _print_session_intro(heading: str, session, **extras) -> None:
    lines = [
        f"## {heading}",
        "",
        f"- workspace: {rt._fmt('inline', session['workspace'])}",
        f"- model: {rt._fmt('inline', session['model'])}",
        f"- mode: {rt._fmt('inline', 'interactive' if session['interactive'] else 'non-interactive')}",
    ]
    if session['system_file'] is not None:
        extras["system file"] = session['system_file'].resolve()
    for key, value in extras.items():
        if value is not None:
            lines.append(f"- {key}: {rt._fmt('inline', value)}")
    if rt._debug_log_path:
        lines.append(f"- debug log: {rt._fmt('inline', rt._debug_log_path)}")
    rt._print(value="\n".join(lines), err=True)
    _, model = rt.split_model_spec(session['model'])
    _set_terminal_title(f"oy · {model} · {session['workspace'].name}")

def workspace_root():
    workspace = Path(os.environ.get("OY_ROOT", ".")).expanduser().resolve()
    if not workspace.is_dir():
        rt.abort(f"Workspace root is not a directory: {rt._fmt('inline', workspace)}")
    return workspace

def resolve_session(
    *,
    interactive: bool | None = None,
    system_prompt: str | None = None,
    include_system_file: bool = True,
):
    resolved_interactive = rt.can_prompt() if interactive is None else interactive
    system_file = rt._sys_file() if include_system_file else None
    return rt.session_context(
        workspace=workspace_root(),
        model=rt._model(None),
        interactive=resolved_interactive,
        system_prompt=(
            load_system_prompt(system_file, resolved_interactive)
            if system_prompt is None
            else system_prompt
        ),
        system_file=system_file,
        yolo=rt.yolo_enabled(),
    )

def audit(focus: str = ""):
    """Run a one-shot security and complexity audit.

    :param focus: Optional area to focus on, such as auth, tests, or a file path.
    """
    session = resolve_session(
        interactive=False,
        system_prompt=AUDIT_SYSTEM_PROMPT,
        include_system_file=False,
    )
    audit_prompt = session_text("audit", "default_user_prompt")
    if focus:
        audit_prompt += session_text("audit", "focus_suffix", focus=focus)
    _print_session_intro("Audit", session, focus=rt.preview(focus, 100) if focus else None)
    code, _ = run_agent(
            audit_prompt,
            session['model'],
            session['workspace'],
            session['system_prompt'],
            rt.unattended_limit_seconds(),
            interactive=False,
    )
    return code

def _create_prompt_session():
    history_path = rt._history_path()
    commands = [
        "/help",
        "/tokens",
        "/model",
        "/debug",
        "/yolo",
        "/ask",
        "/audit",
        "/save",
        "/load",
        "/undo",
        "/clear",
        "/quit",
        "/exit",
    ]
    return rt.prompt_session(
        console=rt.STDERR,
        history=FileHistory(str(history_path)),
        choices=commands,
        multiline=False,
        enable_open_in_editor=True,
    )


def _git_diff_shortstat(workspace: Path) -> str | None:
    try:
        result = rt.run_cmd(
            [
                "git",
                "-C",
                str(workspace),
                "diff",
                "--shortstat",
                "--no-ext-diff",
                "HEAD",
                "--",
            ],
            timeout=5,
        )
    except Exception:
        return None
    if result.returncode != 0:
        return None
    summary = result.stdout.strip()
    return summary or "git diff: clean"

def _read_input(prompt_session, workspace: Path):
    prompt = "\x1b[1;32moy ❯\x1b[0m "
    if summary := _git_diff_shortstat(workspace):
        return prompt_session.prompt(rt.ANSI(f"\x1b[2m{summary}\x1b[0m\n{prompt}"))
    return prompt_session.prompt(rt.ANSI(prompt))

def _chat_command(cmd, transcript, system_prompt, model_spec):
    parts = cmd.strip().split(None, 1)
    name = parts[0].lower()
    arg = parts[1].strip() if len(parts) > 1 else ""
    _, model = rt.split_model_spec(model_spec)
    if name in ("/help", "/?"):
        rt._print(
            value="\n".join(
                [
                    "## Commands",
                    "",
                    "- `/help` -- show this help",
                    "- `/tokens` -- show context usage",
                    "- `/model [filter]` -- show or switch model",
                    "- `/debug` -- toggle debug logging",
                    "- `/yolo` -- allow all tools for the rest of this session",
                    "- `/ask <question>` -- research-only query (read-only, no changes)",
                    "- `/audit [focus]` -- run a security/complexity audit",
                    "- `/save [name]` -- save session transcript",
                    "- `/load [name]` -- load a saved session",
                    "- `/undo` -- remove the last prompt and its follow-up messages",
                    "- `/clear` -- reset conversation (keeps system prompt)",
                    "- `/quit` or `/exit` -- end session",
                    "",
                    "Older conversation history may be packed into TOON before model requests.",
                    "Paste multiline text directly — bracketed paste keeps it intact.",
                    "Press Meta+E to open your $EDITOR for longer prompts.",
                ]
            ),
            err=True,
        )
        return True
    if name == "/tokens":
        total = session_tokens(transcript)
        prepped = prepared_tokens(transcript, model=model)
        budget = transcript["max_context_tokens"]
        rt._print(
            value="\n".join(
                [
                    "## Context",
                    "",
                    f"- messages: {len(transcript["messages"])}",
                    f"- session tokens: {rt.format_tokens(total)}",
                    f"- prepared tokens: {rt.format_tokens(prepped)}",
                    f"- context budget: {rt.format_tokens(budget)}",
                    f"- remaining: ~{rt.format_tokens(max(budget - prepped, 0))}",
                ]
            ),
            err=True,
        )
        return True
    if name == "/model":
        return ("model", arg)
    if name == "/debug":
        return ("debug",)
    if name == "/yolo":
        return ("yolo",)
    if name == "/ask":
        return ("ask", arg)
    if name == "/audit":
        return ("audit", arg)
    if name == "/save":
        return ("save", arg)
    if name == "/load":
        return ("load", arg)
    if name == "/undo":
        if undo_last_turn(transcript):
            rt._note("undid last turn", tag="note")
        else:
            rt._warn("Nothing to undo.")
        return True
    if name == "/clear":
        clear_transcript(transcript, system_prompt)
        rt._note("cleared conversation", tag="note")
        return True
    if name in ("/quit", "/exit"):
        return None
    return False

def _handle_model_switch(arg, current_model):
    if not arg:
        rt._print(value=_current_model_text(current_model), err=True)
        rt._note("use /model <name> to switch, or /model list to browse", tag="note")
        return current_model
    if arg.lower() == "list":
        try:
            chosen = resolve_model_choice()
        except SystemExit:
            return current_model
        return chosen if chosen else current_model
    try:
        all_models = rt.list_all_model_ids()
    except SystemExit:
        rt._warn("Could not load model list.")
        return current_model
    if arg in all_models:
        rt._note(f"switched model: {arg}", tag="note")
        return arg
    matches = [model for model in all_models if arg.lower() in model.lower()]
    if len(matches) == 1:
        rt._note(f"switched model: {matches[0]}", tag="note")
        return matches[0]
    if matches:
        rt.render_model_list(
            matches,
            title="## Matching Models",
            query=arg,
            current=current_model,
            err=True,
        )
        rt._print(
            value="Be more specific or use `/model list` to choose interactively.",
            err=True,
        )
    else:
        rt._warn(f"No models matching {rt._fmt('inline', arg)}.")
    return current_model

def _handle_debug_toggle():
    if rt._debug_logger is not None:
        for handler in list(rt._debug_logger.handlers):
            handler.close()
            rt._debug_logger.removeHandler(handler)
        rt._debug_logger = None
        rt._debug_log_path = None
        rt._note("debug logging disabled", tag="note")
    else:
        os.environ["OY_DEBUG"] = "1"
        rt._debug_logger, rt._debug_log_path = rt._init_debug_log()
        rt._note(f"debug logging enabled: {rt._debug_log_path}", tag="note")


def _handle_yolo_toggle(session):
    if session['yolo']:
        rt._note("yolo already enabled for this session", tag="note")
        return session
    session['yolo'] = True
    return session

def _handle_ask(question, current_model, session, transcript):
    if not question:
        rt._print(
            value="Usage: `/ask <question>` — research the codebase without making changes.",
            err=True,
        )
        return
    read_only_registry = read_only_tool_registry()
    ask_transcript = transcript_with_system_prompt(ask_system_prompt(session['system_prompt']))
    for msg in transcript["messages"][-6:]:
        if msg.get("role") != "system":
            ask_transcript["messages"].append(msg)

    rt._note("research mode (read-only)", tag="note")
    state = new_agent_state(
        root=session['workspace'],
        tool_registry=read_only_registry,
        unattended_limit_seconds=rt.unattended_limit_seconds(),
        interactive=session['interactive'],
    )
    add_user(ask_transcript, question)

    try:
        client = rt.get_client(current_model)
        run_turn(
            client,
            ask_transcript,
            state,
            current_model,
            tool_specs(read_only_registry),
        )
    except KeyboardInterrupt:
        rt._note("research cancelled", tag="note")
    except Exception as exc:
        rt._error(f"Research error: {exc}")

def _handle_audit(focus, current_model, session):
    audit_prompt = session_text("audit", "repo_user_prompt")
    if focus:
        audit_prompt += session_text("audit", "focus_suffix", focus=focus)

    rt._note("audit mode", tag="note")
    audit_transcript = transcript_with_system_prompt(AUDIT_SYSTEM_PROMPT)

    try:
        run_agent(
                audit_prompt,
                current_model,
                session['workspace'],
                AUDIT_SYSTEM_PROMPT,
                rt.unattended_limit_seconds(),
                interactive=False,
                transcript=audit_transcript,
        )
    except KeyboardInterrupt:
        rt._note("audit cancelled", tag="note")
    except Exception as exc:
        rt._error(f"Audit error: {exc}")

def _handle_save(name, transcript, current_model):
    rt._ensure_private_dir(_sessions_dir())
    if not name:
        name = time.strftime("%Y%m%d-%H%M%S")
    path = _session_file(name)
    data = {
        "model": current_model,
        "saved_at": time.strftime("%Y-%m-%dT%H:%M:%S"),
        "transcript": _transcript_data(transcript),
    }
    rt.save_json(path, data)
    rt._note(f"saved session: {path.name}", tag="note")

def _handle_load(name, transcript, current_model, system_prompt):
    sessions_dir = rt._ensure_private_dir(_sessions_dir())
    sessions = sorted(
        sessions_dir.glob("*.json"), key=lambda path: path.stat().st_mtime, reverse=True
    )
    if not sessions:
        rt._warn("No saved sessions found.")
        return transcript, current_model
    if not name:
        lines = ["## Saved Sessions", ""]
        for index, path in enumerate(sessions[:20], 1):
            try:
                meta = rt.load_json(path, {})
                model = meta.get("model", "?")
                saved = meta.get("saved_at", "?")
                msgs = len(meta.get("transcript", {}).get("messages", []))
                lines.append(
                    f"{index}. {rt._fmt('inline', path.stem)} — {model}, {msgs} msgs, {saved}"
                )
            except Exception:
                lines.append(f"{index}. {rt._fmt('inline', path.stem)} — (unreadable)")
        lines.extend(["", "Usage: `/load <name>` or `/load <number>`"])
        rt._print(value="\n".join(lines), err=True)
        return transcript, current_model
    target = None
    if name.isdigit():
        index = int(name) - 1
        if 0 <= index < len(sessions):
            target = sessions[index]
    if target is None:
        candidate = _session_file(name)
        if candidate.exists():
            target = candidate
    if target is None:
        matches = [path for path in sessions if name.lower() in path.stem.lower()]
        if len(matches) == 1:
            target = matches[0]
        elif matches:
            rt._warn(f"Ambiguous — {len(matches)} sessions match. Be more specific.")
            return transcript, current_model
    if target is None:
        rt._warn(f"No session found matching {rt._fmt('inline', name)}.")
        return transcript, current_model
    try:
        data = rt.load_json(target, None)
        if data is None:
            raise ValueError("Empty or invalid session file")
        loaded = _load_transcript(data["transcript"])
        loaded_model = data.get("model", current_model)
        set_system_prompt(loaded, system_prompt)
        rt._note(
            f"loaded session: {target.stem} ({len(loaded['messages'])} messages, model: {loaded_model})",
            tag="note",
        )
        return loaded, loaded_model
    except Exception as exc:
        rt._error(f"Failed to load session: {exc}")
        return transcript, current_model

def chat(*, yolo: bool = False):
    """Start an interactive multi-turn chat session.

    :param yolo: Allow all tools without per-action approval prompts.
    """
    if yolo:
        os.environ["OY_YOLO"] = "1"
    prompt_session = _create_prompt_session()
    session = resolve_session(interactive=True)
    _print_session_intro("Chat", session)
    rt._note(
        "chat mode; /help for commands"
        + ("; yolo on" if session['yolo'] else ""),
        tag="note",
    )

    transcript = transcript_with_system_prompt(session['system_prompt'])
    current_model = session['model']

    while True:
        try:
            rt.print_console(rt.STDERR)
            rt.rule_console(rt.STDERR, style="dim")
            prompt = _read_input(prompt_session, session['workspace'])
        except KeyboardInterrupt:
            rt.print_console(rt.STDERR)
            continue
        except EOFError:
            rt._note("session ended", tag="note")
            break

        if not prompt.strip():
            continue
        if prompt.strip().startswith("/"):
            result = _chat_command(prompt.strip(), transcript, session['system_prompt'], current_model)
            if result is None:
                break
            if isinstance(result, tuple):
                if result[0] == "model":
                    current_model = _handle_model_switch(result[1], current_model)
                    _, model = rt.split_model_spec(current_model)
                    _set_terminal_title(f"oy · {model} · {session['workspace'].name}")
                elif result[0] == "debug":
                    _handle_debug_toggle()
                elif result[0] == "yolo":
                    next_session = _handle_yolo_toggle(session)
                    if next_session is not session:
                        session = next_session
                        rt._note("yolo enabled; all tools allowed for this session", tag="note")
                elif result[0] == "ask":
                    _handle_ask(result[1], current_model, session, transcript)
                elif result[0] == "audit":
                    _handle_audit(result[1], current_model, session)
                elif result[0] == "save":
                    _handle_save(result[1], transcript, current_model)
                elif result[0] == "load":
                    transcript, current_model = _handle_load(
                        result[1], transcript, current_model, session['system_prompt']
                    )
                    _, model = rt.split_model_spec(current_model)
                    _set_terminal_title(f"oy · {model} · {session['workspace'].name}")
                continue
            if result:
                continue
            rt._warn(f"Unknown command: {prompt.strip().split()[0]}")
            continue
        if prompt.strip().lower() in ("exit", "quit"):
            break
        checkpoint_point = checkpoint(transcript)
        try:
            code, _ = run_agent(
                    prompt,
                    current_model,
                    session['workspace'],
                    session['system_prompt'],
                    rt.unattended_limit_seconds(),
                    session['interactive'],
                    yolo=session['yolo'],
                    transcript=transcript,
            )
        except KeyboardInterrupt:
            rollback(transcript, checkpoint_point)
            rt._note("cancelled; prompt still in history (press ↑)", tag="note")
            continue
        except Exception as exc:
            rollback(transcript, checkpoint_point)
            rt._error(f"Agent error: {exc}")
            rt._note("prompt still in history (press ↑)", tag="note")
            continue

        _ = code
        _, model = rt.split_model_spec(current_model)
        prepped = prepared_tokens(transcript, model=model)
        remaining = max(transcript["max_context_tokens"] - prepped, 0)
        rt._note(
            f"context: {rt.format_tokens(prepped)} used, ~{rt.format_tokens(remaining)} remaining",
            tag="note",
        )

    _set_terminal_title("")
    return 0

def run(*task: str):
    """Run a one-shot task.

    :param task: Task text. If omitted, read from stdin or start chat in a TTY.
    """
    task_text = (
        " ".join(task)
        if task
        else (sys.stdin.read().strip() if not rt.has_tty_stdin() else "")
    )
    if not task_text:
        return chat()

    session = resolve_session(interactive=False)
    _print_session_intro("Run", session, prompt=rt.preview(task_text, 100))
    return run_agent(
            task_text,
            session['model'],
            session['workspace'],
            session['system_prompt'],
            rt.unattended_limit_seconds(),
            session['interactive'],
    )[0]

def ralph(*task: str):
    """Run a task in yolo mode every minute until the configured deadline.

    Controlled by `OY_RALPH_LIMIT` (default: `3h`).

    :param task: Task text. If omitted, read from stdin.
    """
    task_text = (
        " ".join(task)
        if task
        else (sys.stdin.read().strip() if not rt.has_tty_stdin() else "")
    )
    if not task_text:
        rt._print(
            value="Usage: `oy ralph <prompt>` — or pipe prompt text on stdin.",
            err=True,
        )
        return 1

    session = resolve_session(interactive=False)
    session['yolo'] = True
    delay_seconds = 60
    limit_seconds = rt.ralph_limit_seconds()
    deadline = time.monotonic() + limit_seconds
    _print_session_intro(
        "Ralph",
        session,
        prompt=rt.preview(task_text, 100),
        schedule=f"until {rt._format_duration(limit_seconds)} deadline, {rt._format_duration(delay_seconds)} delay",
    )

    exit_code = 0
    run_number = 0
    while True:
        now = time.monotonic()
        if run_number > 0 and now >= deadline:
            break
        run_number += 1
        remaining = max(int(deadline - now), 0)
        rt._note(
            f"ralph run {run_number} (~{rt._format_duration(remaining)} remaining)",
            tag="note",
        )
        code, _ = run_agent(
                task_text,
                session['model'],
                session['workspace'],
                session['system_prompt'],
                rt.unattended_limit_seconds(),
                session['interactive'],
                yolo=True,
        )
        if code != 0:
            exit_code = code
        sleep_seconds = deadline - time.monotonic()
        if sleep_seconds <= 0:
            break
        time.sleep(min(delay_seconds, sleep_seconds))
    return exit_code

def _current_model_text(model_spec: str) -> str:
    shim = rt.resolve_active_shim(model_spec)
    _, bare = rt.split_model_spec(model_spec)
    return (
        f"## Current Model\n\n- model: {rt._fmt('inline', bare)}\n"
        f"- shim: {rt._fmt('inline', shim)}"
    )

def resolve_model_choice(model_id=None):
    available, current = rt.list_all_model_ids(), rt._model(None)
    if model_id in available:
        return model_id
    if not rt.can_prompt():
        if model_id:
            matches = [model for model in available if model_id.strip().lower() in model.lower()]
            if matches:
                rt.render_model_list(
                    matches,
                    title="## Matching Models",
                    query=model_id,
                    current=current,
                    err=True,
                )
            rt.abort(
                f"No exact model match for {rt._fmt('inline', model_id)}. Re-run in a TTY to filter and choose interactively."
            )
        return None
    rt._print(
        value=(
            "## Choose a Model\n\n"
            "- Enter an exact model ID to save it.\n"
            "- Enter text to filter the list.\n"
            "- Enter a number to pick from the currently listed models."
        ),
        err=True,
    )
    if model_id is None:
        rt.render_model_list(available, title="## Available Models", current=current, err=True)
    shown = available
    query = model_id or rt.ask("Model or filter", console=rt.STDERR, default=current).strip()
    while True:
        query = query.strip() or current
        if query in available:
            return query
        if query.isdigit() and 1 <= (index := int(query)) <= len(shown):
            return shown[index - 1]
        shown = [model for model in available if query.lower() in model.lower()]
        rt.render_model_list(shown, title="## Matching Models", query=query, current=current, err=True)
        query = rt.ask("Model or filter", console=rt.STDERR).strip()

def model(model: str | None = None):
    """Show or change the default model.

    :param model: Exact model id or filter text to select from available models.
    """
    current = rt._model(None)
    if model is None and not rt.can_prompt():
        rt._print(value=_current_model_text(current))
        return 0
    if current:
        rt._print(value=_current_model_text(current), err=True)
        if model is None and not rt.yes_no(
            "Pick a new model?", console=rt.STDERR, default=False
        ):
            return 0
    chosen = resolve_model_choice(model)
    if chosen is None:
        return 1
    config = rt.save_model_config(chosen)
    rt._print(
        value=(
            f"## Default Model Updated\n\n- selected: {rt._fmt('inline', chosen)}"
            + (f"\n- shim: {rt._fmt('inline', config['shim'])}" if config['shim'] else "")
        )
    )
    return 0

def main(argv: list[str] | None = None):
    """Run the top-level `oy` CLI.

    Global behavior:
    - bare text defaults to `run`
    - `--version` works at the top level
    - other flags must follow an explicit subcommand
    """
    args = list(sys.argv[1:] if argv is None else argv)

    commands = {"run", "chat", "ralph", "model", "audit", "-h", "--help"}
    if not args:
        args = ["run"] if not rt.stdin_is_interactive() else ["--help"]
    elif args[0] in {"-v", "--version"}:
        rt._print(value=f"oy {rt.__version__}")
        return 0
    elif not args[0].startswith("-") and args[0] not in commands:
        args = ["run", *args]
    result = defopt.run(
        [run, chat, ralph, model, audit],
        argv=args,
        version=rt.__version__,
        short={},
        show_defaults=False,
        no_negated_flags=True,
        argparse_kwargs={
            "description": "AI coding assistant for your shell.",
            "epilog": """Examples:
  oy "fix the failing tests"
  oy chat
  oy chat --yolo
  oy ralph "fix the flaky test"
  oy audit auth
  oy model gpt-5""",
            "formatter_class": argparse.RawDescriptionHelpFormatter,
        },
    )
    return 0 if result is None else result

__all__ = [
    "_SESSIONS_DIR",
    "_chat_command",
    "_create_prompt_session",
    "_current_model_text",
    "_git_diff_shortstat",
    "_handle_ask",
    "_handle_audit",
    "_handle_debug_toggle",
    "_handle_load",
    "_handle_model_switch",
    "_handle_save",
    "_print_session_intro",
    "_read_input",
    "resolve_session",
    "_set_terminal_title",
    "workspace_root",
    "audit",
    "chat",
    "main",
    "model",
    "load_system_prompt",
    "ralph",
    "resolve_model_choice",
    "run",
]
