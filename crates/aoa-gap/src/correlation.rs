//! Deterministic rank-correlation statistic for construct-validity reports.
//!
//! Spearman's rho is computed via the tie-corrected definition (the Pearson
//! correlation of the average ranks), not the no-ties `1 - 6*sum(d^2)/(n(n^2-1))`
//! shortcut. The external outcomes R9c correlates against — per-item revert,
//! review-acceptance — are often binary and therefore heavily tied, where the
//! shortcut silently returns the wrong value; average-rank Pearson is correct
//! under ties.
//!
//! Significance is an EXACT two-sided permutation test: the observed `|rho|` is
//! compared against the `rho` of every relabeling of the paired observations.
//! It is exact (no normality / large-sample assumption), fully deterministic,
//! and dependency-free, at the cost of `n!` work. That suits the small-`n`
//! calibration regime (a handful of repos). Samples larger than [`MAX_EXACT_N`]
//! are refused with a typed error rather than silently approximated — a future
//! bead may add a sampled test for larger populations.

use thiserror::Error;

/// Floating tolerance for tie detection and permutation-extremity comparison.
const EPS: f64 = 1e-9;

/// Largest sample size for which the exact permutation test is computed.
/// `10! = 3_628_800` relabelings is the ceiling we enumerate; beyond it the
/// caller must pre-aggregate (e.g. supply a revert *rate* per repo) so the
/// population shrinks, or a sampled significance test must be added.
pub const MAX_EXACT_N: usize = 10;

/// Why a rank correlation could not be computed. (`f64` payloads preclude `Eq`;
/// `PartialEq` is sufficient for callers and tests.)
#[derive(Debug, Clone, PartialEq, Error)]
pub enum CorrelationError {
    /// Fewer than three paired observations: no correlation is meaningful and
    /// the permutation distribution is degenerate.
    #[error("correlation needs at least 3 paired observations, got {0}")]
    TooFewObservations(usize),
    /// Sample exceeds the exact-significance cap; refuse rather than approximate.
    #[error("sample size {n} exceeds the exact-significance cap {cap}; pre-aggregate to a rate or add a sampled test")]
    SampleTooLarge { n: usize, cap: usize },
    /// One variable is constant, so its rank variance is zero and correlation
    /// is undefined (a metric or outcome that never varies cannot be validated).
    #[error("a variable has zero variance (all values equal); correlation is undefined")]
    ZeroVariance,
    /// An observation is NaN or infinite. A non-finite value sorts arbitrarily
    /// and silently poisons the coefficient and p-value, so it is rejected at
    /// the boundary rather than returning a meaningless `Ok`.
    #[error("observation {0} is not finite (NaN or infinite)")]
    NonFiniteObservation(f64),
}

/// A computed rank correlation: signed coefficient, sample size, and the exact
/// two-sided permutation p-value. The sign carries direction; the magnitude and
/// p-value together let a caller decide whether the tie is strong and unlikely
/// to be noise.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RankCorrelation {
    /// Signed Spearman coefficient in `[-1.0, 1.0]`.
    pub coefficient: f64,
    /// Number of paired observations backing the coefficient.
    pub n: usize,
    /// Exact two-sided permutation p-value in `(0.0, 1.0]`.
    pub p_value: f64,
}

/// Compute the tie-corrected Spearman rank correlation and its exact two-sided
/// permutation p-value over paired `(metric, outcome)` observations.
pub fn spearman(observations: &[(f64, f64)]) -> Result<RankCorrelation, CorrelationError> {
    let n = observations.len();
    if n < 3 {
        return Err(CorrelationError::TooFewObservations(n));
    }
    if n > MAX_EXACT_N {
        return Err(CorrelationError::SampleTooLarge {
            n,
            cap: MAX_EXACT_N,
        });
    }
    if let Some(bad) = observations
        .iter()
        .flat_map(|(x, y)| [*x, *y])
        .find(|v| !v.is_finite())
    {
        return Err(CorrelationError::NonFiniteObservation(bad));
    }

    let (xs, ys): (Vec<f64>, Vec<f64>) = observations.iter().copied().unzip();
    let xr = average_ranks(&xs);
    let yr = average_ranks(&ys);

    let xc = centered(&xr);
    let yc = centered(&yr);
    let xnorm = l2_norm(&xc);
    let ynorm = l2_norm(&yc);
    if xnorm <= EPS || ynorm <= EPS {
        return Err(CorrelationError::ZeroVariance);
    }

    let observed_cov = dot(&xc, &yc);
    let coefficient = (observed_cov / (xnorm * ynorm)).clamp(-1.0, 1.0);

    // Exact permutation test. The rank norms are invariant under relabeling, so
    // `|rho_perm| >= |rho_obs|` reduces to `|cov_perm| >= |cov_obs|`; comparing
    // covariances avoids recomputing the (constant) denominator each time.
    let observed_abs = observed_cov.abs();
    let total = factorial(n);
    let mut perm: Vec<usize> = (0..n).collect();
    let mut at_least_as_extreme: u64 = 0;
    loop {
        let cov: f64 = xc
            .iter()
            .zip(perm.iter().map(|&i| yc[i]))
            .map(|(a, b)| a * b)
            .sum();
        if cov.abs() >= observed_abs - EPS {
            at_least_as_extreme += 1;
        }
        if !next_permutation(&mut perm) {
            break;
        }
    }

    let p_value = at_least_as_extreme as f64 / total as f64;
    Ok(RankCorrelation {
        coefficient,
        n,
        p_value,
    })
}

