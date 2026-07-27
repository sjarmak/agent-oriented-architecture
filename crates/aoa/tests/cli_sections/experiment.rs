use super::*;

// --- aoa-dhk: R0 falsification as a codeprobe experiment ----------------------

// AC2 SMOKE: the full pipeline runs end-to-end on a fixture experiment (1 repo /
// 1 identical-pair task across two arms) and emits falsification.json with a
// verdict field. With a single repo the gate cannot establish a cross-repo
// majority, so the verdict is an honest `inconclusive` carrying the
// `too_few_repos` precondition discriminator — never mistakable for a real
// 5-repo abstention. AC4: codeprobe bias warnings are surfaced alongside.
#[test]
fn experiment_pipeline_smoke_emits_verdict_and_surfaces_bias() {
    let dir = TempDir::new().expect("tempdir");
    let input = dir.path().join("falsify_input.json");
    let build_meta = dir.path().join("falsify_input.build.json");
    let falsification = dir.path().join("falsification.json");

    // Step 1: build the FalsifyInput from the experiment's paired arms.
    aoa()
        .args(["eval", "experiment", "--manifest"])
        .arg(fixture("experiment_smoke/manifest.json"))
        .arg("--tasks")
        .arg(fixture("codeprobe_tasks"))
        .arg("--out")
        .arg(&input)
        .assert()
        .success();

    let build: Value =
        serde_json::from_str(&std::fs::read_to_string(&build_meta).expect("build report written"))
            .expect("valid build json");
    assert_eq!(build["repo_count"], 1);
    assert_eq!(build["total_identical_pairs"], 1);
    assert_eq!(build["convention_inputs_degraded"], true);
    let repo0 = &build["repos"][0];
    assert_eq!(repo0["identical_pairs"], 1);
    assert_eq!(
        repo0["eligible"], true,
        "native+high+calibrated repo is eligible"
    );
    // H4: the task present only in the repo arm is excluded as a non-pair.
    let excluded = repo0["excluded_tasks"].as_array().expect("excluded array");
    assert!(
        excluded.iter().any(|e| e["task_id"] == "solo-only-001"),
        "presence-mismatch task must be recorded as excluded, got {excluded:?}"
    );

    // Step 2: run the gate over the built input, with bias warnings attached.
    aoa()
        .args(["falsify", "--repos"])
        .arg(&input)
        .arg("--build-meta")
        .arg(&build_meta)
        .arg("--bias-warnings")
        .arg(fixture("experiment_aggregate.json"))
        .arg("--out")
        .arg(&falsification)
        .assert()
        // A precondition-driven verdict is a non-usable result: non-zero exit.
        .failure();

    let out: Value =
        serde_json::from_str(&std::fs::read_to_string(&falsification).expect("falsification.json"))
            .expect("valid json");
    assert_eq!(out["verdict"], "inconclusive");
    assert_eq!(out["precondition_unmet"], "too_few_repos");
    // AC4: codeprobe bias warnings surfaced alongside the verdict, and the
    // no_independent_baseline warning flagged as gate-invalidating.
    let warnings = out["bias_warnings"]
        .as_array()
        .expect("bias warnings surfaced");
    assert_eq!(warnings.len(), 2);
    assert_eq!(out["bias_gate_invalidating"], true);
}

