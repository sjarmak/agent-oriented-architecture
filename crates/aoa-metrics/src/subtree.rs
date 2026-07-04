//! Mechanical subtree partitioning for monorepos.
//!
//! Partitions a repo into subtrees using **fixed well-known workspace
//! manifests only** — no semantic clustering, no "looks like an app"
//! judgment (ZFC). Detection order is fixed and documented:
//!
//! 1. `Cargo.toml` `[workspace].members` (honoring `exclude`)
//! 2. `go.work` `use` directives
//! 3. `pnpm-workspace.yaml` `packages`
//! 4. `package.json` `workspaces` (array or `{ "packages": [...] }` object)
//!
//! The first manifest that yields at least one member wins. A repo with none
//! of them is a single implicit root subtree (`.`). Glob patterns (`*`, `?`,
//! `[..]`, `**`) are expanded against the filesystem; a glob-expanded member
//! must contain its ecosystem manifest (`Cargo.toml` / `package.json`),
//! matching cargo/npm semantics. Hidden directories are never glob-matched.
//! Patterns starting with `!` are exclusions, applied after expansion.
//!
//! Attribution is longest-prefix match on `/`-separated relative paths.
//! Members are deduplicated, so two distinct prefixes of the same length can
//! never both match one path — the longest-match rule is a total, deterministic
//! tiebreak. Nested members (`app`, `app/plugin`) attribute to the deepest one.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use globset::{GlobBuilder, GlobMatcher};
use serde::{Deserialize, Serialize};

/// Reject pathological manifest files; real workspace manifests are tiny.
const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;

/// A subtree-partitioning failure: an unreadable or malformed manifest.
///
/// "No workspace manifest present" is NOT an error — that is the implicit-root
/// partition. Errors are reserved for manifests that exist but cannot be used.
#[derive(Debug, thiserror::Error)]
pub enum SubtreeError {
    #[error("failed to read {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("malformed workspace manifest {path}: {message}")]
    Manifest { path: String, message: String },
}

/// Which well-known manifest produced the partition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceSource {
    CargoWorkspace,
    GoWork,
    PnpmWorkspace,
    NpmWorkspaces,
    /// No workspace manifest found: the whole repo is one subtree (`.`).
    ImplicitRoot,
}

impl WorkspaceSource {
    /// Human label for log lines (the JSON form is the serde snake_case name).
    pub fn label(self) -> &'static str {
        match self {
            WorkspaceSource::CargoWorkspace => "Cargo.toml [workspace] members",
            WorkspaceSource::GoWork => "go.work use directives",
            WorkspaceSource::PnpmWorkspace => "pnpm-workspace.yaml packages",
            WorkspaceSource::NpmWorkspaces => "package.json workspaces",
            WorkspaceSource::ImplicitRoot => "implicit root (no workspace manifest)",
        }
    }
}

/// A repo partitioned into workspace-member subtrees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubtreePartition {
    /// The repo root's display path, used to relativize absolute span paths.
    repo_root: String,
    /// Member dirs relative to the root, deduplicated and sorted longest-first
    /// (then lexicographic) so the first prefix match is the longest match.
    members: Vec<String>,
    source: WorkspaceSource,
}

impl SubtreePartition {
    fn new(repo_root: &Path, members: Vec<String>, source: WorkspaceSource) -> Self {
        let dedup: BTreeSet<String> = members.into_iter().collect();
        let mut members: Vec<String> = dedup.into_iter().collect();
        // The root sentinel `.` matches every path, so it must be evaluated
        // strictly last — by explicit rule, not by length, or a one-character
        // member like `x` would lose the length tie and misattribute to `.`.
        members.sort_by(|a, b| {
            (a == ".")
                .cmp(&(b == "."))
                .then_with(|| b.len().cmp(&a.len()))
                .then_with(|| a.cmp(b))
        });
        SubtreePartition {
            repo_root: repo_root.display().to_string(),
            members,
            source,
        }
    }

    /// The single-subtree partition for a repo with no workspace manifest.
    pub fn implicit_root(repo_root: &Path) -> Self {
        Self::new(
            repo_root,
            vec![".".to_string()],
            WorkspaceSource::ImplicitRoot,
        )
    }

    pub fn source(&self) -> WorkspaceSource {
        self.source
    }

    /// Member dirs, longest-first then lexicographic (attribution order).
    pub fn members(&self) -> &[String] {
        &self.members
    }

    /// Whether per-subtree reporting is meaningful (more than one member).
    pub fn is_partitioned(&self) -> bool {
        self.members.len() > 1
    }