/// Average (tie-corrected) ranks of `values`, 1-based. Tied values share the
/// mean of the ranks they span.
fn average_ranks(values: &[f64]) -> Vec<f64> {
    let n = values.len();
    let mut idx: Vec<usize> = (0..n).collect();
    idx.sort_by(|&a, &b| {
        values[a]
            .partial_cmp(&values[b])
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut ranks = vec![0.0; n];
    let mut i = 0;
    while i < n {
        let mut j = i;
        while j + 1 < n && (values[idx[j + 1]] - values[idx[i]]).abs() <= EPS {
            j += 1;
        }
        // Positions i..=j are tied; their 1-based ranks average to this.
        let avg_rank = ((i + j) as f64) / 2.0 + 1.0;
        for &k in &idx[i..=j] {
            ranks[k] = avg_rank;
        }
        i = j + 1;
    }
    ranks
}

/// Subtract the mean so the vector is centered on zero.
fn centered(v: &[f64]) -> Vec<f64> {
    let mean = v.iter().sum::<f64>() / v.len() as f64;
    v.iter().map(|x| x - mean).collect()
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn l2_norm(v: &[f64]) -> f64 {
    dot(v, v).sqrt()
}

fn factorial(n: usize) -> u64 {
    // Precondition: callers gate on MAX_EXACT_N first, so `n! <= 10!` fits u64.
    debug_assert!(n <= MAX_EXACT_N, "factorial called past the exact-n cap");
    (1..=n as u64).product()
}

/// Advance `a` to the next lexicographic permutation in place; returns `false`
/// once the final (descending) permutation has been passed. Enumerates all `n!`
/// orderings when seeded with an ascending sequence.
fn next_permutation(a: &mut [usize]) -> bool {
    if a.len() < 2 {
        return false;
    }
    let mut i = a.len() - 1;
    while i > 0 && a[i - 1] >= a[i] {
        i -= 1;
    }
    if i == 0 {
        return false;
    }
    let mut j = a.len() - 1;
    while a[j] <= a[i - 1] {
        j -= 1;
    }
    a.swap(i - 1, j);
    a[i..].reverse();
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn too_few_observations_is_rejected() {
        assert_eq!(
            spearman(&[(1.0, 1.0), (2.0, 2.0)]),
            Err(CorrelationError::TooFewObservations(2))
        );
    }

    #[test]
    fn oversized_sample_is_refused_not_approximated() {
        let obs: Vec<(f64, f64)> = (0..(MAX_EXACT_N + 1))
            .map(|i| (i as f64, i as f64))
            .collect();
        assert_eq!(
            spearman(&obs),
            Err(CorrelationError::SampleTooLarge {
                n: MAX_EXACT_N + 1,
                cap: MAX_EXACT_N
            })
        );
    }

    #[test]
    fn non_finite_observation_is_rejected() {
        // NaN payload can't be compared with `==` (NaN != NaN), so match it.
        let nan = [(1.0, 1.0), (2.0, f64::NAN), (3.0, 3.0)];
        assert!(matches!(
            spearman(&nan),
            Err(CorrelationError::NonFiniteObservation(v)) if v.is_nan()
        ));
        let inf = [(1.0, 1.0), (f64::INFINITY, 2.0), (3.0, 3.0)];
        assert!(matches!(
            spearman(&inf),
            Err(CorrelationError::NonFiniteObservation(v)) if v.is_infinite()
        ));
    }

    #[test]
    fn constant_variable_has_undefined_correlation() {
        // Outcome never varies: zero rank variance.
        let obs = [(1.0, 5.0), (2.0, 5.0), (3.0, 5.0), (4.0, 5.0)];
        assert_eq!(spearman(&obs), Err(CorrelationError::ZeroVariance));
    }

    #[test]
    fn perfect_monotone_increase_is_plus_one() {
        let obs = [(1.0, 10.0), (2.0, 20.0), (3.0, 30.0), (4.0, 40.0)];
        let c = spearman(&obs).expect("well-defined");
        assert!((c.coefficient - 1.0).abs() < 1e-9, "rho={}", c.coefficient);
        // The two extreme orderings (identity and full reversal) are the only
        // ones reaching |rho| = 1, so p = 2/4! = 2/24.
        assert!((c.p_value - 2.0 / 24.0).abs() < 1e-9, "p={}", c.p_value);
    }

    #[test]
    fn perfect_monotone_decrease_is_minus_one() {
        let obs = [(1.0, 40.0), (2.0, 30.0), (3.0, 20.0), (4.0, 10.0)];
        let c = spearman(&obs).expect("well-defined");
        assert!((c.coefficient + 1.0).abs() < 1e-9, "rho={}", c.coefficient);
    }

    #[test]
    fn known_answer_non_monotone() {
        // x = 1,2,3,4,5 ; y = 1,2,4,3,5 — one adjacent swap. No ties, so the
        // d^2 shortcut applies as a cross-check: d = (0,0,-1,1,0), sum d^2 = 2,
        // rho = 1 - 6*2/(5*24) = 1 - 12/120 = 0.9.
        let obs = [(1.0, 1.0), (2.0, 2.0), (3.0, 4.0), (4.0, 3.0), (5.0, 5.0)];
        let c = spearman(&obs).expect("well-defined");
        assert!((c.coefficient - 0.9).abs() < 1e-9, "rho={}", c.coefficient);
    }

    #[test]
    fn tie_corrected_handles_binary_outcome() {
        // Binary outcome with heavy ties; the no-ties shortcut would be wrong.
        // Metric ascending, outcome 0,0,1,1: a strong but imperfect positive
        // association. Average-rank Pearson is the correct definition here.
        let obs = [(1.0, 0.0), (2.0, 0.0), (3.0, 1.0), (4.0, 1.0)];
        let c = spearman(&obs).expect("well-defined");
        // Pearson of ranks (1,2,3,4) vs average ranks (1.5,1.5,3.5,3.5) = 2/sqrt(5).
        assert!(
            (c.coefficient - 2.0 / 5.0_f64.sqrt()).abs() < 1e-9,
            "expected 2/sqrt(5), got {}",
            c.coefficient
        );
    }

    #[test]
    fn small_n_large_rho_is_not_significant() {
        // n=3 perfect monotone: rho = 1.0 but the permutation distribution is
        // tiny — 2 of 3! = 6 orderings reach |rho|=1, so p = 2/6 ~ 0.33, well
        // above any sane alpha. This is the false-positive case the p-value
        // gate exists to catch: a large coefficient on a tiny sample is noise.
        let obs = [(1.0, 1.0), (2.0, 2.0), (3.0, 3.0)];
        let c = spearman(&obs).expect("well-defined");
        assert!((c.coefficient - 1.0).abs() < 1e-9);
        assert!((c.p_value - 2.0 / 6.0).abs() < 1e-9, "p={}", c.p_value);
        assert!(c.p_value > 0.05, "tiny sample must not look significant");
    }

    #[test]
    fn p_value_is_symmetric_under_sign() {
        // Reversing the outcome flips the sign but not the (two-sided) p-value.
        let up = [(1.0, 1.0), (2.0, 2.0), (3.0, 4.0), (4.0, 3.0), (5.0, 5.0)];
        let down = [(1.0, 5.0), (2.0, 4.0), (3.0, 2.0), (4.0, 3.0), (5.0, 1.0)];
        let cu = spearman(&up).expect("ok");
        let cd = spearman(&down).expect("ok");
        assert!((cu.coefficient + cd.coefficient).abs() < 1e-9);
        assert!((cu.p_value - cd.p_value).abs() < 1e-9);
    }
}
