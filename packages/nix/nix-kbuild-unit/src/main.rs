mod cmd_file;
mod harvest;
mod model;
mod render;

use std::io::Read as _;
use std::path::PathBuf;

use clap::Parser as _;
use color_eyre::eyre::WrapErr as _;
use model::Plan;

#[derive(Debug, clap::Parser)]
#[command(
    version,
    about = "Render kbuild .cmd command graphs as composable Nix derivations"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, clap::Subcommand)]
enum Command {
    /// Harvest a completed kbuild objtree into plan JSON on stdout.
    Harvest(HarvestArgs),

    /// Render generated Nix from plan JSON on stdin.
    Render(RenderArgs),
}

#[derive(Debug, clap::Args)]
struct HarvestArgs {
    /// Completed in-tree kbuild output tree.
    #[arg(long, value_name = "PATH")]
    objtree: PathBuf,

    /// Pristine kernel source the build started from.
    #[arg(long, value_name = "PATH")]
    srctree: PathBuf,

    /// Copy the generated-file snapshot (build-created non-unit files) here.
    #[arg(long, value_name = "PATH")]
    generated_out: Option<PathBuf>,
}

#[derive(Debug, clap::Args)]
struct RenderArgs {
    /// Emit CA-derivation attributes on generated units.
    #[arg(long)]
    content_addressed: bool,
}

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    match Cli::parse().command {
        Command::Harvest(args) => {
            let plan =
                harvest::harvest(&args.objtree, &args.srctree, args.generated_out.as_deref())?;
            serde_json::to_writer(std::io::stdout(), &plan).wrap_err("writing plan JSON")?;
            println!();
        }
        Command::Render(args) => {
            let mut input = String::new();
            std::io::stdin()
                .read_to_string(&mut input)
                .wrap_err("reading plan JSON from stdin")?;
            let plan: Plan = serde_json::from_str(&input).wrap_err("parsing plan JSON")?;
            let rendered = render::render_units_nix(&plan, args.content_addressed)
                .wrap_err("rendering plan as Nix")?;
            print!("{rendered}");
        }
    }

    Ok(())
}
