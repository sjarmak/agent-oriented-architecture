use std::ffi::OsStr;
use std::process::Command;

const REPOSITORY_LOCAL_GIT_ENV: [&str; 19] = [
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_OBJECT_DIRECTORY",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_GRAFT_FILE",
    "GIT_INDEX_FILE",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_REPLACE_REF_BASE",
    "GIT_PREFIX",
    "GIT_SHALLOW_FILE",
    "GIT_COMMON_DIR",
    "GIT_CEILING_DIRECTORIES",
    "GIT_DISCOVERY_ACROSS_FILESYSTEM",
    "GIT_NAMESPACE",
    "GIT_QUARANTINE_PATH",
];

const STRIPPED_ENV_PREFIXES: [&[u8]; 2] = [b"GIT_TRACE", b"GIT_REDIRECT_"];

const TRACE2_TARGET_ENV: [&str; 3] = ["GIT_TRACE2", "GIT_TRACE2_EVENT", "GIT_TRACE2_PERF"];

const TRACE_DISABLED: &str = "0";

pub fn git_free_of_inherited_state() -> Command {
    let mut command = Command::new("git");
    for variable in REPOSITORY_LOCAL_GIT_ENV {
        command.env_remove(variable);
    }
    for (variable, _) in std::env::vars_os() {
        if has_stripped_prefix(&variable) {
            command.env_remove(variable);
        }
    }
    for variable in TRACE2_TARGET_ENV {
        command.env(variable, TRACE_DISABLED);
    }
    command
}

fn has_stripped_prefix(variable: &OsStr) -> bool {
    let name = variable.as_encoded_bytes();
    STRIPPED_ENV_PREFIXES.iter().any(|prefix| {
        name.get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
    })
}
