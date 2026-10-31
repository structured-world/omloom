//! Bounded plan dependencies shared by manual and provider consumers.
//! This module schedules no work and grants no authority to perform an effect.
use crate::requirements::{Decision, State};

pub const MAX_STEPS: usize = 64;
pub const MAX_EDGES: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Pending,
    Reported,
    NotPerformed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    Reported,
    Satisfied,
    Failed,
}
#[derive(Clone, Copy, Debug)]
pub struct Dependency {
    pub step: usize,
    pub trigger: Trigger,
}
#[derive(Clone, Copy, Debug)]
pub struct Step<'a> {
    pub after: &'a [Dependency],
    pub outcome: Outcome,
    pub check: Option<Decision>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Readiness {
    Ready,
    Waiting,
    Unselected,
    Complete,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Invalid {
    Limit,
    Dependency,
    Cycle,
}

/// Validate every branch, then derive readiness in O(steps + dependencies).
/// Caller-owned fixed bounds avoid allocation and recursion for untrusted graphs.
pub fn assess(
    steps: &[Step<'_>],
    output: &mut [Readiness],
    mut ancestors: Option<&mut [u64]>,
) -> Result<(), Invalid> {
    if steps.is_empty() || steps.len() > MAX_STEPS || output.len() < steps.len() {
        return Err(Invalid::Limit);
    }
    if let Some(matrix) = &mut ancestors {
        if matrix.len() < steps.len() {
            return Err(Invalid::Limit);
        }
        matrix[..steps.len()].fill(0);
    }
    let mut indegree = [0u8; MAX_STEPS];
    let mut children = [0u64; MAX_STEPS];
    let mut edges = 0;
    for (index, step) in steps.iter().enumerate() {
        if step.after.len() > MAX_EDGES - edges {
            return Err(Invalid::Limit);
        }
        edges += step.after.len();
        let mut seen = 0u64;
        for dependency in step.after {
            if dependency.step >= steps.len() || dependency.step == index {
                return Err(Invalid::Dependency);
            }
            let bit = 1u64 << dependency.step;
            if seen & bit != 0 {
                return Err(Invalid::Dependency);
            }
            seen |= bit;
            indegree[index] += 1;
            children[dependency.step] |= 1u64 << index;
        }
    }
    let mut queue = [0usize; MAX_STEPS];
    let mut tail = 0;
    for (index, &count) in indegree[..steps.len()].iter().enumerate() {
        if count == 0 {
            queue[tail] = index;
            tail += 1;
        }
    }
    output[..steps.len()].fill(Readiness::Waiting);
    let mut head = 0;
    while head < tail {
        let index = queue[head];
        head += 1;
        let step = &steps[index];
        let mut waiting = false;
        let mut excluded = false;
        for dependency in step.after {
            if let Some(matrix) = &mut ancestors {
                let inherited = matrix[dependency.step];
                matrix[index] |= inherited | (1u64 << dependency.step);
            }
            if output[dependency.step] == Readiness::Unselected {
                excluded = true;
                continue;
            }
            let predecessor = &steps[dependency.step];
            let wanted = match (predecessor.outcome, dependency.trigger) {
                (Outcome::Pending, _) => None,
                (Outcome::Reported, Trigger::Reported) => Some(true),
                (Outcome::Reported, Trigger::Satisfied) => {
                    predecessor.check.and_then(|d| match d.state {
                        State::Unknown => None,
                        state => Some(state == State::Satisfied),
                    })
                }
                (Outcome::Reported, Trigger::Failed) => {
                    predecessor.check.and_then(|d| match d.state {
                        State::Unknown => None,
                        state => Some(state == State::Failed),
                    })
                }
                _ => Some(false),
            };
            match wanted {
                Some(true) => {}
                Some(false) => excluded = true,
                None => waiting = true,
            }
        }
        output[index] = match step.outcome {
            Outcome::Reported => Readiness::Complete,
            Outcome::NotPerformed if excluded => Readiness::Unselected,
            Outcome::NotPerformed => Readiness::Complete,
            _ if excluded => Readiness::Unselected,
            _ if waiting => Readiness::Waiting,
            _ => Readiness::Ready,
        };
        let mut successors = children[index];
        while successors != 0 {
            let child = successors.trailing_zeros() as usize;
            successors &= successors - 1;
            debug_assert!(indegree[child] > 0);
            indegree[child] -= 1;
            if indegree[child] == 0 {
                queue[tail] = child;
                tail += 1;
            }
        }
    }
    if tail != steps.len() {
        return Err(Invalid::Cycle);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
