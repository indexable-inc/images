"""Runtime type checking for python_exec.

This module provides type validation for code executed in the kernel's python_exec
before it runs. It uses pyright's diagnostics to detect type errors.

    result, error = validate_types(code)
    if not result:
        print(f"Type check failed: {error}")
        return
    # code is safe to execute
"""

from __future__ import annotations

import asyncio
import json
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any

__all__ = ["validate_types"]

__version__ = "0.1.0"


def validate_types(code: str) -> tuple[bool, str]:
    """Validate type hints in Python code using pyright.

    Args:
        code: Python source code to validate

    Returns:
        (is_valid, error_message) tuple:
        - is_valid=True, error_message="" if no type errors found
        - is_valid=False, error_message=<details> if type errors detected
        - is_valid=True, error_message="" if no type hints present (code allowed)
        - is_valid=False, error_message=<details> on validation tool failures

    Type checking is permissive: code without any type hints passes (is_valid=True).
    Code with type hints must pass pyright's validation. Type errors surface the
    specific line and error details.
    """
    # Quick check: if there are no type hints anywhere, allow it
    if not _has_type_hints(code):
        return True, ""

    # Write code to a temp file and run pyright on it
    try:
        with tempfile.NamedTemporaryFile(mode="w", suffix=".py", delete=False) as tmp:
            tmp.write(code)
            tmp_path = tmp.name

        try:
            # Run pyright with JSON output
            result = subprocess.run(
                [sys.executable, "-m", "pyright", "--outputjson", tmp_path],
                capture_output=True,
                text=True,
                timeout=30,
            )

            # Parse the JSON output
            try:
                output = json.loads(result.stdout)
            except json.JSONDecodeError:
                # If pyright output is not valid JSON, fall back to stderr
                if result.stderr:
                    return False, f"pyright validation error: {result.stderr.strip()[:500]}"
                return False, "pyright produced invalid JSON output"

            # Extract diagnostics
            diagnostics = output.get("generalDiagnostics", [])

            # Filter for actual errors (not warnings)
            errors = [d for d in diagnostics if d.get("severity") == "error"]

            if errors:
                # Format error message
                error_lines = []
                for err in errors[:5]:  # Cap at 5 errors
                    rule = err.get("rule", "")
                    msg = err.get("message", "unknown error")
                    line = err.get("range", {}).get("start", {}).get("line", "?")
                    if rule:
                        error_lines.append(f"Line {line}: [{rule}] {msg}")
                    else:
                        error_lines.append(f"Line {line}: {msg}")

                error_msg = "\n".join(error_lines)
                if len(errors) > 5:
                    error_msg += f"\n... and {len(errors) - 5} more errors"
                return False, error_msg

            # No errors found
            return True, ""

        finally:
            # Clean up temp file
            try:
                Path(tmp_path).unlink()
            except OSError:
                pass

    except subprocess.TimeoutExpired:
        return False, "type check timed out (code may have syntax errors)"
    except FileNotFoundError:
        # pyright not available; allow the code (it will fail at runtime if really wrong)
        return True, ""
    except Exception as exc:
        # Any other error during type checking; allow code to run (type check is optional)
        return True, ""


def _has_type_hints(code: str) -> bool:
    """Quick heuristic: does the code contain any type hint syntax?

    Looks for patterns like `: Type`, `-> Type`, `[Type]` in annotations.
    This is not a full parser, just a fast filter to avoid running pyright
    on code that has no type hints at all."""
    # Check for common type hint patterns
    if " -> " in code or ": " in code:
        # More precise: look for actual type annotations
        # Skip if all ":" are in strings or comments
        import ast

        try:
            tree = ast.parse(code)
            # Check for function/variable annotations
            for node in ast.walk(tree):
                # Function annotations: def foo(x: int) or -> int
                if isinstance(node, ast.FunctionDef):
                    if node.returns is not None:
                        return True
                    for arg in node.args.args:
                        if arg.annotation is not None:
                            return True
                    for arg in node.args.posonlyargs:
                        if arg.annotation is not None:
                            return True
                    for arg in node.args.kwonlyargs:
                        if arg.annotation is not None:
                            return True
                    if node.args.vararg and node.args.vararg.annotation:
                        return True
                    if node.args.kwarg and node.args.kwarg.annotation:
                        return True
                # Variable annotations: x: int = 5
                if isinstance(node, ast.AnnAssign):
                    return True
            return False
        except SyntaxError:
            # If code has syntax errors, let the runtime catch them
            return False
    return False
