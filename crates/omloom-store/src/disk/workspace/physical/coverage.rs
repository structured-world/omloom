//! Current source completeness is independent of usable dimensions and derived availability.
use super::*;
use crate::physical::Region;
use crate::requirements::{Decision, Reason};

pub(super) fn write(row: &mut Properties, value: Option<&api::PhysicalVolume>) -> Result<()> {
    if let Some(value) = value {
        volume(value)?;
        let mut values = Vec::with_capacity(6);
        for axis in [&value.x, &value.y, &value.z] {
            let axis = axis.as_ref().ok_or(Error::Invalid)?;
            values.push(Value::Int(axis.start_um));
            values.push(int(axis.length_um)?);
        }
        row.insert("sizes".into(), Value::Array(values));
    }
    Ok(())
}
pub(super) fn read(row: &Properties) -> Result<Option<api::PhysicalVolume>> {
    let values = match row.get("sizes") {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Array(values)) if values.len() == 6 => values,
        _ => return Err(Error::Corrupt),
    };
    let axis = |index: usize| -> Result<Option<api::PhysicalExtent>> {
        Ok(Some(api::PhysicalExtent {
            start_um: values[index].as_int().ok_or(Error::Corrupt)?,
            length_um: u64::try_from(values[index + 1].as_int().ok_or(Error::Corrupt)?)
                .map_err(|_| Error::Corrupt)?,
        }))
    };
    let value = api::PhysicalVolume {
        x: axis(0)?,
        y: axis(2)?,
        z: axis(4)?,
    };
    volume(&value).map_err(|_| Error::Corrupt)?;
    Ok(Some(value))
}
pub(super) fn assess(coverage: Option<&api::PhysicalVolume>, region: Region) -> Result<Decision> {
    let Some(coverage) = coverage else {
        return Ok(Decision::unknown(Reason::Missing));
    };
    Ok(if region.within(volume(coverage)?) {
        Decision::PASS
    } else {
        Decision::unknown(Reason::Incomplete)
    })
}
