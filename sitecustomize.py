import pathlib, sys

# Add the ix-fleet source directory to Python path for test discovery
src_path = pathlib.Path(__file__).parent / "packages" / "ix-fleet" / "src"
if src_path.is_dir() and str(src_path) not in sys.path:
    sys.path.insert(0, str(src_path))
