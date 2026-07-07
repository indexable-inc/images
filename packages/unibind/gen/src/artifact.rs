//! Read embedded unibind interfaces out of a compiled artifact.
//!
//! The macro plants the serialized IR in a dedicated section
//! ([`unibind_core::embed`]); this module locates that section with `object`
//! and parses the JSON payloads back into [`Interface`] values.

use std::path::Path;

use anyhow::{bail, Context as _};
use object::{Object as _, ObjectSection as _};
use unibind_core::ir::{Interface, IR_VERSION};

/// Every interface embedded in one artifact, in section order.
pub struct EmbeddedInterfaces {
    pub interfaces: Vec<Interface>,
}

/// Section name in ELF (and other non-Apple) objects, matching
/// [`unibind_core::embed::LINK_SECTION_ELF`].
const SECTION_ELF: &str = ".unibind_ir";

/// The section component of the Mach-O `__DATA,__unibind_ir` pair
/// ([`unibind_core::embed::LINK_SECTION_MACH_O`]); `object` reports Mach-O
/// sections by that 16-byte section name alone, without the segment.
const SECTION_MACH_O: &str = "__unibind_ir";

/// Read and parse the unibind IR section of the artifact at `path`: an
/// object file, a cdylib, or a `staticlib` archive (the section is read out
/// of every archive member that carries one).
///
/// # Errors
///
/// Fails when the file cannot be read or parsed as an object file or
/// archive, when no IR section is present, and when the embedded payload
/// does not deserialize to interfaces of the supported [`IR_VERSION`].
pub fn read(path: &Path) -> anyhow::Result<EmbeddedInterfaces> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let interfaces = if bytes.starts_with(ARCHIVE_MAGIC) {
        archive_interfaces(&bytes, path)?
    } else {
        let file = object::File::parse(&*bytes)
            .with_context(|| format!("parsing {} as an object file", path.display()))?;
        object_interfaces(&file, path)?
    };
    if interfaces.is_empty() {
        bail!(
            "no unibind IR section ({SECTION_ELF} / __DATA,{SECTION_MACH_O}) in {}; \
             was the crate built with #[unibind::export]?",
            path.display()
        );
    }
    Ok(EmbeddedInterfaces { interfaces })
}

/// The `ar` header every archive starts with; a `staticlib` artifact is one
/// of these with one member per codegen unit (plus symbol tables).
const ARCHIVE_MAGIC: &[u8] = b"!<arch>\n";

/// The interfaces of every archive member that parses as an object file and
/// carries the IR section. Members that are not objects (symbol tables like
/// darwin's `__.SYMDEF`) are skipped.
fn archive_interfaces(bytes: &[u8], path: &Path) -> anyhow::Result<Vec<Interface>> {
    let archive = object::read::archive::ArchiveFile::parse(bytes)
        .with_context(|| format!("parsing {} as an archive", path.display()))?;
    let mut interfaces = Vec::new();
    for member in archive.members() {
        let member = member.with_context(|| format!("reading a member of {}", path.display()))?;
        let data = member
            .data(bytes)
            .with_context(|| format!("reading a member of {}", path.display()))?;
        let Ok(file) = object::File::parse(data) else {
            continue;
        };
        interfaces.extend(object_interfaces(&file, path)?);
    }
    Ok(interfaces)
}

/// The interfaces embedded in one parsed object, empty when the object has
/// no IR section.
fn object_interfaces(file: &object::File<'_>, path: &Path) -> anyhow::Result<Vec<Interface>> {
    let section = file
        .sections()
        .find(|section| matches!(section.name(), Ok(SECTION_ELF | SECTION_MACH_O)));
    let Some(section) = section else {
        return Ok(Vec::new());
    };
    let data = section
        .data()
        .with_context(|| format!("reading the unibind IR section of {}", path.display()))?;
    parse_ir_bytes(data)
        .with_context(|| format!("parsing the unibind IR embedded in {}", path.display()))
}

/// Parse raw section bytes: one JSON [`Interface`] per `#[used]` static, with
/// whatever NUL padding the linker inserted between and around them.
///
/// # Errors
///
/// Fails when a payload is not valid interface JSON or carries an IR version
/// other than [`IR_VERSION`].
pub fn parse_ir_bytes(bytes: &[u8]) -> anyhow::Result<Vec<Interface>> {
    let mut interfaces = Vec::new();
    let mut rest = bytes;
    // The linker pads and concatenates the embedded statics, so skip the NUL
    // (or stray whitespace) run before each JSON document.
    while let Some(start) = rest
        .iter()
        .position(|byte| *byte != 0 && !byte.is_ascii_whitespace())
    {
        rest = &rest[start..];

        let mut stream = serde_json::Deserializer::from_slice(rest).into_iter::<Interface>();
        let Some(next) = stream.next() else {
            break;
        };
        let interface = next.context("deserializing an embedded unibind interface")?;
        if interface.version != IR_VERSION {
            bail!(
                "interface `{}` carries IR version {}, but this unibind-gen reads version {IR_VERSION}",
                interface.name,
                interface.version
            );
        }
        let consumed = stream.byte_offset();
        interfaces.push(interface);
        rest = &rest[consumed..];
    }
    Ok(interfaces)
}
