# Code Quality Review

This report was reset after the OpenCode/MCP refactor.

The previous findings targeted the deleted native LLM/UI/tool stack. Run the current OpenCode workflow to generate a fresh review:

```bash
oy review
```

The generated `oy-reviewer` agent should use `oy_repo_manifest`, `oy_repo_chunks` or `oy_git_diff_input`, then call `oy_render_review_report` to replace this file with current findings.
