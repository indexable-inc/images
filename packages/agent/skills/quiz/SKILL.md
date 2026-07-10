---
name: quiz
description: "Verify the user actually understands a change before merging; build a short explainer (context, intuition, what was done) then quiz them interactively and grade the answers. Use when the user says quiz me, asks to check their understanding of a change, PR, or session, or wants a comprehension gate before merge."
---

# quiz

After a long working session the diff understates what happened, because behavior depends on existing code paths the diff never shows. Close that gap before merge.

1. **Scope the change.** Default to the current branch against its base (or this session's work); accept a PR number or path range instead.
2. **Brief first.** Before any question, give a compact explainer: the why, the shape of the solution, what was done, and the non-obvious interactions with existing code paths. Use an HTML artifact for large changes, inline prose otherwise.
3. **Quiz interactively.** 4-7 multiple-choice questions via AskUserQuestion, one at a time, each with one defensible correct answer. Aim at what the diff hides: behavior on existing paths, failure modes, why an approach beat its alternative, what breaks if a given line is removed. No trivia the diff states verbatim.
4. **Grade honestly.** After each answer, say correct or incorrect and why. At the end report the score and re-explain anything missed.
5. **Gate.** The bar is a perfect pass before merge; on misses, run a second targeted round on just those areas rather than repeating the whole quiz.
