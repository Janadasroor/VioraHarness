# Task queue for `vioraharness run-loop` (overnight autonomous runs)

One focused session of agent work per file. The runner gives each task a
**fresh session** (full context reset — this is what prevents context rot),
runs its validation commands, commits **only** in-scope files on green,
and quarantines failures for human review.

```bash
# supervised first run (1 task), then the overnight run:
vioraharness run-loop --max-tasks 1 -y
vioraharness run-loop -y
```

## Layout

- `pending/<name>.md` — queued tasks, processed alphabetically
- `done/<name>.md` — completed (commit hash printed in the summary)
- `failed/<name>.md` + `failed/<name>.log` — quarantined with the error log

## Format

```markdown
---
title: Short human title
files:
  - path/relative/to/repo.rs
  - other/dir/
validate:
  - "cargo test -p mycrate"
retries: 2
---
Goal and acceptance criteria. Scope to ~30-60 minutes of agent work.
```

- `title` defaults to the filename; `retries` defaults to 1 (one retry).
- `files` empty = whole tree may change (prefer explicit scopes).
- `validate` empty = accept on agent summary (prefer real gates).
- Body must not be empty: goal + "done when".

## Runbook (enforced by the runner, told to every agent)

1. One task only, inside its files. No scope expansion.
2. The agent never commits — the runner commits on green.
3. The agent appends durable decisions to `DECISIONS.md`.
4. CLI flags: `--queue DIR` (default `tasks`), `--model`, `--max-tasks N`,
   `--token-budget N` (estimated output tokens, stops before next task),
   `-y/--yes` required (also `VIORAHARNESS_AUTO_ALLOW=1`).
5. Exit code is nonzero if any task failed — `tasks/failed/` tells you why.

## Morning review

`git log` (one atomic commit per task), `tasks/done/`, `DECISIONS.md`
diff, and `tasks/failed/*.log`. That review — not babysitting — is the job.
