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

fn lint_outputs_within_five_seconds(dir: &Path) -> [std::process::Output; 2] {
    [&["lint-context", "--json"][..], &["lint-context"][..]].map(|args| {
        assert_cmd::Command::from_std(aoa())
            .current_dir(dir)
            .args(args)
            .timeout(std::time::Duration::from_secs(5))
            .output()
            .expect("run lint-context")
    })
}

#[test]
fn lint_context_output_does_not_depend_on_an_ignore_file_above_the_linted_directory() {
    for name in [".gitignore", ".ignore"] {
        let dir = TempDir::new().expect("tempdir");
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).expect("create repo");
        std::fs::write(repo.join("AGENTS.md"), "# Root\n").expect("write root");
        let above = dir.path().join(name);

        let while_absent = lint_outputs_within_five_seconds(&repo);
        for output in &while_absent {
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(output.status.success(), "{name}: {stderr}");
        }

        make_fifo(&dir.path().join("outside"));
        std::os::unix::fs::symlink("outside", &above).expect("link to a fifo");
        let while_a_fifo = lint_outputs_within_five_seconds(&repo);
        assert_eq!(while_absent, while_a_fifo, "{name}");

        std::fs::remove_file(&above).expect("remove link");
        std::os::unix::fs::symlink("/dev/zero", &above).expect("link to an endless device");
        let while_a_device = lint_outputs_within_five_seconds(&repo);
        assert_eq!(while_absent, while_a_device, "{name}");

        std::fs::remove_file(&above).expect("remove link");
        std::fs::write(&above, "AGENTS.md\n").expect("write ignore file above");
        let while_a_file = lint_outputs_within_five_seconds(&repo);
        assert_eq!(while_absent, while_a_file, "{name}");

        std::fs::set_permissions(&above, std::fs::Permissions::from_mode(0o000))
            .expect("make the ignore file above unreadable");
        let while_unreadable = lint_outputs_within_five_seconds(&repo);
        assert_eq!(while_absent, while_unreadable, "{name}");
    }
}

#[test]
fn lint_context_refuses_an_ignore_file_that_is_a_fifo_instead_of_waiting_on_it() {
    for name in [".gitignore", ".ignore"] {
        let dir = TempDir::new().expect("tempdir");
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).expect("create repo");
        std::fs::write(repo.join("AGENTS.md"), "# Root\n").expect("write root");
        make_fifo(&repo.join(name));

        for output in lint_outputs_within_five_seconds(&repo) {
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(!output.status.success(), "{name}: {stderr}");
            assert!(output.stdout.is_empty(), "{name}");
            assert!(
                stderr.contains(&format!("{name} is not a regular file")),
                "{name}: {stderr}"
            );
        }
    }
}
