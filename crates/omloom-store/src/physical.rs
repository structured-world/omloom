//! Integer physical constraints shared by planning and final admission.
//! Geometry proves only the supplied constraints, never authority or live health.
use crate::requirements::{Decision, Reason};
pub mod power;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatingRole {
    SideA,
    SideB,
    SelfMating,
}
#[derive(Clone, Copy, Debug)]
pub struct Mating<'a> {
    issuer: &'a str,
    key: &'a str,
    basis: &'a str,
    role: MatingRole,
}
impl<'a> Mating<'a> {
    pub fn new(
        issuer: &'a str,
        key: &'a str,
        basis: &'a str,
        role: MatingRole,
    ) -> Result<Self, Invalid> {
        for value in [issuer, key, basis] {
            if value.trim().is_empty() || value.chars().any(char::is_control) {
                return Err(Invalid::Empty);
            }
            if value.len() > 160 {
                return Err(Invalid::Limit);
            }
        }
        Ok(Self {
            issuer,
            key,
            basis,
            role,
        })
    }
}
/// Compare complete declarations, not connector labels or inferred model similarity.
pub fn mating(left: Option<Mating<'_>>, right: Option<Mating<'_>>) -> Decision {
    let (Some(left), Some(right)) = (left, right) else {
        return Decision::unknown(Reason::Missing);
    };
    if left.issuer != right.issuer || left.key != right.key || left.basis != right.basis {
        return Decision::unknown(Reason::Unconfigured);
    }
    match (left.role, right.role) {
        (MatingRole::SideA, MatingRole::SideB)
        | (MatingRole::SideB, MatingRole::SideA)
        | (MatingRole::SelfMating, MatingRole::SelfMating) => Decision::PASS,
        _ => Decision::FAIL,
    }
}

/// Micrometres. Signed coordinates permit an explicit clearance outside a chassis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    start: i64,
    end: i64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Invalid {
    Empty,
    Overflow,
    Reversed,
    Limit,
}
impl Span {
    pub fn new(start: i64, length: u64) -> Result<Self, Invalid> {
        if length == 0 {
            return Err(Invalid::Empty);
        }
        let length = i64::try_from(length).map_err(|_| Invalid::Overflow)?;
        let end = start.checked_add(length).ok_or(Invalid::Overflow)?;
        Ok(Self { start, end })
    }
    pub fn start(self) -> i64 {
        self.start
    }
    pub fn end(self) -> i64 {
        self.end
    }
    pub fn overlaps(self, other: Self) -> bool {
        self.start < other.end && other.start < self.end
    }
    pub fn contains(self, other: Self) -> bool {
        self.start <= other.start && other.end <= self.end
    }
    pub fn expand(self, before: u64, after: u64) -> Result<Self, Invalid> {
        let before = i64::try_from(before).map_err(|_| Invalid::Overflow)?;
        let after = i64::try_from(after).map_err(|_| Invalid::Overflow)?;
        Ok(Self {
            start: self.start.checked_sub(before).ok_or(Invalid::Overflow)?,
            end: self.end.checked_add(after).ok_or(Invalid::Overflow)?,
        })
    }
}
/// A half-open physical volume in its owning enclosure's coordinate system.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Volume {
    pub x: Span,
    pub y: Span,
    pub z: Span,
}
impl Volume {
    pub fn overlaps(self, other: Self) -> bool {
        self.x.overlaps(other.x) && self.y.overlaps(other.y) && self.z.overlaps(other.z)
    }
    pub fn contains(self, other: Self) -> bool {
        self.x.contains(other.x) && self.y.contains(other.y) && self.z.contains(other.z)
    }
}
/// Material and its declared service space are distinct: service spaces may overlap each other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    body: Volume,
    service: Option<Volume>,
}
impl Region {
    pub fn new(body: Volume, service: Volume) -> Result<Self, Invalid> {
        if !service.contains(body) {
            return Err(Invalid::Reversed);
        }
        Ok(Self {
            body,
            service: Some(service),
        })
    }
    /// Known material body without a qualified service envelope.
    pub const fn unqualified(body: Volume) -> Self {
        Self {
            body,
            service: None,
        }
    }
    /// True only when the declared service envelope is known to fit.
    pub fn within(self, space: Volume) -> bool {
        match self.service {
            Some(service) => space.contains(service),
            None => false,
        }
    }
}
/// A sourced interval, retaining uncertainty instead of averaging it away.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Quantity {
    lower: u64,
    upper: u64,
}
impl Quantity {
    pub fn new(lower: u64, upper: u64) -> Result<Self, Invalid> {
        if lower > upper {
            return Err(Invalid::Reversed);
        }
        Ok(Self { lower, upper })
    }
    pub fn lower(self) -> u64 {
        self.lower
    }
    pub fn upper(self) -> u64 {
        self.upper
    }
    pub fn checked_sum(self, other: Self) -> Result<Self, Invalid> {
        Self::new(
            self.lower
                .checked_add(other.lower)
                .ok_or(Invalid::Overflow)?,
            self.upper
                .checked_add(other.upper)
                .ok_or(Invalid::Overflow)?,
        )
    }
}
/// Capacity and demand must already use the same explicit unit and measurement kind.
pub fn capacity(capacity: Option<Quantity>, demand: Option<Quantity>) -> Decision {
    let (Some(capacity), Some(demand)) = (capacity, demand) else {
        return Decision::unknown(Reason::Missing);
    };
    if demand.upper <= capacity.lower {
        return Decision::PASS;
    }
    if demand.lower > capacity.upper {
        return Decision::FAIL;
    }
    Decision::unknown(Reason::Incomplete)
}
/// Compare one candidate with an already bounded set of occupied/reserved volumes.
/// The storage owner supplies authoritative occupants, including hidden ones.
pub fn placement(
    enclosure: Option<Volume>,
    candidate: Option<Region>,
    occupied: &[Region],
) -> Decision {
    if occupied.len() > 2048 {
        return Decision::unknown(Reason::Unsupported);
    }
    let (Some(enclosure), Some(candidate)) = (enclosure, candidate) else {
        return Decision::unknown(Reason::Missing);
    };
    let mut qualified = true;
    if let Some(service) = candidate.service {
        if !enclosure.contains(service) {
            return Decision::FAIL;
        }
    } else {
        qualified = false;
        if !enclosure.contains(candidate.body) {
            return Decision::FAIL;
        }
    }
    for volume in occupied {
        if let Some(service) = volume.service {
            if candidate.body.overlaps(service) {
                return Decision::FAIL;
            }
        } else {
            qualified = false;
            if candidate.body.overlaps(volume.body) {
                return Decision::FAIL;
            }
        }
        if let Some(service) = candidate.service
            && service.overlaps(volume.body)
        {
            return Decision::FAIL;
        }
    }
    if qualified {
        Decision::PASS
    } else {
        Decision::unknown(Reason::Missing)
    }
}

#[cfg(test)]
mod tests;
