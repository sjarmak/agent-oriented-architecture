use anyhow::Result;

use crate::cli::{PolicyArgs, PolicyCommand};
use crate::forge::compile_enforcement;
use crate::output::print_human;

/// Enforcement-plane policy utilities. `compile` exercises the fail-loud forge
/// adapter: an unknown forge propagates a non-zero exit, never a silent no-op.
pub fn run(args: &PolicyArgs) -> Result<i32> {
    match &args.command {
        PolicyCommand::Compile { forge } => {
            let plane = compile_enforcement(forge)?;
            print_human(&plane);
            Ok(0)
        }
    }
}
