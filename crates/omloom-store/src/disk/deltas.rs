//! Bounded inverse custody for an explicit inventory correction. Only changed
//! rows are captured, never a subtree, grants or external operation state.
use super::*;
use serde::{Deserialize, Serialize};

pub(super) const ROWS: usize = 128;
pub(super) const BYTES: usize = 256 * 1024;

#[derive(Default, Serialize, Deserialize)]
pub(super) struct Capture {
    pub rows: BTreeMap<String, Option<Properties>>,
    #[serde(skip)]
    bytes: usize,
}
impl Capture {
    pub fn record(&mut self, key: &str, old: Option<&Properties>, class: Class) -> Result<()> {
        let class = old
            .map(|p| number(p, "class"))
            .transpose()?
            .unwrap_or(class as u64);
        if ![Class::Inventory as u64, Class::Checkpoint as u64].contains(&class)
            || self.rows.contains_key(key)
        {
            return Ok(());
        }
        let size = old.map(|p| number(p, "size")).transpose()?.unwrap_or(0);
        let charge = usize::try_from(size)
            .map_err(|_| Error::Limit)?
            .checked_mul(6)
            .and_then(|v| v.checked_add(key.len() + 256))
            .ok_or(Error::Limit)?;
        let next = self.bytes.checked_add(charge).ok_or(Error::Limit)?;
        if self.rows.len() == ROWS || next > BYTES {
            return Err(Error::Limit);
        }
        self.bytes = next;
        self.rows.insert(key.into(), old.cloned());
        Ok(())
    }
    pub fn encode(&self) -> Result<Vec<u8>> {
        struct Output(Vec<u8>);
        impl std::io::Write for Output {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if bytes.len() > BYTES - self.0.len() {
                    return Err(std::io::ErrorKind::OutOfMemory.into());
                }
                self.0.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut output = Output(Vec::with_capacity(BYTES));
        serde_json::to_writer(&mut output, self).map_err(|_| Error::Limit)?;
        Ok(output.0)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > BYTES {
            return Err(Error::Corrupt);
        }
        let value: Self = serde_json::from_slice(bytes).map_err(|_| Error::Corrupt)?;
        if value.rows.len() > ROWS || value.bytes > BYTES {
            return Err(Error::Corrupt);
        }
        Ok(value)
    }
}
