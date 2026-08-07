//! Real answer-task convention inputs for `aoa eval experiment`.
//!
//! For an answer-shaped (comprehension) repo the builder joins, per identical
//! pair and per arm, the trial trace (codeprobe transcript via the
//! `aoa-codeprobe-shim` path `aoa eval run` exercises), the task's oracle
//! chain (`aoa_bench::OracleChainFacts` resolved against the index's file
//! universe), and the vendored SCIP symbol graph — producing the
//! trace-locality / trace-reach inputs the pre-registered answer conventions
//! bound (see `docs/r0_runbook.md` § "Answer-task convention set").
//!
//! Every non-computable case is an `Err` carrying the exclusion reason; the
//! builder records it per task and drops the pair, so `aoa falsify` only ever
//! scores pairs whose convention inputs are real.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use aoa_bench::{load_task, transcript_path};
use aoa_codeprobe_shim::parse_transcript_file;
use aoa_falsify::UNREACHABLE_TRACE_REACH_DEPTH;
use aoa_metrics::{compute_trace_convention_inputs, trace_footprint, SymbolGraph, TraceReach};
use aoa_scip_graph::index_with_scip;

use crate::error::{BuildError, TaskPurpose};

/// Every fallible step here yields the crate's typed error.
type Result<T> = std::result::Result<T, BuildError>;

/// Per-repo state for computing answer-task convention inputs: the SCIP graph,
/// its file universe, and a task-id-memoized oracle-chain cache (the chain is a
/// task property, identical across seeds and arms).
pub(crate) struct AnswerContext {
    graph: SymbolGraph,
    universe: BTreeSet<String>,
    tasks_dir: PathBuf,
    oracle_cache: BTreeMap<String, BTreeSet<String>>,
}

impl AnswerContext {
    /// Load the repo's declared SCIP index. The index is an operator assertion
    /// (like `confidence`), so a missing/unreadable/empty index is a hard error
    /// — never a silent degrade to sentinel inputs.
    pub(crate) fn load(repo_id: &str, index_path: &Path, tasks_dir: &Path) -> Result<Self> {
        let indexed =
            index_with_scip(index_path).map_err(|source| BuildError::ScipIndexUnreadable {
                repo_id: repo_id.to_string(),
                path: index_path.to_path_buf(),
                source,
            })?;
        let universe: BTreeSet<String> = indexed.graph.node_paths.values().cloned().collect();
        if indexed.graph.nodes.is_empty() || universe.is_empty() {
            return Err(BuildError::ScipIndexEmpty {
                repo_id: repo_id.to_string(),
                path: index_path.to_path_buf(),
            });
        }
        Ok(Self {
            graph: indexed.graph,
            universe,
            tasks_dir: tasks_dir.to_path_buf(),
            oracle_cache: BTreeMap::new(),
        })
    }

    /// Compute one arm's typed observation metrics from the same oracle/index
    /// join used by the paired falsification convention.
    pub(crate) fn observation_inputs(
        &mut self,
        task_id: &str,
        run_dir: &Path,
        arm: &str,
    ) -> Result<(f64, u32)> {
        let oracle = self.oracle_chain(task_id)?;
        self.arm_inputs(run_dir, task_id, &oracle, arm)
    }

    /// One arm's (trace-locality, trace-reach depth). The unreachable outcome is
    /// a measurement, not missing data: it saturates to
    /// [`UNREACHABLE_TRACE_REACH_DEPTH`] so finite depth-k conventions exclude
    /// the task from THEIR tally without removing the pair's evidence.
    fn arm_inputs(
        &self,
        run_dir: &Path,
        task_id: &str,
        oracle: &BTreeSet<String>,
        arm: &str,
    ) -> Result<(f64, u32)> {
        let transcript = transcript_path(run_dir, task_id);
        let shim = parse_transcript_file(&transcript).map_err(|source| {
            BuildError::TranscriptUnreadable {
                arm: arm.to_string(),
                path: transcript.clone(),
                source,
            }
        })?;
        let footprint = trace_footprint(&shim.trace, &self.universe);
        // A relative span path that only resolves as a sub-repo suffix of a
        // universe file (missing e.g. its `src/` prefix) makes the footprint
        // ambiguous — excluded with the reason, never silently dropped (see
        // `trace_footprint`'s asymmetry rationale).
        if !footprint.ambiguous_relative.is_empty() {
            return Err(BuildError::AmbiguousTracePaths {
                arm: arm.to_string(),
                paths: footprint.ambiguous_relative.iter().cloned().collect(),
            });
        }
        let inputs = compute_trace_convention_inputs(&self.graph, &footprint.files, oracle)
            .map_err(|cause| BuildError::ConventionInputs {
                arm: arm.to_string(),
                cause,
            })?;
        let depth = match inputs.trace_reach {
            TraceReach::Depth(d) => d,
            TraceReach::Unreachable => UNREACHABLE_TRACE_REACH_DEPTH,
        };
        Ok((inputs.trace_locality, depth))
    }

    /// The task's oracle chain, resolved against the index universe (memoized).
    /// An empty resolution is an exclusion reason: without an oracle chain the
    /// pair has no measurable convention inputs.
    fn oracle_chain(&mut self, task_id: &str) -> Result<BTreeSet<String>> {
        if let Some(chain) = self.oracle_cache.get(task_id) {
            return Ok(chain.clone());
        }
        let task = load_task(self.tasks_dir.join(task_id)).map_err(|source| {
            BuildError::TaskUnreadable {
                task_id: task_id.to_string(),
                purpose: TaskPurpose::Oracle,
                tasks_dir: self.tasks_dir.clone(),
                source: Box::new(source),
            }
        })?;
        let chain = task.oracle_chain.resolve(&self.universe);
        if chain.is_empty() {
            return Err(BuildError::OracleChainUnresolvable);
        }
        self.oracle_cache.insert(task_id.to_string(), chain.clone());
        Ok(chain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A relative trace path that is only a sub-repo suffix of a universe file
    /// (missing its `src/` prefix) excludes the trial with the reason instead
    /// of silently shrinking the footprint (the pre-fix behavior).
    #[test]
    fn ambiguous_relative_trace_path_is_excluded_with_reason() {
        let dir = std::env::temp_dir().join(format!("aoa-answer-ambiguous-{}", std::process::id()));
        let trial = dir.join("task-1");
        std::fs::create_dir_all(&trial).unwrap();

        let index = dir.join("index.aoa.json");
        std::fs::write(
            &index,
            r#"{"documents": [{"relative_path": "src/pkg/app.py",
                "occurrences": [{"symbol": "pkg/app#app().", "roles": ["definition"]}]}],
                "aoa": {"writable": []}}"#,
        )
        .unwrap();
        std::fs::write(
            trial.join("agent_output.txt"),
            concat!(
                r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","#,
                r#""name":"Read","input":{"file_path":"pkg/app.py"}}]}}"#,
                "\n"
            ),
        )
        .unwrap();

        let ctx = AnswerContext::load("sample/repo", &index, &dir).unwrap();
        let oracle: BTreeSet<String> = ["src/pkg/app.py".to_string()].into();
        let err = ctx
            .arm_inputs(&dir, "task-1", &oracle, "repo")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("ambiguous") && err.contains("pkg/app.py"),
            "exclusion must carry the ambiguity reason and path, got: {err}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
