---
name: unknowns
description: "Run a blind spot pass before starting unfamiliar work; surface the user's unknown unknowns (architecture-changing decisions, what good looks like, prior art, potholes, vocabulary) and end with a rewritten prompt. Use when the user says blind spot pass, unknown unknowns, find my unknowns, what am I missing, or starts work in a domain or codebase area they say they don't know."
---

# unknowns

Map the gap between what the user asked for and what the territory requires, before work begins.

1. **Anchor on the user's starting point.** Infer from context (or ask once, briefly) what they already know, their experience with this domain and codebase area, and the outcome they care about. The pass is calibrated to them, not to a generic reader.
2. **Sweep the territory.** In parallel where possible: the relevant code paths and their history (git log, prior PRs and issues), existing conventions and prior art in the repo, and external references when the domain itself is unfamiliar. Hunt specifically for what would surprise the user given their stated starting point.
3. **Report by category**, shortest useful form:
   - **Decisions that change the architecture.** Choices where the user's answer redirects the work; frame each as a question with a recommended default.
   - **What good looks like.** The quality bar, reference implementations, or examples the user can react to.
   - **Prior art and potholes.** Historical attempts, known failure modes, guards, and constraints not visible from the entry point.
   - **Vocabulary.** Terms the user needs in order to prompt precisely.
4. **End with a rewritten prompt.** Produce the prompt the user should have written, with the resolved unknowns folded in, then offer to proceed with it directly.

Keep it a pass, not a project: minutes of scanning, one structured reply. Deep dives happen only after the user picks which unknown matters.
