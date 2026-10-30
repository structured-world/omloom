//! Bounded current equipment claims shared by wire, storage and import admission.
use crate::{Error, wire::*};
use alloc::vec::Vec;
use prost::Message;

pub const FIELDS: &[&str] = &[
    "manufacturer",
    "family",
    "generation",
    "model",
    "variant",
    "drive_bays",
    "drive_bus",
    "height",
    "width",
    "depth",
    "weight",
    "power_rating",
];
pub const MAX_BYTES: usize = 8192;
/// Field semantics shared by intent, source evidence and reviewed imports.
pub fn validate_field(name: &str, value: &EquipmentClaim) -> Result<(), Error> {
    validate(value)?;
    if !FIELDS.contains(&name) {
        return Err(Error::Value);
    }
    let unit = match name {
        "drive_bays" => Some("count"),
        "height" | "width" | "depth" => Some("mm"),
        "weight" => Some("g"),
        "power_rating" => Some("W"),
        _ => None,
    };
    match (&value.assertion, unit) {
        (Some(equipment_claim::Assertion::Quantity(q)), Some(expected)) if q.unit == expected => {
            Ok(())
        }
        (Some(equipment_claim::Assertion::Alternatives(_)), None) => Ok(()),
        (Some(equipment_claim::Assertion::Model(_)), None)
            if matches!(name, "model" | "variant") =>
        {
            Ok(())
        }
        _ => Err(Error::Value),
    }
}
/// Bounded decoded import with live accounting until the caller finishes use.
pub struct Input {
    value: EquipmentClaim,
    _reservation: crate::preflight::Reservation,
}
impl Input {
    pub fn take(&mut self) -> EquipmentClaim {
        core::mem::take(&mut self.value)
    }
}
pub fn decode(bytes: &[u8], budget: &crate::MemoryBudget) -> Result<Input, Error> {
    let charge = crate::preflight::check_equipment(bytes)?;
    let reservation = budget.reserve(charge)?;
    let value = EquipmentClaim::decode(bytes).map_err(|_| Error::Wire)?;
    validate(&value)?;
    Ok(Input {
        value,
        _reservation: reservation,
    })
}
/// Check stored claim size and structural work without allocating a decoded value.
/// Storage owns its result budget; semantic field validation follows decoding.
pub fn check_encoded(bytes: &[u8]) -> Result<(), Error> {
    crate::preflight::check_equipment(bytes).map(|_| ())
}
/// Decode a bounded CSV cell, reserving its temporary bytes before allocation.
pub fn decode_hex(hex: &str, budget: &crate::MemoryBudget) -> Result<Input, Error> {
    if hex.len() > MAX_BYTES * 2 {
        return Err(Error::Limit);
    }
    if !hex.len().is_multiple_of(2) {
        return Err(Error::Wire);
    }
    let _encoded = budget.reserve(hex.len() / 2)?;
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    fn digit(c: u8) -> Result<u8, Error> {
        match c {
            b'0'..=b'9' => Ok(c - b'0'),
            b'a'..=b'f' => Ok(c - b'a' + 10),
            b'A'..=b'F' => Ok(c - b'A' + 10),
            _ => Err(Error::Wire),
        }
    }
    for pair in hex.as_bytes().as_chunks::<2>().0 {
        bytes.push(digit(pair[0])? * 16 + digit(pair[1])?);
    }
    decode(&bytes, budget)
}
fn text(value: &str, max: usize, empty: bool) -> Result<(), Error> {
    if value.len() > max
        || (!empty && value.trim().is_empty())
        || value.chars().any(char::is_control)
    {
        return Err(Error::Value);
    }
    Ok(())
}
fn reference(value: &EquipmentReference) -> Result<(), Error> {
    text(&value.issuer, 160, false)?;
    text(&value.key, 160, false)?;
    text(&value.basis, 160, false)
}
/// Validate before canonicalization or allocating candidate indexes/copies.
pub fn validate(value: &EquipmentClaim) -> Result<(), Error> {
    use equipment_claim::Assertion as A;
    if value.encoded_len() > MAX_BYTES {
        return Err(Error::Limit);
    }
    text(&value.basis, 512, false)?;
    match value.assertion.as_ref().ok_or(Error::Missing)? {
        A::Model(v) => reference(v),
        A::Quantity(v) => {
            if v.scale > 6
                || !matches!(v.unit.as_str(), "count" | "mm" | "g" | "W")
                || (v.unit == "count" && v.scale != 0)
                || (v.lower.is_none() && v.upper.is_none())
                || v.lower.is_some_and(|n| n < 0)
                || v.upper.is_some_and(|n| n < 0)
                || matches!((v.lower,v.upper), (Some(a),Some(b)) if a>b)
            {
                return Err(Error::Value);
            }
            Ok(())
        }
        A::Alternatives(v) => {
            if v.candidates.len() > 16 || v.excluded.len() > 16 {
                return Err(Error::Limit);
            }
            if v.candidates.is_empty() && v.excluded.is_empty() {
                return Err(Error::Missing);
            }
            text(&v.domain, 256, true)?;
            if let Some(basis) = &v.calibration_basis {
                text(basis, 256, false)?;
            }
            let probabilities = v.candidates.iter().any(|c| c.probability_ppm.is_some());
            let scores = v.candidates.iter().any(|c| c.score_ppm.is_some());
            if probabilities && scores {
                return Err(Error::Value);
            }
            if probabilities || scores {
                text(&v.domain, 256, false)?;
            }
            let mut sum = 0u64;
            for (i, c) in v.candidates.iter().enumerate() {
                text(&c.value, 160, false)?;
                if let Some(r) = &c.reference {
                    reference(r)?;
                }
                if c.probability_ppm.is_some_and(|n| n > 1_000_000)
                    || c.score_ppm.is_some_and(|n| n > 1_000_000)
                    || (probabilities && c.probability_ppm.is_none())
                    || (scores && c.score_ppm.is_none())
                {
                    return Err(Error::Value);
                }
                if v.candidates[..i]
                    .iter()
                    .any(|old| same_identity(&old.value, &old.reference, &c.value, &c.reference))
                {
                    return Err(Error::Duplicate);
                }
                sum += u64::from(c.probability_ppm.unwrap_or(0));
            }
            if probabilities {
                if v.calibration_basis.is_none() {
                    return Err(Error::Missing);
                }
                let residual = v.residual_ppm.ok_or(Error::Missing)?;
                if sum + u64::from(residual) != 1_000_000
                    || (!v.exhaustive && residual == 0)
                    || (v.exhaustive && residual != 0)
                {
                    return Err(Error::Value);
                }
            } else if v.residual_ppm.is_some() || v.calibration_basis.is_some() {
                return Err(Error::Value);
            }
            if v.exhaustive && v.candidates.is_empty() {
                return Err(Error::Value);
            }
            for (i, e) in v.excluded.iter().enumerate() {
                text(&e.value, 160, false)?;
                text(&e.reason, 512, false)?;
                text(&e.coverage, 256, false)?;
                if let Some(r) = &e.reference {
                    reference(r)?;
                }
                if v.excluded[..i]
                    .iter()
                    .any(|old| same_identity(&old.value, &old.reference, &e.value, &e.reference))
                {
                    return Err(Error::Duplicate);
                }
            }
            Ok(())
        }
    }
}
fn same_identity(
    left: &str,
    left_ref: &Option<EquipmentReference>,
    right: &str,
    right_ref: &Option<EquipmentReference>,
) -> bool {
    match (left_ref, right_ref) {
        (Some(a), Some(b)) => a == b,
        (None, None) => left == right,
        _ => false,
    }
}
fn identity(value: &Option<EquipmentReference>) -> (&str, &str, &str) {
    match value {
        Some(r) => (&r.issuer, &r.key, &r.basis),
        None => ("", "", ""),
    }
}
/// One bounded semantic encoding for retries, changed-only storage and CSV.
/// Candidate/exclusion order is irrelevant; source and interpretation basis are not.
pub fn encode(value: &EquipmentClaim) -> Result<Vec<u8>, Error> {
    validate(value)?;
    let canonical = match &value.assertion {
        Some(equipment_claim::Assertion::Quantity(q)) => {
            q.scale == 0
                || q.lower.is_some_and(|n| n % 10 != 0)
                || q.upper.is_some_and(|n| n % 10 != 0)
        }
        Some(equipment_claim::Assertion::Alternatives(v)) => {
            v.candidates.is_sorted_by(|a, b| {
                (&a.value, identity(&a.reference)) <= (&b.value, identity(&b.reference))
            }) && v.excluded.is_sorted_by(|a, b| {
                (&a.value, identity(&a.reference)) <= (&b.value, identity(&b.reference))
            })
        }
        _ => true,
    };
    if canonical {
        return Ok(value.encode_to_vec());
    }
    let mut value = value.clone();
    if let Some(equipment_claim::Assertion::Quantity(q)) = &mut value.assertion {
        while q.scale > 0
            && q.lower.is_none_or(|n| n % 10 == 0)
            && q.upper.is_none_or(|n| n % 10 == 0)
        {
            if let Some(n) = &mut q.lower {
                *n /= 10;
            }
            if let Some(n) = &mut q.upper {
                *n /= 10;
            }
            q.scale -= 1;
        }
    }
    if let Some(equipment_claim::Assertion::Alternatives(v)) = &mut value.assertion {
        v.candidates.sort_unstable_by(|a, b| {
            (&a.value, identity(&a.reference)).cmp(&(&b.value, identity(&b.reference)))
        });
        v.excluded.sort_unstable_by(|a, b| {
            (&a.value, identity(&a.reference)).cmp(&(&b.value, identity(&b.reference)))
        });
    }
    Ok(value.encode_to_vec())
}

#[cfg(test)]
mod tests;
