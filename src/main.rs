//! fitnessoverlay — GUI editor or CLI overlay renderer.

use anyhow::Result;
use clap::Parser;
use fitoverlay::{run_cli, Cli};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() <= 1 {
        fitoverlay::gui::run_gui()
    } else {
        run_cli(Cli::parse())
    }
}
