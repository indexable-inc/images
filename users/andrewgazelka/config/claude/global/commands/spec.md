---
description: Interview user in depth and write a detailed spec to a file
argument-hint: <spec-file.md>
---

# Spec Interview & Writing

Read the file `$ARGUMENTS` if it exists (it may be empty or contain a draft), then conduct an in-depth interview with the user to flesh out a complete specification.

## Instructions

1. **Read the spec file** at `$ARGUMENTS` to understand any existing context or draft

2. **Deep codebase exploration** using the **Explore agent** (`Task` tool with `subagent_type: Explore`). This is CRITICAL - thorough exploration enables asking informed, codebase-specific questions. Explore:
   - Existing architecture and module structure relevant to the feature
   - Similar features or patterns already implemented (how does the codebase handle X?)
   - Data models, types, and state management patterns in use
   - Error handling conventions and patterns
   - Testing patterns and infrastructure
   - Integration points and boundaries between modules
   - Any existing code that would be modified or extended

   Use multiple Explore calls if needed. The goal is to understand the codebase deeply enough to ask questions like "I see you use X pattern in module Y - should we follow that here?" rather than generic questions.

3. **Conduct a codebase-informed interview** using AskUserQuestion. Ground your questions in what you discovered during exploration. Ask about:
   - Technical implementation details (architecture, data flow, state management, error handling, concurrency)
   - UI/UX considerations (user flows, edge cases, accessibility, responsiveness, loading states)
   - Concerns and constraints (performance budgets, security, privacy, backwards compatibility)
   - Tradeoffs (build vs buy, complexity vs maintainability, speed vs correctness)
   - Integration points (APIs, dependencies, external systems)
   - Failure modes (what happens when X fails? how do we recover?)
   - Testing strategy (unit, integration, e2e, property-based)
   - Deployment and rollout (feature flags, gradual rollout, rollback plan)

4. **Ask non-obvious, codebase-grounded questions**. Dig into:
   - Second-order effects: "This touches module X which also affects Y - how should we handle that?"
   - Existing patterns: "I noticed the codebase uses pattern A for similar features - deviate or follow?"
   - Assumptions that seem implicit in both the request AND the existing code
   - Things that could go wrong given the current architecture
   - How this interacts with existing systems you discovered during exploration

5. **Iterate between exploration and interviewing**. As the user answers questions:
   - Use Explore to dig deeper into areas they mention
   - Discover related code that informs follow-up questions
   - Validate assumptions by checking actual implementations
   - Continue until you have enough detail to write a comprehensive spec grounded in the actual codebase

6. **Write the final spec** to `$ARGUMENTS` with:
   - Overview and goals
   - Non-goals (what this explicitly does NOT do)
   - Technical design
   - Open questions (if any remain)
   - Rollout plan
   - What is still unimplemented/implemented/etc
   - Links to relevant code (use @ and reference file/folder)

Be relentless in your questioning AND your exploration. A good spec prevents bugs before they happen by grounding decisions in the actual codebase, not abstract requirements.
