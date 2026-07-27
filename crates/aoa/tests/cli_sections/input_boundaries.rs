use super::eval_run::tasks_dir;
use super::r0b::{r0b_baseline, r0b_migrated};
use super::*;

// --- aoa-5cnl: operator-authored input boundaries reject unknown fields -------

/// One well-formed `TaskOutcome`, for run-result bodies whose typo lives elsewhere.
const CLEAN_TASK: &str = r#"{ "visible_success": true, "held_out_success": false }"#;

fn compare_run(run_json: &str) -> assert_cmd::assert::Assert {
    let dir = TempDir::new().expect("tempdir");
    let run = dir.path().join("run.json");
    std::fs::write(&run, run_json).expect("run written");
    aoa()
        .args(["eval", "compare"])
        .arg(&run)
        .arg(fixture("migrated.json"))
        .assert()
}

// `deny_unknown_fields` is per-struct, so each of the three structs a
// run-result file reaches is exercised separately. See `RunResult`'s doc in
// aoa-gap for why a tolerated typo here fails the leakage gate open.
#[test]
fn compare_run_file_rejects_unknown_fields_at_every_boundary() {
    let cases = [
        (
            "canaires",
            format!(
                r#"{{ "tasks": [{CLEAN_TASK}], "held_out_provenance": "native_composed",
                     "canaires": [] }}"#
            ),
        ),
        (
            "visible_sucess",
            r#"{ "tasks": [{ "visible_success": true, "held_out_success": false,
                             "visible_sucess": true }],
                 "held_out_provenance": "native_composed" }"#
                .to_string(),
        ),
        (
            "expected_heldout",
            format!(
                r#"{{ "tasks": [{CLEAN_TASK}], "held_out_provenance": "native_composed",
                     "canaries": [{{ "id": "c0", "held_out_success": true,
                                     "expected_held_out": true,
                                     "expected_heldout": false }}] }}"#
            ),
        ),
    ];

    for (typo, run_json) in cases {
        compare_run(&run_json)
            .failure()
            .stderr(predicate::str::contains("unknown field"))
            .stderr(predicate::str::contains(typo));
    }
}

// The strictness above must not reject the documented run-result schema. The
// `canaries` key is optional, so both the present and absent forms must parse.
#[test]
fn compare_run_file_accepts_documented_schema() {
    for canaries in [
        r#", "canaries": [{ "id": "c0", "held_out_success": false, "expected_held_out": false }]"#,
        "",
    ] {
        compare_run(&format!(
            r#"{{ "tasks": [{CLEAN_TASK}], "held_out_provenance": "native_composed"{canaries} }}"#
        ))
        .success();
    }
}

// A neighbouring shape (`expected_visible` alongside `expected_held_out`) is the
// realistic drift from the schema `--canary`'s help publishes; see `CanarySpec`.
#[test]
fn r0b_canary_manifest_rejects_unknown_fields() {
    let dir = TempDir::new().expect("tempdir");
    let manifest = dir.path().join("canary.json");
    std::fs::write(
        &manifest,
        r#"[{ "id": "external-filelist-000", "expected_held_out": false,
              "expected_visible": true }]"#,
    )
    .expect("manifest written");
    aoa()
        .args(["eval", "r0b", "--json", "--baseline"])
        .arg(r0b_baseline())
        .arg("--migrated")
        .arg(r0b_migrated())
        .arg("--tasks")
        .arg(tasks_dir())
        .arg("--canary")
        .arg(&manifest)
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown field"))
        .stderr(predicate::str::contains("expected_visible"));
}
