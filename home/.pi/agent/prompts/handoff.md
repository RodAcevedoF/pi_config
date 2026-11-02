---
description: Write the continuation prompt for a new session to odd/handoff/<slug>.md
argument-hint: "[focus for the next session]"
---
I want to continue in a new session. Follow the "Session handoff" rule in ~/.pi/agent/AGENTS.md now: write the continuation prompt to `odd/handoff/<slug>.md` at the project root, then reply with the prompt and its path. If no work is in progress, say so and do not write the file.

Focus for the next session, also used to name the slug when given: ${@:-whatever is pending}
