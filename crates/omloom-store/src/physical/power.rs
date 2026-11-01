//! Recorded dependency-path disjointness, not live electrical redundancy.
use super::{Decision, Invalid, Reason};

pub const MAX_NODES: usize = 128;
pub const MAX_EDGES: usize = 256;
pub const MAX_SOURCES: usize = 8;

/// An authorized, bounded graph in one read snapshot. An edge points upstream.
pub struct Graph<'a> {
    pub complete: &'a [bool],
    pub edges: &'a [(usize, usize)],
    pub sources: &'a [usize],
    pub boundaries: &'a [usize],
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Assessment {
    pub decision: Decision,
    pub subject: Option<usize>,
}
fn assessment(decision: Decision, subject: Option<usize>) -> Assessment {
    Assessment { decision, subject }
}

impl Graph<'_> {
    /// Missing paths, cycles and unqualified source lists never establish a pass.
    /// The fixed scratch and bounded passes require no allocation or recursion.
    pub fn evaluate(&self) -> Result<Assessment, Invalid> {
        let n = self.complete.len();
        if n == 0 || self.sources.len() < 2 {
            return Err(Invalid::Empty);
        }
        if n > MAX_NODES
            || self.edges.len() > MAX_EDGES
            || self.sources.len() > MAX_SOURCES
            || self.boundaries.len() > MAX_SOURCES
        {
            return Err(Invalid::Limit);
        }
        if self.sources.iter().chain(self.boundaries).any(|&i| i >= n)
            || self.edges.iter().any(|&(a, b)| a >= n || b >= n)
        {
            return Err(Invalid::Reversed);
        }
        let mut masks = [0u8; MAX_NODES];
        for (role, &source) in self.sources.iter().enumerate() {
            if masks[source] != 0 {
                return Ok(assessment(Decision::FAIL, Some(source)));
            }
            masks[source] = 1 << role;
        }
        for _ in 0..n {
            let mut changed = false;
            for &(child, parent) in self.edges {
                let mask = masks[parent] | masks[child];
                if mask.count_ones() > 1 {
                    return Ok(assessment(Decision::FAIL, Some(parent)));
                }
                if mask != masks[parent] {
                    masks[parent] = mask;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        let mut valid = [false; MAX_NODES];
        for i in 0..n {
            if masks[i] == 0 {
                continue;
            }
            if !self.complete[i] {
                return Ok(assessment(Decision::unknown(Reason::Missing), Some(i)));
            }
            let boundary = self.boundaries.contains(&i);
            let upstream = self.edges.iter().any(|&(child, _)| child == i);
            if boundary == upstream {
                return Ok(assessment(
                    Decision::unknown(Reason::ScopeMismatch),
                    Some(i),
                ));
            }
            valid[i] = boundary;
        }
        for _ in 0..n {
            let mut changed = false;
            for i in 0..n {
                if masks[i] == 0 || valid[i] {
                    continue;
                }
                let mut ready = true;
                for &(child, parent) in self.edges {
                    if child == i && !valid[parent] {
                        ready = false;
                        break;
                    }
                }
                if ready {
                    valid[i] = true;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        for &source in self.sources {
            if !valid[source] {
                return Ok(assessment(Decision::unknown(Reason::Cycle), Some(source)));
            }
        }
        Ok(assessment(Decision::PASS, None))
    }
}

#[cfg(test)]
mod tests;
