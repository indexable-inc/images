//! Read linking metadata back out of a compiled binary.
//!
//! Deliberately minimal parsers for the two object formats the macro side
//! targets — 64-bit little-endian ELF and 64-bit Mach-O — so consumers get a
//! reader with no object-file dependency. Everything is best-effort: a
//! missing section, an unknown format, or a malformed blob is `None`, never
//! an error.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Upper bound on the metadata blob; anything bigger is not ours.
const MAX_META_LEN: u64 = 64 * 1024;

/// Upper bound on the ELF section-name string table we are willing to load.
const MAX_STRTAB_LEN: u64 = 1024 * 1024;

/// Upper bound on the Mach-O load-command region we are willing to load.
const MAX_LOAD_COMMANDS_LEN: u32 = 16 * 1024 * 1024;

/// ELF `sh_type` for a section with no bytes in the file.
const SHT_NOBITS: u32 = 8;

/// Mach-O load command tag for a 64-bit segment.
const LC_SEGMENT_64: u32 = 0x19;

/// The stdout lens declared by the binary at `path`, if any.
#[must_use]
pub fn stdout_lens(path: &Path) -> Option<String> {
    let raw = raw_metadata(path)?;
    parse_stdout_lens(&raw)
}

/// The raw metadata blob embedded in the binary at `path`, if any.
#[must_use]
pub fn raw_metadata(path: &Path) -> Option<Vec<u8>> {
    let mut file = File::open(path).ok()?;
    let mut magic = [0u8; 4];
    file.read_exact(&mut magic).ok()?;
    if magic == [0x7f, b'E', b'L', b'F'] {
        return elf_section(&mut file);
    }
    if magic == 0xfeed_facf_u32.to_le_bytes() {
        return macho_section(&mut file);
    }
    None
}

/// Extract the `stdout.lens` name from a raw metadata blob.
///
/// The blob is versioned JSON: `{"v":1,"stdout":{"lens":"<name>"}}`. Only
/// `v == 1` is understood; unknown keys are reserved for future metadata and
/// ignored.
fn parse_stdout_lens(raw: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(raw)
        .ok()?
        .trim_matches(|c: char| c == '\0' || c.is_whitespace());
    let doc: serde_json::Value = serde_json::from_str(text).ok()?;
    if doc.get("v")?.as_u64()? != 1 {
        return None;
    }
    Some(doc.get("stdout")?.get("lens")?.as_str()?.to_owned())
}

fn read_at(file: &mut File, pos: u64, buf: &mut [u8]) -> Option<()> {
    file.seek(SeekFrom::Start(pos)).ok()?;
    file.read_exact(buf).ok()
}

fn u16_at(buf: &[u8], off: usize) -> Option<u16> {
    Some(u16::from_le_bytes(buf.get(off..off + 2)?.try_into().ok()?))
}

fn u32_at(buf: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_le_bytes(buf.get(off..off + 4)?.try_into().ok()?))
}

fn u64_at(buf: &[u8], off: usize) -> Option<u64> {
    Some(u64::from_le_bytes(buf.get(off..off + 8)?.try_into().ok()?))
}

/// Walk a 64-bit little-endian ELF's section headers for `.ix.link`.
fn elf_section(file: &mut File) -> Option<Vec<u8>> {
    let mut ehdr = [0u8; 64];
    read_at(file, 0, &mut ehdr)?;
    // EI_CLASS == ELFCLASS64, EI_DATA == ELFDATA2LSB.
    if ehdr[4] != 2 || ehdr[5] != 1 {
        return None;
    }
    let shoff = u64_at(&ehdr, 0x28)?;
    let shentsize = u64::from(u16_at(&ehdr, 0x3a)?);
    let shnum = u64::from(u16_at(&ehdr, 0x3c)?);
    let shstrndx = u64::from(u16_at(&ehdr, 0x3e)?);
    if shoff == 0 || shentsize < 64 || shnum == 0 || shstrndx >= shnum {
        return None;
    }

    let mut shdr = [0u8; 64];
    let header_at = |file: &mut File, index: u64, out: &mut [u8; 64]| -> Option<()> {
        read_at(file, shoff.checked_add(index.checked_mul(shentsize)?)?, out)
    };

    // Load the section-name string table.
    header_at(file, shstrndx, &mut shdr)?;
    let strtab_off = u64_at(&shdr, 0x18)?;
    let strtab_len = u64_at(&shdr, 0x20)?;
    if strtab_len == 0 || strtab_len > MAX_STRTAB_LEN {
        return None;
    }
    let mut strtab = vec![0u8; usize::try_from(strtab_len).ok()?];
    read_at(file, strtab_off, &mut strtab)?;

    for index in 0..shnum {
        header_at(file, index, &mut shdr)?;
        let name_off = usize::try_from(u32_at(&shdr, 0)?).ok()?;
        let Some(name) = strtab.get(name_off..) else {
            continue;
        };
        let name = &name[..name.iter().position(|&b| b == 0)?];
        if name != crate::ELF_SECTION.as_bytes() {
            continue;
        }
        if u32_at(&shdr, 4)? == SHT_NOBITS {
            return None;
        }
        return section_bytes(file, u64_at(&shdr, 0x18)?, u64_at(&shdr, 0x20)?);
    }
    None
}

