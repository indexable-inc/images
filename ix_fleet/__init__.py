import pathlib, importlib.util, sys

# Resolve the path to the actual ix_fleet package source within the repository.
repo_root = pathlib.Path(__file__).resolve().parents[1]
src_path = repo_root / "packages" / "ix-fleet" / "src" / "ix_fleet"

# Insert the source path to sys.path temporarily for any relative imports.
if str(src_path.parent) not in sys.path:
    sys.path.insert(0, str(src_path.parent))

# Load the real package as a separate module to avoid recursion.
spec = importlib.util.spec_from_file_location("ix_fleet_src", src_path / "__init__.py")
if spec is None or spec.loader is None:
    raise ImportError(f"Cannot find ix_fleet source at {src_path}")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

# Re-export all public symbols from the real package.
globals().update({k: v for k, v in module.__dict__.items() if not k.startswith("_")})

# Ensure the package appears as 'ix_fleet' to callers.
sys.modules[__name__] = module
