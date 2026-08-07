//! `architecture/model.c4` is a maintained contract, not a snapshot (aoa-enzj8).
//!
//! The model draws every crate in the workspace and the dependencies between
//! them, and it is published as a website. Nothing read it, so it drifted: it
//! asserted `aoa-recommend -> aoa-migrate` for months after aoa-4s25v removed
//! that edge, drew three more dependencies that never existed, and had no
//! component for `aoa-domain` at all. `cargo build` does not read a diagram,
//! and a reviewer who trusts a wrong diagram is worse off than one who has
//! none.
//!
//! This file and [`architecture_doc`] hold two different documents to the same
//! crates and are not variants of each other. `architecture_doc` owns CLAUDE.md
//! layer *membership and direction* — which layer a crate is in, and that no
//! crate depends on a later one. This file owns model-to-manifest *soundness
//! and completeness* — that every arrow is a real dependency and every real
//! dependency between drawn library crates is an arrow. A crate can sit in the
//! right layer and still be drawn with an edge it does not have.
//!
//! The contract, stated once here and once in the model's own header:
//!
//! - **Membership.** Every crate under `crates/` is exactly one element.
//! - **Soundness.** Every relationship whose two ends both resolve to crates is
//!   a production dependency of the source on the target.
//! - **Completeness, library-to-library only.** Every production dependency
//!   between two library crates is drawn. This is what stops the cheap repair —
//!   deleting an arrow rather than fixing the code it describes.
//! - **The CLI is the one asymmetry.** `aoa` depends on all eighteen libraries,
//!   so drawing every one of its edges would say nothing. Its arrows are drawn
//!   where they carry meaning and are checked for truth, never for
//!   exhaustiveness.
//! - **`#conceptual`** marks a relationship that is deliberately not a code
//!   edge. Each one is registered in [`CONCEPTUAL_EDGES`] with a reason, and
//!   [`conceptual_edges_are_registered_and_still_conceptual`] deletes the
//!   licence the moment the edge becomes real.
//!
//! `views.c4` is held to a weaker rule on purpose, and the asymmetry is the
//! point rather than an omission: its dynamic views are walkthroughs of what
//! happens at runtime, not claims about the build graph. `operator -> falsify`
//! and `audit -> operator` are steps in the same file, and no manifest will
//! ever back them. So the views are checked for the one thing they do owe —
//! that every endpoint names an element that exists — and their flow steps are
//! left to say what a run actually does.

mod common;

use common::{declared_dependencies, library_crates, workspace_root, CLI_CRATE};
use std::collections::{BTreeMap, BTreeSet};

/// The decision record behind this test, and the bead that produced it. Both
/// are asserted to appear in the model itself by
/// [`the_model_carries_its_own_decision_record`]: a contract whose terms live
/// only in a test file is one a contributor edits without ever meeting.
const DECISION_RECORD: &str = "docs/adr/0006-architecture-model-conformance.md";
const BEAD: &str = "aoa-enzj8";

/// Relationships that are deliberately not code edges, each with the reason it
/// is one. An entry is a licence for an arrow to exist with no dependency
/// behind it, so it names what the arrow means instead.
const CONCEPTUAL_EDGES: &[(&str, &str, &str)] = &[(
    "aoa-falsify",
    "aoa-migrate",
    "R0 decides whether migrate is worth trusting on a repository at all. The \
     verdict is read by an operator, not called by a crate — wiring it as a \
     dependency is the thing the gate exists to prevent",
)];

// ───────────────────────────────────────────────────────────────────────────
// The model.
// ───────────────────────────────────────────────────────────────────────────

/// One `.c4` element and the source links it declares.
struct Element {
    fqn: String,
    links: Vec<String>,
}

/// One `a -> b` relationship and the tags on it.
struct Relationship {
    source: String,
    target: String,
    tags: BTreeSet<String>,
    line: usize,
}

struct Model {
    elements: Vec<Element>,
    relationships: Vec<Relationship>,
}

