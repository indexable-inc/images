#!/usr/bin/env python3
"""Generate src/keymap.rs: macOS virtual keycode (kVK) -> evdev keycode.

The table is derived from the keycodemapdb project, the same dataset QEMU
and libvirt generate their keymaps from:

    https://gitlab.com/keycodemap/keycodemapdb

Usage:
    python3 tools/gen_keymap.py [path/to/keymaps.csv] > src/keymap.rs

Without an argument the CSV is fetched from the gitlab raw URL. The output
is committed (src/keymap.rs) so builds never touch the network.
"""

import csv
import io
import sys
import urllib.request
from pathlib import Path

CSV_URL = "https://gitlab.com/keycodemap/keycodemapdb/-/raw/master/data/keymaps.csv"


def load_csv() -> str:
    if len(sys.argv) > 1:
        return Path(sys.argv[1]).read_text(encoding="utf-8")
    # S310 guards variable URLs reaching file:/custom schemes; CSV_URL is a
    # fixed https constant, so the audit is satisfied by inspection.
    with urllib.request.urlopen(CSV_URL, timeout=30) as resp:  # noqa: S310
        return resp.read().decode("utf-8")


def main() -> None:
    rows = list(csv.reader(io.StringIO(load_csv())))
    header = rows[0]
    col = {name: header.index(name) for name in
           ("Linux Name", "Linux Keycode", "OS-X Name", "OS-X Keycode")}

    # kVK -> (evdev keycode, linux name, osx name). The CSV has one row per
    # X11 keysym, so the same key appears several times; entries agree on the
    # keycode pair and the first row wins.
    table: dict[int, tuple[int, str, str]] = {}
    for row in rows[1:]:
        osx_raw = row[col["OS-X Keycode"]].strip()
        lin_raw = row[col["Linux Keycode"]].strip()
        if not osx_raw or not lin_raw:
            continue
        osx = int(osx_raw, 16)
        lin = int(lin_raw, 0)  # column mixes decimal and 0x-prefixed values
        name = row[col["Linux Name"]]
        if osx == 0xFF or name == "KEY_RESERVED":  # explicit "unmapped" marker
            continue
        if osx in table:
            if table[osx][0] != lin:
                raise SystemExit(f"conflicting mapping for kVK {osx:#x}")
            continue
        if osx > 0x7F:
            raise SystemExit(f"kVK {osx:#x} out of the expected 7-bit range")
        if lin > 0xFFFF:
            raise SystemExit(f"evdev keycode {lin} does not fit u16")
        table[osx] = (lin, name, row[col["OS-X Name"]])

    out = sys.stdout
    out.write(f"""\
//! macOS virtual keycode (kVK, `NSEvent.keyCode`) -> evdev keycode.
//!
//! GENERATED FILE, do not edit by hand. Produced by `tools/gen_keymap.py`
//! from the keycodemapdb project's `data/keymaps.csv` ("OS-X Keycode"
//! column -> "Linux Keycode" column), the dataset QEMU and libvirt derive
//! their keymaps from:
//!
//!     {CSV_URL}
//!
//! Regenerate with: `python3 tools/gen_keymap.py > src/keymap.rs`

/// Dense kVK -> evdev table. kVK codes are 7-bit; 0 marks "no mapping"
/// (`KEY_RESERVED`, which never travels the wire).
const KVK_TO_EVDEV: [u16; 128] = [
""")
    for kvk in range(128):
        if kvk in table:
            lin, lname, oname = table[kvk]
            out.write(f"    {lin}, // {kvk:#04x} {oname} -> {lname}\n")
        else:
            out.write(f"    0, // {kvk:#04x} (unmapped)\n")
    out.write("""\
];

/// The evdev keycode for a macOS virtual keycode, `None` when unmapped.
pub fn evdev_from_kvk(kvk: u16) -> Option<u32> {
    let code = *KVK_TO_EVDEV.get(usize::from(kvk))?;
    (code != 0).then_some(u32::from(code))
}

#[cfg(test)]
mod tests {
    use super::evdev_from_kvk;

    #[test]
    fn spot_checks_against_keycodemapdb() {
        assert_eq!(evdev_from_kvk(0x00), Some(30)); // ANSI_A -> KEY_A
        assert_eq!(evdev_from_kvk(0x24), Some(28)); // Return -> KEY_ENTER
        assert_eq!(evdev_from_kvk(0x35), Some(1)); // Escape -> KEY_ESC
        assert_eq!(evdev_from_kvk(0x7E), Some(103)); // UpArrow -> KEY_UP
        assert_eq!(evdev_from_kvk(0x39), Some(58)); // CapsLock -> KEY_CAPSLOCK
    }

    #[test]
    fn out_of_range_and_unmapped_are_none() {
        assert_eq!(evdev_from_kvk(0x80), None);
        assert_eq!(evdev_from_kvk(u16::MAX), None);
    }
}
""")


if __name__ == "__main__":
    main()
