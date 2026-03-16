# Audit Findings

> Last audit: 2025-06-18 (OWASP ASVS 5.0.0 / MSVS)

## Summary

Total issues found: 5

| Severity | Open | Resolved |
|----------|------|----------|
| High     | 1    | 0        |
| Medium   | 2    | 0        |
| Low      | 0    | 2        |

## High Severity (1)

### 1. Missing Unit Tests and Test Coverage

- **Location**: Project root
- **Category**: security, quality
- **Standard**: ASVS V8: Secure Software Lifecycle

The project has no unit tests or test suite. This is critical for a security-sensitive tool that:
- Executes arbitrary shell commands
- Modifies user files
- Handles authentication credentials
- Parses untrusted input from LLMs

Without tests, there's no way to verify:
- Path traversal protections work correctly
- Input validation is comprehensive
- Error handling behaves as expected
- Security patches don't introduce regressions

**Recommendation**: 
1. Add a test suite using pytest
2. Achieve minimum 80% code coverage
3. Include specific security tests for:
   - Path traversal prevention (resolve_path)
   - Command injection scenarios
   - Input validation edge cases
   - Error handling paths
4. Add tests to CI/CD pipeline (see missing CI configuration below)

**Status**: OPEN - Critical for security assurance

---

## Medium Severity (2)

### 1. Missing CI/CD Pipeline for Automated Security Checks

- **Location**: `.github/workflows/`
- **Category**: security, quality
- **Standard**: ASVS V8: Secure Deployment

The project has a release workflow but lacks PR/CI workflows for:
- Automated testing on pull requests
- Automated linting and formatting checks
- Automated security scanning
- Build verification before merge
- Dependency vulnerability scanning

Current workflow (release.yml) only runs on release publication.

**Recommendation**:
1. Add GitHub Actions workflow for CI:
   - Run ruff lint and format checks
   - Run tests (once added)
   - Build verification
   - Dependency vulnerability scanning (pip-audit, dependabot)
2. Require all checks to pass before merging PRs
3. Add branch protection rules

**Status**: OPEN - Recommended for secure development lifecycle

---

### 2. Missing Pre-commit Hooks

- **Location**: Project configuration
- **Category**: security, quality
- **Standard**: ASVS V8: Secure Development Practices

No pre-commit hooks are configured to catch common issues before commit:
- Syntax errors
- Formatting violations
- Basic linting issues
- Secret/key detection

**Recommendation**:
1. Add `.pre-commit-config.yaml` with hooks for:
   - ruff (linting and formatting)
   - check-yaml
   - check-json
   - detect-secrets or gitleaks
2. Document setup in CONTRIBUTING.md

**Status**: OPEN - Recommended for development hygiene

---

## Low Severity (2)

### 1. Hardcoded Default Model Without Validation

- **Location**: `oy_cli.py` (model selection)
- **Category**: security, usability
- **Standard**: ASVS V5: Configuration

The `DEFAULT_MODEL` was hardcoded to `'moonshotai.kimi-k2.5'` without validation that this model exists or is appropriate for the endpoint.

**Recommendation**: 
1. Validate model availability on startup or first use
2. Provide clear error messages when default model is unavailable
3. Consider making default model configurable per installation

**Status**: RESOLVED - Removed hardcoded default. `oy` now prompts users to pick and save a model on first run, or aborts with instructions in non-interactive mode.

---

### 2. Hardcoded Timeouts and Limits Could Cause Production Issues

- **Location**: `oy_cli.py` and `shim.py` (various constants)
- **Category**: complexity, operations
- **Standard**: ASVS V8: Resource Management

Several operational limits are hardcoded as constants:
- MAX_TOOL_OUTPUT_TOKENS = 4096 (configurable via `OY_MAX_TOOL_OUTPUT_TOKENS`)
- DEFAULT_MAX_STEPS = 512 (configurable via `OY_DEFAULT_MAX_STEPS`)
- DEFAULT_MAX_TOOL_CALLS = 512 (configurable via `OY_DEFAULT_MAX_TOOL_CALLS`)
- Default bash timeout = 120 seconds
- Default httpx timeout = 20 seconds

These cannot be tuned for different environments or use cases without code changes.

**Recommendation**:
1. Make these configurable via environment variables
2. Document recommended values for different scenarios
3. Add config file support for operational parameters

**Status**: RESOLVED - All operational parameters are now configurable via `OY_*` environment variables and documented in README

---

## Security Strengths

The codebase demonstrates several good security practices:

1. **Path Traversal Protection**: `resolve_path()` implements proper path resolution with explicit ValueError on traversal attempts

2. **Header Redaction**: `_redact_header()` properly redacts sensitive headers (Authorization, Cookie, etc.) in httpx output

3. **No Dangerous Patterns**: No use of eval(), exec(), pickle, marshal, or other high-risk patterns

4. **Subprocess Safety**: Uses `subprocess.run()` with explicit argument lists, not `shell=True` (bash tool correctly uses `-c` which is intended design)

5. **Error Recovery**: Non-interactive mode includes documented error recovery guidance for resilience

6. **Credential Handling**: AWS credentials are obtained via official AWS CLI, not stored in files

7. **Small Attack Surface**: ~4000 lines across two modules of straightforward code is auditable

8. **Proper Exception Handling**: Exception handlers use Python 3.14 PEP 758 bare-comma syntax (`except A, B:`) which ruff formats automatically; both bare and parenthesised forms are valid

---

## Recommendations Summary

**High Priority**:
1. Add unit tests with security-focused test cases

**Medium Priority**:
1. Add CI workflow for PR validation
2. Configure pre-commit hooks

**Low Priority**:
1. ~~Make operational parameters configurable~~ RESOLVED
2. ~~Add model validation with clear error messages~~ RESOLVED
