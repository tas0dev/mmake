mod cache;
mod cli;
mod executor;
mod fingerprint;
mod model;
mod parser;
mod progress;

use anyhow::{Context, Result, bail};
use clap::Parser;
use std::collections::HashSet;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

use cli::Args;
use crossterm::{
    execute,
    style::{Color, Print, ResetColor, SetForegroundColor},
};
use executor::Executor;
use parser::Parser as MakefileParser;

fn main() {
    if let Err(error) = run() {
        let mut stderr = io::stderr();

        let _ = execute!(
            stderr,
            SetForegroundColor(Color::Red),
            Print("Error:"),
            ResetColor,
            Print(format!(" {}\n", error)),
        );

        let _ = stderr.flush();

        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = Args::parse();

    let makefile = fs::canonicalize(&args.file)
        .with_context(|| format!("could not find Makefile: {}", args.file.display()))?;

    let root = makefile
        .parent()
        .context("Makefile has no parent directory")?
        .to_path_buf();

    let source = fs::read_to_string(&makefile)
        .with_context(|| format!("could not read {}", makefile.display()))?;

    let jobs = args.jobs.unwrap_or_else(num_cpus::get).max(1);

    let mut parser = MakefileParser::new(root.clone(), jobs);

    let project = parser.parse(&makefile, &source)?;

    let target = args.target.as_deref().unwrap_or("all");

    if !project.targets.contains_key(target) {
        bail!("target '{}' was not found", target);
    }

    let mut executor = Executor::new(project, root, jobs, args.dry_run, args.verbose);

    executor.run(target)?;

    Ok(())
}
