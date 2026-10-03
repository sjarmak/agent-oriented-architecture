use std::path::{Path, PathBuf};

use aoa_metrics::SubtreePartition;

use super::generated::GeneratedMarks;
use super::verification::{is_test_file, TEST_DIRS};
use super::{collect_source_line_counts, common_subtree, median, MIN_FILES_FOR_MEDIAN};
use crate::error::AuditError;
use crate::punch::{
    FindingKind, MeasuredCost, OutlierFile, OutlierGroup, PunchItem, SizeOutlierDetail,
    MAX_LISTED_OUTLIERS,
};
use crate::tier::Tier;

const TEST_SUPPORT_DIRS: &[&str] = &["specs", "e2e", "fixtures", "__fixtures__", "testdata"];

pub(super) struct SizeOutliers {
    median_lines: u64,
    largest_first: Vec<(PathBuf, u64)>,
}

impl SizeOutliers {
    pub(super) fn count(&self) -> u64 {
        self.largest_first.len() as u64
    }

    fn detail(&self, repo: &Path) -> SizeOutlierDetail {
        let (test, production): (Vec<_>, Vec<_>) = self
            .largest_first
            .iter()
            .map(|(path, lines)| (path.strip_prefix(repo).unwrap_or(path), *lines))
            .partition(|(relative, _)| is_test_path(relative));
        SizeOutlierDetail {
            median_lines: self.median_lines,
            production: outlier_group(production),
            test: outlier_group(test),
        }
    }
}

