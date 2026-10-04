#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;

use super::audit_observe_lint::lint_outputs;
use super::*;

#[test]
fn lint_context_output_does_not_depend_on_what_an_ignore_file_link_leaving_the_directory_reaches() {
    for name in [".gitignore", ".ignore"] {
        let dir = TempDir::new().expect("tempdir");
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).expect("create repo");
        std::fs::write(repo.join("AGENTS.md"), "# Root\n").expect("write root");
        std::os::unix::fs::symlink("../probed", repo.join(name)).expect("link the ignore file");
        let probed = dir.path().join("probed");

        let while_absent = lint_outputs(&repo);
        std::fs::write(&probed, "AGENTS.md\n").expect("write outside file");
        let while_a_file = lint_outputs(&repo);
        std::fs::set_permissions(&probed, std::fs::Permissions::from_mode(0o000))
            .expect("make the outside file unreadable");
        let while_unreadable = lint_outputs(&repo);
        std::fs::remove_file(&probed).expect("remove outside file");
        std::fs::create_dir(&probed).expect("create outside directory");
        let while_a_directory = lint_outputs(&repo);

        assert_eq!(while_absent, while_a_file, "{name}");
        assert_eq!(while_absent, while_unreadable, "{name}");
        assert_eq!(while_absent, while_a_directory, "{name}");
        for output in &while_absent {
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(!output.status.success(), "{name}: {stderr}");
            assert!(output.stdout.is_empty(), "{name}");
            assert!(
                stderr.contains(&format!("{name} is a link that leaves")),
                "{name}: {stderr}"
            );
        }
    }
}