// Answer-task conventions (pre-registered 2026-07-04, aoa-dhk.1): an
// answer-shaped repo (task_shape "answer" + scip_index) gets REAL per-pair
// trace-locality/trace-reach inputs joined from both arms' trial traces, the
// task oracle chain, and the SCIP graph. The pair whose trials carry no
// instrumented file access is excluded with the reason; the surviving pair's
// inputs are exact; convention_inputs_degraded is an honest computed false and
// the config carries the answer-family convention set.
#[test]
fn experiment_answer_shape_computes_real_convention_inputs() {
    let dir = TempDir::new().expect("tempdir");
    let input = dir.path().join("falsify_input.json");
    let build_meta = dir.path().join("falsify_input.build.json");
    let falsification = dir.path().join("falsification.json");

    aoa()
        .args(["eval", "experiment", "--manifest"])
        .arg(fixture("experiment_answer/manifest.json"))
        .arg("--tasks")
        .arg(fixture("answer_tasks"))
        .arg("--out")
        .arg(&input)
        .assert()
        .success();

    let build: Value =
        serde_json::from_str(&std::fs::read_to_string(&build_meta).expect("build report written"))
            .expect("valid build json");
    assert_eq!(build["task_shape"], "answer");
    assert_eq!(
        build["convention_inputs_degraded"], false,
        "every admitted pair carries real inputs, so the flag is computed false"
    );
    assert_eq!(build["repos"][0]["identical_pairs"], 1);
    let excluded = build["repos"][0]["excluded_tasks"]
        .as_array()
        .expect("excluded array");
    let noreads = excluded
        .iter()
        .find(|e| e["task_id"] == "comprehension-noreads-001")
        .expect("prose-only-transcript pair is excluded");
    assert!(
        noreads["reason"]
            .as_str()
            .unwrap()
            .contains("trace footprint is empty"),
        "exclusion carries the computed reason, got {noreads:?}"
    );

    // The built input carries the exact per-arm inputs: repo arm read exactly
    // the two oracle-chain files (locality 1.0, depth 0); harness arm read one
    // chain file and one off-chain file (locality 0.5) whose reach to the
    // remaining chain file is one undirected hop (depth 1).
    let parsed: Value =
        serde_json::from_str(&std::fs::read_to_string(&input).expect("input written"))
            .expect("valid input json");
    let task = &parsed["repos"][0]["runs"][0]["tasks"][0];
    let inputs = &task["convention_inputs"];
    assert_eq!(inputs["family"], "answer");
    assert_eq!(inputs["repo_trace_locality"], 1.0);
    assert_eq!(inputs["harness_trace_locality"], 0.5);
    assert_eq!(inputs["repo_trace_reach_depth"], 0);
    assert_eq!(inputs["harness_trace_reach_depth"], 1);
    let conventions = parsed["config"]["conventions"]
        .as_array()
        .expect("conventions");
    assert!(conventions
        .iter()
        .any(|c| c["name"] == "trace_locality_floor" && c["family"] == "answer"));

    // The gate still abstains on too_few_repos (1 repo), but WITHOUT any
    // convention-degradation marker: that blocker is genuinely cleared.
    aoa()
        .args(["falsify", "--repos"])
        .arg(&input)
        .arg("--build-meta")
        .arg(&build_meta)
        .arg("--out")
        .arg(&falsification)
        .assert()
        .failure();
    let out: Value =
        serde_json::from_str(&std::fs::read_to_string(&falsification).expect("falsification"))
            .expect("valid json");
    assert_eq!(out["precondition_unmet"], "too_few_repos");
    let notes = out["notes"].as_array().unwrap();
    assert!(
        !notes
            .iter()
            .any(|n| n.as_str().unwrap_or_default().contains("degraded")),
        "no degraded-convention note may appear for an answer-shape build, got {notes:?}"
    );
}

/// Writes `manifest_json` to a temp dir and runs the experiment builder over
/// it against the `answer_tasks` fixture, expecting failure; callers assert
/// on stderr.
fn experiment_manifest_failure(manifest_json: &str) -> assert_cmd::assert::Assert {
    let dir = TempDir::new().expect("tempdir");
    let manifest = dir.path().join("manifest.json");
    std::fs::write(&manifest, manifest_json).expect("manifest written");
    aoa()
        .args(["eval", "experiment", "--manifest"])
        .arg(&manifest)
        .arg("--tasks")
        .arg(fixture("answer_tasks"))
        .arg("--out")
        .arg(dir.path().join("falsify_input.json"))
        .assert()
        .failure()
}

// A manifest declaring answer shape without the index it needs fails loud —
// the builder never silently degrades an operator-declared answer repo.
#[test]
fn experiment_answer_shape_without_index_fails_loud() {
    experiment_manifest_failure(
        r#"{
          "k_runs": 3, "min_holdout_size": 1,
          "repos": [{
            "repo_id": "sample/answers", "confidence": "high", "calibrated": true,
            "task_shape": "answer",
            "runs": [
              { "seed": 1, "repo_arm": "seed1/repo_arm", "harness_arm": "seed1/harness_arm" },
              { "seed": 2, "repo_arm": "seed2/repo_arm", "harness_arm": "seed2/harness_arm" },
              { "seed": 3, "repo_arm": "seed3/repo_arm", "harness_arm": "seed3/harness_arm" }
            ]
          }]
        }"#,
    )
    .stderr(predicate::str::contains("requires scip_index"));
}

// `deny_unknown_fields` is per-struct, so each of the three manifest
// boundaries (Manifest, RepoManifest, RunManifest) is exercised separately;
// the parse fails at the unknown key, so one run entry suffices regardless
// of `k_runs`. Rationale for the strictness lives on the Manifest doc.
#[test]
fn experiment_manifest_rejects_unknown_fields_at_every_boundary() {
    let repo = r#""repo_id": "sample/repo", "confidence": "high", "calibrated": true"#;
    let run = r#""seed": 1, "repo_arm": "seed1/repo_arm", "harness_arm": "seed1/harness_arm""#;
    let cases = [
        (
            "min_effect_szie",
            format!(
                r#"{{ "k_runs": 3, "min_holdout_size": 1, "min_effect_szie": 0.05,
                     "repos": [{{ {repo}, "runs": [{{ {run} }}] }}] }}"#
            ),
        ),
        (
            "task_shpae",
            format!(
                r#"{{ "k_runs": 3, "min_holdout_size": 1,
                     "repos": [{{ {repo}, "task_shpae": "answer", "runs": [{{ {run} }}] }}] }}"#
            ),
        ),
        (
            "sede",
            format!(
                r#"{{ "k_runs": 3, "min_holdout_size": 1,
                     "repos": [{{ {repo}, "runs": [{{ "sede": 1, {run} }}] }}] }}"#
            ),
        ),
    ];

    for (typo, manifest_json) in cases {
        experiment_manifest_failure(&manifest_json)
            .stderr(predicate::str::contains("unknown field"))
            .stderr(predicate::str::contains(typo));
    }
}

