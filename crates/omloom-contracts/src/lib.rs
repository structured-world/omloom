//! Protobuf is an untrusted transport representation. Admit a bounded record
//! with [`decode`]; generated values alone provide no domain or authority proof.
//!
//! Host code cannot substitute an incarnation ID for an asset ID:
//! ```compile_fail
//! use omloom_contracts::ids::{AssetId, IncarnationId};
//! let asset: AssetId = IncarnationId::generate();
//! ```
//!
//! An omitted body is rejected, rather than becoming an empty accepted record:
//! ```
//! use omloom_contracts::{decode, Error, Limits, MemoryBudget};
//! let budget = MemoryBudget::new(16 * 1_048_576);
//! assert_eq!(decode(&[], Limits::default(), &budget).err(), Some(Error::Missing));
//! assert_eq!(budget.used(), 0);
//! ```
#![deny(unsafe_code)]

pub mod wire {
    include!(concat!(env!("OUT_DIR"), "/omloom.v1.rs"));
}
pub mod workspace {
    include!(concat!(env!("OUT_DIR"), "/omloom.workspace.v1.rs"));
}
pub mod ids;
mod preflight;
mod validate;

pub use preflight::{Limits, MemoryBudget};
use prost::Message;

/// Stable rejection classes, with no attacker-controlled content in diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Limit,
    Wire,
    UnknownField,
    DuplicateField,
    Missing,
    Identifier,
    Value,
    Enumeration,
    Duplicate,
    Cycle,
    Reference,
    Outcome,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Error {}

/// Validated immutable value. Its conservative memory reservation is retained
/// until drop. No Clone or mutable access can escape that accounting lifetime.
pub struct Record {
    value: wire::Record,
    _reservation: preflight::Reservation,
}

/// Bounded application frame. Keep this owner alive while executing its request;
/// the reservation covers decoding and bounded validation scratch together.
pub struct WorkspaceRequest {
    value: workspace::Request,
    _reservation: preflight::Reservation,
}
impl WorkspaceRequest {
    pub fn take(&mut self) -> workspace::Request {
        core::mem::take(&mut self.value)
    }
}
pub fn decode_workspace(
    bytes: &[u8],
    limits: Limits,
    budget: &MemoryBudget,
) -> Result<WorkspaceRequest, Error> {
    let charge = preflight::check_workspace(bytes, limits)?;
    let reservation = budget.reserve(charge)?;
    let value = workspace::Request::decode(bytes).map_err(|_| Error::Wire)?;
    if value.command.is_none() {
        return Err(Error::Missing);
    }
    Ok(WorkspaceRequest {
        value,
        _reservation: reservation,
    })
}
impl Record {
    pub fn wire(&self) -> &wire::Record {
        &self.value
    }

    /// Encode under the same live memory budget, charging capacity before allocation.
    pub fn encode(&self) -> Result<Encoded, Error> {
        let len = self.value.encoded_len();
        let reservation = self._reservation.budget.reserve(len)?;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(len).map_err(|_| Error::Limit)?;
        self.value.encode(&mut bytes).map_err(|_| Error::Wire)?;
        Ok(Encoded {
            bytes,
            _reservation: reservation,
        })
    }
}

pub struct Encoded {
    bytes: Vec<u8>,
    _reservation: preflight::Reservation,
}
impl AsRef<[u8]> for Encoded {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}

/// Preflight borrows the frame without allocating. It bounds parser work,
/// nesting and a conservative allocation envelope *before* prost runs.
/// Domain validation is also shared by every caller of this entry point.
pub fn decode(bytes: &[u8], limits: Limits, budget: &MemoryBudget) -> Result<Record, Error> {
    let charge = preflight::check(bytes, limits)?;
    let reservation = budget.reserve(charge)?;
    let value = wire::Record::decode(bytes).map_err(|_| Error::Wire)?;
    validate::record(&value)?;
    Ok(Record {
        value,
        _reservation: reservation,
    })
}
