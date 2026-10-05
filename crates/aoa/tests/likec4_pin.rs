mod common;

use common::read;
use serde_norway::Value;

const PR_WORKFLOW: &str = ".github/workflows/rust-ci.yml";
const PAGES_WORKFLOW: &str = ".github/workflows/likec4-pages.yml";
const RECORD: &str = "docs/adr/0006-architecture-model-conformance.md";
const PACKAGE: &str = "likec4@";
const ACTION: &str = "likec4/actions@";
const ACTION_VERSION_INPUT: &str = "likec4-version";
const VALIDATE: &str = "validate architecture";
const DISABLING_KEYS: [&str; 2] = ["if", "continue-on-error"];
const PATH_FILTERS: [&str; 2] = ["paths", "paths-ignore"];

struct Step {
    job: String,
    conditional: bool,
    run: String,
    uses: String,
    action_version: Option<String>,
}

fn parse(workflow: &str) -> Value {
    serde_norway::from_str(&read(workflow))
        .unwrap_or_else(|e| panic!("{workflow} parses as YAML: {e}"))
}

fn text(node: &Value, key: &str) -> String {
    node.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn is_conditional(node: &Value) -> bool {
    DISABLING_KEYS.iter().any(|key| node.get(key).is_some())
}

fn steps(workflow: &str) -> Vec<Step> {
    let document = parse(workflow);
    let jobs = document
        .get("jobs")
        .and_then(Value::as_mapping)
        .unwrap_or_else(|| panic!("{workflow} declares a jobs mapping"));
    jobs.iter()
        .flat_map(|(name, job)| {
            let name = name.as_str().unwrap_or_default().to_string();
            let listed = job
                .get("steps")
                .and_then(Value::as_sequence)
                .cloned()
                .unwrap_or_default();
            listed
                .into_iter()
                .map(|step| Step {
                    job: name.clone(),
                    conditional: is_conditional(job) || is_conditional(&step),
                    run: text(&step, "run"),
                    uses: text(&step, "uses"),
                    action_version: step
                        .get("with")
                        .and_then(|with| with.get(ACTION_VERSION_INPUT))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn package_specs(run: &str) -> Vec<String> {
    run.match_indices(PACKAGE)
        .map(|(at, needle)| {
            run[at + needle.len()..]
                .chars()
                .take_while(|c| !c.is_whitespace())
                .collect()
        })
        .collect()
}

fn likec4_versions(workflow: &str) -> Vec<String> {
    steps(workflow)
        .into_iter()
        .flat_map(|step| {
            if step.uses.starts_with(ACTION) {
                let version = step.action_version.unwrap_or_else(|| {
                    panic!(
                        "{workflow} job `{}` runs `{}` without a `{ACTION_VERSION_INPUT}` \
                         input, so that step runs the likec4 bundled with the action \
                         instead of the pinned one and the pull-request gate stops \
                         predicting it. {RECORD} records the pin.",
                        step.job, step.uses
                    )
                });
                vec![version]
            } else {
                package_specs(&step.run)
            }
        })
        .collect()
}

fn is_exact_version(spec: &str) -> bool {
    let (core, suffix) = match spec.find(['-', '+']) {
        Some(at) => (&spec[..at], Some(&spec[at + 1..])),
        None => (spec, None),
    };
    let numeric = |part: &str| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit());
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3
        && parts.into_iter().all(numeric)
        && suffix.is_none_or(|rest| {
            !rest.is_empty()
                && rest
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
        })
}

#[test]
fn an_exact_version_names_one_release_and_nothing_that_moves() {
    for exact in [
        "1.59.2",
        "1.60.0-next.1",
        "1.60.0+build.5",
        "2.0.0-rc.1+sha",
    ] {
        assert!(is_exact_version(exact), "`{exact}` names one release");
    }
    for moving in [
        "latest", "next", "^1.59.2", "~1.59.2", "1.59", "1", "1.x.0", "1.59.2-", ">=1.59.2", "",
    ] {
        assert!(
            !is_exact_version(moving),
            "`{moving}` can resolve to more than one release"
        );
    }
}

#[test]
fn every_pull_request_runs_the_real_validator_unconditionally() {
    let triggers = parse(PR_WORKFLOW)
        .get("on")
        .cloned()
        .unwrap_or_else(|| panic!("{PR_WORKFLOW} declares its triggers under `on`"));
    let pull_request = triggers.get("pull_request").unwrap_or_else(|| {
        panic!("{PR_WORKFLOW} no longer triggers on pull_request, so nothing it runs gates one")
    });
    for filter in PATH_FILTERS {
        assert!(
            pull_request.get(filter).is_none(),
            "{PR_WORKFLOW} filters pull_request by `{filter}`. The validator has to run on \
             every pull request that touches what {PAGES_WORKFLOW} deploys from; check the \
             filter covers those paths, then teach this test to compare the two lists."
        );
    }

    let validating: Vec<Step> = steps(PR_WORKFLOW)
        .into_iter()
        .filter(|step| step.run.contains(PACKAGE) && step.run.contains(VALIDATE))
        .collect();
    assert!(
        !validating.is_empty(),
        "no step in {PR_WORKFLOW} runs `likec4 {VALIDATE}`, so a model LikeC4 rejects merges \
         green and breaks the Pages deploy afterwards. {RECORD} records why \
         architecture_model.rs does not substitute for it."
    );
    for step in validating {
        assert!(
            !step.conditional,
            "{PR_WORKFLOW} job `{}` runs `likec4 {VALIDATE}` behind one of {DISABLING_KEYS:?}, \
             so there are pull requests it does not gate.",
            step.job
        );
    }
}

#[test]
fn the_gate_and_the_deploy_run_one_exact_likec4() {
    let gate = likec4_versions(PR_WORKFLOW);
    let deploy = likec4_versions(PAGES_WORKFLOW);
    for (workflow, versions) in [(PR_WORKFLOW, &gate), (PAGES_WORKFLOW, &deploy)] {
        assert!(
            !versions.is_empty(),
            "{workflow} runs no likec4 at all. If the invocation moved, move this check with \
             it: an empty listing makes the agreement check below pass on nothing."
        );
        for version in versions {
            assert!(
                is_exact_version(version),
                "{workflow} runs likec4 `{version}`, which is not one exact release. {RECORD} \
                 records why the version is pinned and how to bump it."
            );
        }
    }
    let all: Vec<&String> = gate.iter().chain(deploy.iter()).collect();
    assert!(
        all.windows(2).all(|pair| pair[0] == pair[1]),
        "{PR_WORKFLOW} validates with likec4 {gate:?} while {PAGES_WORKFLOW} builds with \
         {deploy:?}. The pull-request job exists to predict that deploy; bump every \
         occurrence together, as {RECORD} describes."
    );
}
