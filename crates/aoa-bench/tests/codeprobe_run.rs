use aoa_bench::leg_pass;

/// codeprobe's canonical PASS_THRESHOLD is 0.5. AOA owns the decoding seam, so
/// its score-only fallback must preserve that exact inclusive boundary.
#[test]
fn score_only_legs_match_codeprobes_canonical_pass_threshold() {
    assert_eq!(leg_pass(None, Some(0.49)), Some(false));
    assert_eq!(leg_pass(None, Some(0.5)), Some(true));
    assert_eq!(leg_pass(None, Some(0.7)), Some(true));
}

#[test]
fn explicit_pass_flags_remain_authoritative_at_any_score() {
    assert_eq!(leg_pass(Some(false), Some(1.0)), Some(false));
    assert_eq!(leg_pass(Some(true), Some(0.0)), Some(true));
    assert_eq!(leg_pass(None, None), None);
}
