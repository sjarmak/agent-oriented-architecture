//! Builds R0 falsification inputs and their evidence reports from paired
//! codeprobe experiment runs.

mod answer;
mod build;
mod error;
mod evidence;
mod exposure;
mod manifest;
mod report;

pub use build::build;
pub use error::{FalsifyBuildError, Result};
pub use manifest::{Manifest, TaskShape};
pub use report::{BuildReport, DroppedRepo, ExcludedTask, RepoBuild};
