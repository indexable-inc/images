---
description: Open relevant file in Cursor based on natural language query
argument-hint: <query>
model: haiku
---

# Open in Cursor

Find and open the most relevant file/location in Cursor based on the user's query.

## Query

$ARGUMENTS

## Instructions

1. **Search the codebase** to find files relevant to the query:
   - Use Glob and Grep to find matching files
   - Look for function names, class names, variable names, comments that match
   - Consider file names and paths

2. **Identify the most relevant location**:
   - Find the specific file and line number that best matches the query
   - If multiple matches, pick the most relevant one (definition over usage, main implementation over tests)

3. **Open in Cursor** using the goto syntax:
   ```bash
   cursor --goto <file>:<line>:<column>
   ```
   - Line and column are 1-indexed
   - Column can be 1 if not specifically relevant

4. **Report** what you opened and why it matched the query

## Examples

- Query: "where we handle auth" → finds auth handler, opens `src/auth/handler.ts:42:1`
- Query: "database connection" → finds db setup, opens `src/db/connect.rs:15:1`
- Query: "main function" → finds entry point, opens `src/main.rs:1:1`
