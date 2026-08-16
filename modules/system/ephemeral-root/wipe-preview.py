"""Everything the next boot destroys.

Staying on the root filesystem's device is the whole mechanism and the
reason there is no list of exclusions to keep correct. Every whitelisted
path is a bind mount, so the walk stops at it; so are /nix, /proc, /sys,
/dev, /run and /tmp; and btrfs gives each subvolume its own device number,
so it stops at those too. An exclusion list would have to be kept in step
with all of that and would hide a genuinely doomed path the day it fell
out of step.

argv carries the whitelist entries deployed as symlinks (the module bakes
them in at eval time): a symlink is not a mount boundary, so it shows up
in the doomed listing even though its target persists.

Output goes through `sys.stdout.buffer` with `os.fsencode`, because the
listing is filesystem contents: a filename does not have to be valid
UTF-8, and a preview that crashes on one would hide everything after it.
"""

import os
import sys


def emit(line: str) -> None:
    """Write one output line as raw filesystem bytes."""
    sys.stdout.buffer.write(os.fsencode(line) + b"\n")


def walk(directory: str, root_dev: int) -> bool:
    """Print every path under `directory` on the device `root_dev`.

    The moral equivalent of `find <dir> -xdev -mindepth 1 -print`: a mount
    point is listed but never descended into, symlinks are never followed,
    and an unreadable directory is reported without aborting the walk.
    Returns whether every directory could be read.
    """
    complete = True
    try:
        entries = sorted(os.scandir(directory), key=lambda e: e.name)
    except OSError as error:
        print(f"ix-wipe-preview: {error}", file=sys.stderr)
        return False
    for entry in entries:
        emit(entry.path)
        try:
            descend = (
                entry.is_dir(follow_symlinks=False)
                and entry.stat(follow_symlinks=False).st_dev == root_dev
            )
        except OSError as error:
            print(f"ix-wipe-preview: {error}", file=sys.stderr)
            complete = False
            continue
        if descend:
            complete = walk(entry.path, root_dev) and complete
    return complete


def main() -> int:
    """Print the doomed listing, then the surviving-symlink footnote."""
    emit("# doomed: everything below dies on the next boot")
    complete = walk("/", os.lstat("/").st_dev)

    surviving = sys.argv[1:]
    if surviving:
        emit("")
        emit("# listed above but surviving anyway: a symlinked entry is not a")
        emit("# mount boundary, so the walk lists it. Its target persists.")
        for path in surviving:
            emit(f"  {path}")

    sys.stdout.buffer.flush()
    return 0 if complete else 1


if __name__ == "__main__":
    sys.exit(main())
