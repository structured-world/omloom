use super::*;

// Exhausting a supported statement memory ceiling is admission failure, not corruption.
#[test]
fn statement_memory_exhaustion_is_a_resource_limit() {
    let error = Error::from(DatabaseError::Budget(
        coordinode_core::budget::BudgetStop::Memory {
            requested: 4096,
            used: 8192,
            limit: 8192,
        },
    ));
    assert!(matches!(error, Error::Limit));
}
