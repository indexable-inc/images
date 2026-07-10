from __future__ import annotations

from typing import Any


class FakeLinearPort:
    """In-memory Linear port that records mutations for assertions."""

    def __init__(self) -> None:
        self.search_results: dict[str, list[dict[str, Any]]] = {}
        self.created: list[dict[str, Any]] = []
        self.commented: list[tuple[str, str]] = []
        self._next_id = 1

    async def search(self, term: str) -> list[dict[str, Any]]:
        return list(self.search_results.get(term, []))

    async def create(
        self,
        *,
        team_id: str,
        title: str,
        description: str,
        parent_id: str,
        label_ids: list[str],
        priority: int,
    ) -> dict[str, Any]:
        issue: dict[str, Any] = {
            "id": f"issue-{self._next_id:04d}",
            "title": title,
            "description": description,
            "identifier": f"ENG-{self._next_id + 1}",
            "state": {"id": "state-todo", "name": "Todo", "type": "unstarted"},
        }
        self._next_id += 1
        self.created.append(issue)
        return issue

    async def comment(self, issue_id: str, body: str) -> dict[str, Any]:
        comment = {"id": f"comment-{len(self.commented) + 1}", "url": "#"}
        self.commented.append((issue_id, body))
        return comment