    /// Attribute a file path to its subtree by longest-prefix match.
    ///
    /// The path is relativized first: a repo-root prefix and a leading `./`
    /// are stripped. Prefixes match only on `/` segment boundaries, so member
    /// `app` never claims `app-other/...`. The root member `.` (go.work style)
    /// matches every relative path, but only after every deeper member has had
    /// its chance. Returns `None` for paths outside all members.
    pub fn attribute(&self, path: &str) -> Option<&str> {
        let rel = self.relativize(path);
        // An absolute path outside the repo root attributes to nothing.
        if rel.starts_with('/') {
            return None;
        }
        self.members
            .iter()
            .find(|m| {
                m.as_str() == "."
                    || rel == m.as_str()
                    || (rel.starts_with(m.as_str()) && rel.as_bytes().get(m.len()) == Some(&b'/'))
            })
            .map(String::as_str)
    }

    fn relativize<'p>(&self, path: &'p str) -> &'p str {
        let rel = path
            .strip_prefix(&self.repo_root)
            .and_then(|rest| rest.strip_prefix('/'))
            .unwrap_or(path);
        rel.strip_prefix("./").unwrap_or(rel)
    }
}

/// Discover the subtree partition of a repo from its workspace manifests.
///
/// Tries the fixed manifest order documented at module level; falls back to
/// the implicit root partition when no manifest yields members. Errors only
/// on a manifest that exists but is unreadable or malformed — callers should
/// surface that and fall back to repo-wide reporting, never guess.
pub fn discover_partition(repo_root: &Path) -> Result<SubtreePartition, SubtreeError> {
    type Detector = fn(&Path) -> Result<Option<Vec<String>>, SubtreeError>;
    const DETECTORS: [(Detector, WorkspaceSource); 4] = [
        (cargo_members, WorkspaceSource::CargoWorkspace),
        (go_work_members, WorkspaceSource::GoWork),
        (pnpm_members, WorkspaceSource::PnpmWorkspace),
        (npm_members, WorkspaceSource::NpmWorkspaces),
    ];
    for (detect, source) in DETECTORS {
        if let Some(members) = detect(repo_root)? {
            return Ok(SubtreePartition::new(repo_root, members, source));
        }
    }
    Ok(SubtreePartition::implicit_root(repo_root))
}

/// Read a manifest if it exists; `Ok(None)` when absent.
///
/// A manifest checked into the repo as a symlink is rejected, never followed:
/// the repo under analysis is untrusted, and a followed link would read (and,
/// via parse-error messages, disclose) any file the scanning process can see.
/// This mirrors the no-follow discipline `child_dirs` applies to directories.
fn read_manifest(path: &Path) -> Result<Option<String>, SubtreeError> {
    use std::io::Read;
    let io_err = |source| SubtreeError::Io {
        path: path.display().to_string(),
        source,
    };
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io_err(e)),
    };
    if !meta.is_file() {
        return Err(SubtreeError::Manifest {
            path: path.display().to_string(),
            message: "not a regular file (symlinked manifests are not followed)".to_string(),
        });
    }
    // Bounded read on the open handle (no metadata-then-read TOCTOU window),
    // one byte past the cap so oversize is detectable without reading it all.
    let mut raw = String::new();
    let file = fs::File::open(path).map_err(io_err)?;
    file.take(MAX_MANIFEST_BYTES + 1)
        .read_to_string(&mut raw)
        .map_err(io_err)?;
    if raw.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(SubtreeError::Manifest {
            path: path.display().to_string(),
            message: format!("file exceeds {MAX_MANIFEST_BYTES} bytes"),
        });
    }
    Ok(Some(raw))
}

fn manifest_err(path: &Path, message: impl ToString) -> SubtreeError {
    SubtreeError::Manifest {
        path: path.display().to_string(),
        message: message.to_string(),
    }
}

/// `Cargo.toml` `[workspace].members`, expanded, minus `[workspace].exclude`.
fn cargo_members(root: &Path) -> Result<Option<Vec<String>>, SubtreeError> {
    let path = root.join("Cargo.toml");
    let Some(raw) = read_manifest(&path)? else {
        return Ok(None);
    };
    let value: toml::Table = toml::from_str(&raw).map_err(|e| manifest_err(&path, e))?;
    let Some(workspace) = value.get("workspace") else {
        return Ok(None); // a plain package manifest, not a workspace
    };
    let patterns = toml_string_array(workspace.get("members"));
    if patterns.is_empty() {
        return Ok(None);
    }
    let excludes = toml_string_array(workspace.get("exclude"));
    let members = expand_members(root, &path, &patterns, &excludes, "Cargo.toml")?;
    Ok((!members.is_empty()).then_some(members))
}

