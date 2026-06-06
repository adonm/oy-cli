# Code Quality Review

## Machine-readable findings

```json oy-findings
[
  {
    "body": "This produces a response shaped like `{ \"result\": { \"error\": ... } }` instead of a JSON-RPC top-level `error`, so clients can treat unsupported methods as successful calls. Return an error from the match arm, introduce a response enum, or have the dispatcher write prebuilt JSON-RPC error responses without wrapping them in `result`. Add a protocol-level test for an unknown method.",
    "evidence": "`handle_request` returns `Ok(jsonrpc_error(...))` for unknown methods at src/mcp.rs:62, while the caller at src/mcp.rs:43 wraps every `Ok` value in a top-level `{ \"result\": ... }`.",
    "locations": [
      {
        "line": 52,
        "path": "src/mcp.rs"
      },
      {
        "line": 62,
        "path": "src/mcp.rs"
      },
      {
        "line": 43,
        "path": "src/mcp.rs"
      }
    ],
    "severity": "Medium",
    "title": "Unknown MCP methods are returned as successful results instead of JSON-RPC errors"
  },
  {
    "body": "`std::fs::rename` does not reliably replace an existing destination on Windows. If a later write in the batch fails, rollback may fail to restore a backed-up file, and the error is discarded, leaving the workspace in a partially modified state despite the atomicity guarantee. Replace the destination before renaming (or use a cross-platform replace helper) and surface/log rollback failures; add a Windows-oriented or mocked failure test for multi-file rollback.",
    "evidence": "`restore_workspace_backups` uses `fs::rename(&backup, &committed_write.path)` over an existing destination and ignores the result at src/cli/config/atomic_write.rs:75.",
    "locations": [
      {
        "line": 72,
        "path": "src/cli/config/atomic_write.rs"
      },
      {
        "line": 75,
        "path": "src/cli/config/atomic_write.rs"
      },
      {
        "line": 77,
        "path": "src/cli/config/atomic_write.rs"
      }
    ],
    "severity": "Medium",
    "title": "Rollback can silently leave modified files in place on Windows"
  }
]
```
