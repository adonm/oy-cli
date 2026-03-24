from __future__ import annotations

import asyncio
import os
import re
import sys
import time
from pathlib import Path

import defopt
import msgspec

from . import runtime as rt
from .agent import AgentState, Transcript, run_agent, run_turn
from .providers import SystemMessage
from .runtime import (
    AUDIT_SYSTEM_PROMPT,
    active_system_prompt,
    ask_system_prompt,
    read_only_tool_specs,
    session_text,
)
run_cmd = rt.run_cmd
save_json = rt.save_json

_SESSIONS_DIR: Path | None = None

def _sessions_dir() -> Path:
    return _SESSIONS_DIR or (rt.CONFIG_PATH.parent / "sessions")

def _pick_model():
    return rt._pick_model()

def _save_cfg(data):
    return rt._save_cfg(data)

def read_system_prompt(system_file, interactive):
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
        f"- workspace: {rt._fmt('inline', session.workspace)}",
        f"- model: {rt._fmt('inline', session.model)}",
        f"- mode: {rt._fmt('inline', 'interactive' if session.interactive else 'non-interactive')}",
    ]
    if session.system_file is not None:
        extras["system file"] = session.system_file.resolve()
    for key, value in extras.items():
        if value is not None:
            lines.append(f"- {key}: {rt._fmt('inline', value)}")
    if rt._debug_log_path:
        lines.append(f"- debug log: {rt._fmt('inline', rt._debug_log_path)}")
    rt._print(value="\n".join(lines), err=True)
    _, model = rt.split_model_spec(session.model)
    _set_terminal_title(f"oy · {model} · {session.workspace.name}")

def _workspace():
    workspace = Path(os.environ.get("OY_ROOT", ".")).expanduser().resolve()
    if not workspace.is_dir():
        rt.abort(f"Workspace root is not a directory: {rt._fmt('inline', workspace)}")
    return workspace

def _resolve_session(
    *,
    interactive: bool | None = None,
    system_prompt: str | None = None,
    include_system_file: bool = True,
):
    resolved_interactive = (
        sys.stdin.isatty() and not rt._flag("OY_NON_INTERACTIVE", False)
        if interactive is None
        else interactive
    )
    system_file = rt._sys_file() if include_system_file else None
    return rt.SessionContext(
        workspace=_workspace(),
        model=rt._model(None),
        interactive=resolved_interactive,
        system_prompt=(
            read_system_prompt(system_file, resolved_interactive)
            if system_prompt is None
            else system_prompt
        ),
        system_file=system_file,
    )

def audit(prompt: str = ""):
    session = _resolve_session(
        interactive=False,
        system_prompt=AUDIT_SYSTEM_PROMPT,
        include_system_file=False,
    )
    audit_prompt = session_text("audit", "default_user_prompt")
    if prompt:
        audit_prompt += session_text("audit", "focus_suffix", focus=prompt)
    _print_session_intro("Audit", session, focus=rt.preview(prompt, 100) if prompt else None)
    code, _ = asyncio.run(
        run_agent(
            audit_prompt,
            session.model,
            session.workspace,
            session.system_prompt,
            rt.DEFAULT_UNATTENDED_TIMEOUT_SECONDS,
            interactive=False,
        )
    )
    return code

def _create_prompt_session():
    from prompt_toolkit import PromptSession
    from prompt_toolkit.completion import WordCompleter
    from prompt_toolkit.history import FileHistory

    history_path = rt.CONFIG_PATH.parent / "history"
    rt._ensure_private_dir(history_path.parent)
    history_path.touch(mode=0o600, exist_ok=True)
    history_path.chmod(0o600)

    commands = [
        "/help",
        "/tokens",
        "/model",
        "/debug",
        "/ask",
        "/audit",
        "/save",
        "/load",
        "/undo",
        "/clear",
        "/quit",
        "/exit",
    ]
    return PromptSession(
        history=FileHistory(str(history_path)),
        completer=WordCompleter(commands, sentence=True),
        multiline=False,
        enable_open_in_editor=True,
    )

