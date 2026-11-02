//! Six current margins use the existing typed numeric-array column; expanded geometry is never persisted.
use super::*;
use crate::physical::Region;

pub(super) fn write(row: &mut Properties, value: Option<&api::PhysicalClearance>) -> Result<()> {
    if let Some(v) = value {
        let values = [
            v.before_x_um,
            v.after_x_um,
            v.before_y_um,
            v.after_y_um,
            v.before_z_um,
            v.after_z_um,
        ];
        row.insert(
            "counts".into(),
            Value::Array(values.into_iter().map(int).collect::<Result<Vec<_>>>()?),
        );
    }
    Ok(())
}
pub(super) fn read(row: &Properties) -> Result<Option<api::PhysicalClearance>> {
    let values = match row.get("counts") {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Array(values)) if values.len() == 6 => values,
        _ => return Err(Error::Corrupt),
    };
    let number = |i: usize| -> Result<u64> {
        u64::try_from(values[i].as_int().ok_or(Error::Corrupt)?).map_err(|_| Error::Corrupt)
    };
    Ok(Some(api::PhysicalClearance {
        before_x_um: number(0)?,
        after_x_um: number(1)?,
        before_y_um: number(2)?,
        after_y_um: number(3)?,
        before_z_um: number(4)?,
        after_z_um: number(5)?,
    }))
}
pub(super) fn region(body: Volume, margin: Option<&api::PhysicalClearance>) -> Result<Region> {
    let service = match margin {
        None => return Ok(Region::unqualified(body)),
        Some(m) => Volume {
            x: body
                .x
                .expand(m.before_x_um, m.after_x_um)
                .map_err(|_| Error::Invalid)?,
            y: body
                .y
                .expand(m.before_y_um, m.after_y_um)
                .map_err(|_| Error::Invalid)?,
            z: body
                .z
                .expand(m.before_z_um, m.after_z_um)
                .map_err(|_| Error::Invalid)?,
        },
    };
    Region::new(body, service).map_err(|_| Error::Invalid)
}
// An explicitly requested allocation or enclosure coverage is already a region,
// not an equipment profile with missing service margins.
pub(super) fn allocation(body: Volume) -> Result<Region> {
    Region::new(body, body).map_err(|_| Error::Invalid)
}
impl Tx<'_> {
    pub(super) fn current_regions(
        &self,
        bodies: &[(AssetRef, Volume)],
        items: &BTreeSet<AssetRef>,
    ) -> Result<Vec<Region>> {
        if bodies.len() > 2048 || items.len() > 2048 {
            return Err(Error::Limit);
        }
        let mut margins = BTreeMap::new();
        if !items.is_empty() {
            let keys = items
                .iter()
                .map(|item| text(format!("physical/{}", network::identity(*item))))
                .collect();
            for row in self.query("UNWIND $keys AS key MATCH (n:OmRow {key:key}) WHERE n.counts IS NOT NULL RETURN {asset:n.asset,incarnation:n.incarnation,counts:n.counts} AS p LIMIT 2048",params([("keys",Value::Array(keys))]))?{
                if let Some(value)=read(&row)?{margins.insert(target(&row)?,value);}
            }
        }
        bodies
            .iter()
            .map(|(item, body)| region(*body, margins.get(item)))
            .collect()
    }
}
