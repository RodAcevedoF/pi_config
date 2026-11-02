---
name: session-handoff
description: Write a continuation prompt so a new session can resume work in progress. Trigger: the user wants to continue in a new session ("abrimos nueva sesión", "dame un prompt para la siguiente sesión", "handoff"), context size, /handoff.
---

# Session handoff

## When

- The user wants to continue in a new session and work is in progress. Do it without being asked for the format.
- No work in progress: say so and write nothing.

## Write

- Path: `odd/handoff/<slug>.md` at the project root. Sessions may share a checkout, so `<slug>` names the task and role (`login-fix`, `login-fix-review`); use the focus the user gave when there is one.
- A session owns the file it wrote or continued from: rewrite it on the next handoff. Ask before replacing a file you do not own.
- Never mirror handoffs to Engram.
- Self-contained: the next session has only this file, git and the code. Check every fact against git and the code before writing it.

## Template

```markdown
# Handoff: <task> (<role>)

- Date: <YYYY-MM-DD>
- Branch: <branch>
- Working directory: <absolute path>

## Done
- <what is finished, with commits or files>

## Pending
- <next steps, in order>

## Decisions
- <decision and why>

## How to verify
- <exact commands and expected results>

## Rules for the session that continues
- Check this file against git and the code before acting; they win over it.
- You own this file: rewrite it on your next handoff and delete it when the task is done.
- Never mirror it to Engram.
```

## Reply

Reply with the prompt and the line `Read odd/handoff/<slug>.md and continue.`
