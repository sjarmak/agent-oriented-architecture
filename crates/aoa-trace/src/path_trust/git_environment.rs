use std::process::Command;

const REPOSITORY_LOCAL_GIT_ENV: [&str; 17] = [
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
];

const TRACE_ENV_PREFIX: &[u8] = b"GIT_TRACE";

const TRACE2_TARGET_ENV: [&str; 3] = ["GIT_TRACE2", "GIT_TRACE2_EVENT", "GIT_TRACE2_PERF"];

const TRACE_DISABLED: &str = "0";

pub fn git_free_of_inherited_state() -> Command {
    let mut command = Command::new("git");
    for variable in REPOSITORY_LOCAL_GIT_ENV {
        command.env_remove(variable);
    }
    for (variable, _) in std::env::vars_os() {
        let traces = variable
            .as_encoded_bytes()
            .get(..TRACE_ENV_PREFIX.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(TRACE_ENV_PREFIX));
        if traces {
            command.env_remove(variable);
        }
    }
    for variable in TRACE2_TARGET_ENV {
        command.env(variable, TRACE_DISABLED);
    }
    command
}
