# LLM Evaluation

`oy` has two different quality bars:

1. Deterministic Rust/CI tests for the code `oy` owns: evidence preparation,
   path safety, repository chunking, report rendering, and package/setup behavior.
2. Live model evaluations for the behavior opencode owns: whether the `oy`
   agent and skills find useful findings, avoid noise, and follow the audit/review
   protocol with a real model.

Do not mix them. CI should stay deterministic and provider-free. Prompt changes
should be judged with pinned public repositories, fixed model/provider settings,
and a before/after scorecard.

## Capability Inventory

Current generated capabilities:

| Surface | Owner | Evaluation posture |
|---|---|---|
| Primary agent: `oy` | concise autonomous prompt under user-managed OpenCode permissions | Compare with tagged OpenCode 2 Build; score completion, verification, worktree safety, concision |
| `oy-audit`, `oy-review`, `oy-enhance` skills | Canonical generated workflow protocols | Live corpus plus adapter/source-drift tests |
| `oy-audit` execution | Current `oy` agent plus file-backed prepare/finalize | Live audit corpus; verify complete reads, finalization, and report shape |
| `oy-review` execution | Current `oy` agent plus file-backed prepare/finalize | Live diff and whole-workspace review corpus |
| `oy-enhance` execution | Current `oy` agent under the user's effective permissions | Disposable repos only; verify tests after one finding |

Normal live runs must exercise the published file protocol: `prepare` writes an
index and bounded chunks, OpenCode reads the index, previous report when present,
and every indexed chunk. Candidate Markdown and findings JSON are written separately,
and `finalize` verifies and normalizes the report.

## Evaluation Corpus

Use small, pinned, public open-source projects rather than only self-reviewing
this repo. Keep clones and model outputs under `.tmp/eval/` so they never enter
the release artifact or git history.

The tracked seed corpus is [`docs/eval-corpus.toml`](eval-corpus.toml). It was
seeded from recent public GitHub activity for `adonm`: Rust/DataFusion/geospatial
work in `apache/sedona-db`, `tomtom215/quack-rs`,
`datafusion-contrib/datafusion-ducklake`, and `adonm/zuko`.

Start with three lanes:

| Lane | Purpose | Good candidates |
|---|---|---|
| Recall canaries | Check that audits find known bug classes | `OWASP/NodeGoat`, `juice-shop/juice-shop`, small historical vulnerable tags |
| Regression diffs | Check that reviews understand a real change | Security/bug-fix commits from small projects; review `base..fix` and `fix..base` where useful |
| Precision baselines | Check that reports stay sparse on mature code | `BurntSushi/ripgrep`, `sharkdp/bat`, `pallets/flask`, `expressjs/express` |

Prefer tasks that fit within `--max-chunks 80` at the fixed bound-workflow chunk target.
For larger projects, evaluate a documented path focus instead of raising chunk
budgets until the task becomes impossible to compare.

For each task, record a rubric, not an exact output snapshot:

- repository URL, license, pinned commit/tag, and checked-out path
- command (`audit`, `review`, or `enhance`) and focus text
- expected issue classes, affected files/symbols, and unacceptable false-positive
  categories
- required report shape: transparency line, valid `oy-findings` JSON, path/line
  evidence where available, and a clear no-findings verdict when appropriate
- model/provider, opencode version, oy commit, date, elapsed time, and chunk count

## Local Runner

Use the local runner for repeatability. `just eval` validates the corpus only; it
does not clone repos or call a model.

```bash
just eval
python3 scripts/eval_runner.py list
python3 scripts/eval_runner.py run --dry-run
python3 scripts/eval_runner.py run --opencode-model openai/gpt-5.5 \
  --model-slug openai-gpt-5.5 \
  --task sedona-geoparquet-aws-allowlist-review
python3 scripts/eval_runner.py compare \
  .tmp/eval/runs/<baseline>/summary.json \
  .tmp/eval/runs/<candidate>/summary.json
```

The runner:

- reads `docs/eval-corpus.toml`
- clones/fetches pinned public refs under `.tmp/eval/repos/`
- builds the local `oy` binary unless `--skip-build` is passed
- prepends `target/debug` to `PATH` so the packaged skills invoke the candidate
  binary for `prepare` and `finalize`
- runs `oy setup --workspace`, then the configured `oy audit` or `oy review`
  through oy's managed OpenCode workflow
