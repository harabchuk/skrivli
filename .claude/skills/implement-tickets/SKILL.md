---
name: implement-tickets
description: Implement open GitHub issues for this repo (Skrivli) that are labelled "ready for dev". Picks them one at a time in issue-number order, creates an Orca-managed worktree for each and hands it to a subagent working there, which implements the ticket from its goal and the agent-written implementation plan (test first, per AGENTS.md), then opens a pull request and swaps the issue label "ready for dev" for "review". Use when the user asks to implement, build, work on, or ship ready GitHub tickets/issues.
---

# Implement GitHub tickets

You are the **orchestrator**. You pick tickets, brief subagents, check their result, open the PR, and relabel. You do **not** edit code yourself. Each ticket is implemented by **one subagent** in **its own Orca worktree** (created with the `orca` CLI so it shows up in the Orca app, linked to the issue). Never use raw `git worktree` or the Agent tool's `isolation: "worktree"` for tickets. Work through tickets **strictly one at a time**: finish ticket N (PR + label) before starting ticket N+1.

Tickets reach `ready for dev` through the `clarify-tickets` skill, which posts a comment headed `## Implementation plan from agent`. The issue body (and human comments) is the **goal**; that comment is the **plan**.

## 1. Find eligible issues

An issue is eligible when it is **open**, has the `ready for dev` label, and does **not** have `review` or `waiting for human`.

```bash
gh issue list --state open --label "ready for dev" --limit 200 --json number,title,labels \
  --jq '[.[] | select([.labels[].name] | all(. != "review" and . != "waiting for human"))]
        | sort_by(.number) | .[] | "\(.number)\t\(.title)"'
```

If nothing is eligible, say so and stop. If the user named specific issue numbers, process only those (in the order given) and skip any that fail the label rule, saying why.

Before the first ticket, check that `main` is up to date and that no open PR already covers the issue:

```bash
git fetch origin
gh pr list --state open --search "<N> in:body" --json number,title,headRefName
```

If an open PR already references the issue, skip it (`skipped: PR #… already open`).

Also check that the Orca CLI works and that no Orca worktree already exists for the issue:

```bash
orca worktree current --json            # must succeed; gives the main checkout's repo id
orca worktree list --json               # look for a worktree whose linked issue is <N> or branch is agent/issue-<N>-*
```

Use `$ORCA_CLI_COMMAND` instead of `orca` if that variable is set. If Orca reports it is not running, run `orca open --json` and retry. If the CLI is missing or still fails, stop and report the exact error; do not fall back to raw git worktrees. If an Orca worktree for the issue already exists, skip the ticket (`skipped: Orca worktree … already exists`) rather than reusing or deleting it.

## 2. For each issue, in order

### 2a. Read the ticket and find the plan

```bash
gh issue view <N> --json number,title,body,labels,comments,url
```

Read the body and **every** comment. Take the **latest** comment whose body starts with `## Implementation plan from agent` as the plan. Human comments posted after it, and earlier `Question from agent:` threads with their answers, are part of the requirements and override the plan where they conflict.

If there is no plan comment, do not guess: skip the ticket (`skipped: no implementation plan`) and suggest running `clarify-tickets` on it.

Write the brief for the subagent to `<scratchpad>/issue-<N>-brief.md`: issue title and URL, the full body, the plan comment verbatim, and any later human comments verbatim. Do not summarise the plan away; the subagent needs it exactly.

### 2b. Create the Orca worktree

Pick a short kebab-case slug from the title. Create an independent worktree on a fresh branch from the latest `origin/main`, linked to the issue, without starting an Orca agent in it:

```bash
orca worktree create \
  --name "issue-<N>-<slug>" \
  --branch "agent/issue-<N>-<slug>" \
  --base-branch origin/main \
  --no-parent \
  --issue <N> \
  --comment "implementing #<N>" \
  --json
```

From the JSON `result.worktree`, record:

- `id` (the full `<repoId>::<path>` value): the Orca selector, used later as `--worktree "id:<id>"`.
- `path`: the absolute checkout path the subagent works in.
- `branch`: confirm it is `refs/heads/agent/issue-<N>-<slug>`. If Orca added a numeric suffix because the name was taken, use the branch it reports everywhere below.