fn toml_string_array(value: Option<&toml::Value>) -> Vec<String> {
    value
        .and_then(toml::Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// `go.work` `use` directives: `use ./dir` or a `use ( ... )` block.
/// go.work has no globs; comments (`//`) are stripped mechanically.
fn go_work_members(root: &Path) -> Result<Option<Vec<String>>, SubtreeError> {
    let path = root.join("go.work");
    let Some(raw) = read_manifest(&path)? else {
        return Ok(None);
    };
    let mut members = Vec::new();
    let mut in_block = false;
    for line in raw.lines() {
        let line = line.split("//").next().unwrap_or("").trim();
        if in_block {
            if line == ")" {
                in_block = false;
            } else if !line.is_empty() {
                members.push(clean_member(line));
            }
        } else if let Some(rest) = line.strip_prefix("use") {
            // Require a word boundary so e.g. a `used` token is not a directive.
            let Some(first) = rest.chars().next() else {
                continue;
            };
            if first != '(' && !first.is_whitespace() {
                continue;
            }
            let rest = rest.trim();
            if rest == "(" {
                in_block = true;
            } else if !rest.is_empty() {
                members.push(clean_member(rest));
            }
        }
    }
    // go.work legitimately allows `use ../outside`, but a member outside the
    // repo root is meaningless as a subtree; reject and let the caller fall
    // back to repo-wide reporting rather than emit an escaping label.
    for member in &members {
        validate_member(&path, member)?;
    }
    Ok((!members.is_empty()).then_some(members))
}

/// `pnpm-workspace.yaml` `packages` patterns (with `!` exclusions).
fn pnpm_members(root: &Path) -> Result<Option<Vec<String>>, SubtreeError> {
    let path = root.join("pnpm-workspace.yaml");
    let Some(raw) = read_manifest(&path)? else {
        return Ok(None);
    };
    let value: serde_norway::Value =
        serde_norway::from_str(&raw).map_err(|e| manifest_err(&path, e))?;
    let patterns: Vec<String> = value
        .get("packages")
        .and_then(serde_norway::Value::as_sequence)
        .map(|seq| {
            seq.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    expand_npm_style(root, &path, patterns)
}

/// `package.json` `workspaces`: an array of patterns or `{ "packages": [...] }`.
fn npm_members(root: &Path) -> Result<Option<Vec<String>>, SubtreeError> {
    let path = root.join("package.json");
    let Some(raw) = read_manifest(&path)? else {
        return Ok(None);
    };
    let value: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| manifest_err(&path, e))?;
    let workspaces = match value.get("workspaces") {
        Some(serde_json::Value::Array(arr)) => arr.as_slice(),
        Some(serde_json::Value::Object(obj)) => obj
            .get("packages")
            .and_then(|p| p.as_array())
            .map(Vec::as_slice)
            .unwrap_or_default(),
        _ => return Ok(None),
    };
    let patterns: Vec<String> = workspaces
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    expand_npm_style(root, &path, patterns)
}

/// Shared npm/pnpm expansion: split `!` exclusions, require `package.json`.
fn expand_npm_style(
    root: &Path,
    manifest: &Path,
    patterns: Vec<String>,
) -> Result<Option<Vec<String>>, SubtreeError> {
    let (includes, excludes): (Vec<String>, Vec<String>) =
        patterns.into_iter().partition(|p| !p.starts_with('!'));
    if includes.is_empty() {
        return Ok(None);
    }
    let excludes: Vec<String> = excludes.iter().map(|p| p[1..].to_string()).collect();
    let members = expand_members(root, manifest, &includes, &excludes, "package.json")?;
    Ok((!members.is_empty()).then_some(members))
}

/// Expand member patterns against the filesystem and apply exclusions.
///
/// Literal (glob-free) patterns are taken as declared. Glob patterns expand to
/// existing directories that contain `required_manifest`, per cargo/npm
/// semantics. Exclusions remove exact paths and glob matches.
fn expand_members(
    root: &Path,
    manifest: &Path,
    includes: &[String],
    excludes: &[String],
    required_manifest: &str,
) -> Result<Vec<String>, SubtreeError> {
    let mut members: BTreeSet<String> = BTreeSet::new();
    for pattern in includes {
        let pattern = clean_member(pattern);
        if !has_glob(&pattern) {
            validate_member(manifest, &pattern)?;
            members.insert(pattern);
            continue;
        }
        for dir in expand_glob_dirs(root, manifest, &pattern)? {
            if root.join(&dir).join(required_manifest).is_file() {
                members.insert(dir);
            }
        }
    }

    let mut matchers: Vec<GlobMatcher> = Vec::new();
    let mut literals: BTreeSet<String> = BTreeSet::new();
    for pattern in excludes {
        let pattern = clean_member(pattern);
        if has_glob(&pattern) {
            matchers.push(segment_glob(manifest, &pattern)?);
        } else {
            literals.insert(pattern);
        }
    }
    Ok(members
        .into_iter()
        .filter(|m| !literals.contains(m) && !matchers.iter().any(|g| g.is_match(m)))
        .collect())
}

/// Reject a declared member path that points outside the repo root.
///
/// Members are attribution labels and are never joined back onto the
/// filesystem here, but an absolute or `..`-carrying member is a latent
/// traversal primitive for any future consumer and lets a hostile manifest
/// inject arbitrary subtree labels into reports. Cargo itself rejects
/// out-of-workspace members, so erroring matches ecosystem semantics; the
/// caller degrades to repo-wide reporting with a visible warning.
fn validate_member(manifest: &Path, member: &str) -> Result<(), SubtreeError> {
    if member.starts_with('/') || member.split('/').any(|seg| seg == "..") {
        return Err(manifest_err(
            manifest,
            format!("member path escapes the repo root: {member:?}"),
        ));
    }
    Ok(())
}

/// Normalize a declared member path: trim, strip quotes, `./`, trailing `/`.
fn clean_member(raw: &str) -> String {
    let p = raw.trim().trim_matches('"').trim_matches('\'');
    let p = p.strip_prefix("./").unwrap_or(p);
    p.trim_end_matches('/').to_string()
}

fn has_glob(pattern: &str) -> bool {
    pattern.contains(['*', '?', '['])
}

/// A glob whose `*` stays within one path segment (`**` still crosses).
fn segment_glob(manifest: &Path, pattern: &str) -> Result<GlobMatcher, SubtreeError> {
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map(|g| g.compile_matcher())
        .map_err(|e| manifest_err(manifest, format!("bad glob {pattern:?}: {e}")))
}

/// Expand a glob pattern to existing directories, one `/`-segment at a time.
///
/// `**` descends recursively (matching zero or more segments); other glob
/// segments match direct child directories. Hidden directories are skipped.
/// Results are sorted for determinism.
fn expand_glob_dirs(
    root: &Path,
    manifest: &Path,
    pattern: &str,
) -> Result<Vec<String>, SubtreeError> {
    let mut frontier: Vec<String> = vec![String::new()];
    for segment in pattern.split('/') {
        let mut next: Vec<String> = Vec::new();
        if segment == "**" {
            for base in &frontier {
                if !base.is_empty() {
                    next.push(base.clone()); // `**` matches zero segments
                }
                descendant_dirs(root, base, 0, &mut next)?;
            }
        } else if has_glob(segment) {
            let matcher = segment_glob(manifest, segment)?;
            for base in &frontier {
                for child in child_dirs(root, base)? {
                    if matcher.is_match(&child) {
                        next.push(join_rel(base, &child));
                    }
                }
            }
        } else {
            for base in &frontier {
                let candidate = join_rel(base, segment);
                if root.join(&candidate).is_dir() {
                    next.push(candidate);
                }
            }
        }
        frontier = next;
        frontier.sort_unstable();
        frontier.dedup();
        if frontier.is_empty() {
            break;
        }
    }
    Ok(frontier)
}

fn join_rel(base: &str, child: &str) -> String {
    if base.is_empty() {
        child.to_string()
    } else {
        format!("{base}/{child}")
    }
}

/// Non-hidden direct child directory names of `root/base`, sorted.
fn child_dirs(root: &Path, base: &str) -> Result<Vec<String>, SubtreeError> {
    let dir = root.join(base);
    let io_err = |source| SubtreeError::Io {
        path: dir.display().to_string(),
        source,
    };
    let mut out = Vec::new();
    for entry in fs::read_dir(&dir).map_err(io_err)? {
        let entry = entry.map_err(io_err)?;
        // file_type() does not follow symlinks: a symlinked dir is skipped, so
        // a link pointing outside the repo cannot widen the partition.
        if !entry.file_type().map_err(io_err)?.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue; // non-UTF-8 names cannot match string members
        };
        if name.starts_with('.') {
            continue;
        }
        out.push(name.to_string());
    }
    out.sort_unstable();
    Ok(out)
}

/// Defense-in-depth recursion cap for `**` expansion, mirroring the walk cap
/// aoa-audit applies to every recursive directory walk: a pathologically deep
/// tree degrades to a truncated expansion instead of overflowing the stack.
const MAX_WALK_DEPTH: usize = 64;

/// All non-hidden descendant directories of `root/base` (relative to `root`).
fn descendant_dirs(
    root: &Path,
    base: &str,
    depth: usize,
    out: &mut Vec<String>,
) -> Result<(), SubtreeError> {
    if depth >= MAX_WALK_DEPTH {
        return Ok(());
    }
    for child in child_dirs(root, base)? {
        let rel = join_rel(base, &child);
        descendant_dirs(root, &rel, depth + 1, out)?;
        out.push(rel);
    }
    Ok(())
}