// The strictness above must not reject the documented R11 attestation key
// (docs/r0_runbook.md § "R11 scope note" tells answer-shape operators to
// declare `calibrated_basis`). Reaching the post-parse `requires scip_index`
// diagnostic proves the manifest deserialized cleanly with the key present.
#[test]
fn experiment_manifest_accepts_documented_calibrated_basis_key() {
    experiment_manifest_failure(
        r#"{
          "k_runs": 3, "min_holdout_size": 1,
          "repos": [{
            "repo_id": "sample/answers", "confidence": "high", "calibrated": true,
            "calibrated_basis": "consensus-verified-answer-oracles-r11-scope-note-2026-07-05",
            "task_shape": "answer",
            "runs": [
              { "seed": 1, "repo_arm": "seed1/repo_arm", "harness_arm": "seed1/harness_arm" }
            ]
          }]
        }"#,
    )
    .stderr(predicate::str::contains("requires scip_index"))
    .stderr(predicate::str::contains("unknown field").not());
}

// aoa-g2g5: R0 determinism evidence is only valid across INDEPENDENT,
// identity-aligned runs. The builder's old run-count/min-max-count checks let
// three integrity violations through; each must now fail loud.

// A repo whose runs reuse a seed is K copies of one draw, not K independent
// replications — reject it before any evidence is assembled. (Bails pre-read,
// so the run dirs need not exist.)
#[test]
fn experiment_rejects_duplicate_seed_across_runs() {
    experiment_manifest_failure(
        r#"{
          "k_runs": 3, "min_holdout_size": 1,
          "repos": [{
            "repo_id": "sample/dup-seed", "confidence": "high", "calibrated": true,
            "runs": [
              { "seed": 1, "repo_arm": "seed1/repo_arm", "harness_arm": "seed1/harness_arm" },
              { "seed": 1, "repo_arm": "seed2/repo_arm", "harness_arm": "seed2/harness_arm" },
              { "seed": 3, "repo_arm": "seed3/repo_arm", "harness_arm": "seed3/harness_arm" }
            ]
          }]
        }"#,
    )
    .stderr(predicate::str::contains(
        "seed 1 is used by more than one run",
    ));
}

// Reusing an arm run directory across runs reads that arm's outcomes from
// identical files — again not an independent replication. (Distinct seeds here,
// so the seed gate passes and the directory gate is what fires.)
#[test]
fn experiment_rejects_reused_run_directory() {
    experiment_manifest_failure(
        r#"{
          "k_runs": 3, "min_holdout_size": 1,
          "repos": [{
            "repo_id": "sample/dup-dir", "confidence": "high", "calibrated": true,
            "runs": [
              { "seed": 1, "repo_arm": "seed1/repo_arm", "harness_arm": "seed1/harness_arm" },
              { "seed": 2, "repo_arm": "seed1/repo_arm", "harness_arm": "seed2/harness_arm" },
              { "seed": 3, "repo_arm": "seed3/repo_arm", "harness_arm": "seed3/harness_arm" }
            ]
          }]
        }"#,
    )
    .stderr(predicate::str::contains("run directory"))
    .stderr(predicate::str::contains("used by more than one run/arm"));
}

// Directory distinctness resolves ./.. (and symlinks): two runs naming the SAME
// physical dir via different spellings must still be rejected, or distinct seeds
// could read identical outcomes (vacuous replication). Uses absolute paths into
// an existing fixture so canonicalize() actually resolves the `..` alias.
#[test]
fn experiment_rejects_aliased_run_directory() {
    let smoke = fixture("experiment_smoke");
    let smoke = smoke.to_str().unwrap();
    let manifest = format!(
        r#"{{
          "k_runs": 3, "min_holdout_size": 1,
          "repos": [{{
            "repo_id": "sample/alias", "confidence": "high", "calibrated": true,
            "runs": [
              {{ "seed": 1, "repo_arm": "{smoke}/seed1/repo_arm", "harness_arm": "{smoke}/seed1/harness_arm" }},
              {{ "seed": 2, "repo_arm": "{smoke}/seed2/../seed1/repo_arm", "harness_arm": "{smoke}/seed2/harness_arm" }},
              {{ "seed": 3, "repo_arm": "{smoke}/seed3/repo_arm", "harness_arm": "{smoke}/seed3/harness_arm" }}
            ]
          }}]
        }}"#
    );
    experiment_manifest_failure(&manifest)
        .stderr(predicate::str::contains("used by more than one run/arm"));
}