- maps `--opencode-model provider/model#variant` to `OY_OPENCODE_MODEL` for
  the oy workflow instead of bypassing oy with a host command
- copies reports and writes `summary.json`/`summary.md` under `.tmp/eval/runs/`
- checks report shape, `oy-findings` JSON, keyword/path scorecard hints, and
  unexpected source mutations outside `.opencode/`, `.oy/runs/`, and `.oy-eval/`

Full runs require opencode and a configured model provider. They are deliberately
not part of default CI.

## Manual Run Protocol

Evaluate the candidate `oy` binary that contains the prompt changes. Packaged
skills resolve `oy audit|review prepare/finalize` from `PATH`, so put the local
build first or install the candidate binary before running evals.

```bash
cargo build --locked
export PATH="$PWD/target/debug:$PATH"
export OY_OPENCODE_MODEL="provider/model#variant" # optional; omit #variant if unused
RUN_ID="$(date -u +%Y%m%dT%H%M%SZ)-<model-slug>"
mkdir -p .tmp/eval/repos .tmp/eval/runs/"$RUN_ID"

git clone --depth 1 --branch <tag-or-branch> \
  https://github.com/<owner>/<repo>.git \
  .tmp/eval/repos/<repo>

# For diff reviews, also fetch the target ref/SHA with enough history for
# `git diff <target-ref>` to work inside the clone.

(
  cd .tmp/eval/repos/<repo>
  OY_ROOT="$PWD" oy setup --workspace
  OY_ROOT="$PWD" oy audit --out ".oy-eval/$RUN_ID/ISSUES.md" \
    --max-chunks 80 "<focus>"
  # Omit the positional target for whole-repo precision reviews.
  OY_ROOT="$PWD" oy review <target-ref> \
    --out ".oy-eval/$RUN_ID/REVIEW.md" \
    --max-chunks 80 --focus "<focus>"
)

mkdir -p .tmp/eval/runs/"$RUN_ID"/<repo>
cp .tmp/eval/repos/<repo>/.oy-eval/"$RUN_ID"/*.md \
  .tmp/eval/runs/"$RUN_ID"/<repo>/
```

If the repo needs dependencies or tests for an `oy enhance` pass, install and run
them only inside the clone. Do not point `OY_ROOT` at a parent directory that
contains secrets or unrelated projects.

## Scorecard

Score before and after a prompt change with the same model, opencode version,
commands, focus, and refs.

| Metric | Pass signal | Fail signal |
|---|---|---|
| Protocol | Exactly one generated report, valid structure, hash-verified evidence, validated candidates, and observed reads of every indexed chunk | Missing report, malformed `oy-findings`, changed evidence, skipped indexed chunks, stale carry-forward |
| Recall | Expected bug class or design issue is found with concrete evidence | Known issue missed or described without an affected path/symbol |
| Precision | Findings are few, specific, and defensible | Generic advice, speculative vulnerabilities, duplicate findings |
| Actionability | Fix is local, testable, and removes the bug class | Vague remediation or framework churn without evidence |
| Cost/latency | Similar or lower chunks/time than baseline | Prompt bloat increases time/cost without better findings |
| Safety | Audit/review write only `.oy/runs/` artifacts and the report; enhancer changes one finding and verifies | Unexpected repo mutation or broad tool use |

Use a simple verdict per task: `better`, `same`, `worse`, or `inconclusive`.
Accept prompt changes only when they improve at least one target lane without a
material regression in the others.

## Prompt Iteration Rules

1. Make one prompt change at a time.
2. Run the old and new prompts on the same pinned corpus.
3. Prefer shorter prompts unless longer text measurably improves the scorecard.
4. Preserve skill/file-protocol structure and Rust safety constraints in tests.
5. Put the scorecard summary in the PR or release notes; keep raw eval artifacts
   under `.tmp/eval/`.
6. Do not exact-match model prose. Match behavior: evidence, report schema,
   finding quality, and false-positive rate.

## Planned work

[`ROADMAP.md`](https://github.com/adonm/oy-cli/blob/main/ROADMAP.md) is the canonical backlog for deterministic fixtures,
corpus expansion, report examples, and workflow automation. Keep future work there
so evaluation guidance describes the current method rather than maintaining a
second roadmap.
