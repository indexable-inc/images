from typing import TypedDict

__version__: str

class Node(TypedDict):
    path: str
    kind: str
    start: int
    end: int
    line: int
    column: int
    text: str

class Edit(TypedDict):
    path: str
    start: int
    end: int
    replacement: str

class Finding(TypedDict):
    file: str
    line: int
    column: int
    endLine: int
    endColumn: int
    rule: str
    severity: str
    message: str
    text: str

class Suppressed(Finding):
    commentLine: int
    commentText: str

class Relation(TypedDict):
    columns: list[str]
    rows: list[dict[str, Node | str]]

def query(
    rules: str,
    paths: list[str],
    relation: str | None = None,
) -> dict[str, Relation]: ...
def scan(rules: str, paths: list[str]) -> list[Finding]: ...
def suppressed(rules: str, paths: list[str]) -> list[Suppressed]: ...
def fixes(rules: str, paths: list[str]) -> list[Edit]: ...
def fix(rules: str, paths: list[str], write: bool = False) -> str: ...
