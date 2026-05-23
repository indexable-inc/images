import pathlib, sys, importlib.util

# Resolve path to the src/ix_fleet package within this repository.
repo_root = pathlib.Path(__file__).resolve().parent
src_path = repo_root / "src" / "ix_fleet"
# Add parent directory to sys.path if not present.
if str(src_path.parent) not in sys.path:
    sys.path.insert(0, str(src_path.parent))
# Load the actual module and re-export its symbols.
spec = importlib.util.spec_from_file_location("ix_fleet_src", src_path / "__init__.py")
if spec is None or spec.loader is None:
    raise ImportError(f"Cannot locate ix_fleet source at {src_path}")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
# Re-export public symbols.
globals().update({k: v for k, v in module.__dict__.items() if not k.startswith("_")})
# Register module under the expected name.
sys.modules[__name__] = module
