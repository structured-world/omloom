use super::*;

// Every growing class requires finite admission bounds; invalid policies cannot mean unlimited.
#[test]
fn policy_rejects_missing_or_unrepresentable_bounds() {
    let mut policy = Policy {
        classes: [Budget {
            rows: 1,
            bytes: 1024,
        }; CLASSES],
        total_bytes: 4096,
        detail_age_ms: 1,
        batch_rows: 1,
    };
    assert!(policy.valid());
    policy.classes[0].bytes = 0;
    assert!(!policy.valid());
    policy.classes[0].bytes = 1024;
    policy.batch_rows = 129;
    assert!(!policy.valid());
    policy.batch_rows = 1;
    policy.detail_age_ms = u64::MAX;
    assert!(!policy.valid());
}
