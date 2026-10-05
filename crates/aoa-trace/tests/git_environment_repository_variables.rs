use std::ffi::OsStr;

use aoa_trace::git_free_of_inherited_state;

const REPOSITORY_VARIABLES_GIT_MUST_NOT_INHERIT: [&str; 17] = [
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

#[test]
fn removes_every_repository_variable_from_the_git_command() {
    let command = git_free_of_inherited_state();

    let kept: Vec<&str> = REPOSITORY_VARIABLES_GIT_MUST_NOT_INHERIT
        .into_iter()
        .filter(|variable| {
            !command
                .get_envs()
                .any(|(name, value)| name == OsStr::new(variable) && value.is_none())
        })
        .collect();

    assert!(
        kept.is_empty(),
        "git would still inherit from the caller: {kept:?}"
    );
}
