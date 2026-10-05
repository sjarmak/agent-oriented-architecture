//! The enforcement plane's liveness surface, driven through the real CLI.
//!
//! AOA could already tell "hook set not installed" from "hook set out of date".
//! It could not tell **installed and enforcing** from **installed and silently
//! emitting nothing** — settings.json reads identically in both, and the absent
//! live log reads as no-activity rather than as broken instrumentation
//! (aoa-dpluh). These are the boundary tests for that answer: the silent state
//! must be reported as silent and must never be reported as enforcing.
//!
//! The fourth state is the one a clean checkout is in. Registration is tracked
//! (`.claude/settings.json` and the wrapper are force-included in `.gitignore`)
//! and telemetry is ignored (`.aoa/`), so every fresh clone carries a registered
//! plane and no local runtime footprint at all — and used to audit as a Tier-1
//! silent plane on that basis alone, failing the CI self-audit gate on a
//! condition no checkout can satisfy (aoa-rsixa). That state is reported, and it
//! still raises a finding, but at the tier its evidence actually supports.

use super::enforce::{aoa_stdin, hook_payload, observe_enforce};
use super::*;

/// `aoa audit --json` for `repo`, parsed.
fn audit_json(repo: &Path) -> Value {
    let output = aoa_stdin()
        .args(["audit", "--json", "--repo"])
        .arg(repo)
        .output()
        .expect("audit runs");
    serde_json::from_slice(&output.stdout).expect("audit emits JSON")
}

fn liveness_state(report: &Value) -> String {
    report["enforcement_liveness"]["state"]
        .as_str()
        .expect("the audit reports an enforcement-liveness state")
        .to_string()
}

/// What git hands a fresh clone: the tracked registration and no telemetry.
///
/// `observe --enforce` provisions `.aoa/traces/` as part of installing, so that
/// directory is the local install's own footprint and removing it leaves exactly
/// the checkout shape. The install is driven through the real command rather
/// than a transcribed settings.json for the reason `planes.rs` records: a
/// hand-spelled hook fixture drifted from `hook_command` and kept passing.
fn registered_checkout() -> TempDir {
    let repo = TempDir::new().unwrap();
    observe_enforce(repo.path());
    std::fs::remove_dir_all(repo.path().join(".aoa")).unwrap();
    repo
}

/// Satisfy the other Tier-1 plane, so a `--fail-on tier1` assertion below turns
/// on the enforcement plane's liveness and not on a missing CI workflow.
fn present_ci_plane(repo: &Path) {
    std::fs::create_dir_all(repo.join(".github/workflows")).unwrap();
}

/// Criterion (d), the sharp direction: hooks installed and nothing able to run
/// them. The plane must read as installed-but-silent, and must NOT read as
/// enforcing.
///
/// `observe --enforce` provisions `.aoa/traces/` as part of installing, so this
/// is also the case where the *directory* exists and holds no log — the shape
/// that used to be indistinguishable from a healthy repo between sessions.
#[test]
fn installed_hooks_that_never_ran_report_silent_and_never_enforcing() {
    let repo = TempDir::new().unwrap();
    observe_enforce(repo.path());

    let report = audit_json(repo.path());
    assert_eq!(
        liveness_state(&report),
        "installed-but-silent",
        "installed hooks with no emitted span are silent, not enforcing: {report}"
    );
    assert_eq!(
        report["enforcement_liveness"]["silence"], "no-live-logs",
        "the traces dir exists and holds no live log; that is a distinct fact \
         from the dir being absent"
    );

    // (c) Never a pass: the silent plane is a Tier-1 finding on the punch-list,
    // so a consumer reading only `items` still cannot mistake it for healthy.
    let silent: Vec<&Value> = report["items"]
        .as_array()
        .expect("items array")
        .iter()
        .filter(|item| item["kind"] == "silent_plane")
        .collect();
    assert_eq!(
        silent.len(),
        1,
        "an installed-but-silent plane must raise exactly one finding: {report}"
    );
    assert_eq!(silent[0]["tier"], "tier-1");
    assert_eq!(silent[0]["plane"], "runtime-hook");
    assert_eq!(silent[0]["measured_cost"]["unit"], "silent plane");
}

