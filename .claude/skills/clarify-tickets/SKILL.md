---
name: clarify-tickets
description: Triage open GitHub issues for this repo (Skrivli). Reads each issue labelled bug, improvement, or feature that is not yet labelled "waiting for human", "ready for dev", or "review"; checks it against the codebase and .specs; then posts either an implementation plan (and labels it "ready for dev") or questions prefixed "Question from agent:" (and labels it "waiting for human"). Use when the user asks to clarify, triage, groom, or plan GitHub tickets/issues.
---

# Clarify GitHub tickets

Work through the eligible issues **one at a time**. For each one, either post an implementation plan or post questions. Never both. Never edit code in this skill.

## 1. Find eligible issues

An issue is eligible when it is **open**, has **at least one** of `bug`, `improvement`, `feature`, and has **none** of `waiting for human`, `ready for dev`, `review`.

```bash
gh issue list --state open --limit 200 --json number,title,labels \
  --jq '[.[] | select(
      ([.labels[].name] | any(. == "bug" or . == "improvement" or . == "feature")) and
      ([.labels[].name] | all(. != "waiting for human" and . != "ready for dev" and . != "review"))
    )] | sort_by(.number) | .[] | "\(.number)\t\(.title)"'
```

If nothing is eligible, say so and stop. If the user named specific issue numbers, process only those, and still skip any that fail the label rule (say why).

## 2. For each issue, in order

### 2a. Read the whole ticket

```bash
gh issue view <N> --json number,title,body,labels,author,comments,url
```

Read the body **and every comment**. A ticket may have come back from `waiting for human`: earlier comments starting with `Question from agent:` are your previous questions, and later human comments are the answers. Treat answers as part of the requirements. Do not re-ask a question that has been answered. Look at any linked issues, PRs, or images mentioned.

### 2b. Check it against the codebase

Read `AGENTS.md` first if it is not already in context. Then:

- Find the code the ticket touches (Grep/Glob under `src/`, `tests/`). Read the relevant functions, not just file names.
- Read the relevant parts of `.specs/skrivli-product.md` (user-facing scope) and `.specs/skrivli-app-design.md` (architecture). Check `.specs/decisions.md` for constraints that apply.
- For a **bug**: locate the likely cause. Confirm the described behaviour matches what the code does. Note if it is already fixed on `main`.
- For a **feature/improvement**: check whether it fits product scope, whether part of it already exists, and which layers (`domain/`, `services/`, `infrastructure/`, `ui/`, `views/`) it needs to touch, respecting the layering rules in `AGENTS.md`.

### 2c. Decide: clear or unclear

The ticket is **clear** only when you can name the expected behaviour, the files to change, and how to test it, without guessing about something the user would care about.

It is **unclear** when any of these hold:

- Expected behaviour is ambiguous, or there are several reasonable interpretations with different user-visible results.
- It contradicts `.specs/skrivli-product.md`, `AGENTS.md` rules, or `.specs/decisions.md`.
- A bug cannot be matched to the code (cannot find the cause, or the code seems to already behave as asked) and repro steps are missing.
- It needs a product decision (UI wording, settings defaults, privacy, scope).

Small technical choices with an obvious default are not reasons to ask. Pick the default and state it in the plan.

## 3a. Clear → post an implementation plan

Write the plan to a temp file in the scratchpad (avoids shell-quoting problems), then post it and relabel:

```bash
gh issue comment <N> --body-file <scratchpad>/issue-<N>-plan.md
gh issue edit <N> --add-label "ready for dev"
```

Plan format (Markdown, written for a human reviewer; explain *why*, keep each action concrete):

```markdown
## Implementation plan from agent

**Summary.** One or two sentences: what changes for the user and why.

**What I found.** Short explanation of the current behaviour and where it lives,
with links like `src/views/record.rs` (`fn render_transcript`). For bugs, the root cause.

**Approach.** A short paragraph on the chosen approach and any default I picked
(and the alternative I rejected, if relevant).

### Steps
1. **Add** a failing test `wraps_long_transcript_lines` in `src/views/record.rs` (`mod tests`) that ...
2. **Change** `render_transcript` in `src/views/record.rs` to ... because ...
3. **Remove** the unused `TruncateMode` enum from `src/ui/components/...`.
4. **Update** `.specs/skrivli-product.md` → section "...": describe ...

### How to verify
- `cargo test <name>` passes; `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` are clean.
- Manual: run the app, ..., expect ....

### Out of scope
- Things deliberately not done here.
```

Rules for the steps:

- Start every step with a verb: **Add**, **Change**, **Remove**, **Move**, **Rename**, **Update**, **Test**.
- Name real files, types, and functions you verified exist. Mark new ones as new.
- Follow `AGENTS.md`: test first (Arrange/Act/Assert), OS calls only under `infrastructure/platform/`, no blocking work on the GPUI thread.
- Include the `.specs/` updates the change requires (see the table in `AGENTS.md`).

## 3b. Unclear → ask questions

Every question comment **must start with the exact line in bold** `Question from agent:`. Post one comment per ticket with all questions numbered, then relabel:

```bash
gh issue comment <N> --body-file <scratchpad>/issue-<N>-questions.md
gh issue edit <N> --add-label "waiting for human"
```

Question format:

```markdown
**Question from agent**:

I looked at `src/...` and `.specs/skrivli-product.md` (section "..."). Before planning I need:

1. **<Topic>.** Context in one sentence. Should it A or B?
   _My suggestion: A, because ..._
2. ...

Once answered, remove the `waiting for human` label and I will write the plan.
```

Make each question answerable in one line: offer options and your recommendation. Say what you already found, so the human does not repeat it.

## 4. Finish

After all issues are processed, report a short table to the user: issue number, title, outcome (`plan posted` / `questions posted` / `skipped: reason`), and the comment URL printed by `gh issue comment`.

## Guardrails

- Only comment and add labels. Do not close issues, remove labels, edit issue bodies, push code, or open PRs.
- Process sequentially; finish one issue (comment + label) before reading the next.
- If `gh` fails (auth, network, missing label), stop and report the error instead of guessing.
- Never paste secrets, local absolute paths, or transcript/audio content into comments.