def _git_diff_shortstat(workspace: Path) -> str | None:
    try:
        result = run_cmd(
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
    from prompt_toolkit.formatted_text import ANSI

    prompt = "\x1b[1;32moy ❯\x1b[0m "
    if summary := _git_diff_shortstat(workspace):
        return prompt_session.prompt(ANSI(f"\x1b[2m{summary}\x1b[0m\n{prompt}"))
    return prompt_session.prompt(ANSI(prompt))

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
                    "- `/model [query]` -- show or switch model",
                    "- `/debug` -- toggle debug logging",
                    "- `/ask <question>` -- research-only query (read-only, no changes)",
                    "- `/audit [focus]` -- run a security/complexity audit",
                    "- `/save [name]` -- save session transcript",
                    "- `/load [name]` -- load a saved session",
                    "- `/undo` -- remove the last prompt and its follow-up messages",
                    "- `/clear` -- reset conversation (keeps system prompt)",
                    "- `/quit` or `/exit` -- end session",
                    "",
                    "Context is compressed with Headroom before model requests.",
                    "Paste multiline text directly — bracketed paste keeps it intact.",
                    "Press Meta+E to open your $EDITOR for longer prompts.",
                ]
            ),
            err=True,
        )
        return True
    if name == "/tokens":
        total = transcript.session_tokens()
        prepped = transcript.prepared_tokens(model=model)
        budget = transcript.max_context_tokens
        rt._print(
            value="\n".join(
                [
                    "## Context",
                    "",
                    f"- messages: {len(transcript.messages)}",
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
    if name == "/ask":
        return ("ask", arg)
    if name == "/audit":
        return ("audit", arg)
    if name == "/save":
        return ("save", arg)
    if name == "/load":
        return ("load", arg)
    if name == "/undo":
        if transcript.undo_last_turn():
            rt._print(value="Undid last turn.", err=True)
        else:
            rt._print("warning", "Nothing to undo.", err=True)
        return True
    if name == "/clear":
        transcript.clear(system_prompt)
        rt._print(value="Conversation cleared.", err=True)
        return True
    if name in ("/quit", "/exit"):
        return None
    return False

def render_model_list(items, *, title, query=None, current=None, err=False, limit=None):
    return rt.render_model_list(
        items, title=title, query=query, current=current, err=err, limit=limit
    )

def _handle_model_switch(arg, current_model):
    if not arg:
        rt._print(value=f"Current model: {rt._fmt('inline', current_model)}", err=True)
        rt._print(
            value="Usage: `/model <name>` to switch, or `/model list` to browse.",
            err=True,
        )
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
        rt._print("warning", "Could not load model list.", err=True)
        return current_model
    if arg in all_models:
        rt._print(value=f"Switched to {rt._fmt('inline', arg)}", err=True)
        return arg
    matches = [model for model in all_models if arg.lower() in model.lower()]
    if len(matches) == 1:
        rt._print(value=f"Switched to {rt._fmt('inline', matches[0])}", err=True)
        return matches[0]
    if matches:
        render_model_list(
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
        rt._print("warning", f"No models matching {rt._fmt('inline', arg)}.", err=True)
    return current_model

def _handle_debug_toggle():
    if rt._debug_logger is not None:
        for handler in list(rt._debug_logger.handlers):
            handler.close()
            rt._debug_logger.removeHandler(handler)
        rt._debug_logger = None
        rt._debug_log_path = None
        rt._print(value="Debug logging **disabled**.", err=True)
    else:
        os.environ["OY_DEBUG"] = "1"
        rt._debug_logger, rt._debug_log_path = rt._init_debug_log()
        rt._print(
            value=f"Debug logging **enabled** → {rt._fmt('inline', rt._debug_log_path)}",
            err=True,
        )

def _handle_ask(question, current_model, session, transcript):
    if not question:
        rt._print(
            value="Usage: `/ask <question>` — research the codebase without making changes.",
            err=True,
        )
        return
    read_only_registry = read_only_tool_specs()
    ask_transcript = Transcript.with_system_prompt(ask_system_prompt(session.system_prompt))
    for msg in transcript.messages[-6:]:
        if not isinstance(msg, SystemMessage):
            ask_transcript.messages.append(msg)

    rt._print(value="*Researching (read-only)…*", err=True)
    state = AgentState.new(
        root=session.workspace,
        tool_specs=read_only_registry,
        unattended_timeout_seconds=rt.DEFAULT_UNATTENDED_TIMEOUT_SECONDS,
    )
    ask_transcript.add_user(question)

    async def _run():
        client = rt.get_client(current_model)
        return await run_turn(
            client,
            ask_transcript,
            state,
            current_model,
            read_only_registry.specs(),
        )

    try:
        asyncio.run(_run())
    except KeyboardInterrupt:
        rt._print(value="\nResearch cancelled.", err=True)
    except Exception as exc:
        rt._print("error", f"Research error: {exc}", err=True)

def _handle_audit(focus, current_model, session):
    audit_prompt = session_text("audit", "repo_user_prompt")
    if focus:
        audit_prompt += session_text("audit", "focus_suffix", focus=focus)

    rt._print(value="*Running audit…*", err=True)
    audit_transcript = Transcript.with_system_prompt(AUDIT_SYSTEM_PROMPT)

    try:
        asyncio.run(
            run_agent(
                audit_prompt,
                current_model,
                session.workspace,
                AUDIT_SYSTEM_PROMPT,
                rt.DEFAULT_UNATTENDED_TIMEOUT_SECONDS,
                interactive=False,
                transcript=audit_transcript,
            )
        )
    except KeyboardInterrupt:
        rt._print(value="\nAudit cancelled.", err=True)
    except Exception as exc:
        rt._print("error", f"Audit error: {exc}", err=True)

def _handle_save(name, transcript, current_model):
    sessions_dir = _sessions_dir()
    rt._ensure_private_dir(sessions_dir)
    if not name:
        name = time.strftime("%Y%m%d-%H%M%S")
    safe_name = re.sub(r"[^a-zA-Z0-9_\-]", "_", name)
    path = sessions_dir / f"{safe_name}.json"
    data = {
        "model": current_model,
        "saved_at": time.strftime("%Y-%m-%dT%H:%M:%S"),
        "transcript": msgspec.to_builtins(transcript),
    }
    save_json(path, data)
    rt._print(value=f"Session saved to {rt._fmt('inline', path.name)}", err=True)

def _handle_load(name, transcript, current_model, system_prompt):
    sessions_dir = _sessions_dir()
    rt._ensure_private_dir(sessions_dir)
    sessions = sorted(
        sessions_dir.glob("*.json"), key=lambda path: path.stat().st_mtime, reverse=True
    )
    if not sessions:
        rt._print("warning", "No saved sessions found.", err=True)
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
        candidate = sessions_dir / f"{re.sub(r'[^a-zA-Z0-9_\-]', '_', name)}.json"
        if candidate.exists():
            target = candidate
    if target is None:
        matches = [path for path in sessions if name.lower() in path.stem.lower()]
        if len(matches) == 1:
            target = matches[0]
        elif matches:
            rt._print(
                "warning",
                f"Ambiguous — {len(matches)} sessions match. Be more specific.",
                err=True,
            )
            return transcript, current_model
    if target is None:
        rt._print(
            "warning", f"No session found matching {rt._fmt('inline', name)}.", err=True
        )
        return transcript, current_model
    try:
        data = rt.load_json(target, None)
        if data is None:
            raise ValueError("Empty or invalid session file")
        loaded = msgspec.convert(data["transcript"], Transcript)
        loaded_model = data.get("model", current_model)
        loaded.set_system_prompt(system_prompt)
        rt._print(
            value=f"Loaded session {rt._fmt('inline', target.stem)} — "
            f"{len(loaded.messages)} messages, model: {rt._fmt('inline', loaded_model)}",
            err=True,
        )
        return loaded, loaded_model
    except Exception as exc:
        rt._print("error", f"Failed to load session: {exc}", err=True)
        return transcript, current_model

def chat():
    prompt_session = _create_prompt_session()
    session = _resolve_session(interactive=True)
    _print_session_intro("Chat", session)
    rt._print(value="Type `/help` for commands.", err=True)

    transcript = Transcript.with_system_prompt(session.system_prompt)
    current_model = session.model

    while True:
        try:
            rt.STDERR.print()
            rt.STDERR.rule(style="dim")
            prompt = _read_input(prompt_session, session.workspace)
        except KeyboardInterrupt:
            rt.STDERR.print()
            continue
        except EOFError:
            rt._print(value="\n## Session Ended", err=True)
            break

        if not prompt.strip():
            continue
        if prompt.strip().startswith("/"):
            result = _chat_command(prompt.strip(), transcript, session.system_prompt, current_model)
            if result is None:
                break
            if isinstance(result, tuple):
                if result[0] == "model":
                    current_model = _handle_model_switch(result[1], current_model)
                    _, model = rt.split_model_spec(current_model)
                    _set_terminal_title(f"oy · {model} · {session.workspace.name}")
                elif result[0] == "debug":
                    _handle_debug_toggle()
                elif result[0] == "ask":
                    _handle_ask(result[1], current_model, session, transcript)
                elif result[0] == "audit":
                    _handle_audit(result[1], current_model, session)
                elif result[0] == "save":
                    _handle_save(result[1], transcript, current_model)
                elif result[0] == "load":
                    transcript, current_model = _handle_load(
                        result[1], transcript, current_model, session.system_prompt
                    )
                    _, model = rt.split_model_spec(current_model)
                    _set_terminal_title(f"oy · {model} · {session.workspace.name}")
                continue
            if result:
                continue
            rt._print("warning", f"Unknown command: {prompt.strip().split()[0]}", err=True)
            continue
        if prompt.strip().lower() in ("exit", "quit"):
            break

        checkpoint = transcript.checkpoint()
        try:
            code, _ = asyncio.run(
                run_agent(
                    prompt,
                    current_model,
                    session.workspace,
                    session.system_prompt,
                    rt.DEFAULT_UNATTENDED_TIMEOUT_SECONDS,
                    session.interactive,
                    transcript=transcript,
                )
            )
        except KeyboardInterrupt:
            transcript.rollback(checkpoint)
            rt._print(
                value="\nCancelled — your message is in history (press ↑).",
                err=True,
            )
            continue
        except Exception as exc:
            transcript.rollback(checkpoint)
            rt._print("error", f"Agent error: {exc}", err=True)
            rt._print(value="Your message is in history (press ↑).", err=True)
            continue

        _ = code
        _, model = rt.split_model_spec(current_model)
        prepped = transcript.prepared_tokens(model=model)
        remaining = max(transcript.max_context_tokens - prepped, 0)
        rt.STDERR.print(
            rt._ansi("2", f"| {rt.format_tokens(prepped)} used, ~{rt.format_tokens(remaining)} remaining")
        )

    _set_terminal_title("")
    return 0

def run(*prompt: str):
    task = (
        " ".join(prompt)
        if prompt
        else (sys.stdin.read().strip() if not sys.stdin.isatty() else "")
    )
    if not task:
        return chat()

    session = _resolve_session()
    _print_session_intro("Run", session, prompt=rt.preview(task, 100))
    return asyncio.run(
        run_agent(
            task,
            session.model,
            session.workspace,
            session.system_prompt,
            rt.DEFAULT_UNATTENDED_TIMEOUT_SECONDS,
            session.interactive,
        )
    )[0]

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
    if not sys.stdin.isatty():
        if model_id:
            matches = [model for model in available if model_id.strip().lower() in model.lower()]
            if matches:
                render_model_list(
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
        render_model_list(available, title="## Available Models", current=current, err=True)
    shown = available
    query = model_id or rt.Prompt.ask("Model or filter", console=rt.STDERR, default=current).strip()
    while True:
        query = query.strip() or current
        if query in available:
            return query
        if query.isdigit() and 1 <= (index := int(query)) <= len(shown):
            return shown[index - 1]
        shown = [model for model in available if query.lower() in model.lower()]
        render_model_list(shown, title="## Matching Models", query=query, current=current, err=True)
        query = rt.Prompt.ask("Model or filter", console=rt.STDERR).strip()

def model(query: str | None = None):
    current = rt._model(None)
    if query is None and not sys.stdin.isatty():
        rt._print(value=_current_model_text(current))
        return 0
    if current:
        rt._print(value=_current_model_text(current), err=True)
        if (
            rt.Prompt.ask(
                "\nPick a new model?", console=rt.STDERR, choices=["y", "n"], default="n"
            )
            != "y"
        ):
            return 0
    chosen = resolve_model_choice(query)
    if chosen is None:
        return 1
    shim, bare_model = rt.split_model_spec(chosen)
    cfg = {**rt._load_cfg(), "model": bare_model}
    (cfg.__setitem__("shim", shim) if shim else cfg.pop("shim", None))
    _save_cfg(cfg)
    rt._print(
        value=f"## Default Model Updated\n\n- selected: {rt._fmt('inline', chosen)}"
        + (f"\n- shim: {rt._fmt('inline', shim)}" if shim else "")
    )
    return 0

def main(argv: list[str] | None = None):
    args = list(sys.argv[1:] if argv is None else argv)
    commands = {"run", "chat", "model", "audit", "-h", "--help"}
    if not args:
        args = ["run"] if not sys.stdin.isatty() else ["--help"]
    elif args[0] in {"-v", "--version"}:
        rt._print(value=f"oy {rt.__version__}")
        return 0
    elif args[0] not in commands:
        args = ["run", *args]
    result = defopt.run([run, chat, model, audit], argv=args, version=False, short={})
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
    "_pick_model",
    "_print_session_intro",
    "_read_input",
    "_resolve_session",
    "_set_terminal_title",
    "_workspace",
    "audit",
    "chat",
    "main",
    "model",
    "read_system_prompt",
    "render_model_list",
    "resolve_model_choice",
    "run",
]
