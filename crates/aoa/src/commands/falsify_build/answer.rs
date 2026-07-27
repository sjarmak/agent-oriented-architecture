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

use anyhow::{anyhow, bail, Context, Result};

use aoa_bench::{load_task, transcript_path};
use aoa_codeprobe_shim::parse_transcript_file;
use aoa_falsify::{ConventionInputs, UNREACHABLE_TRACE_REACH_DEPTH};
use aoa_metrics::{compute_trace_convention_inputs, trace_footprint, SymbolGraph, TraceReach};
use aoa_scip_graph::index_with_scip;

use crate::output::escape_terminal;

/// Per-repo state for computing answer-task convention inputs: the SCIP graph,
/// its file universe, and a task-id-memoized oracle-chain cache (the chain is a
/// task property, identical across seeds and arms).
pub(super) struct AnswerContext {
    graph: SymbolGraph,
    universe: BTreeSet<String>,
    tasks_dir: PathBuf,
    oracle_cache: BTreeMap<String, BTreeSet<String>>,
}

impl AnswerContext {
    /// Load the repo's declared SCIP index. The index is an operator assertion
    /// (like `confidence`), so a missing/unreadable/empty index is a hard error
    /// — never a silent degrade to sentinel inputs.
    pub(super) fn load(repo_id: &str, index_path: &Path, tasks_dir: &Path) -> Result<Self> {
        let indexed = index_with_scip(index_path).with_context(|| {
            format!(
                "repo {repo_id}: failed to read declared scip_index {}",
                index_path.display()
            )
        })?;
        let universe: BTreeSet<String> = indexed.graph.node_paths.values().cloned().collect();
        if indexed.graph.nodes.is_empty() || universe.is_empty() {
            bail!(
                "repo {repo_id}: scip_index {} yields no definitions with document paths; \
                 answer-task convention inputs cannot be derived from it",
                index_path.display()
            );
        }
        Ok(Self {
            graph: indexed.graph,
            universe,
            tasks_dir: tasks_dir.to_path_buf(),
            oracle_cache: BTreeMap::new(),
        })
    }

    /// Compute the pair's convention inputs from both arms' trial traces, or
    /// fail with the exclusion reason.
    pub(super) fn pair_inputs(
        &mut self,
        task_id: &str,
        repo_arm_dir: &Path,
        harness_arm_dir: &Path,
    ) -> Result<ConventionInputs> {
        let oracle = self.oracle_chain(task_id)?;
        let (repo_trace_locality, repo_trace_reach_depth) =
            self.arm_inputs(repo_arm_dir, task_id, &oracle, "repo")?;
        let (harness_trace_locality, harness_trace_reach_depth) =
            self.arm_inputs(harness_arm_dir, task_id, &oracle, "harness")?;
        Ok(ConventionInputs::Answer {
            repo_trace_locality,
            harness_trace_locality,
            repo_trace_reach_depth,
            harness_trace_reach_depth,
        })
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
        let shim = parse_transcript_file(&transcript).with_context(|| {
            format!(
                "{arm} arm: cannot read trial transcript {}",
                transcript.display()
            )
        })?;
        let footprint = trace_footprint(&shim.trace, &self.universe);
        // A relative span path that only resolves as a sub-repo suffix of a
        // universe file (missing e.g. its `src/` prefix) makes the footprint
        // ambiguous — excluded with the reason, never silently dropped (see
        // `trace_footprint`'s asymmetry rationale).
        if !footprint.ambiguous_relative.is_empty() {
            bail!(
                "{arm} arm: trace path(s) [{}] are ambiguous sub-repo-relative suffixes of \
                 universe files; the trial's footprint cannot be measured",
                footprint
                    .ambiguous_relative
                    .iter()
                    .map(|p| escape_terminal(p).to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        let inputs = compute_trace_convention_inputs(&self.graph, &footprint.files, oracle)
            .map_err(|e| anyhow!("{arm} arm: {e}"))?;
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
        let task = load_task(self.tasks_dir.join(task_id))
            .with_context(|| format!("failed to load task {task_id} oracle"))?;
        let chain = task.oracle_chain.resolve(&self.universe);
        if chain.is_empty() {
            bail!(
                "oracle chain unresolvable: no answer/consensus/defining-file/symbol reference \
                 resolves against the scip_index file universe"
            );
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
        let err = ctx.arm_inputs(&dir, "task-1", &oracle, "repo").unwrap_err();
        assert!(
            format!("{err:#}").contains("ambiguous") && format!("{err:#}").contains("pkg/app.py"),
            "exclusion must carry the ambiguity reason and path, got: {err:#}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
