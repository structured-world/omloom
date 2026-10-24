//! Current-state persistence. The disk backend requires `std`; admission policy
//! types remain usable on an alloc-only target. This is a trusted host API,
//! not an authentication endpoint or an authorization implementation.
#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code, clippy::unwrap_used)]

/// Independently bounded data classes. Recovery checkpoints never expire with detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum Class {
    Inventory,
    Evidence,
    Pending,
    Work,
    Detail,
    Checkpoint,
}
pub const CLASSES: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    pub rows: u64,
    pub bytes: u64,
}

/// Logical application ceilings, including pinned rows. Physical engine files
/// and maintenance have a separate provider qualification, not this accounting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    pub classes: [Budget; CLASSES],
    pub total_bytes: u64,
    pub detail_age_ms: u64,
    pub batch_rows: u16,
}
impl Policy {
    pub fn valid(&self) -> bool {
        self.total_bytes > 0
            && self.total_bytes <= i64::MAX as u64
            && self.detail_age_ms > 0
            && self.detail_age_ms <= i64::MAX as u64
            && self.batch_rows > 0
            && self.batch_rows <= 128
            && self.classes.iter().all(|b| {
                b.rows > 0 && b.rows <= i64::MAX as u64 && b.bytes > 0 && b.bytes <= i64::MAX as u64
            })
    }
}

#[cfg(feature = "std")]
mod disk;
#[cfg(feature = "std")]
pub use disk::*;

#[cfg(test)]
mod tests;
