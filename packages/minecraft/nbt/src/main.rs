use std::{
    fs,
    io::BufWriter,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use minecraft_nbt::{Document, decode_document};
use quartz_nbt::{
    NbtTag,
    io::{self, Flavor},
};
use serde_json::Value;

#[derive(Debug, Parser)]
#[command(about = "Encode a JSON-described Minecraft NBT tree as SNBT or binary NBT")]
struct Args {
    #[arg(long, value_enum)]
    format: OutputFormat,

    #[arg(long, value_enum, default_value_t = NbtFlavor::Uncompressed)]
    flavor: NbtFlavor,

    #[arg(long, default_value = "")]
    root_name: String,

    #[arg(long)]
    input: PathBuf,

    #[arg(long)]
    output: PathBuf,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum OutputFormat {
    Nbt,
    Snbt,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum NbtFlavor {
    Uncompressed,
    Gzip,
    Zlib,
}

impl From<NbtFlavor> for Flavor {
    fn from(value: NbtFlavor) -> Self {
        match value {
            NbtFlavor::Uncompressed => Self::Uncompressed,
            NbtFlavor::Gzip => Self::GzCompressed,
            NbtFlavor::Zlib => Self::ZlibCompressed,
        }
    }
}

fn main() -> Result<()> {
    let args = Args::parse();
    let input = fs::read_to_string(&args.input)
        .with_context(|| format!("failed to read {}", args.input.display()))?;
    let value: Value = serde_json::from_str(&input)
        .with_context(|| format!("failed to parse JSON from {}", args.input.display()))?;
    let document = decode_document(&value, &args.root_name)?;

    match args.format {
        OutputFormat::Nbt => write_binary(&args.output, &document, args.flavor.into()),
        OutputFormat::Snbt => write_snbt(&args.output, &document),
    }
}

fn write_binary(path: &Path, document: &Document, flavor: Flavor) -> Result<()> {
    let file =
        fs::File::create(path).with_context(|| format!("failed to create {}", path.display()))?;
    let mut output = BufWriter::new(file);

    io::write_nbt(
        &mut output,
        Some(document.root_name.as_str()),
        &document.compound,
        flavor,
    )
    .with_context(|| format!("failed to write binary NBT to {}", path.display()))
}

fn write_snbt(path: &Path, document: &Document) -> Result<()> {
    let snbt = NbtTag::Compound(document.compound.clone()).to_pretty_snbt();

    fs::write(path, format!("{snbt}\n"))
        .with_context(|| format!("failed to write SNBT to {}", path.display()))
}
