//! Observe → live-corpus bridge: the second ingestion adapter of the AOA shim
//! family (aoa-d6t.23).
//!
//! `aoa-codeprobe-shim` mines **past** commits: codeprobe replays history and
//! the mined ground-truth commit is the held-out oracle. This crate ingests
//! the **present**: `aoa observe` (and the `aoa enforce` hooks it installs)
//! captures live sessions under `<repo>/.aoa/traces/`, where the dev's
//! eventual real edit is the contamination-free held-out ground truth (see
//! [`held_out_edits`]). Both adapters emit the same [`aoa_trace::Trace`]
//! stream, so the four locality metrics score either source through one
//! instrument.
//!
//! Two corpus file shapes are ingested (see [`load_corpus`]):
//!
//! - `live-<session>.jsonl` — appended span-per-line by the enforce hooks;
//! - `<name>.json` — whole traces landed via `aoa_audit::write_trace`.
//!
//! A session counts as one held-out behavioral observation only when it
//! carries a landed edit ([`held_out_edits`]); edit-free sessions accumulate
//! but supply no signal. The observation count feeds the greenfield/cold-start
//! precondition (`aoa_gap::BehavioralSignal`): below the behavioral-signal
//! window the behavioral metrics report insufficient-data; once enough
//! edit-carrying sessions accumulate they light up.
//!
//! [`ObserveLiveLog`] implements the versioned cross-agent
//! [`aoa_codeprobe_shim::TraceBackend`] contract with `native` provenance —
//! the hooks emit spans directly from the live session, nothing is
//! reconstructed. Conformance is asserted in tests via
//! [`aoa_codeprobe_shim::run_conformance`].

mod backend;
mod corpus;
mod error;

pub use backend::{parse_live_log, ObserveLiveLog};
pub use corpus::{held_out_edits, load_corpus, ObservedSession, TraceCorpus};
pub use error::ObserveShimError;