// The core aoa-g2g5 case: two runs admit the SAME NUMBER of identical pairs but
// DIFFERENT task identities (run 0/2 admit `alpha`, run 1 admits `beta`). The
// old min/max-count check passed this (counts all equal 1); the identity check
// must fail it, naming the missing/extra ids, because the positional PairTask
// ids and run-indexed determinism check would otherwise compare mismatched
// tasks.
#[test]
fn experiment_rejects_equal_count_different_task_identities() {
    let dir = TempDir::new().expect("tempdir");
    aoa()
        .args(["eval", "experiment", "--manifest"])
        .arg(fixture("experiment_divergent/manifest.json"))
        .arg("--tasks")
        .arg(fixture("experiment_divergent"))
        .arg("--out")
        .arg(dir.path().join("falsify_input.json"))
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "admits a different identical-pair set than run 0",
        ))
        .stderr(predicate::str::contains("\"alpha\""))
        .stderr(predicate::str::contains("\"beta\""));
}

// H2/AC4: given a real >=5-repo input but a build report flagging degraded
// convention inputs, the gate abstains to `inconclusive` with the
// `convention_inputs_degraded` precondition rather than asserting a verdict the
// R0' convention-invariance check cannot back. The gate's deltas are still
// emitted for transparency.
#[test]
fn falsify_abstains_on_degraded_convention_inputs() {
    let dir = TempDir::new().expect("tempdir");
    let out = dir.path().join("falsification.json");

    aoa()
        .args(["falsify", "--repos"])
        .arg(fixture("falsify_input.json"))
        .arg("--build-meta")
        .arg(fixture("build_meta_degraded.json"))
        .arg("--bias-warnings")
        .arg(fixture("experiment_aggregate.json"))
        .arg("--out")
        .arg(&out)
        .assert()
        .failure();

    let parsed: Value =
        serde_json::from_str(&std::fs::read_to_string(&out).expect("written")).expect("json");
    assert_eq!(parsed["verdict"], "inconclusive");
    assert_eq!(parsed["precondition_unmet"], "convention_inputs_degraded");
    // Deltas preserved for transparency even when abstaining.
    assert!(
        parsed.get("repo_delta").is_some(),
        "repo_delta kept for transparency"
    );
    assert_eq!(parsed["bias_gate_invalidating"], true);
}

// A genuine >=5-repo gate verdict carries NO precondition discriminator and exits
// zero — the property that keeps a real abstention distinguishable from a
// precondition-driven one.
#[test]
fn falsify_real_verdict_has_no_precondition_marker() {
    let dir = TempDir::new().expect("tempdir");
    let out = dir.path().join("falsification.json");

    aoa()
        .args(["falsify", "--repos"])
        .arg(fixture("falsify_input.json"))
        .arg("--build-meta")
        .arg(fixture("build_meta_ok.json"))
        .arg("--out")
        .arg(&out)
        .assert()
        .success();

    let parsed: Value =
        serde_json::from_str(&std::fs::read_to_string(&out).expect("written")).expect("json");
    assert!(parsed.get("verdict").is_some());
    assert!(
        parsed.get("precondition_unmet").is_none(),
        "a real gate verdict must not carry a precondition discriminator"
    );
}

// Security: untrusted free-text from codeprobe's aggregate.json (bias warning
// messages) must be escaped before reaching the terminal — a crafted message
// must not inject raw control sequences into human output.
#[test]
fn falsify_escapes_untrusted_bias_warning_text() {
    let dir = TempDir::new().expect("tempdir");
    let out = dir.path().join("falsification.json");

    let assert = aoa()
        .args(["falsify", "--repos"])
        .arg(fixture("falsify_input.json"))
        .arg("--build-meta")
        .arg(fixture("build_meta_ok.json"))
        .arg("--bias-warnings")
        .arg(fixture("bias_warnings_malicious.json"))
        .arg("--out")
        .arg(&out)
        .assert()
        .success();

    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8");
    // The raw ESC (0x1b) and BEL (0x07) control bytes must NOT appear in human
    // output; the escaped textual form must.
    assert!(
        !stdout.contains('\u{1b}') && !stdout.contains('\u{07}'),
        "raw control bytes leaked into terminal output"
    );
    assert!(
        stdout.contains("\\u{1b}"),
        "escaped form of the control byte expected"
    );
}
