use std::collections::BTreeMap;
use std::ffi::OsStr;

use serde::Serialize;

pub(crate) const INDEXED_LANGUAGE: &str = "Python";

const NEGLIGIBLE_SHARE_DENOMINATOR: usize = 10;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct GraphCoverage {
    pub indexed: BTreeMap<String, usize>,
    pub unindexed: BTreeMap<String, usize>,
}

impl GraphCoverage {
    pub(crate) fn record_indexed(&mut self, language: &str) {
        *self.indexed.entry(language.to_string()).or_default() += 1;
    }

    pub(crate) fn record_unindexed(&mut self, language: &str) {
        *self.unindexed.entry(language.to_string()).or_default() += 1;
    }

    #[must_use]
    pub fn indexed_files(&self) -> usize {
        self.indexed.values().sum()
    }

    #[must_use]
    pub fn source_files(&self) -> usize {
        self.indexed_files() + self.unindexed.values().sum::<usize>()
    }

    #[must_use]
    pub fn is_negligible(&self) -> bool {
        self.indexed_files() * NEGLIGIBLE_SHARE_DENOMINATOR < self.source_files()
    }

    #[must_use]
    pub fn summary(&self) -> String {
        let total = self.source_files();
        let indexed = if self.indexed.is_empty() {
            format!("0 of {total} source files indexed")
        } else {
            let languages = self
                .indexed
                .iter()
                .map(|(language, count)| format!("{language} {count}"))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{languages} of {total} source files indexed")
        };
        if self.unindexed.is_empty() {
            return indexed;
        }
        let mut skipped: Vec<(&String, &usize)> = self.unindexed.iter().collect();
        skipped.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        let skipped = skipped
            .into_iter()
            .map(|(language, count)| {
                let noun = if *count == 1 { "file" } else { "files" };
                format!("{language} ({count} {noun})")
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("{indexed}; not indexed: {skipped}")
    }

    #[must_use]
    pub fn unindexed_notice(&self) -> String {
        format!(
            "best-effort graph indexes {INDEXED_LANGUAGE} only: {}",
            self.summary()
        )
    }

    pub(crate) fn degrade_reason(&self) -> String {
        format!(
            "{}; pass --scip-index <file> for a weighted graph",
            self.unindexed_notice()
        )
    }
}

pub(crate) fn language_of(extension: &OsStr) -> Option<&'static str> {
    let language = match extension.to_str()? {
        "py" => INDEXED_LANGUAGE,
        "ts" | "tsx" | "mts" | "cts" => "TypeScript",
        "js" | "jsx" | "mjs" | "cjs" => "JavaScript",
        "kt" | "kts" => "Kotlin",
        "java" => "Java",
        "rs" => "Rust",
        "go" => "Go",
        "c" | "h" => "C",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" => "C++",
        "cs" => "C#",
        "rb" => "Ruby",
        "php" => "PHP",
        "swift" => "Swift",
        "scala" => "Scala",
        _ => return None,
    };
    Some(language)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn coverage(indexed: usize, unindexed: &[(&str, usize)]) -> GraphCoverage {
        let mut coverage = GraphCoverage::default();
        for _ in 0..indexed {
            coverage.record_indexed(INDEXED_LANGUAGE);
        }
        for (language, count) in unindexed {
            for _ in 0..*count {
                coverage.record_unindexed(language);
            }
        }
        coverage
    }

    #[test]
    fn a_repo_with_no_source_files_is_not_negligible_coverage() {
        assert!(!GraphCoverage::default().is_negligible());
    }

    #[test]
    fn coverage_is_negligible_below_a_tenth_of_the_source_files() {
        assert!(coverage(1, &[("TypeScript", 100)]).is_negligible());
        assert!(coverage(0, &[("TypeScript", 1)]).is_negligible());
        assert!(!coverage(1, &[("TypeScript", 9)]).is_negligible());
        assert!(coverage(1, &[("TypeScript", 10)]).is_negligible());
        assert!(!coverage(2, &[]).is_negligible());
    }

    #[test]
    fn summary_leads_with_the_largest_unindexed_language() {
        let summary = coverage(1, &[("Kotlin", 1), ("TypeScript", 100)]).summary();
        assert_eq!(
            summary,
            "Python 1 of 102 source files indexed; \
             not indexed: TypeScript (100 files), Kotlin (1 file)"
        );
    }

    #[test]
    fn summary_of_a_fully_indexed_repo_lists_nothing_skipped() {
        assert_eq!(
            coverage(2, &[]).summary(),
            "Python 2 of 2 source files indexed"
        );
    }

    #[test]
    fn degrade_reason_names_the_language_and_the_way_out() {
        let reason = coverage(0, &[("TypeScript", 5)]).degrade_reason();
        assert!(reason.contains("0 of 5 source files indexed"), "{reason}");
        assert!(reason.contains("TypeScript (5 files)"), "{reason}");
        assert!(reason.contains("--scip-index"), "{reason}");
    }

    #[test]
    fn unindexed_notice_recommends_no_flag() {
        assert_eq!(
            coverage(0, &[("TypeScript", 5)]).unindexed_notice(),
            "best-effort graph indexes Python only: 0 of 5 source files indexed; \
             not indexed: TypeScript (5 files)"
        );
    }

    #[test]
    fn extensions_map_to_their_language() {
        assert_eq!(language_of(OsStr::new("py")), Some("Python"));
        assert_eq!(language_of(OsStr::new("tsx")), Some("TypeScript"));
        assert_eq!(language_of(OsStr::new("kt")), Some("Kotlin"));
        assert_eq!(language_of(OsStr::new("md")), None);
    }
}
