use crate::Error;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

/// Hard ceilings cannot be raised by callers. The schema has no recursive
/// messages; this additionally protects future extensions and malformed frames.
pub const MAX_BYTES: usize = 1_048_576;
pub const MAX_FIELDS: usize = 4096;
pub const MAX_DEPTH: usize = 16;
pub const MAX_MEMORY: usize = 16 * 1_048_576;

#[derive(Clone, Copy)]
pub struct Limits {
    pub bytes: usize,
    pub fields: usize,
    pub depth: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            bytes: MAX_BYTES,
            fields: MAX_FIELDS,
            depth: MAX_DEPTH,
        }
    }
}

#[derive(Clone)]
pub struct MemoryBudget(Arc<Inner>);
struct Inner {
    limit: usize,
    used: AtomicUsize,
}
impl MemoryBudget {
    pub fn new(limit: usize) -> Self {
        Self(Arc::new(Inner {
            limit,
            used: AtomicUsize::new(0),
        }))
    }
    pub fn used(&self) -> usize {
        self.0.used.load(Ordering::Acquire)
    }
    pub(crate) fn reserve(&self, size: usize) -> Result<Reservation, Error> {
        let mut used = self.0.used.load(Ordering::Acquire);
        loop {
            let total = used.checked_add(size).ok_or(Error::Limit)?;
            if total > self.0.limit {
                return Err(Error::Limit);
            }
            match self.0.used.compare_exchange_weak(
                used,
                total,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(current) => used = current,
            }
        }
        Ok(Reservation {
            budget: self.clone(),
            size,
        })
    }
}
pub(crate) struct Reservation {
    pub(crate) budget: MemoryBudget,
    size: usize,
}
impl Drop for Reservation {
    fn drop(&mut self) {
        self.budget.0.used.fetch_sub(self.size, Ordering::AcqRel);
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Text,
    Bytes,
    Varint(u64),
    Message(usize),
}
struct Shape {
    tag: u32,
    kind: Kind,
    repeated: bool,
    group: u32,
    max_count: u16,
}
include!(concat!(env!("OUT_DIR"), "/shapes.rs"));

fn varint(bytes: &mut &[u8]) -> Result<u64, Error> {
    // https://protobuf.dev/programming-guides/encoding/#varints : ten-byte u64.
    let mut result = 0;
    for shift in (0..70).step_by(7) {
        let (&byte, rest) = bytes.split_first().ok_or(Error::Wire)?;
        *bytes = rest;
        if shift == 63 && byte > 1 {
            return Err(Error::Wire);
        }
        result |= u64::from(byte & 127) << shift;
        if byte < 128 {
            return Ok(result);
        }
    }
    Err(Error::Wire)
}

struct Work {
    fields: usize,
    charge: usize,
    limits: Limits,
}
impl Work {
    fn field(&mut self) -> Result<(), Error> {
        if self.fields == self.limits.fields {
            return Err(Error::Limit);
        }
        self.fields += 1;
        // 2048 covers generated inline messages and Vec minimum/doubling
        // capacity (guarded by size assertions in tests), plus validation scratch.
        self.charge = self.charge.checked_add(2048).ok_or(Error::Limit)?;
        Ok(())
    }
    fn message(&mut self, mut bytes: &[u8], schema: usize, depth: usize) -> Result<(), Error> {
        if depth > self.limits.depth {
            return Err(Error::Limit);
        }
        let mut seen = 0u64;
        let mut groups = 0u64;
        let mut counts = [0u16; 64];
        while !bytes.is_empty() {
            self.field()?;
            let key = varint(&mut bytes)?;
            let tag = u32::try_from(key >> 3).map_err(|_| Error::Wire)?;
            let shape = SHAPES[schema]
                .iter()
                .find(|f| f.tag == tag)
                .ok_or(Error::UnknownField)?;
            let mask = 1u64.checked_shl(tag).ok_or(Error::Wire)?;
            if shape.max_count != 0 {
                let count = &mut counts[tag as usize];
                if *count == shape.max_count {
                    return Err(Error::Limit);
                }
                *count += 1;
            }
            if !shape.repeated && seen & mask != 0 {
                return Err(Error::DuplicateField);
            }
            if shape.group != 0 {
                let group = 1u64.checked_shl(shape.group).ok_or(Error::Wire)?;
                if groups & group != 0 {
                    return Err(Error::DuplicateField);
                }
                groups |= group;
            }
            seen |= mask;
            match (key & 7, shape.kind) {
                (0, Kind::Varint(max)) => {
                    if varint(&mut bytes)? > max {
                        return Err(Error::Wire);
                    }
                }
                (2, kind) => {
                    let len = usize::try_from(varint(&mut bytes)?).map_err(|_| Error::Wire)?;
                    let body = bytes.get(..len).ok_or(Error::Wire)?;
                    bytes = &bytes[len..];
                    match kind {
                        Kind::Message(child) => {
                            if child == EQUIPMENT_ROOT && body.len() > crate::equipment::MAX_BYTES {
                                return Err(Error::Limit);
                            }
                            self.message(body, child, depth + 1)?;
                        }
                        Kind::Text => {
                            std::str::from_utf8(body).map_err(|_| Error::Wire)?;
                        }
                        Kind::Bytes => (),
                        Kind::Varint(max) if shape.repeated => {
                            let mut packed = body;
                            while !packed.is_empty() {
                                self.field()?;
                                if varint(&mut packed)? > max {
                                    return Err(Error::Wire);
                                }
                            }
                        }
                        _ => return Err(Error::Wire),
                    }
                }
                _ => return Err(Error::Wire),
            }
        }
        Ok(())
    }
}

pub(crate) fn check(bytes: &[u8], limits: Limits) -> Result<usize, Error> {
    check_root(bytes, limits, ROOT)
}
pub(crate) fn check_workspace(bytes: &[u8], limits: Limits) -> Result<usize, Error> {
    check_root(bytes, limits, WORKSPACE_ROOT)
}
pub(crate) fn check_equipment(bytes: &[u8]) -> Result<usize, Error> {
    check_root(
        bytes,
        Limits {
            bytes: crate::equipment::MAX_BYTES,
            fields: 256,
            depth: 8,
        },
        EQUIPMENT_ROOT,
    )
}
fn check_root(bytes: &[u8], limits: Limits, root: usize) -> Result<usize, Error> {
    let limits = Limits {
        bytes: limits.bytes.min(MAX_BYTES),
        fields: limits.fields.min(MAX_FIELDS),
        depth: limits.depth.min(MAX_DEPTH),
    };
    if bytes.len() > limits.bytes {
        return Err(Error::Limit);
    }
    // Input copies and transient UTF-8 work are covered before decoder allocation.
    let mut work = Work {
        fields: 0,
        charge: bytes.len() * 2 + 4096,
        limits,
    };
    work.message(bytes, root, 1)?;
    if work.charge > MAX_MEMORY {
        return Err(Error::Limit);
    }
    Ok(work.charge)
}