/// The negative direction of (d): a repo genuinely emitting spans reports
/// enforcing. Driven through the real `aoa enforce` hook path rather than a
/// hand-written file, so the test proves the surface reads what the installed
/// hooks actually write.
#[test]
fn a_repo_emitting_spans_through_the_hook_path_reports_enforcing() {
    let repo = TempDir::new().unwrap();
    observe_enforce(repo.path());

    aoa_stdin()
        .args(["enforce", "record"])
        .write_stdin(hook_payload("Bash", Some("cargo test"), repo.path()))
        .assert()
        .success();

    let report = audit_json(repo.path());
    assert_eq!(
        liveness_state(&report),
        "enforcing",
        "a live log carrying a recorded span is the enforcing state: {report}"
    );
    assert_eq!(report["enforcement_liveness"]["spans"], 1);
    assert!(
        report["items"]
            .as_array()
            .expect("items array")
            .iter()
            .all(|item| item["kind"] != "silent_plane"),
        "an enforcing plane must raise no silence finding: {report}"
    );
}

/// (b) Loud, not blank. The silence has to be legible without parsing JSON —
/// the operator reading the human register is the one who missed this state for
/// a whole night of sessions.
#[test]
fn the_silent_state_is_loud_in_the_human_register() {
    let repo = TempDir::new().unwrap();
    observe_enforce(repo.path());

    aoa_stdin()
        .args(["audit", "--repo"])
        .arg(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("INSTALLED BUT SILENT"))
        .stdout(predicate::str::contains("NOT enforcing"));
}

/// A repo with no hook set at all is a state of its own, and it is distinct from
/// silence: there is nothing installed to be silent about, and the existing
/// missing-plane finding already covers it. Reporting silence here would tell an
/// operator to debug an install that was never done.
#[test]
fn a_repo_with_no_hook_set_reports_not_installed() {
    let repo = TempDir::new().unwrap();

    let report = audit_json(repo.path());
    assert_eq!(liveness_state(&report), "not-installed");
    let items = report["items"].as_array().expect("items array");
    assert!(
        items.iter().all(|item| item["kind"] != "silent_plane"),
        "an uninstalled plane raises the missing-plane finding, not silence: {report}"
    );
    assert!(
        items
            .iter()
            .any(|item| item["kind"] == "missing_plane" && item["plane"] == "runtime-hook"),
        "the uninstalled runtime plane is still a missing-plane finding: {report}"
    );
}

/// A tree with a registered plane and no `.aoa/` at all is the shape of every
/// clean checkout, and it is not the shape of a plane that went silent. See
/// [`registered_checkout`] for why that shape is what it is.
#[test]
fn a_checkout_with_no_local_telemetry_is_unobserved_rather_than_silent() {
    let repo = registered_checkout();

    let report = audit_json(repo.path());
    assert_eq!(
        liveness_state(&report),
        "installed-unobserved",
        "a registered plane with no local runtime footprint has not been \
         observed running; asserting it went silent claims a measurement that \
         was never taken: {report}"
    );
    assert!(
        report["enforcement_liveness"]["silence"].is_null(),
        "the unobserved state carries no silence reason — it is not silence: {report}"
    );
}

/// The regression proper (aoa-rsixa). `audit --fail-on tier1` is the CI
/// self-audit gate, and on a pristine checkout it exited 2 on the finding above
/// — a gate keyed on local runtime state that `.gitignore` guarantees no
/// checkout can carry, so no CI run could ever have passed it.
#[test]
fn a_checkout_with_no_local_telemetry_passes_the_tier1_gate() {
    let repo = registered_checkout();
    present_ci_plane(repo.path());

    aoa_stdin()
        .args(["audit", "--fail-on", "tier1", "--repo"])
        .arg(repo.path())
        .assert()
        .success();
}

/// Still a finding, and still loud — the checkout case is downgraded, not
/// dropped. A consumer reading only `items` must not be able to mistake an
/// unobserved plane for a healthy one, which is the guarantee the silent state
/// was given and the reason this is not simply omitted from the punch-list.
#[test]
fn an_unobserved_plane_is_still_a_finding_in_both_registers() {
    let repo = registered_checkout();

    let report = audit_json(repo.path());
    let silent: Vec<&Value> = report["items"]
        .as_array()
        .expect("items array")
        .iter()
        .filter(|item| item["kind"] == "silent_plane")
        .collect();
    assert_eq!(
        silent.len(),
        1,
        "an unobserved plane still raises exactly one finding: {report}"
    );
    assert_eq!(
        silent[0]["tier"], "tier-3",
        "the audit has no measurement showing this plane stopped emitting, so \
         the finding sits at the asserted-but-unsupported tier: {report}"
    );
    assert_eq!(silent[0]["plane"], "runtime-hook");
    assert_eq!(
        silent[0]["measured_cost"]["unit"], "unobserved plane",
        "the cost unit is the only place `items` says which of the two \
         not-emitting states this is, since both share a kind: {report}"
    );

    aoa_stdin()
        .args(["audit", "--repo"])
        .arg(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("NEVER OBSERVED RUNNING"))
        .stdout(predicate::str::contains("not known to be enforcing"));
}

