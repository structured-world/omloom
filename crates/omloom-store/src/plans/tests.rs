use super::*;
use crate::requirements::Reason;

// Reported work unlocks a check, but cannot impersonate its observed success.
#[test]
fn reports_and_guarded_checks_have_distinct_meanings() -> Result<(), Invalid> {
    let after_report = [Dependency {
        step: 0,
        trigger: Trigger::Reported,
    }];
    let positive = [Dependency {
        step: 1,
        trigger: Trigger::Satisfied,
    }];
    let negative = [Dependency {
        step: 1,
        trigger: Trigger::Failed,
    }];
    let mut steps = [
        Step {
            after: &[],
            outcome: Outcome::Reported,
            check: None,
        },
        Step {
            after: &after_report,
            outcome: Outcome::Reported,
            check: Some(Decision::unknown(Reason::Missing)),
        },
        Step {
            after: &positive,
            outcome: Outcome::Pending,
            check: None,
        },
        Step {
            after: &negative,
            outcome: Outcome::Pending,
            check: None,
        },
    ];
    let mut result = [Readiness::Waiting; 4];
    assess(&steps, &mut result, None)?;
    assert_eq!(
        result,
        [
            Readiness::Complete,
            Readiness::Complete,
            Readiness::Waiting,
            Readiness::Waiting
        ]
    );
    steps[1].check = Some(Decision::FAIL);
    assess(&steps, &mut result, None)?;
    assert_eq!(
        result,
        [
            Readiness::Complete,
            Readiness::Complete,
            Readiness::Unselected,
            Readiness::Ready
        ]
    );
    steps[1].check = Some(Decision::PASS);
    assess(&steps, &mut result, None)?;
    assert_eq!(
        result,
        [
            Readiness::Complete,
            Readiness::Complete,
            Readiness::Ready,
            Readiness::Unselected
        ]
    );
    // Declining an excluded branch differs from abandoning selected mandatory work.
    steps[3].outcome = Outcome::NotPerformed;
    steps[2].outcome = Outcome::NotPerformed;
    assess(&steps, &mut result, None)?;
    assert_eq!(result[3], Readiness::Unselected);
    assert_eq!(result[2], Readiness::Complete);
    Ok(())
}

// Cycles and duplicate dependency edges cannot hide in an unselected branch.
#[test]
fn malformed_graphs_are_rejected() {
    let cyclic = [Dependency {
        step: 1,
        trigger: Trigger::Reported,
    }];
    let back = [Dependency {
        step: 0,
        trigger: Trigger::Reported,
    }];
    let mut out = [Readiness::Waiting; 2];
    assert_eq!(
        assess(
            &[
                Step {
                    after: &cyclic,
                    outcome: Outcome::Pending,
                    check: None
                },
                Step {
                    after: &back,
                    outcome: Outcome::Pending,
                    check: None
                }
            ],
            &mut out,
            None
        ),
        Err(Invalid::Cycle)
    );
    let duplicate = [back[0], back[0]];
    assert_eq!(
        assess(
            &[
                Step {
                    after: &[],
                    outcome: Outcome::Pending,
                    check: None
                },
                Step {
                    after: &duplicate,
                    outcome: Outcome::Pending,
                    check: None
                }
            ],
            &mut out,
            None
        ),
        Err(Invalid::Dependency)
    );
}

// The longest supported chain is iterative and includes the highest bit safely.
#[test]
fn full_length_chain_is_bounded() -> Result<(), Invalid> {
    let edges: [Dependency; MAX_STEPS] = core::array::from_fn(|i| Dependency {
        step: if i == 0 { 0 } else { i - 1 },
        trigger: Trigger::Reported,
    });
    let steps: [Step<'_>; MAX_STEPS] = core::array::from_fn(|i| Step {
        after: if i == 0 { &[] } else { &edges[i..i + 1] },
        outcome: Outcome::Reported,
        check: None,
    });
    let mut out = [Readiness::Waiting; MAX_STEPS];
    let mut ancestors = [0u64; MAX_STEPS];
    assess(&steps, &mut out, Some(&mut ancestors))?;
    assert_eq!(ancestors[MAX_STEPS - 1], u64::MAX >> 1);
    assert_eq!(out, [Readiness::Complete; MAX_STEPS]);
    Ok(())
}