Then mark it in progress:

```bash
orca worktree set --worktree "id:<id>" --workspace-status in-progress --json
```

If `worktree create` fails, stop and report the error (see Guardrails). Do not pass `--agent`: the implementation runs as your own subagent so its report comes back to you.

### 2c. Launch one subagent in that worktree

Call the **Agent** tool with:

- `subagent_type`: `general-purpose`
- no `isolation` (the Orca worktree is the isolation; never let a subagent work in the main checkout)
- `description`: `Implement issue #<N>`
- `prompt`: the template in section 3, filled in with the worktree path and branch, with the brief contents pasted inline.

Then **wait** for its completion notification. Do not start the next ticket, and do not poll or predict its result. One subagent per ticket; if you need to follow up with it (for example it stopped early on something you can answer from the ticket), use **SendMessage** to the same agent rather than spawning a new one.

### 2d. Check the subagent's report

The subagent must end with the report block from section 3. Treat it as a claim and verify it:

```bash
git fetch origin
git log --oneline origin/main..origin/<branch>          # commits exist and are pushed
git diff --stat origin/main...origin/<branch>           # files match the plan, specs touched if needed
```

Proceed to the PR only when **all** hold:

- `STATUS: done`, the branch is pushed, and it has at least one commit ahead of `origin/main`.
- The report shows `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` passing.
- The diff touches what the plan says (including the `.specs/` updates the plan lists). No unrelated files, no `.claude/` files, no build output.

If the subagent reports `STATUS: blocked`, or verification fails and one SendMessage round does not fix it, go to section 2g.

### 2e. Open the pull request

Write the body to `<scratchpad>/issue-<N>-pr.md`, then:

```bash
gh pr create --base main --head <branch> \
  --title "<concise imperative title> (#<N>)" \
  --body-file <scratchpad>/issue-<N>-pr.md
```

PR body format:

```markdown
Closes #<N>

## Summary
One or two sentences: what changes for the user and why.

## Changes
- `src/...`: what changed and why (one bullet per meaningful change).
- `.specs/...`: what was updated.

## Deviations from the plan
None. / What differed from the plan comment and why.

## Verification
- `cargo fmt --check` ✅
- `cargo clippy --all-targets -- -D warnings` ✅
- `cargo test` ✅ (new tests: `test_name`, ...)
- Manual check still needed: ... (from the plan's "How to verify", if any).

🤖 Generated with [Claude Code](https://claude.com/claude-code)
```

### 2f. Relabel the issue and update the Orca worktree

Only after `gh pr create` has printed the PR URL:

```bash
gh issue edit <N> --remove-label "ready for dev" --add-label "review"
orca worktree set --worktree "id:<id>" --pr <PR number> --workspace-status in-review \
  --comment "PR #<PR number> open for #<N>" --json
```

A failed `orca worktree set` is not a reason to stop; mention it in the final report. Leave the worktree in place (the human reviews from it); never `orca worktree rm` it. Then move on to the next ticket.

### 2g. When a ticket cannot be finished

Do **not** open a PR for partial or failing work. Post one comment on the issue (from `<scratchpad>/issue-<N>-blocked.md`) and swap the label so the ticket goes back to the human:

```markdown
Question from agent:

I tried to implement this from the plan above but stopped because ...

1. **<Topic>.** What I found, the options, and _my suggestion_.

Work so far is on branch `<branch>` (not merged, no PR). Once answered, set the label back to `ready for dev`.
```

```bash
gh issue comment <N> --body-file <scratchpad>/issue-<N>-blocked.md
gh issue edit <N> --remove-label "ready for dev" --add-label "waiting for human"
orca worktree set --worktree "id:<id>" --workspace-status todo --unread \
  --comment "blocked: waiting for human on #<N>" --json
```

Push the partial branch only if it has useful work; otherwise say "no branch pushed". Keep the Orca worktree so work can resume there. Then continue with the next ticket.

## 3. Subagent prompt template

Fill in `<…>` and paste the brief inline. Keep the rules section verbatim.

