import sys, pathlib, json

# Add ix-fleet src to path for test imports
repo_root = pathlib.Path(__file__).resolve().parents[3]  # D:\index
src_path = repo_root / "packages" / "ix-fleet" / "src"
if src_path.is_dir() and str(src_path) not in sys.path:
    sys.path.insert(0, str(src_path))

