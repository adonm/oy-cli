# Audit Issues

This report was reset after the OpenCode/MCP refactor.

The previous findings targeted the deleted standalone AI CLI stack. Run the current OpenCode workflow to generate a fresh audit:

```bash
oy audit
```

The generated `oy-auditor` agent should use `oy_repo_manifest` and `oy_repo_chunks`, then call `oy_render_audit_report` to replace this file with current findings.
