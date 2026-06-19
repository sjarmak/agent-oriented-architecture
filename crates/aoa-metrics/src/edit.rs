use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::common::ConditionedOn;
use crate::error::MetricError;
use crate::input::{Confidence, MetricInput};

/// Edit-locality: file inflation measured against both an intersection floor and
/// a union ceiling drawn from two or more accepted solutions.
///
/// `floor_inflation` divides by the union size (the loosest valid set), so it is
/// the smaller, more forgiving ratio; `ceiling_inflation` divides by the
/// intersection size (the strictest valid set), the larger ratio. By
/// construction `floor_inflation <= ceiling_inflation`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditLocality {
    /// Size of the agent's changed-file set `F_edit`.
    pub f_edit_size: usize,
    /// Size of the intersection of accepted solutions (the floor file set).
    pub intersection_size: usize,
    /// Size of the union of accepted solutions (the ceiling file set).
    pub union_size: usize,
    /// `|F_edit| / |union|` — inflation against the most permissive valid set.
    pub floor_inflation: f64,
    /// `|F_edit| / |intersection|` — inflation against the strictest valid set.
    pub ceiling_inflation: f64,
    pub conditioned_on: ConditionedOn,
    pub confidence: Confidence,
    pub weight: f64,
}

/// Compute edit-locality. Requires at least two accepted solutions so that the
/// intersection floor and union ceiling are both well-defined.
pub fn compute_edit_locality(input: &MetricInput) -> Result<EditLocality, MetricError> {
    if input.accepted_solutions.len() < 2 {
        return Err(MetricError::InsufficientAcceptedSolutions(
            input.accepted_solutions.len(),
        ));
    }

    let mut iter = input.accepted_solutions.iter();
    let first = iter.next().expect("checked len >= 2");
    let mut intersection: BTreeSet<String> = first.clone();
    let mut union: BTreeSet<String> = first.clone();
    for sol in iter {
        intersection = intersection.intersection(sol).cloned().collect();
        union = union.union(sol).cloned().collect();
    }

    let f_edit = input.edited_files.len();
    let floor_inflation = f_edit as f64 / union.len().max(1) as f64;
    let ceiling_inflation = f_edit as f64 / intersection.len().max(1) as f64;

    Ok(EditLocality {
        f_edit_size: f_edit,
        intersection_size: intersection.len(),
        union_size: union.len(),
        floor_inflation,
        ceiling_inflation,
        conditioned_on: ConditionedOn::HeldOut,
        confidence: input.graph.quality.confidence(),
        weight: input.graph.quality.weight(),
    })
}
