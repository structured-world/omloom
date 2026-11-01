//! One bounded rail declaration on the current profile, no rows per rack unit.
use super::*;

fn validate(grid: &api::PhysicalRackGrid) -> Result<Span> {
    if grid.unit_count == 0
        || grid.first_unit == 0
        || grid.first_unit.checked_add(grid.unit_count - 1).is_none()
        || !matches!(
            api::PhysicalRackDirection::try_from(grid.direction),
            Ok(api::PhysicalRackDirection::TopDown | api::PhysicalRackDirection::BottomUp)
        )
    {
        return Err(Error::Invalid);
    }
    Span::new(grid.top_um, u64::from(grid.unit_count) * 44_450).map_err(|_| Error::Invalid)
}

pub(super) fn write(
    row: &mut Properties,
    value: Option<&api::PhysicalRackGrid>,
    envelope: Option<&api::PhysicalVolume>,
) -> Result<()> {
    let Some(value) = value else {
        return Ok(());
    };
    let rails = validate(value)?;
    if let Some(envelope) = envelope
        && !volume(envelope)?.y.contains(rails)
    {
        return Err(Error::Invalid);
    }
    // These four integers are consumed together by the selected rack's profile read.
    // No range index or materialized slot inventory is needed.
    row.insert("working".into(), Value::Binary(value.encode_to_vec()));
    Ok(())
}

pub(super) fn read(row: &Properties) -> Result<Option<api::PhysicalRackGrid>> {
    if !row.contains_key("working") {
        return Ok(None);
    }
    let bytes = binary(row, "working")?;
    if bytes.len() > 64 {
        return Err(Error::Corrupt);
    }
    let value = api::PhysicalRackGrid::decode(bytes).map_err(|_| Error::Corrupt)?;
    validate(&value).map_err(|_| Error::Corrupt)?;
    Ok(Some(value))
}
