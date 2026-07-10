from __future__ import annotations

from ix_notebook_mcp import outputs


def test_mcp_output_prefers_llm_mime_over_human_html() -> None:
    content = outputs.to_mcp(
        [
            {
                "output_type": "execute_result",
                "data": {
                    "text/html": "<strong>human-only</strong>",
                    "text/plain": "fallback",
                    outputs.IX_LLM_MIME: {"text": "model-only", "images": []},
                },
            }
        ]
    )

    assert [getattr(item, "text", None) for item in content] == ["model-only"]
