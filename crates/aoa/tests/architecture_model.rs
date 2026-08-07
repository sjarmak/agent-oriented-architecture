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
//! The contract's terms — membership, soundness, library-to-library
//! completeness, the CLI asymmetry, the grouping rule, and `#conceptual` — are
//! written where a contributor editing the diagram meets them: the header of
//! `architecture/model.c4` itself, recorded in [`DECISION_RECORD`]. They are
//! deliberately not restated here. A second copy is a second thing to drift,
//! with nothing holding the two together, which is the defect this whole file
//! exists to close. What each test enforces is stated at that test, and each
//! failure message says what the rule is and why.

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

/// One `.c4` element, the keyword that declared it, and the source links it
/// declares.
struct Element {
    fqn: String,
    /// `actor`, `externalSystem`, `system`, `container`, `component` — the word
    /// after the `=`. It is what marks the system boundary, which is how
    /// [`Model::elements_standing_for_nothing`] knows where a crate is expected
    /// to be and where its absence is honest.
    kind: String,
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
                // `entry` rather than `insert`, so the common path moves the
                // name in without cloning and only the duplicate path reads it
                // back.
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

    /// Every element's fully qualified name.
    fn fqns(&self) -> BTreeSet<&str> {
        self.elements
            .iter()
            .map(|element| element.fqn.as_str())
            .collect()
    }

    /// The systems this file declares — the boundaries inside which an element
    /// is a claim about code. `externalSystem` is a different keyword and is
    /// deliberately not one of these: the repository under test, CodeProbe and
    /// the forge are things AOA talks to, and none of them is a crate.
    fn system_roots(&self) -> Vec<&str> {
        self.elements
            .iter()
            .filter(|element| element.kind == "system")
            .map(|element| element.fqn.as_str())
            .collect()
    }

    /// Elements that claim to be code and stand for no crate at all — neither
    /// linking one nor holding one underneath.
    ///
    /// [`resolve`] answers [`Endpoint::Outside`] for an element with no crate
    /// under it — the right answer for an actor or an external system, and the
    /// wrong one for anything claiming to be code — and [`crate_edges`] then
    /// drops every arrow at it as an arrow to the outside world. The element
    /// and its arrows still render on the published diagram, so such a claim is
    /// visible to every reader and invisible to every test.
    ///
    /// An element claims to be code two ways, and both are checked because
    /// either alone leaves the other as a way around this. Sitting inside a
    /// system is one: everything in the AOA boundary is one of its crates.
    /// Being declared `container` or `component` is the other, and it does not
    /// depend on where the element sits — a `component` written at the top
    /// level of the file, outside every boundary, renders and draws arrows just
    /// the same. Bounding the rule to the system boundary alone was the first
    /// fix for this and did not hold: the ghost simply moved one line out of
    /// the system and passed again.
    fn elements_standing_for_nothing(&self) -> Vec<&str> {
        let owners = self.crate_owners();
        let roots = self.system_roots();

        self.elements
            .iter()
            .filter(|element| {
                element.kind == "container"
                    || element.kind == "component"
                    || roots.iter().any(|root| {
                        element.fqn == *root || element.fqn.starts_with(&format!("{root}."))
                    })
            })
            .filter(|element| matches!(resolve(&element.fqn, &owners), Endpoint::Outside))
            .map(|element| element.fqn.as_str())
            .collect()
    }
}

/// What a relationship endpoint stands for.
enum Endpoint {
    /// Exactly one crate: the element links it, or it is a grouping with a
    /// single crate-backed element under it.
    Crate(String),
    /// No crate anywhere under it — an actor, or one of the external systems.
    /// An arrow here claims nothing about the build graph.
    Outside,
    /// A grouping standing for several crates.
    Grouping(usize),
}