impl Model {
    /// Which element declares a link into each crate's directory.
    ///
    /// Resolution is by path *component*, never by string prefix:
    /// `../crates/aoa` is a prefix of `../crates/aoa-metrics/src/lib.rs`, so a
    /// prefix match would have the CLI container claim `aoa-metrics` and report
    /// a duplicate that is not there. An element may declare several links into
    /// the same crate — the CLI names both `cli.rs` and `main.rs` — and claims
    /// it once.
    fn crate_owners(&self) -> BTreeMap<String, &str> {
        let mut owners: BTreeMap<String, &str> = BTreeMap::new();
        let mut duplicates: Vec<String> = Vec::new();

        for element in &self.elements {
            for name in element.linked_crates() {
                match owners.entry(name) {
                    std::collections::btree_map::Entry::Vacant(slot) => {
                        slot.insert(element.fqn.as_str());
                    }
                    std::collections::btree_map::Entry::Occupied(slot) => {
                        if *slot.get() != element.fqn {
                            duplicates.push(format!(
                                "{} is claimed by both {} and {}",
                                slot.key(),
                                slot.get(),
                                element.fqn
                            ));
                        }
                    }
                }
            }
        }

        assert!(
            duplicates.is_empty(),
            "two elements link the same crate, so an arrow at either one is \
             ambiguous about what it claims: {duplicates:?}"
        );
        owners
    }

    /// The crate a relationship endpoint stands for, if any.
    ///
    /// An element that links a crate directly is that crate. A grouping element
    /// resolves to the single crate in its subtree when there is exactly one:
    /// `aoa.migrate` holds only `aoa.migrate.migrator`, so an arrow drawn at
    /// the container claims exactly what an arrow at the component would, and
    /// checking only the second would leave the first as a way around this
    /// test. A grouping with several crates under it — `aoa.substrate`,
    /// `aoa.measure` — stands for no single crate and is left alone.
    fn resolve<'a>(&'a self, fqn: &str, owners: &BTreeMap<String, &'a str>) -> Option<String> {
        if let Some((name, _)) = owners.iter().find(|(_, owner)| **owner == fqn) {
            return Some(name.clone());
        }

        let prefix = format!("{fqn}.");
        let mut under: Vec<String> = owners
            .iter()
            .filter(|(_, owner)| owner.starts_with(&prefix))
            .map(|(name, _)| name.clone())
            .collect();
        match under.len() {
            1 => under.pop(),
            _ => None,
        }
    }
}

impl Element {
    fn linked_crates(&self) -> BTreeSet<String> {
        self.links
            .iter()
            .filter_map(|link| link.strip_prefix("../crates/"))
            .filter_map(|rest| rest.split('/').next())
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .collect()
    }
}

