mod common;

use common::read;
use serde_norway::Value;

const PR_WORKFLOW: &str = ".github/workflows/rust-ci.yml";
const PAGES_WORKFLOW: &str = ".github/workflows/likec4-pages.yml";
const RECORD: &str = "docs/adr/0006-architecture-model-conformance.md";
const PACKAGE: &str = "likec4";
const ACTION: &str = "likec4/actions@";
const ACTION_VERSION_INPUT: &str = "likec4-version";
const VALIDATE: &str = "validate architecture";
const DISABLING_KEYS: [&str; 4] = ["if", "continue-on-error", "needs", "shell"];

struct Step {
    job: String,
    disabled_by: Vec<&'static str>,
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

fn disabling_keys(node: &Value) -> Vec<&'static str> {
    DISABLING_KEYS
        .into_iter()
        .filter(|key| node.get(key).is_some())
        .collect()
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
                    disabled_by: disabling_keys(job)
                        .into_iter()
                        .chain(disabling_keys(&step))
                        .collect(),
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

fn package_spec(token: &str) -> Option<String> {
    if token == PACKAGE {
        return Some(String::new());
    }
    token
        .strip_prefix(PACKAGE)?
        .strip_prefix('@')
        .map(str::to_string)
}

fn package_specs(run: &str) -> Vec<String> {
    run.split_whitespace().filter_map(package_spec).collect()
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

fn validating_invocation(run: &str) -> Option<&str> {
    let version = run
        .trim()
        .strip_prefix("npx -y ")?
        .strip_prefix(PACKAGE)?
        .strip_prefix('@')?
        .strip_suffix(VALIDATE)?
        .strip_suffix(' ')?;
    (!version.is_empty() && !version.chars().any(char::is_whitespace)).then_some(version)
}

fn is_identifier_list(part: Option<&str>) -> bool {
    part.is_none_or(|list| {
        list.split('.').all(|identifier| {
            !identifier.is_empty()
                && identifier
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
    })
}

fn is_exact_version(spec: &str) -> bool {
    let (version, build) = spec
        .split_once('+')
        .map_or((spec, None), |(version, build)| (version, Some(build)));
    let (core, prerelease) = version
        .split_once('-')
        .map_or((version, None), |(core, prerelease)| {
            (core, Some(prerelease))
        });
    let numeric = |part: &str| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit());
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3
        && parts.into_iter().all(numeric)
        && is_identifier_list(prerelease)
        && is_identifier_list(build)
}

#[test]
fn an_exact_version_names_one_release_and_nothing_that_moves() {
    for exact in [
        "1.59.2",
        "1.60.0-next.1",
        "1.60.0+build.5",
        "2.0.0-rc.1+sha",
        "1.0.0-rc-1",
        "1.0.0-0.3.7",
    ] {
        assert!(is_exact_version(exact), "`{exact}` names one release");
    }
    for moving in [
        "latest",
        "next",
        "^1.59.2",
        "~1.59.2",
        "1.59",
        "1",
        "1.x.0",
        "1.59.2-",
        ">=1.59.2",
        "",
        "1.59.2-rc..1",
        "1.59.2-rc.",
        "1.59.2-.1",
        "1.59.2-rc_1",
        "1.59.2+",
        "1.59.2+build..1",
        "1.59.2-rc.1+",
        "1.59.2+a+b",
    ] {
        assert!(
            !is_exact_version(moving),
            "`{moving}` can resolve to more than one release"
        );
    }
}

#[test]
fn a_run_step_names_likec4_with_or_without_a_version() {
    assert_eq!(
        package_specs("npx -y likec4@1.59.2 validate architecture"),
        ["1.59.2"]
    );
    assert_eq!(
        package_specs("npx -y likec4 export json architecture -o model.json"),
        [""]
    );
    assert_eq!(
        package_specs("npx -y likec4@latest build\nnpx likec4 export json"),
        ["latest", ""]
    );
    assert!(package_specs("node architecture/site/check-links.mjs").is_empty());
    assert!(package_specs("npx -y @likec4/cli").is_empty());
}

#[test]
fn the_validating_step_is_exactly_the_pinned_invocation() {
    assert_eq!(
        validating_invocation("npx -y likec4@1.59.2 validate architecture"),
        Some("1.59.2")
    );
    assert_eq!(
        validating_invocation("  npx -y likec4@1.59.2 validate architecture\n"),
        Some("1.59.2")
    );
    for wrapped in [
        "echo npx -y likec4@1.59.2 validate architecture",
        "npx -y likec4@1.59.2 validate architecture || true",
        "npx -y likec4@1.59.2 validate architecture; true",
        "npx -y likec4 validate architecture",
        "npx -y likec4@1.59.2 validate architecture --strict",
        "npx -y likec4@1.59.2  validate architecture",
    ] {
        assert_eq!(
            validating_invocation(wrapped),
            None,
            "`{wrapped}` is not exactly the pinned invocation"
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
    let filters: Vec<&str> = match pull_request {
        Value::Null => Vec::new(),
        Value::Mapping(filters) => filters
            .keys()
            .map(|key| key.as_str().unwrap_or_default())
            .collect(),
        other => panic!("{PR_WORKFLOW} declares pull_request as `{other:?}`, not as a mapping"),
    };
    assert!(
        filters.is_empty(),
        "{PR_WORKFLOW} filters pull_request by {filters:?}. The validator has to run on \
         every pull request that touches what {PAGES_WORKFLOW} deploys from; check the \
         filter covers those, then teach this test to compare the two."
    );

    let validating: Vec<Step> = steps(PR_WORKFLOW)
        .into_iter()
        .filter(|step| step.run.contains(VALIDATE) && !package_specs(&step.run).is_empty())
        .collect();
    assert!(
        !validating.is_empty(),
        "no step in {PR_WORKFLOW} runs `likec4 {VALIDATE}`, so a model LikeC4 rejects merges \
         green and breaks the Pages deploy afterwards. {RECORD} records why \
         architecture_model.rs does not substitute for it."
    );
    for step in validating {
        assert!(
            validating_invocation(&step.run).is_some(),
            "{PR_WORKFLOW} job `{}` runs `{}`, not exactly `npx -y {PACKAGE}@<version> \
             {VALIDATE}`, so the validator's exit status is not the step's.",
            step.job,
            step.run.trim()
        );
        assert!(
            step.disabled_by.is_empty(),
            "{PR_WORKFLOW} job `{}` runs `likec4 {VALIDATE}` behind {:?}, so there are pull \
             requests it does not gate.",
            step.job,
            step.disabled_by
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
                "{workflow} runs likec4 `{version}`, which {} one exact release. {RECORD} \
                 records why the version is pinned and how to bump it.",
                if version.is_empty() {
                    "names no version at all, so npm resolves `latest` instead of"
                } else {
                    "is not"
                }
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
