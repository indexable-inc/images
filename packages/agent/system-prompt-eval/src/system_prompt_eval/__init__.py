"""Behavioral eval for the house system prompt.

Spawns fresh ``claude -p`` rollouts that load the committed system prompt the
same way a production session does, then an LLM judge scores whether each target
default behavior (reproduce-before-fix, first-principles root cause, experiment
by default, tie-work-to-an-issue, named subagents, report-to-playbook) emerged
on a neutral task. The score is committed under ``eval-results/`` so the prompt's
behavior is tracked over time.
"""