fn outlier_file(relative: &Path, lines: u64) -> OutlierFile {
    let path = relative
        .iter()
        .map(|component| component.to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    OutlierFile { path, lines }
}

fn outlier_group(largest_first: Vec<(&Path, u64)>) -> OutlierGroup {
    OutlierGroup {
        count: largest_first.len() as u64,
        largest: largest_first
            .into_iter()
            .take(MAX_LISTED_OUTLIERS)
            .map(|(relative, lines)| outlier_file(relative, lines))
            .collect(),
    }
}

fn is_test_path(relative: &Path) -> bool {
    let mut names = relative.iter().map(|component| component.to_string_lossy());
    let Some(file_name) = names.next_back() else {
        return false;
    };
    is_test_file(&file_name)
        || names.any(|dir| {
            TEST_DIRS.contains(&dir.as_ref()) || TEST_SUPPORT_DIRS.contains(&dir.as_ref())
        })
}

pub(super) fn module_size_outlier_item(
    repo: &Path,
    k: f64,
    partition: &SubtreePartition,
) -> Result<Option<PunchItem>, AuditError> {
    let Some(outliers) = module_size_outliers(repo, k)? else {
        return Ok(None);
    };
    if outliers.count() == 0 {
        return Ok(None);
    }

    Ok(Some(PunchItem {
        title: format!("source files exceeding {k:.1}x the repo median size"),
        kind: FindingKind::ModuleSizeOutlier,
        tier: Tier::Tier3,
        measured_cost: MeasuredCost::new(outliers.count(), "outlier files"),
        plane: None,
        subtree: common_subtree(
            partition,
            outliers.largest_first.iter().map(|(path, _)| path),
        ),
        size_outliers: Some(outliers.detail(repo)),
    }))
}

pub(super) fn module_size_outliers(
    repo: &Path,
    k: f64,
) -> Result<Option<SizeOutliers>, AuditError> {
    let mut walked: Vec<(PathBuf, u64)> = Vec::new();
    collect_source_line_counts(repo, &mut walked, 0)?;
    let mut marks = GeneratedMarks::new(repo);
    let mut files: Vec<(PathBuf, u64)> = Vec::with_capacity(walked.len());
    for (path, lines) in walked {
        if !marks.is_generated(&path)? {
            files.push((path, lines));
        }
    }

    if files.len() < MIN_FILES_FOR_MEDIAN {
        return Ok(None);
    }

    let mut line_counts: Vec<u64> = files.iter().map(|(_, n)| *n).collect();
    line_counts.sort_unstable();
    let median_lines = median(&line_counts);
    if median_lines == 0 {
        return Ok(None);
    }

    let threshold = median_lines as f64 * k;
    let mut largest_first: Vec<(PathBuf, u64)> = files
        .into_iter()
        .filter(|(_, n)| *n as f64 > threshold)
        .collect();
    largest_first.sort_by(|(a_path, a), (b_path, b)| b.cmp(a).then_with(|| a_path.cmp(b_path)));
    Ok(Some(SizeOutliers {
        median_lines,
        largest_first,
    }))
}

#[cfg(test)]
mod tests {
    use super::super::tests::{implicit, tmp};
    use super::*;
    use std::fs;

    fn lines(n: usize) -> String {
        "x\n".repeat(n)
    }

    fn write(root: &Path, relative: &str, n: usize) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, lines(n)).unwrap();
    }

    fn small_files(root: &Path, count: usize) {
        for i in 0..count {
            write(root, &format!("src/small_{i}.rs"), 10);
        }
    }

    fn detail(root: &Path) -> SizeOutlierDetail {
        module_size_outlier_item(root, 4.0, &implicit(root))
            .unwrap()
            .expect("outliers present")
            .size_outliers
            .expect("the size finding carries its itemization")
    }

    #[test]
    fn outliers_are_itemized_largest_first_with_repo_relative_paths() {
        let dir = tmp("size-itemized");
        small_files(&dir, 6);
        write(&dir, "src/big.rs", 100);
        write(&dir, "src/bigger.rs", 300);
        write(&dir, "src/deep/biggest.rs", 500);

        let detail = detail(&dir);
        assert_eq!(detail.median_lines, 10);
        assert_eq!(detail.production.count, 3);
        assert_eq!(
            detail.production.largest,
            vec![
                OutlierFile {
                    path: "src/deep/biggest.rs".into(),
                    lines: 500
                },
                OutlierFile {
                    path: "src/bigger.rs".into(),
                    lines: 300
                },
                OutlierFile {
                    path: "src/big.rs".into(),
                    lines: 100
                },
            ]
        );
        assert_eq!(detail.test.count, 0);
        assert!(detail.test.largest.is_empty());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn equal_sized_outliers_are_listed_in_path_order() {
        let dir = tmp("size-tiebreak");
        small_files(&dir, 6);
        write(&dir, "src/zeta.rs", 200);
        write(&dir, "src/alpha.rs", 200);

        let listed: Vec<String> = detail(&dir)
            .production
            .largest
            .into_iter()
            .map(|file| file.path)
            .collect();
        assert_eq!(listed, ["src/alpha.rs", "src/zeta.rs"]);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn test_and_production_outliers_are_counted_separately() {
        let dir = tmp("size-test-split");
        small_files(&dir, 20);
        write(&dir, "src/engine.rs", 400);
        write(&dir, "tests/acceptance.rs", 300);
        write(&dir, "web/__tests__/render.ts", 250);
        write(&dir, "web/button.spec.tsx", 240);
        write(&dir, "svc/handler_test.go", 230);
        write(&dir, "svc/testdata/sample.go", 220);
        write(&dir, "e2e/login.ts", 210);
        write(&dir, "web/fixtures/payload.js", 200);

        let item = module_size_outlier_item(&dir, 4.0, &implicit(&dir))
            .unwrap()
            .expect("outliers present");
        assert_eq!(item.measured_cost.value, 8);
        let detail = item.size_outliers.expect("itemized");
        assert_eq!(detail.production.count, 1);
        assert_eq!(detail.production.largest[0].path, "src/engine.rs");
        assert_eq!(detail.test.count, 7);
        assert_eq!(detail.test.largest[0].path, "tests/acceptance.rs");
        assert_eq!(detail.test.largest.len(), 7);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_test_named_directory_above_the_repo_root_does_not_classify_its_files() {
        let outer = tmp("size-outer-fixtures");
        let dir = outer.join("fixtures").join("repo");
        fs::create_dir_all(&dir).unwrap();
        small_files(&dir, 6);
        write(&dir, "src/engine.rs", 400);

        let detail = detail(&dir);
        assert_eq!(detail.production.count, 1);
        assert_eq!(detail.test.count, 0);
        fs::remove_dir_all(&outer).ok();
    }

    #[test]
    fn each_listing_is_bounded_while_its_count_stays_exact() {
        let dir = tmp("size-bounded");
        small_files(&dir, 2 * MAX_LISTED_OUTLIERS + 20);
        for i in 0..MAX_LISTED_OUTLIERS + 3 {
            write(&dir, &format!("src/big_{i}.rs"), 100 + i);
            write(&dir, &format!("tests/big_{i}.rs"), 100 + i);
        }

        let detail = detail(&dir);
        let expected = (MAX_LISTED_OUTLIERS + 3) as u64;
        for group in [&detail.production, &detail.test] {
            assert_eq!(group.count, expected);
            assert_eq!(group.largest.len(), MAX_LISTED_OUTLIERS);
            assert_eq!(group.largest[0].lines, 100 + MAX_LISTED_OUTLIERS as u64 + 2);
            assert!(group
                .largest
                .windows(2)
                .all(|pair| pair[0].lines >= pair[1].lines));
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn files_marked_linguist_generated_are_neither_counted_nor_listed() {
        let dir = tmp("size-generated");
        small_files(&dir, 6);
        write(&dir, "src/engine.rs", 400);
        write(&dir, "src/gen/schema.rs", 5000);
        write(&dir, "api/client.pb.go", 4000);
        fs::write(
            dir.join(".gitattributes"),
            "src/gen/** linguist-generated=true\n*.pb.go linguist-generated\n",
        )
        .unwrap();

        let item = module_size_outlier_item(&dir, 4.0, &implicit(&dir))
            .unwrap()
            .expect("the hand-written outlier remains");
        assert_eq!(item.measured_cost.value, 1);
        let detail = item.size_outliers.expect("itemized");
        assert_eq!(detail.production.count, 1);
        assert_eq!(detail.production.largest[0].path, "src/engine.rs");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn generated_files_do_not_shift_the_median() {
        let dir = tmp("size-generated-median");
        small_files(&dir, 5);
        write(&dir, "src/moderate.rs", 50);
        for i in 0..7 {
            write(&dir, &format!("gen/made_{i}.rs"), 1000);
        }
        fs::write(dir.join(".gitattributes"), "gen/** linguist-generated\n").unwrap();

        let detail = detail(&dir);
        assert_eq!(detail.median_lines, 10);
        assert_eq!(detail.production.largest[0].path, "src/moderate.rs");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_nested_attributes_file_marks_only_its_own_subtree() {
        let dir = tmp("size-generated-nested");
        small_files(&dir, 6);
        write(&dir, "pkg/out/bundle.js", 900);
        write(&dir, "other/out/bundle.js", 800);
        fs::write(
            dir.join("pkg").join(".gitattributes"),
            "out/** linguist-generated\n",
        )
        .unwrap();

        let detail = detail(&dir);
        assert_eq!(detail.production.count, 1);
        assert_eq!(detail.production.largest[0].path, "other/out/bundle.js");
        fs::remove_dir_all(&dir).ok();
    }
}
