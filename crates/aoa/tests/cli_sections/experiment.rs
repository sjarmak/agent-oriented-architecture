use super::*;
use sha2::Digest;

// --- aoa-dhk: R0 falsification as a codeprobe experiment ----------------------

// Edit-shaped runs cannot honestly supply the four required metrics yet. The
// builder emits content-addressed exclusions for every arm candidate rather
// than laundering the historical degraded sentinels into measured evidence.
#[test]
fn experiment_edit_shape_emits_observations_but_no_measured_pairs() {
    let dir = TempDir::new().expect("tempdir");
    let input = dir.path().join("falsify_input.json");
    let build_meta = dir.path().join("falsify_input.build.json");

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
    assert_eq!(build["repo_count"], 0);
    assert_eq!(build["total_identical_pairs"], 0);
    assert_eq!(build["observation_count"], 8);
    assert_eq!(build["convention_inputs_degraded"], true);
    let excluded = build["dropped_repos"][0]["excluded_tasks"]
        .as_array()
        .expect("excluded array");
    assert!(
        excluded.iter().any(|e| e["task_id"] == "solo-only-001"),
        "presence-mismatch task must be recorded as excluded, got {excluded:?}"
    );
    let observations = std::fs::read_to_string(dir.path().join("falsify_input.observations.jsonl"))
        .expect("sidecar");
    assert!(observations
        .lines()
        .all(|line| line.contains("\"status\":\"excluded\"")));
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
    let observations_path = dir.path().join("falsify_input.observations.jsonl");
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
    assert_eq!(build["repos"][0]["candidate_pairs"], 2);
    assert_eq!(build["repos"][0]["pair_yield"], 0.5);
    assert_eq!(build["observation_count"], 12);
    assert_eq!(build["observation_ids"].as_array().unwrap().len(), 12);
    assert_eq!(
        build["observations_path"],
        observations_path.display().to_string()
    );
    let observation_bytes = std::fs::read(&observations_path).expect("observation sidecar");
    assert_eq!(
        build["observations_sha256"],
        format!("{:x}", sha2::Sha256::digest(&observation_bytes))
    );
    let observations: Vec<Value> = String::from_utf8(observation_bytes)
        .expect("UTF-8 JSONL")
        .lines()
        .map(|line| serde_json::from_str(line).expect("observation JSON"))
        .collect();
    assert_eq!(observations.len(), 12);
    assert!(observations
        .iter()
        .all(|observation| observation["id"].as_str().unwrap().len() == 64));
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
    assert_eq!(task["task_id"], "comprehension-boolean-000");
    for field in ["repo_observation_id", "harness_observation_id"] {
        let id = task[field].as_str().expect("observation id");
        assert!(
            observations
                .iter()
                .any(|observation| observation["id"] == id),
            "{field} must reference the emitted sidecar"
        );
    }
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

#[test]
fn experiment_pair_yield_preflight_stops_low_yield_before_full_campaign() {
    let dir = TempDir::new().expect("tempdir");
    let input = dir.path().join("falsify_input.json");

    aoa()
        .args(["eval", "experiment", "--manifest"])
        .arg(fixture("experiment_answer/manifest.json"))
        .arg("--tasks")
        .arg(fixture("answer_tasks"))
        .arg("--out")
        .arg(&input)
        .args(["--min-pair-yield", "0.8"])
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "pair-yield preflight failed: sample/answers admitted 1/2 pairs (0.500), below 0.800",
        ));

    assert!(
        input.exists(),
        "the diagnostic build artifact remains inspectable after preflight failure"
    );
    assert!(
        dir.path().join("falsify_input.build.json").exists(),
        "the pair-yield evidence remains inspectable after preflight failure"
    );

    aoa()
        .args(["eval", "experiment", "--manifest"])
        .arg(fixture("experiment_answer/manifest.json"))
        .arg("--tasks")
        .arg(fixture("answer_tasks"))
        .arg("--out")
        .arg(dir.path().join("passing.json"))
        .args(["--min-pair-yield", "0.5"])
        .assert()
        .success();
}

#[test]
fn experiment_missing_calibration_is_excluded_data_not_a_command_failure() {
    let dir = TempDir::new().expect("tempdir");
    let fixture_dir = fixture("experiment_answer");
    let mut manifest: Value = serde_json::from_str(
        &std::fs::read_to_string(fixture_dir.join("manifest.json")).expect("manifest"),
    )
    .expect("manifest JSON");
    let repo = &mut manifest["repos"][0];
    repo["calibration_artifact"] = Value::String(
        dir.path()
            .join("missing-calibration.json")
            .display()
            .to_string(),
    );
    for field in ["repo_arm_config", "harness_arm_config", "scip_index"] {
        let relative = repo[field].as_str().expect("relative path");
        repo[field] = Value::String(fixture_dir.join(relative).display().to_string());
    }
    for run in repo["runs"].as_array_mut().expect("runs") {
        for field in ["repo_arm", "harness_arm"] {
            let relative = run[field].as_str().expect("relative run path");
            run[field] = Value::String(fixture_dir.join(relative).display().to_string());
        }
    }
    let manifest_path = dir.path().join("manifest.json");
    std::fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).expect("serialize"),
    )
    .expect("write manifest");

    aoa()
        .args(["eval", "experiment", "--manifest"])
        .arg(&manifest_path)
        .arg("--tasks")
        .arg(fixture("answer_tasks"))
        .arg("--out")
        .arg(dir.path().join("falsify_input.json"))
        .assert()
        .success();

    let build: Value = serde_json::from_slice(
        &std::fs::read(dir.path().join("falsify_input.build.json")).expect("build report"),
    )
    .expect("build JSON");
    assert_eq!(build["repo_count"], 0);
    let observations = std::fs::read_to_string(dir.path().join("falsify_input.observations.jsonl"))
        .expect("sidecar");
    assert!(observations
        .lines()
        .all(|line| line.contains("\"reason\":\"calibration_missing\"")));
}

#[test]
fn experiment_observation_sidecar_is_byte_reproducible() {
    let dir = TempDir::new().expect("tempdir");
    let run = |name: &str| {
        let out = dir.path().join(format!("{name}.json"));
        aoa()
            .args(["eval", "experiment", "--manifest"])
            .arg(fixture("experiment_answer/manifest.json"))
            .arg("--tasks")
            .arg(fixture("answer_tasks"))
            .arg("--out")
            .arg(&out)
            .assert()
            .success();
        std::fs::read(out.with_extension("observations.jsonl")).expect("sidecar")
    };

    assert_eq!(run("first"), run("second"));
}

/// Writes `manifest_json` to a temp dir and runs the experiment builder over
/// it against the `answer_tasks` fixture, expecting failure; callers assert
/// on stderr.
#[test]
fn experiment_drops_divergent_unmeasured_edit_runs() {
    let dir = TempDir::new().expect("tempdir");
    let output = aoa()
        .args(["eval", "experiment", "--manifest"])
        .arg(fixture("experiment_divergent/manifest.json"))
        .arg("--tasks")
        .arg(fixture("experiment_divergent"))
        .arg("--out")
        .arg(dir.path().join("falsify_input.json"))
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let rendered = String::from_utf8(output).expect("human output");
    assert!(rendered.contains("DROPPED: no identical pairs"));
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
