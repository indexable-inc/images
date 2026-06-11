"""Make the package importable when pytest runs from anywhere in the repo."""

import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
