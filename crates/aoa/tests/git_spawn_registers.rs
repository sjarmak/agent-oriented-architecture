use std::collections::{BTreeMap, BTreeSet};

mod common;

use common::{read, workspace_root};

const ADR: &str = "docs/adr/0007-git-environment-and-config-for-subprocesses.md";
const CHOOSER_HEADING: &str = "### Which builder a new call takes";
const SHARED_STRIP: &str = "crates/aoa-path-trust/src/git_environment.rs";
const STRIP_CALL: &str = "git_free_of_inherited_state()";
const RAW_CONSTRUCTOR: &str = "Command::new(\"git\")";
const TEST_MODULE_MARKER: &str = "\n#[cfg(test)]\nmod tests";

struct Classified {
    path: &'static str,
    resolver: usize,
    audit: usize,
    data_readers: usize,
    test_only: usize,
}

impl Classified {
    fn calls(&self) -> usize {
        self.resolver + self.audit + self.data_readers + self.test_only
    }
}

const CLASSIFIED: &[Classified] = &[
    Classified {
        path: "crates/aoa-audit/src/planes.rs",
        resolver: 0,
        audit: 1,
        data_readers: 0,
        test_only: 0,
    },
    Classified {
        path: "crates/aoa-path-trust/src/root.rs",
        resolver: 1,
        audit: 0,
        data_readers: 0,
        test_only: 0,
    },
    Classified {
        path: "crates/aoa-path-trust/src/submodule.rs",
        resolver: 1,
        audit: 0,
        data_readers: 0,
        test_only: 0,
    },
    Classified {
        path: "crates/aoa/src/commands/enforce/scope.rs",
        resolver: 0,
        audit: 0,
        data_readers: 0,
        test_only: 1,
    },
    Classified {
        path: "crates/aoa/src/commands/git.rs",
        resolver: 0,
        audit: 0,
        data_readers: 2,
        test_only: 0,
    },
];

fn production_part(source: &str) -> &str {
    match source.find(TEST_MODULE_MARKER) {
        Some(end) => &source[..end],
        None => source,
    }
}

fn crate_sources() -> BTreeSet<String> {
    let root = workspace_root();
    let crates = std::fs::read_dir(root.join("crates")).expect("crates/ is readable");
    let mut pending: Vec<_> = crates
        .map(|entry| entry.expect("crates/ entry is readable").path().join("src"))
        .filter(|src| src.is_dir())
        .collect();
    assert!(
        !pending.is_empty(),
        "no crate under crates/ has a src directory"
    );
    let mut sources = BTreeSet::new();
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).expect("source directory is readable") {
            let path = entry.expect("source entry is readable").path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let relative = path
                    .strip_prefix(&root)
                    .expect("the walk started at the workspace root")
                    .to_string_lossy()
                    .replace('\\', "/");
                sources.insert(relative);
            }
        }
    }
    assert!(
        sources.contains(SHARED_STRIP),
        "the walk did not reach {SHARED_STRIP}, so it is not reading the sources this file classifies"
    );
    sources
}

fn production_calls() -> BTreeMap<String, usize> {
    crate_sources()
        .into_iter()
        .filter(|relative| relative != SHARED_STRIP)
        .filter_map(|relative| {
            let calls = production_part(&read(&relative))
                .matches(STRIP_CALL)
                .count();
            (calls > 0).then_some((relative, calls))
        })
        .collect()
}

#[test]
fn every_production_git_call_is_in_a_register() {
    let found = production_calls();
    let registered: BTreeMap<&str, usize> =
        CLASSIFIED.iter().map(|c| (c.path, c.calls())).collect();
    let paths: BTreeSet<&str> = found
        .keys()
        .map(String::as_str)
        .chain(registered.keys().copied())
        .collect();
    let unclassified: Vec<String> = paths
        .into_iter()
        .filter(|path| {
            found.get(*path).copied().unwrap_or(0) != registered.get(path).copied().unwrap_or(0)
        })
        .map(|path| {
            format!(
                "{path}: {} call(s) to {STRIP_CALL} outside its test module; {ADR} accounts for {}",
                found.get(path).copied().unwrap_or(0),
                registered.get(path).copied().unwrap_or(0)
            )
        })
        .collect();
    assert!(
        unclassified.is_empty(),
        "git calls that {ADR} has not placed in a register:\n  {}\n\
         Decide by what the caller does with git's answer, under \"{CHOOSER_HEADING}\" in {ADR}: \
         locating a repository takes the shared strip as it comes (resolver); reading a refusal as \
         a fact takes planes::git (audit); parsing output configuration can reshape takes \
         reading_repository_data (data readers); a call under #[cfg(test)] is test-only. \
         Then update CLASSIFIED in {}.",
        unclassified.join("\n  "),
        file!()
    );
}

#[test]
fn the_shared_strip_is_the_only_git_constructor() {
    let constructors = production_part(&read(SHARED_STRIP))
        .matches(RAW_CONSTRUCTOR)
        .count();
    assert_eq!(
        constructors, 1,
        "{SHARED_STRIP} builds the git command exactly once"
    );
    let bypasses: Vec<String> = crate_sources()
        .into_iter()
        .filter(|relative| relative != SHARED_STRIP)
        .filter(|relative| production_part(&read(relative)).contains(RAW_CONSTRUCTOR))
        .collect();
    assert!(
        bypasses.is_empty(),
        "{RAW_CONSTRUCTOR} outside {SHARED_STRIP}, so the strip {ADR} decides on does not apply:\n  {}",
        bypasses.join("\n  ")
    );
}

#[test]
fn the_record_names_the_chooser_this_file_and_every_registered_source() {
    let record = read(ADR);
    assert!(
        record.contains(CHOOSER_HEADING),
        "{ADR} has lost \"{CHOOSER_HEADING}\""
    );
    let this_file = std::path::Path::new(file!())
        .file_name()
        .expect("this file has a name")
        .to_string_lossy()
        .into_owned();
    assert!(
        record.contains(&this_file),
        "{ADR} no longer names {this_file}"
    );
    let unnamed: Vec<&str> = CLASSIFIED
        .iter()
        .filter(|c| c.calls() > c.test_only)
        .map(|c| c.path)
        .filter(|path| {
            let name = std::path::Path::new(path)
                .file_name()
                .expect("a source has a name");
            !record.contains(&*name.to_string_lossy())
        })
        .collect();
    assert!(
        unnamed.is_empty(),
        "{ADR} places these sources in a register without naming them:\n  {}",
        unnamed.join("\n  ")
    );
}
