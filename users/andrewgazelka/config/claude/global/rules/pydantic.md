---
paths: "**/*.py"
---

# Pydantic

## Use `model_config` Instead of `class Config`

**Always use `model_config = ConfigDict(...)` instead of the deprecated `class Config:`.**

```python
# BAD - deprecated in Pydantic v2, removed in v3
class MyModel(BaseModel):
    name: str

    class Config:
        extra = "forbid"

# GOOD - modern Pydantic v2 style
from pydantic import BaseModel, ConfigDict

class MyModel(BaseModel):
    name: str

    model_config = ConfigDict(extra="forbid")
```

Common ConfigDict options:
- `extra="forbid"` - error on unknown fields
- `extra="ignore"` - silently ignore unknown fields
- `frozen=True` - make model immutable
- `str_strip_whitespace=True` - strip whitespace from strings