/// Resolve an endpoint against the crates the model links.
///
/// An element that links a crate directly is that crate. A grouping resolves to
/// the single crate in its subtree when there is exactly one: `aoa.migrate`
/// holds only `aoa.migrate.migrator`, so an arrow drawn at the container claims
/// exactly what an arrow at the component would, and checking only the second
/// would leave the first as a way around this test.
///
/// A grouping over several crates is [`Endpoint::Grouping`] rather than "no
/// crate". The two are not the same and collapsing them was a hole:
/// `aoa.substrate -> aoa.migrate` would have been dropped as unresolvable and
/// gone unchecked, while rendering as an arrow between two halves of the system
/// — a dependency claim with nothing behind it, which is the whole defect this
/// file exists to catch. [`crate_edges`] refuses it.
fn resolve(fqn: &str, owners: &BTreeMap<String, &str>) -> Endpoint {
    if let Some((name, _)) = owners.iter().find(|(_, owner)| **owner == fqn) {
        return Endpoint::Crate(name.clone());
    }

    let prefix = format!("{fqn}.");
    let mut under = owners
        .iter()
        .filter(|(_, owner)| owner.starts_with(&prefix))
        .map(|(name, _)| name.clone());
    match (under.next(), under.next()) {
        (None, _) => Endpoint::Outside,
        (Some(only), None) => Endpoint::Crate(only),
        (Some(_), Some(_)) => Endpoint::Grouping(2 + under.count()),
    }
}