/// The direction that must not move. Once the install has run in this tree, its
/// telemetry directory exists, and a plane that then emits nothing is a measured
/// silence — Tier-1, and it still fails the gate. Without this the change above
/// would read as "stop failing on silence" rather than "stop failing on the
/// absence of a measurement".
#[test]
fn an_installed_plane_that_emitted_nothing_still_fails_the_tier1_gate() {
    let repo = TempDir::new().unwrap();
    observe_enforce(repo.path());
    present_ci_plane(repo.path());

    assert_eq!(
        liveness_state(&audit_json(repo.path())),
        "installed-but-silent",
        "the install provisioned .aoa/traces, so its emptiness is a measurement"
    );

    aoa_stdin()
        .args(["audit", "--fail-on", "tier1", "--repo"])
        .arg(repo.path())
        .assert()
        .failure();
}

/// Deleting the telemetry directory downgrades a measured silence to an
/// unobserved plane, and the gate then passes. Pinned deliberately, because it
/// is the accepted cost of the change and not an oversight: `.aoa/` is ignored
/// and self-ignoring, so `rm -rf .aoa` and a routine `git clean -xdf` both reach
/// it.
///
/// It cannot be had both ways from repository state alone. The only thing that
/// made deletion pointless before was reporting an absent directory as Tier-1 —
/// which is the same behavior that made the gate unpassable by every checkout,
/// not a separable protection sitting beside it. Closing this properly needs
/// evidence that survives the working tree (aoa-zswh6), not a tier.
///
/// The finding does survive the deletion, at Tier-3 and in the human register,
/// so a reader is still told. Only the exit code moves.
#[test]
fn deleting_the_telemetry_directory_downgrades_a_measured_silence() {
    let repo = TempDir::new().unwrap();
    observe_enforce(repo.path());
    present_ci_plane(repo.path());

    aoa_stdin()
        .args(["audit", "--fail-on", "tier1", "--repo"])
        .arg(repo.path())
        .assert()
        .failure();

    std::fs::remove_dir_all(repo.path().join(".aoa")).unwrap();

    let report = audit_json(repo.path());
    assert_eq!(
        liveness_state(&report),
        "installed-unobserved",
        "the measurement was deleted, so the audit stops claiming to hold one: {report}"
    );
    assert!(
        report["items"]
            .as_array()
            .expect("items array")
            .iter()
            .any(|item| item["kind"] == "silent_plane" && item["tier"] == "tier-3"),
        "the finding must survive the deletion, only its tier moves: {report}"
    );
    aoa_stdin()
        .args(["audit", "--fail-on", "tier1", "--repo"])
        .arg(repo.path())
        .assert()
        .success();
}

fn foreign_hook_set(repo: &Path) {
    std::fs::create_dir_all(repo.join(".claude/hooks")).unwrap();
    std::fs::write(repo.join(".claude/hooks/guard.sh"), "#!/bin/sh\n").unwrap();
    std::fs::write(
        repo.join(".claude/settings.json"),
        r#"{"hooks":{
            "PreToolUse":[{"matcher":"Edit|Write","hooks":[
                {"type":"command","command":"\"$CLAUDE_PROJECT_DIR\"/.claude/hooks/guard.sh pre"}
            ]}],
            "PostToolUse":[{"hooks":[
                {"type":"command","command":"\"$CLAUDE_PROJECT_DIR\"/.claude/hooks/guard.sh post"}
            ]}],
            "SessionStart":[{"hooks":[{"type":"command","command":"echo session"}]}]
        }}"#,
    )
    .unwrap();
}

fn runtime_plane_items(report: &Value) -> Vec<&Value> {
    report["items"]
        .as_array()
        .expect("items array")
        .iter()
        .filter(|item| item["plane"] == "runtime-hook")
        .collect()
}

#[test]
fn a_foreign_hook_set_is_reported_apart_from_a_repo_with_no_hooks() {
    let foreign = TempDir::new().unwrap();
    foreign_hook_set(foreign.path());
    let bare = TempDir::new().unwrap();

    let foreign_report = audit_json(foreign.path());
    let bare_report = audit_json(bare.path());

    assert_eq!(liveness_state(&foreign_report), "foreign-hooks");
    assert_eq!(liveness_state(&bare_report), "not-installed");

    let foreign_items = runtime_plane_items(&foreign_report);
    assert_eq!(foreign_items.len(), 1, "{foreign_report}");
    assert_eq!(foreign_items[0]["kind"], "missing_plane");
    assert_eq!(foreign_items[0]["tier"], "tier-1");
    assert_eq!(
        foreign_items[0]["title"],
        "missing enforcement plane: runtime hook (agent hooks present, AOA enforcement not \
         installed)"
    );

    let bare_items = runtime_plane_items(&bare_report);
    assert_eq!(bare_items.len(), 1, "{bare_report}");
    assert_eq!(bare_items[0]["tier"], "tier-1");
    assert_eq!(
        bare_items[0]["title"],
        "missing enforcement plane: runtime hook"
    );
}

