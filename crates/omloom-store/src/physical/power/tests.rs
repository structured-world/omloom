use super::*;

// Different inlet identities do not hide a shared upstream UPS or utility feed.
#[test]
fn shared_upstream_is_not_disjoint() -> Result<(), Invalid> {
    let graph = Graph {
        complete: &[true; 3],
        edges: &[(0, 2), (1, 2)],
        sources: &[0, 1],
        boundaries: &[2],
    };
    assert_eq!(graph.evaluate()?.decision, Decision::FAIL);
    assert_eq!(
        Graph {
            boundaries: &[],
            ..graph
        }
        .evaluate()?
        .subject,
        Some(2)
    );
    assert_eq!(
        Graph {
            complete: &[false; 3],
            ..graph
        }
        .evaluate()?
        .decision,
        Decision::FAIL
    );
    Ok(())
}

// A positive scoped result requires explicit complete paths and terminal boundaries.
#[test]
fn absence_and_early_boundary_are_not_independence() -> Result<(), Invalid> {
    let graph = Graph {
        complete: &[true; 4],
        edges: &[(0, 2), (1, 3)],
        sources: &[0, 1],
        boundaries: &[2, 3],
    };
    assert_eq!(graph.evaluate()?.decision, Decision::PASS);
    assert_eq!(
        Graph {
            complete: &[true, false, true, true],
            ..graph
        }
        .evaluate()?
        .decision,
        Decision::unknown(Reason::Missing)
    );
    assert_eq!(
        Graph {
            boundaries: &[0, 1],
            ..graph
        }
        .evaluate()?
        .decision,
        Decision::unknown(Reason::ScopeMismatch)
    );
    assert_eq!(
        Graph {
            edges: &[],
            ..graph
        }
        .evaluate()?
        .decision,
        Decision::unknown(Reason::ScopeMismatch)
    );
    Ok(())
}

// Cycles terminate within a fixed work bound and cannot qualify a complete path.
#[test]
fn cycle_duplicate_and_invalid_indices_are_bounded() -> Result<(), Invalid> {
    let graph = Graph {
        complete: &[true; 4],
        edges: &[(0, 2), (2, 0), (1, 3)],
        sources: &[0, 1],
        boundaries: &[3],
    };
    assert_eq!(graph.evaluate()?.decision, Decision::unknown(Reason::Cycle));
    assert_eq!(
        Graph {
            sources: &[0, 0],
            ..graph
        }
        .evaluate()?
        .decision,
        Decision::FAIL
    );
    assert_eq!(
        Graph {
            edges: &[(0, 4)],
            ..graph
        }
        .evaluate(),
        Err(Invalid::Reversed)
    );
    Ok(())
}

// Neither oversized graph storage nor source fan-out starts the bounded evaluator.
#[test]
fn graph_limits_are_checked_before_work() {
    let nodes = [true; MAX_NODES + 1];
    let graph = Graph {
        complete: &nodes,
        edges: &[],
        sources: &[0, 1],
        boundaries: &[0, 1],
    };
    assert_eq!(graph.evaluate(), Err(Invalid::Limit));
    assert_eq!(
        Graph {
            complete: &[true; 2],
            sources: &[0; MAX_SOURCES + 1],
            ..graph
        }
        .evaluate(),
        Err(Invalid::Limit)
    );
    assert_eq!(
        Graph {
            complete: &[true; 2],
            edges: &[(0, 1); MAX_EDGES + 1],
            ..graph
        }
        .evaluate(),
        Err(Invalid::Limit)
    );
}