````markdown
You are implementing GitHub issue #<N> for Skrivli, a Windows desktop app in Rust
(repo: <owner/repo>). Your workspace is an Orca-managed git worktree at:

    <worktree path>

on branch `<branch>`, already created from the latest `origin/main`. Do all work there
and never edit files or run git/cargo in the main checkout (<main checkout path>).

## Ticket (goal and plan)

<contents of issue-<N>-brief.md>

## Setup

1. Your shell does not start in the worktree. Before any git or cargo command, change into
   it (PowerShell: `Set-Location '<worktree path>'`) and confirm with
   `git rev-parse --show-toplevel` and `git branch --show-current` that you are in
   `<worktree path>` on `<branch>`. Use absolute paths under `<worktree path>` for every
   Read, Edit, Write, Glob and Grep call. If either check fails, stop and report blocked.
2. Read `AGENTS.md` at the worktree root and follow it exactly.
3. Do not create another branch or worktree, and do not run `orca worktree` commands;
   the orchestrator owns the Orca workspace.
4. To avoid rebuilding whisper.cpp and all dependencies from scratch, build with the
   main checkout's target dir: set `CARGO_TARGET_DIR=<absolute path of main checkout>/target`
   for every cargo command.

## Rules

- Implement the plan step by step. Human comments after the plan override it.
  Small deviations are fine when the code demands it; record each one with the reason.
- Tests first (red, green, refactor) with Arrange / Act / Assert comments, as AGENTS.md says.
  Run each new test and see it fail for the right reason before writing the code.
- Respect the layering rules: no OS calls outside `infrastructure/platform/`, nothing
  blocking on the GPUI thread, no `unwrap`/`expect` outside tests.
- Update `.specs/` as the plan and the AGENTS.md table require, in the same branch.
- Stay in scope. Do not refactor unrelated code or fix unrelated issues; mention them
  in the report instead.
- Before finishing, all three must pass:
  `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.
  Default `cargo test` must not need hardware.
- Commit in small logical commits with clear messages. End every commit message with
  a blank line and then:
  `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`
  Never commit `.claude/`, `target/`, or other build output. Do not use `--no-verify`.
- Push the branch: `git push -u origin <branch>`.
- Do NOT open a PR, comment on the issue, or change labels. The orchestrator does that.
- If the plan is wrong, contradicts AGENTS.md or `.specs/`, or needs a product decision
  you cannot make from the ticket, stop and report `STATUS: blocked` with specific
  questions. Do not guess on anything user-visible.

## Final report (your last message, exactly this shape)

```
STATUS: done | blocked
BRANCH: <branch> (pushed: yes/no)
COMMITS: <hash> <subject> (one per line)
FILES: <path> — <one-line why> (one per line)
SPECS: <spec files updated, or "none needed: reason">
TESTS ADDED: <test names>
CHECKS: fmt=<pass/fail> clippy=<pass/fail> test=<pass/fail> (<n> passed)
DEVIATIONS: <none, or each deviation from the plan with the reason>
MANUAL CHECK: <what a human should verify in the running app, or "none">
QUESTIONS: <only if blocked: numbered questions with options and a suggestion>
NOTES: <unrelated problems noticed, follow-ups>
```
````

## 4. Finish

After all tickets are processed, report a short table: issue number, title, outcome (`PR #… opened` / `blocked: reason` / `skipped: reason`), and the PR or comment URL, plus the Orca worktree name for each. Mention any manual checks the PRs still need.

## Guardrails

- Strictly sequential: one ticket, one subagent, one Orca worktree at a time.
- Worktrees come only from `orca worktree create`. Never remove an Orca worktree; that is the human's call after review.
- The orchestrator never edits code, never merges, never pushes to `main`, never force-pushes, and never closes issues.
- Labels change only in two ways: `ready for dev` → `review` after a PR exists, or `ready for dev` → `waiting for human` when blocked.
- If `gh` or `git push` fails (auth, network, missing label, branch exists), stop and report the error instead of working around it.
- Never put secrets, local absolute paths, or transcript/audio content into PRs, comments, or commits.