#[test]
fn a_decoy_command_naming_the_wrapper_reports_the_runtime_plane_absent() {
    let repo = TempDir::new().unwrap();
    observe_enforce(repo.path());
    present_ci_plane(repo.path());
    let settings_path = repo.path().join(".claude/settings.json");
    let installed = audit_json(repo.path());
    assert!(runtime_plane_items(&installed)
        .iter()
        .all(|item| item["kind"] != "missing_plane"));

    let mut settings: Value =
        serde_json::from_slice(&std::fs::read(&settings_path).unwrap()).unwrap();
    let mut decoyed = 0;
    for groups in settings["hooks"].as_object_mut().unwrap().values_mut() {
        for group in groups.as_array_mut().unwrap() {
            for hook in group["hooks"].as_array_mut().unwrap() {
                let verb = aoa_audit::ENFORCE_HOOK_SET
                    .iter()
                    .map(|(_, verb)| *verb)
                    .find(|verb| hook["command"] == aoa_audit::hook_command(verb));
                if let Some(verb) = verb {
                    hook["command"] =
                        format!("true # decoy {} {verb}", aoa_audit::ENFORCE_WRAPPER_REL).into();
                    decoyed += 1;
                }
            }
        }
    }
    assert_eq!(decoyed, aoa_audit::ENFORCE_HOOK_SET.len());
    std::fs::write(&settings_path, settings.to_string()).unwrap();

    let report = audit_json(repo.path());
    assert_eq!(liveness_state(&report), "not-installed");
    let items = runtime_plane_items(&report);
    assert_eq!(items.len(), 1, "{report}");
    assert_eq!(items[0]["kind"], "missing_plane");
    assert_eq!(items[0]["tier"], "tier-1");
    assert!(
        report["enforce_hook_warning"]
            .as_str()
            .is_some_and(|warning| warning.contains("rerun `aoa observe --enforce`")),
        "{report}"
    );

    aoa_stdin()
        .args(["audit", "--fail-on", "tier1", "--repo"])
        .arg(repo.path())
        .assert()
        .failure()
        .stdout(predicate::str::contains(
            "missing enforcement plane: runtime hook",
        ));
}

#[test]
fn a_foreign_hook_set_draws_no_stamp_warning_and_still_fails_the_tier1_gate() {
    let repo = TempDir::new().unwrap();
    foreign_hook_set(repo.path());
    present_ci_plane(repo.path());

    let report = audit_json(repo.path());
    assert!(
        report.get("enforce_hook_warning").is_none(),
        "a settings file AOA never wrote has no AOA stamp to be missing: {report}"
    );

    aoa_stdin()
        .args(["audit", "--fail-on", "tier1", "--repo"])
        .arg(repo.path())
        .assert()
        .failure()
        .stdout(predicate::str::contains(
            "enforcement plane: agent hooks present, AOA enforcement not installed",
        ))
        .stdout(predicate::str::contains(
            "missing enforcement plane: runtime hook (agent hooks present, AOA enforcement not \
             installed)",
        ))
        .stdout(predicate::str::contains("enforce hook stamp").not());
}

#[test]
fn a_codeowners_only_repo_is_told_which_write_boundary_surface_is_absent() {
    let repo = TempDir::new().unwrap();
    std::fs::create_dir_all(repo.path().join(".github")).unwrap();
    std::fs::write(repo.path().join(".github/CODEOWNERS"), "* @owner\n").unwrap();

    let report = audit_json(repo.path());
    let item = report["items"]
        .as_array()
        .expect("items array")
        .iter()
        .find(|item| item["kind"] == "write_safety_zone")
        .unwrap_or_else(|| panic!("no write-boundary item: {report}"));

    assert_eq!(item["measured_cost"]["value"], 1);
    let title = item["title"].as_str().unwrap();
    assert!(title.contains(".aoa/write-policy.toml"), "{title}");
    assert!(!title.contains("CODEOWNERS"), "{title}");

    aoa_stdin()
        .args(["audit", "--repo"])
        .arg(repo.path())
        .assert()
        .success()
        .stdout(predicate::str::contains(title.to_string()));
}
