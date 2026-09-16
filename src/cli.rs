use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "mmake")]
#[command(version)]
#[command(about = "A fast Makefile-like build tool")]
pub struct Args {
    /// Target to build
    pub target: Option<String>,

    /// Number of parallel jobs
    #[arg(short = 'j', long = "jobs")]
    pub jobs: Option<usize>,

    /// Makefile path
    #[arg(short = 'f', long = "file", default_value = "Makefile")]
    pub file: PathBuf,

    /// Print commands without executing them
    #[arg(short = 'n', long = "dry-run")]
    pub dry_run: bool,

    /// Show command output
    #[arg(short, long)]
    pub verbose: bool,
}