impl Relationship {
    fn is_conceptual(&self) -> bool {
        self.tags.contains("conceptual")
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Parsing.
// ───────────────────────────────────────────────────────────────────────────

/// Each line with its string literals emptied and full-line comments dropped,
/// paired with its 1-based number.
///
/// Emptying the literals first is what makes everything downstream a plain
/// string match: `-> `, `{`, `}` and `#` all appear inside descriptions in this
/// file, and a scan that did not know where a string started would find them
/// there. A `//` is a comment only at the start of a line, because `//` also
/// appears mid-line inside the `https://` link on the CodeProbe element.
///
/// Two restrictions are enforced rather than handled, because both would make
/// the classifier below quietly wrong rather than loudly broken: a string may
/// not span lines, and a statement may not.
fn sanitized_lines(source: &str, path: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();

    for (index, raw) in source.lines().enumerate() {
        let line = index + 1;
        if raw.trim_start().starts_with("//") {
            continue;
        }

        let mut sanitized = String::with_capacity(raw.len());
        let mut in_string = false;
        for c in raw.chars() {
            match c {
                '\'' => {
                    in_string = !in_string;
                    sanitized.push('\'');
                }
                _ if in_string => {}
                _ => sanitized.push(c),
            }
        }
        assert!(
            !in_string,
            "{path}:{line}: a string literal is left open at end of line. \
             This parser does not read multi-line strings, and reading this one \
             as if it ended would silently mis-classify the rest of the file"
        );

        out.push((line, sanitized));
    }

    out
}

/// Parse `architecture/model.c4` into elements and relationships.
///
/// Every statement is classified against a whitelist and anything unrecognized
/// is a panic. That is the difference between a parser and a scanner that
/// reports green on a file it did not understand: a new `.c4` construct would
/// otherwise be read as "no elements here", and a test that checks nothing
/// passes.
fn parse_model(source: &str, path: &str) -> Model {
    /// What the enclosing `{` opened. Only `Element` contributes to a fully
    /// qualified name; a relationship's trailing block holds tags and must not
    /// be mistaken for a nested element, or every name after it is wrong.
    enum Scope {
        Anonymous,
        Element(String),
        Relationship(usize),
    }

    let mut model = Model {
        elements: Vec::new(),
        relationships: Vec::new(),
    };
    let mut scopes: Vec<Scope> = Vec::new();

    for (line, sanitized) in sanitized_lines(source, path) {
        let statement = sanitized.trim();
        if statement.is_empty() {
            continue;
        }

        if statement == "}" {
            assert!(
                scopes.pop().is_some(),
                "{path}:{line}: a closing brace with nothing open"
            );
            continue;
        }

        if statement.starts_with('#') {
            let tag = statement.trim_start_matches('#').to_owned();
            if let Some(Scope::Relationship(index)) = scopes.last() {
                model.relationships[*index].tags.insert(tag);
            }
            continue;
        }

        if let Some((source_fqn, rest)) = statement.split_once("->") {
            let source_fqn = source_fqn.trim();
            let mut rest = rest.split_whitespace();
            let target_fqn = rest
                .next()
                .unwrap_or_else(|| panic!("{path}:{line}: a relationship with no target"));
            let opens_block = rest.clone().last() == Some("{");

            model.relationships.push(Relationship {
                source: source_fqn.to_owned(),
                target: target_fqn.to_owned(),
                tags: BTreeSet::new(),
                line,
            });
            if opens_block {
                scopes.push(Scope::Relationship(model.relationships.len() - 1));
            }
            continue;
        }

        if let Some(rest) = statement.strip_prefix("link ") {
            let target = rest
                .split_whitespace()
                .next()
                .unwrap_or_else(|| panic!("{path}:{line}: a link with no target"));
            match scopes.last() {
                Some(Scope::Element(fqn)) => {
                    let fqn = fqn.clone();
                    model
                        .elements
                        .iter_mut()
                        .find(|element| element.fqn == fqn)
                        .expect("the open element was recorded when its scope opened")
                        .links
                        .push(target.to_owned());
                }
                _ => panic!("{path}:{line}: a link outside any element"),
            }
            continue;
        }

        if statement == "model {" {
            scopes.push(Scope::Anonymous);
            continue;
        }

        if ["description", "technology", "title", "style", "notation"]
            .iter()
            .any(|keyword| statement.starts_with(&format!("{keyword} ")))
        {
            continue;
        }

        if let Some((name, declaration)) = statement.split_once('=') {
            let name = name.trim();
            let mut declaration = declaration.split_whitespace();
            let kind = declaration.next().unwrap_or_default();
            assert!(
                !name.is_empty() && !name.contains(char::is_whitespace) && !kind.is_empty(),
                "{path}:{line}: not a recognizable element declaration: {statement:?}"
            );

            let parent: Vec<&str> = scopes
                .iter()
                .filter_map(|scope| match scope {
                    Scope::Element(fqn) => Some(fqn.as_str()),
                    _ => None,
                })
                .collect();
            let fqn = match parent.last() {
                Some(parent) => format!("{parent}.{name}"),
                None => name.to_owned(),
            };

            model.elements.push(Element {
                fqn: fqn.clone(),
                links: Vec::new(),
            });
            if statement.ends_with('{') {
                scopes.push(Scope::Element(fqn));
            }
            continue;
        }

        panic!(
            "{path}:{line}: unrecognized statement {statement:?}. Add it to this \
             parser's whitelist rather than loosening the fall-through: a \
             construct read as nothing turns every assertion in this file into a \
             vacuous pass"
        );
    }

    assert!(
        scopes.is_empty(),
        "{path}: the file ends with an unclosed block"
    );
    assert!(
        !model.elements.is_empty() && !model.relationships.is_empty(),
        "{path}: parsed {} elements and {} relationships. Finding nothing is how \
         this test would pass while checking nothing",
        model.elements.len(),
        model.relationships.len()
    );
    model
}

fn read(relative: &str) -> String {
    let path = workspace_root().join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{relative} is readable: {e}"))
}

const MODEL: &str = "architecture/model.c4";
const VIEWS: &str = "architecture/views.c4";

fn model() -> Model {
    parse_model(&read(MODEL), MODEL)
}

/// Every relationship reduced to the crate edge it claims, dropping the ones
/// that do not claim a crate edge at all: arrows to actors, to the external
/// systems, and to groupings that stand for more than one crate.
fn crate_edges(model: &Model) -> Vec<(String, String, &Relationship)> {
    let owners = model.crate_owners();
    let declared: BTreeSet<&str> = model
        .elements
        .iter()
        .map(|element| element.fqn.as_str())
        .collect();

    let mut edges = Vec::new();
    for relationship in &model.relationships {
        for endpoint in [&relationship.source, &relationship.target] {
            assert!(
                declared.contains(endpoint.as_str()),
                "{MODEL}:{}: {endpoint} is not an element declared in this file. \
                 An endpoint nobody declared is an arrow nothing checks",
                relationship.line
            );
        }

        let (Some(source), Some(target)) = (
            model.resolve(&relationship.source, &owners),
            model.resolve(&relationship.target, &owners),
        ) else {
            continue;
        };
        edges.push((source, target, relationship));
    }
    edges
}

// ───────────────────────────────────────────────────────────────────────────
// Membership.
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn every_crate_is_drawn_exactly_once() {
    let model = model();
    let owners = model.crate_owners();

    let mut on_disk = library_crates();
    on_disk.insert(CLI_CRATE.to_owned());
    let drawn: BTreeSet<String> = owners.keys().cloned().collect();

    let undrawn: Vec<&String> = on_disk.difference(&drawn).collect();
    assert!(
        undrawn.is_empty(),
        "these crates exist but no element in {MODEL} links them: {undrawn:?} — \
         a crate the model does not know is one a reviewer reading the model \
         cannot see. `aoa-domain` was in this state from the day aoa-ynqcn \
         created it"
    );

    let phantom: Vec<&String> = drawn.difference(&on_disk).collect();
    assert!(
        phantom.is_empty(),
        "{MODEL} links {phantom:?} under crates/, which is not a crate"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// Soundness: every arrow is a dependency.
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn every_drawn_edge_is_a_real_production_dependency() {
    let model = model();

    let mut phantom = Vec::new();
    for (source, target, relationship) in crate_edges(&model) {
        if relationship.is_conceptual() {
            continue;
        }
        if !declared_dependencies(&source).contains(&target) {
            phantom.push(format!(
                "{MODEL}:{}: {source} -> {target}",
                relationship.line
            ));
        }
    }

    assert!(
        phantom.is_empty(),
        "these arrows claim a dependency the manifests do not declare: \
         {phantom:?} — either the edge was removed from the code and not from \
         the diagram (aoa-4s25v removed aoa-recommend -> aoa-migrate and left \
         the arrow), or what the arrow describes happens in the composition \
         root and belongs on the CLI, or it is a decision rather than a call \
         and belongs in CONCEPTUAL_EDGES with a reason"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// Completeness: every library dependency is an arrow.
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn every_production_dependency_between_library_crates_is_drawn() {
    let model = model();

    let drawn: BTreeSet<(String, String)> = crate_edges(&model)
        .into_iter()
        .filter(|(_, _, relationship)| !relationship.is_conceptual())
        .map(|(source, target, _)| (source, target))
        .collect();

    let mut undrawn = Vec::new();
    for source in library_crates() {
        for target in declared_dependencies(&source) {
            if !drawn.contains(&(source.clone(), target.clone())) {
                undrawn.push(format!("{source} -> {target}"));
            }
        }
    }

    assert!(
        undrawn.is_empty(),
        "these production dependencies exist but {MODEL} does not draw them: \
         {undrawn:?} — without this the cheap way to satisfy the soundness \
         check is to delete the arrow instead of fixing what it describes. The \
         CLI composition root is exempt (it depends on everything, so drawing \
         all of it would say nothing); nothing else is"
    );
}

#[test]
fn conceptual_edges_are_registered_and_still_conceptual() {
    let model = model();
    let edges = crate_edges(&model);

    let mut unregistered = Vec::new();
    for (source, target, relationship) in &edges {
        if !relationship.is_conceptual() {
            continue;
        }
        if !CONCEPTUAL_EDGES
            .iter()
            .any(|(from, to, _)| from == source && to == target)
        {
            unregistered.push(format!(
                "{MODEL}:{}: {source} -> {target}",
                relationship.line
            ));
        }
    }
    assert!(
        unregistered.is_empty(),
        "these arrows are tagged #conceptual but say nowhere why: \
         {unregistered:?} — the tag exempts an arrow from every other check in \
         this file, so it is registered with a reason or it is not used"
    );

    let mut stale = Vec::new();
    for (source, target, reason) in CONCEPTUAL_EDGES {
        let drawn = edges.iter().any(|(from, to, relationship)| {
            from == source && to == target && relationship.is_conceptual()
        });
        if !drawn {
            stale.push(format!("{source} -> {target} is registered but not drawn"));
        }
        if declared_dependencies(source).contains(*target) {
            stale.push(format!(
                "{source} -> {target} is now a real dependency, so it is an \
                 ordinary edge and the registered reason ({reason:.40}…) no \
                 longer holds"
            ));
        }
    }
    assert!(
        stale.is_empty(),
        "CONCEPTUAL_EDGES no longer describes the model: {stale:?} — a registry \
         nobody prunes stops recording known exceptions and starts hiding new ones"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// The contract is reachable from the artifact it governs.
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn the_model_carries_its_own_decision_record() {
    let source = read(MODEL);

    assert!(
        source.contains(DECISION_RECORD),
        "{MODEL} does not name {DECISION_RECORD}, so a contributor editing the \
         model never meets the decision that makes it a contract"
    );
    assert!(
        source.contains(BEAD),
        "{MODEL} does not name {BEAD}, the bead that recorded what this file is \
         and what is deliberately left unenforced"
    );
}

// ───────────────────────────────────────────────────────────────────────────
// Views.
// ───────────────────────────────────────────────────────────────────────────

#[test]
fn every_view_step_names_an_element_that_exists() {
    let model = model();
    let declared: BTreeSet<&str> = model
        .elements
        .iter()
        .map(|element| element.fqn.as_str())
        .collect();

    let views = read(VIEWS);
    let mut steps = 0usize;
    let mut dangling = Vec::new();
    for (line, sanitized) in sanitized_lines(&views, VIEWS) {
        let Some((source, rest)) = sanitized.trim().split_once("->") else {
            continue;
        };
        let Some(target) = rest.split_whitespace().next() else {
            continue;
        };
        steps += 1;
        for endpoint in [source.trim(), target] {
            if !declared.contains(endpoint) {
                dangling.push(format!("{VIEWS}:{line}: {endpoint}"));
            }
        }
    }

    assert!(
        steps > 0,
        "{VIEWS} declares no walkthrough steps, so this test checked nothing"
    );
    assert!(
        dangling.is_empty(),
        "these walkthrough steps name elements {MODEL} does not declare: \
         {dangling:?} — a dynamic view describes what happens at run time and \
         is deliberately not held to the dependency graph, but a step pointing \
         at a renamed or deleted element describes nothing at all"
    );
}