impl Element {
    /// The crates this element's links point into.
    ///
    /// The name has to be a plain directory name. Stripping `../crates/` off a
    /// link is a text operation, so a link written `../crates/../Cargo.toml`
    /// would otherwise yield `..` and be carried onward as if it were a crate:
    /// [`declared_dependencies`] would read the workspace manifest, find no
    /// `aoa-*` entries in it, and report a phantom arrow — a true failure with
    /// a misleading reason. Rejecting the name here says what is actually
    /// wrong, and keeps every path this file builds inside `crates/`.
    fn linked_crates(&self) -> BTreeSet<String> {
        self.links
            .iter()
            .filter_map(|link| link.strip_prefix("../crates/"))
            .filter_map(|rest| rest.split('/').next())
            .filter(|name| !name.is_empty())
            .map(|name| {
                assert!(
                    !name.contains(['.', std::path::MAIN_SEPARATOR]),
                    "{} links ../crates/{name}/…, which is not a crate directory name",
                    self.fqn
                );
                name.to_owned()
            })
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
///
/// Two more are constraints on the `.c4` files rather than on this function,
/// and are written down because nothing else states them. A description must
/// spell an apostrophe as `’` and not `'`, which is what both files already do:
/// a straight apostrophe toggles `in_string`, and while an odd number of them
/// on a line trips the assertion below, an even number would silently swap
/// which halves of the line count as string. And a comment must be a whole
/// line, since only a leading `//` is stripped; a trailing one survives into
/// the statement, where the whitelist rejects it rather than misreading it.
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
///
/// It is over the workspace's usual function-length bar, and deliberately so.
/// The whitelist is the fail-closed property, and it only reads as exhaustive
/// while every arm and the final `panic!` are visible together; split across
/// per-construct helpers, "is anything missing" stops being answerable by
/// reading one screen. The branches it would split into are what [`fail_closed`]
/// tests one by one instead.
fn parse_model(source: &str, path: &str) -> Model {
    /// What the enclosing `{` opened, as an index into the model it opened
    /// into. Only `Element` contributes to a fully qualified name; a
    /// relationship's trailing block holds tags and must not be mistaken for a
    /// nested element, or every name after it is wrong.
    enum Scope {
        Anonymous,
        Element(usize),
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

        // A block is opened by a trailing `{`. String literals are already
        // emptied, so no description can end in one.
        let opens_block = statement.ends_with('{');

        if let Some((source_fqn, rest)) = statement.split_once("->") {
            let target_fqn = rest
                .split_whitespace()
                .next()
                .unwrap_or_else(|| panic!("{path}:{line}: a relationship with no target"));

            model.relationships.push(Relationship {
                source: source_fqn.trim().to_owned(),
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
            let Some(Scope::Element(index)) = scopes.last() else {
                panic!("{path}:{line}: a link outside any element");
            };
            model.elements[*index].links.push(target.to_owned());
            continue;
        }

        if statement == "model {" {
            scopes.push(Scope::Anonymous);
            continue;
        }

        let keyword = statement
            .split_once(char::is_whitespace)
            .map_or("", |(keyword, _)| keyword);
        if ["description", "technology", "title", "style", "notation"].contains(&keyword) {
            // A whitelisted statement is skipped, not read — but `style { … }`
            // still opens a block, and skipping one without pushing a scope
            // hands its closing brace to the enclosing element instead. Every
            // element declared after it is then named under the wrong parent
            // and the file ends one scope short, so the only complaint is
            // "unclosed block" against the last line of the file, which is the
            // one line that is not the problem.
            if opens_block {
                scopes.push(Scope::Anonymous);
            }
            continue;
        }

        if let Some((name, declaration)) = statement.split_once('=') {
            let name = name.trim();
            let kind = declaration.split_whitespace().next().unwrap_or_default();
            assert!(
                !name.is_empty() && !name.contains(char::is_whitespace) && !kind.is_empty(),
                "{path}:{line}: not a recognizable element declaration: {statement:?}"
            );

            // The innermost open element is the parent; a relationship's tag
            // block or the anonymous `model {` never names anything.
            let fqn = match scopes.iter().rev().find_map(|scope| match scope {
                Scope::Element(index) => Some(model.elements[*index].fqn.as_str()),
                _ => None,
            }) {
                Some(parent) => format!("{parent}.{name}"),
                None => name.to_owned(),
            };

            model.elements.push(Element {
                fqn,
                kind: kind.to_owned(),
                links: Vec::new(),
            });
            if opens_block {
                scopes.push(Scope::Element(model.elements.len() - 1));
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

/// Every relationship reduced to the crate edge it claims, dropping only the
/// ones that claim no crate edge at all: arrows to the actors and the external
/// systems.
///
/// An arrow may not be drawn at a grouping that stands for several crates. Such
/// an arrow reads on the rendered diagram as a dependency between two halves of
/// the system while naming neither crate, so there is nothing to check it
/// against — it would be the one shape of claim that could sit in a published
/// model, look authoritative, and never be wrong enough to fail. Draw it
/// between the components that actually have the dependency.
fn crate_edges(model: &Model) -> Vec<(String, String, &Relationship)> {
    let owners = model.crate_owners();
    let declared = model.fqns();

    let mut edges = Vec::new();
    for relationship in &model.relationships {
        let ends = [&relationship.source, &relationship.target].map(|endpoint| {
            assert!(
                declared.contains(endpoint.as_str()),
                "{MODEL}:{}: {endpoint} is not an element declared in this file. \
                 An endpoint nobody declared is an arrow nothing checks",
                relationship.line
            );
            let end = resolve(endpoint, &owners);
            if let Endpoint::Grouping(crates) = end {
                panic!(
                    "{MODEL}:{}: this arrow is drawn at {endpoint}, a grouping over \
                     {crates} crates, so it claims a dependency without naming \
                     either end of it and nothing can check it. Draw it between \
                     the components that have the dependency",
                    relationship.line
                );
            }
            end
        });

        let [Endpoint::Crate(source), Endpoint::Crate(target)] = ends else {
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

/// The other direction of membership: every crate is drawn, and everything
/// drawn as code is a crate. The two shapes this was found to miss are in
/// [`known_holes`].
#[test]
fn every_element_that_claims_to_be_code_stands_for_a_crate() {
    let model = model();

    assert!(
        !model.system_roots().is_empty(),
        "{MODEL} declares no `system`, so half of this rule had no boundary to \
         check inside of"
    );

    let standing_for_nothing = model.elements_standing_for_nothing();
    assert!(
        standing_for_nothing.is_empty(),
        "these elements are drawn as code — inside the AOA system, or as a \
         `container` or `component` anywhere — but link no crate and hold none \
         underneath: {standing_for_nothing:?} — the title is what the diagram \
         renders and nothing here reads it, so an element that only names a \
         crate is a box on a published picture no test can contradict, and \
         every arrow at it is silently dropped. Link the crate it stands for, \
         or draw it as an actor or an external system, where having no code \
         behind it is what the shape already says"
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

/// `views.c4` is held to a weaker rule on purpose, and the asymmetry is the
/// point rather than an omission: its dynamic views are walkthroughs of what
/// happens at run time, not claims about the build graph. `operator -> falsify`
/// and `audit -> operator` are steps in the same file, and no manifest will
/// ever back them. So the views are checked for the one thing they do owe —
/// that every endpoint names an element that exists — and their flow steps are
/// left to say what a run actually does.
#[test]
fn every_view_step_names_an_element_that_exists() {
    let model = model();
    let declared = model.fqns();

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

// ───────────────────────────────────────────────────────────────────────────
// The two holes the model is known to have had.
// ───────────────────────────────────────────────────────────────────────────

/// The real `model.c4` is well-formed, so the checks above pass on it whether
/// or not they work. Each snippet here is the smallest input that reproduces a
/// hole this file was found to have, asserted directly rather than through the
/// model, so a regression fails with the shape that caused it rather than
/// waiting for someone to draw it again.
mod known_holes {
    use super::parse_model;

    const PATH: &str = "<snippet>";

    /// The shape the review of aoa-enzj8 demonstrated. `recommendGhost` names
    /// `aoa-recommend` where the diagram renders it and links nothing, so it
    /// resolves to the outside world; the arrow out of it was dropped by
    /// `crate_edges` and drew the removed `aoa-recommend -> aoa-migrate`
    /// dependency through a green suite.
    #[test]
    fn an_element_that_only_names_a_crate_is_reported() {
        let model = parse_model(
            concat!(
                "model {\n",
                "  aoa = system 'AOA' {\n",
                "    measure = container 'Read-only measurement' {\n",
                "      recommend = component 'aoa-recommend' {\n",
                "        link ../crates/aoa-recommend 'crates/aoa-recommend/'\n",
                "      }\n",
                "      recommendGhost = component 'aoa-recommend'\n",
                "    }\n",
                "  }\n",
                "  aoa.measure.recommendGhost -> aoa.measure.recommend 'phantom'\n",
                "}\n",
            ),
            PATH,
        );

        assert_eq!(
            model.elements_standing_for_nothing(),
            ["aoa.measure.recommendGhost"],
            "an element inside the system that links no crate has to be \
             reported here; nothing else in this file will ever see it"
        );
    }

    /// The same ghost, moved one line out of the system boundary. It is still a
    /// `component`, still renders, and still draws its arrow, so bounding the
    /// rule to what sits inside a system left the whole defect reachable by
    /// deleting two levels of indentation.
    #[test]
    fn an_element_outside_every_boundary_does_not_escape_by_leaving_it() {
        let model = parse_model(
            concat!(
                "model {\n",
                "  aoa = system 'AOA' {\n",
                "    migrate = container 'Repository-mutating migration' {\n",
                "      migrator = component 'aoa-migrate' {\n",
                "        link ../crates/aoa-migrate 'crates/aoa-migrate/'\n",
                "      }\n",
                "    }\n",
                "  }\n",
                "  ghost = component 'aoa-recommend'\n",
                "  ghost -> aoa.migrate.migrator 'joins migration availability per finding'\n",
                "}\n",
            ),
            PATH,
        );

        assert_eq!(
            model.elements_standing_for_nothing(),
            ["ghost"],
            "a component that stands for no crate is a component wherever it is \
             written; the system boundary is not what makes the claim checkable"
        );
    }

    /// `style { … }` is skipped rather than read, and skipping a block-opener
    /// without pushing a scope makes its own closing brace pop the element it
    /// sits in. Before the fix this snippet did not merely mis-name the
    /// element: the link landed on the enclosing system and the file ran out of
    /// scopes early, so the parse died on the last brace, naming a line nowhere
    /// near the `style` that caused it.
    #[test]
    fn a_skipped_block_does_not_close_the_element_around_it() {
        let model = parse_model(
            concat!(
                "model {\n",
                "  aoa = system 'AOA' {\n",
                "    trace = component 'aoa-trace' {\n",
                "      style {\n",
                "      }\n",
                "      link ../crates/aoa-trace 'crates/aoa-trace/'\n",
                "    }\n",
                "  }\n",
                "  aoa.trace -> aoa.trace 'self'\n",
                "}\n",
            ),
            PATH,
        );

        assert_eq!(
            model.crate_owners().get("aoa-trace"),
            Some(&"aoa.trace"),
            "the link belongs to the component it was written inside"
        );
    }
}

// ───────────────────────────────────────────────────────────────────────────
// The parser's fail-closed branches.
// ───────────────────────────────────────────────────────────────────────────

/// Everything above runs against the real `model.c4`, which is well-formed, so
/// none of it ever reaches a rejection path. That leaves the property the whole
/// file rests on — that an unrecognized construct panics instead of being read
/// as nothing — asserted by no test at all. A regression in the whitelist, or
/// in the order its arms are tried, would turn the rejection into a silent skip
/// and every check above into a vacuous pass, with the suite still green.
///
/// These snippets are synthetic on purpose: each one is the smallest input that
/// reaches one branch.
mod fail_closed {
    use super::{parse_model, Model};

    const PATH: &str = "<snippet>";

    fn parse(source: &str) -> Model {
        parse_model(source, PATH)
    }

    #[test]
    #[should_panic(expected = "unrecognized statement")]
    fn a_construct_the_whitelist_does_not_know_is_rejected() {
        parse("model {\n  bogusStatement someArgument\n}\n");
    }

    #[test]
    #[should_panic(expected = "a closing brace with nothing open")]
    fn a_brace_closing_nothing_is_rejected() {
        parse("}\n");
    }

    #[test]
    #[should_panic(expected = "the file ends with an unclosed block")]
    fn a_block_left_open_is_rejected() {
        parse("model {\n  a = component 'x' {\n");
    }

    #[test]
    #[should_panic(expected = "a link outside any element")]
    fn a_link_belonging_to_no_element_is_rejected() {
        parse("model {\n  link ../crates/aoa-trace 'x'\n}\n");
    }

    #[test]
    #[should_panic(expected = "a string literal is left open")]
    fn a_string_running_past_the_line_is_rejected() {
        parse("model {\n  a = component 'unterminated\n}\n");
    }

    #[test]
    #[should_panic(expected = "a relationship with no target")]
    fn an_arrow_pointing_at_nothing_is_rejected() {
        parse("model {\n  a ->\n}\n");
    }

    #[test]
    #[should_panic(expected = "not a recognizable element declaration")]
    fn a_nameless_declaration_is_rejected() {
        parse("model {\n   = component 'x' {\n  }\n}\n");
    }

    /// The guard that matters most: a parse that finds nothing must not be
    /// mistaken for a model with nothing wrong in it.
    #[test]
    #[should_panic(expected = "Finding nothing is how")]
    fn a_parse_that_yields_no_model_is_rejected() {
        parse("model {\n}\n");
    }

    #[test]
    #[should_panic(expected = "which is not a crate directory name")]
    fn a_link_escaping_the_crates_directory_is_rejected() {
        parse(concat!(
            "model {\n",
            "  a = component 'x' {\n",
            "    link ../crates/../Cargo.toml 'escape'\n",
            "  }\n",
            "  a -> a 'self'\n",
            "}\n",
        ))
        .crate_owners();
    }
}
