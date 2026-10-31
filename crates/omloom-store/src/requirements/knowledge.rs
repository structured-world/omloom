//! Predicate comparison over current claims, independently of their producer.
use super::{Decision, Reason};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reference<'a> {
    pub issuer: &'a str,
    pub key: &'a str,
    pub basis: &'a str,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Atom<'a> {
    Text(&'a str),
    Integer(i64),
    Flag(bool),
    Reference(Reference<'a>),
}
#[derive(Clone, Copy, Debug)]
pub struct Quantity<'a> {
    pub lower: Option<i64>,
    pub upper: Option<i64>,
    pub unit: &'a str,
    pub scale: u32,
}
#[derive(Clone, Copy, Debug)]
pub enum Predicate<'a> {
    Is(Atom<'a>),
    OneOf(&'a [Atom<'a>]),
    Minimum {
        value: i64,
        unit: &'a str,
        scale: u32,
    },
    Maximum {
        value: i64,
        unit: &'a str,
        scale: u32,
    },
}

/// A borrowed, bounded view avoids copying a wire producer's candidate arrays.
/// Probabilities and scores deliberately supply no operational truth value.
pub trait Alternatives {
    fn candidates(&self) -> usize;
    fn candidate(&self, index: usize) -> Atom<'_>;
    fn exclusions(&self) -> usize;
    fn exclusion(&self, index: usize) -> Atom<'_>;
    fn exhaustive(&self) -> bool;
}
#[derive(Clone, Copy)]
pub enum Claim<'a, A> {
    Known(Atom<'a>),
    Quantity(Quantity<'a>),
    Alternatives(&'a A),
    Missing,
}
fn equal(a: Atom<'_>, b: Atom<'_>) -> Option<bool> {
    match (a, b) {
        (Atom::Text(a), Atom::Text(b)) => Some(a == b),
        (Atom::Integer(a), Atom::Integer(b)) => Some(a == b),
        (Atom::Flag(a), Atom::Flag(b)) => Some(a == b),
        (Atom::Reference(a), Atom::Reference(b)) => Some(a == b),
        _ => None,
    }
}
fn numeric(value: i64, scale: u32) -> i128 {
    // At most six places: every scaled i64 fits in i128, including MIN.
    debug_assert!(scale <= 6);
    i128::from(value) * 10i128.pow(6 - scale)
}
fn atom(value: Atom<'_>, predicate: Predicate<'_>) -> Decision {
    match predicate {
        Predicate::Is(expected) => match equal(value, expected) {
            Some(true) => Decision::PASS,
            Some(false) => Decision::FAIL,
            None => Decision::unknown(Reason::WrongEvidence),
        },
        Predicate::OneOf(values) => {
            if values.is_empty() || values.len() > 16 {
                return Decision::unknown(Reason::Incomplete);
            }
            let mut compatible_type = false;
            for &expected in values {
                match equal(value, expected) {
                    Some(true) => return Decision::PASS,
                    Some(false) => compatible_type = true,
                    None => {}
                }
            }
            if compatible_type {
                Decision::FAIL
            } else {
                Decision::unknown(Reason::WrongEvidence)
            }
        }
        Predicate::Minimum {
            value: expected,
            unit: "count",
            scale: 0,
        } => match value {
            Atom::Integer(value) if value >= expected => Decision::PASS,
            Atom::Integer(_) => Decision::FAIL,
            _ => Decision::unknown(Reason::WrongEvidence),
        },
        Predicate::Maximum {
            value: expected,
            unit: "count",
            scale: 0,
        } => match value {
            Atom::Integer(value) if value <= expected => Decision::PASS,
            Atom::Integer(_) => Decision::FAIL,
            _ => Decision::unknown(Reason::WrongEvidence),
        },
        _ => Decision::unknown(Reason::WrongEvidence),
    }
}

pub fn compare<A: Alternatives>(claim: Claim<'_, A>, predicate: Predicate<'_>) -> Decision {
    match claim {
        Claim::Missing => Decision::unknown(Reason::Missing),
        Claim::Known(value) => atom(value, predicate),
        Claim::Quantity(quantity) => {
            if quantity.scale > 6
                || matches!((quantity.lower, quantity.upper), (Some(a), Some(b)) if a > b)
            {
                return Decision::unknown(Reason::Conflict);
            }
            let (bound, unit, scale, minimum) = match predicate {
                Predicate::Minimum { value, unit, scale } => (value, unit, scale, true),
                Predicate::Maximum { value, unit, scale } => (value, unit, scale, false),
                Predicate::Is(Atom::Integer(value)) if quantity.unit == "count" => {
                    (value, "count", 0, true)
                }
                _ => return Decision::unknown(Reason::WrongEvidence),
            };
            if unit != quantity.unit || scale > 6 {
                return Decision::unknown(Reason::WrongEvidence);
            }
            let bound = numeric(bound, scale);
            let lower = quantity.lower.map(|n| numeric(n, quantity.scale));
            let upper = quantity.upper.map(|n| numeric(n, quantity.scale));
            if matches!(predicate, Predicate::Is(_)) {
                if lower == Some(bound) && upper == Some(bound) {
                    return Decision::PASS;
                }
                if lower.is_some_and(|n| n > bound) || upper.is_some_and(|n| n < bound) {
                    return Decision::FAIL;
                }
            } else if minimum {
                if lower.is_some_and(|n| n >= bound) {
                    return Decision::PASS;
                }
                if upper.is_some_and(|n| n < bound) {
                    return Decision::FAIL;
                }
            } else {
                if upper.is_some_and(|n| n <= bound) {
                    return Decision::PASS;
                }
                if lower.is_some_and(|n| n > bound) {
                    return Decision::FAIL;
                }
            }
            Decision::unknown(Reason::Incomplete)
        }
        Claim::Alternatives(values) => {
            let count = values.candidates();
            let exclusions = values.exclusions();
            if count > 16 || exclusions > 16 {
                return Decision::unknown(Reason::Incomplete);
            }
            for c in 0..count {
                for e in 0..exclusions {
                    if equal(values.candidate(c), values.exclusion(e)) == Some(true) {
                        return Decision::unknown(Reason::Conflict);
                    }
                }
            }
            // A supported exclusion can disprove an exact desired value even
            // when the candidate list is incomplete; a low score cannot.
            if let Predicate::Is(expected) = predicate {
                for e in 0..exclusions {
                    if equal(expected, values.exclusion(e)) == Some(true) {
                        return Decision::FAIL;
                    }
                }
            }
            if !values.exhaustive() || count == 0 {
                return Decision::unknown(Reason::Incomplete);
            }
            let mut pass = 0;
            let mut fail = 0;
            for i in 0..count {
                match atom(values.candidate(i), predicate).state {
                    super::State::Satisfied => pass += 1,
                    super::State::Failed => fail += 1,
                    super::State::Unknown => return Decision::unknown(Reason::WrongEvidence),
                }
            }
            if pass == count {
                Decision::PASS
            } else if fail == count {
                Decision::FAIL
            } else {
                Decision::unknown(Reason::Incomplete)
            }
        }
    }
}

fn supports<A: Alternatives>(claim: &Claim<'_, A>, value: Atom<'_>) -> Option<bool> {
    match claim {
        Claim::Missing => Some(true),
        Claim::Known(known) => equal(*known, value),
        Claim::Quantity(q) => match value {
            Atom::Integer(n) if q.unit == "count" && q.scale <= 6 => {
                let n = numeric(n, 0);
                Some(
                    q.lower.is_none_or(|low| numeric(low, q.scale) <= n)
                        && q.upper.is_none_or(|high| numeric(high, q.scale) >= n),
                )
            }
            _ => None,
        },
        Claim::Alternatives(set) => {
            if set.candidates() > 16 || set.exclusions() > 16 {
                return None;
            }
            for i in 0..set.exclusions() {
                if equal(set.exclusion(i), value) == Some(true) {
                    return Some(false);
                }
            }
            if !set.exhaustive() {
                return Some(true);
            }
            let mut comparable = false;
            for i in 0..set.candidates() {
                match equal(set.candidate(i), value) {
                    Some(true) => return Some(true),
                    Some(false) => comparable = true,
                    None => {}
                }
            }
            if comparable { Some(false) } else { None }
        }
    }
}

/// Intersect current source assertions before testing the desired predicate.
/// Two incompatible models cannot pass merely because both are allowed models.
pub fn consistent<A: Alternatives>(claims: &[Claim<'_, A>]) -> Decision {
    if claims.len() > 32 {
        return Decision::unknown(Reason::Incomplete);
    }
    let mut finite = None;
    for claim in claims {
        match claim {
            Claim::Known(_) => {
                finite = Some(claim);
                break;
            }
            Claim::Alternatives(set) if set.exhaustive() => finite = Some(claim),
            _ => {}
        }
    }
    if let Some(base) = finite {
        let count = match base {
            Claim::Known(_) => 1,
            Claim::Alternatives(set) => set.candidates(),
            _ => 0,
        };
        if count > 16 {
            return Decision::unknown(Reason::Incomplete);
        }
        let mut uncertain = false;
        for index in 0..count {
            let value = match base {
                Claim::Known(value) => *value,
                Claim::Alternatives(set) => set.candidate(index),
                _ => return Decision::unknown(Reason::WrongEvidence),
            };
            let mut possible = true;
            for claim in claims {
                match supports(claim, value) {
                    Some(true) => {}
                    Some(false) => {
                        possible = false;
                        break;
                    }
                    None => {
                        uncertain = true;
                        possible = false;
                        break;
                    }
                }
            }
            if possible {
                return Decision::PASS;
            }
        }
        return Decision::unknown(if uncertain {
            Reason::WrongEvidence
        } else {
            Reason::Conflict
        });
    }
    let mut unit = None;
    let mut low = i128::MIN;
    let mut high = i128::MAX;
    for claim in claims {
        if let Claim::Quantity(q) = claim {
            if q.scale > 6 || unit.is_some_and(|unit| unit != q.unit) {
                return Decision::unknown(Reason::WrongEvidence);
            }
            unit = Some(q.unit);
            if let Some(n) = q.lower {
                low = low.max(numeric(n, q.scale));
            }
            if let Some(n) = q.upper {
                high = high.min(numeric(n, q.scale));
            }
        }
    }
    if low > high {
        Decision::unknown(Reason::Conflict)
    } else {
        Decision::PASS
    }
}
