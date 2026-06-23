mod cli;
mod commands;
mod forge;
mod output;

use std::process::ExitCode;

use clap::Parser;

use cli::{Cli, Command};

fn main() -> ExitCode {
    let cli = Cli::parse();

    let result = match &cli.command {
        Command::Observe(args) => commands::run_observe(args),
        Command::Audit(args) => commands::run_audit(args),
        Command::Migrate(args) => commands::run_migrate(args),
        Command::LintContext(args) => commands::run_lint(args),
        Command::Eval(args) => commands::run_eval(args),
        Command::Gap(args) => commands::run_gap(args),
        Command::Recommend(args) => commands::run_recommend(args),
        Command::Falsify(args) => commands::run_falsify(args),
        Command::Policy(args) => commands::run_policy(args),
    };

    match result {
        Ok(code) => ExitCode::from(code as u8),
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}
