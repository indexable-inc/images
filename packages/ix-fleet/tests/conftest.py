import sys, pathlib, json

# Add ix-fleet src to path for test imports
repo_root = pathlib.Path(__file__).resolve().parents[3]  # D:\index
src_path = repo_root / "packages" / "ix-fleet" / "src"
if src_path.is_dir() and str(src_path) not in sys.path:
    sys.path.append(str(src_path))

# Write sys.path to a file for debugging
debug_file = pathlib.Path(__file__).parent / "sys_path_debug.txt"
with open(debug_file, "w", encoding="utf-8") as f:
    json.dump(sys.path, f, indent=2)