/// Walk a 64-bit Mach-O's `LC_SEGMENT_64` load commands for `__ix_link`.
fn macho_section(file: &mut File) -> Option<Vec<u8>> {
    let mut header = [0u8; 32];
    read_at(file, 0, &mut header)?;
    let ncmds = u32_at(&header, 16)?;
    let sizeofcmds = u32_at(&header, 20)?;
    if sizeofcmds == 0 || sizeofcmds > MAX_LOAD_COMMANDS_LEN {
        return None;
    }
    let mut cmds = vec![0u8; usize::try_from(sizeofcmds).ok()?];
    read_at(file, 32, &mut cmds)?;

    let (macho_segment, macho_sectname) = crate::MACHO_SECTION.split_once(',')?;

    let mut cursor = 0usize;
    for _ in 0..ncmds {
        let cmd = u32_at(&cmds, cursor)?;
        let cmdsize = usize::try_from(u32_at(&cmds, cursor + 4)?).ok()?;
        if cmdsize < 8 {
            return None;
        }
        if cmd == LC_SEGMENT_64 {
            // segment_command_64: segname[16] at +8, nsects (u32) at +64,
            // followed by nsects section_64 records of 80 bytes each.
            let nsects = usize::try_from(u32_at(&cmds, cursor + 64)?).ok()?;
            for sect in 0..nsects {
                let base = cursor.checked_add(72)?.checked_add(sect.checked_mul(80)?)?;
                let sectname = c_str(cmds.get(base..base + 16)?);
                let segname = c_str(cmds.get(base + 16..base + 32)?);
                if sectname == macho_sectname.as_bytes()
                    && segname.starts_with(macho_segment.as_bytes())
                {
                    let size = u64_at(&cmds, base + 40)?;
                    let offset = u64::from(u32_at(&cmds, base + 48)?);
                    return section_bytes(file, offset, size);
                }
            }
        }
        cursor = cursor.checked_add(cmdsize)?;
    }
    None
}

/// Read `size` bytes at `offset`, bounds-checked against [`MAX_META_LEN`].
fn section_bytes(file: &mut File, offset: u64, size: u64) -> Option<Vec<u8>> {
    if size == 0 || size > MAX_META_LEN {
        return None;
    }
    let mut raw = vec![0u8; usize::try_from(size).ok()?];
    read_at(file, offset, &mut raw)?;
    Some(raw)
}

/// The bytes of a fixed-width, NUL-padded name field up to the first NUL.
fn c_str(field: &[u8]) -> &[u8] {
    let end = field
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(field.len());
    &field[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_v1_stdout_lens() {
        assert_eq!(
            parse_stdout_lens(br#"{"v":1,"stdout":{"lens":"json"}}"#).as_deref(),
            Some("json")
        );
    }

    #[test]
    fn rejects_unknown_version() {
        assert_eq!(parse_stdout_lens(br#"{"v":2,"stdout":{"lens":"json"}}"#), None);
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse_stdout_lens(b"\x00\x01\x02"), None);
        assert_eq!(parse_stdout_lens(b"not json"), None);
    }

    #[test]
    fn non_object_files_are_none() {
        let path = std::env::temp_dir().join(format!(
            "link-meta-read-test-{}.txt",
            std::process::id()
        ));
        std::fs::write(&path, "#!/bin/sh\necho hi\n").expect("write temp file");
        assert_eq!(stdout_lens(&path), None);
        let _ = std::fs::remove_file(path);
    }
}
