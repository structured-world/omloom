//! Physical consumers read the canonical accepted equipment claims, without copying them.
use super::*;

fn upper(row: Option<&Properties>, scale: u32) -> Result<Option<u64>> {
    let Some(row) = row else {
        return Ok(None);
    };
    let field = inventory::read_field(row)?;
    let Some(field::Value::Equipment(claim)) = field.value else {
        return Ok(None);
    };
    let Some(wire::equipment_claim::Assertion::Quantity(quantity)) = claim.assertion else {
        return Ok(None);
    };
    let Some(value) = quantity.upper else {
        return Ok(None);
    };
    let value = u64::try_from(value).map_err(|_| Error::Corrupt)?;
    let value = if quantity.scale <= scale {
        value
            .checked_mul(10_u64.pow(scale - quantity.scale))
            .ok_or(Error::Limit)?
    } else {
        let divisor = 10_u64.pow(quantity.scale - scale);
        value / divisor + u64::from(value % divisor != 0)
    };
    Ok(Some(value))
}
impl Tx<'_> {
    pub(super) fn equipment_envelope(
        &self,
        context: &Context,
        item: AssetRef,
    ) -> Result<Option<api::PhysicalVolume>> {
        let root = self.visible_asset(context, item)?;
        let kind = i32::try_from(number(&root, "kind")?).map_err(|_| Error::Corrupt)?;
        for name in ["width", "height", "depth"] {
            if !context.field(item, kind, name, false) {
                return Err(Error::Denied);
            }
        }
        let prefix = format!("{}/intent/", asset_key(item));
        let keys = ["width", "height", "depth"].map(|name| text(format!("{prefix}{name}")));
        let rows = self.query(
            "UNWIND $keys AS key MATCH (p:OmRow {key:key}) RETURN properties(p) AS p LIMIT 3",
            params([("keys", Value::Array(keys.into()))]),
        )?;
        let mut axes = [None; 3];
        for row in rows {
            let name = string(&row, "field")?;
            let index = match name.as_str() {
                "width" => 0,
                "height" => 1,
                "depth" => 2,
                _ => return Err(Error::Corrupt),
            };
            axes[index] = upper(Some(&row), 3)?;
        }
        let [Some(x), Some(y), Some(z)] = axes else {
            return Ok(None);
        };
        if x == 0 || y == 0 || z == 0 {
            return Ok(None);
        }
        let axis = |length_um| {
            Some(api::PhysicalExtent {
                start_um: 0,
                length_um,
            })
        };
        Ok(Some(api::PhysicalVolume {
            x: axis(x),
            y: axis(y),
            z: axis(z),
        }))
    }
    pub(super) fn physical_loads(
        &self,
        items: &BTreeSet<AssetRef>,
        needed: [bool; 3],
    ) -> Result<[Option<u64>; 3]> {
        if items.len() > 2048 {
            return Err(Error::Limit);
        }
        let mut keys = Vec::with_capacity(
            items.len() * (usize::from(needed[0]) + usize::from(needed[1] || needed[2])),
        );
        for item in items {
            if needed[1] || needed[2] {
                keys.push(text(format!("physical/{}", network::identity(*item))));
            }
            if needed[0] {
                keys.push(text(format!("{}/intent/weight", asset_key(*item))));
            }
        }
        let mut rows = BTreeMap::new();
        if !keys.is_empty() {
            for row in self.query("UNWIND $keys AS key MATCH (p:OmRow {key:key}) RETURN properties(p) AS p LIMIT 4096",params([("keys",Value::Array(keys))]))? {
                rows.insert(string(&row,"key")?,row);
            }
        }
        let mut total = needed.map(|needed| if needed { Some(0_u64) } else { None });
        for item in items {
            let profile = rows.get(&format!("physical/{}", network::identity(*item)));
            let weight = if needed[0] {
                upper(rows.get(&format!("{}/intent/weight", asset_key(*item))), 0)?
            } else {
                None
            };
            let quantity = |key: &str| {
                profile
                    .filter(|row| row.contains_key(key))
                    .map(|row| number(row, key))
                    .transpose()
            };
            for (sum, value) in total.iter_mut().zip([
                weight,
                if needed[1] {
                    quantity("demand_mw")?
                } else {
                    None
                },
                if needed[2] {
                    quantity("cooling_mw")?
                } else {
                    None
                },
            ]) {
                *sum = match (*sum, value) {
                    (Some(a), Some(b)) => Some(a.checked_add(b).ok_or(Error::Limit)?),
                    _ => None,
                };
            }
        }
        Ok(total)
    }
    pub(super) fn physical_equipment_projection(
        &self,
        context: &Context,
        item: AssetRef,
        view: &mut api::PhysicalView,
    ) -> Result<()> {
        let root = self.visible_asset(context, item)?;
        let kind = i32::try_from(number(&root, "kind")?).map_err(|_| Error::Corrupt)?;
        if !matches!(
            wire::AssetKind::try_from(kind),
            Ok(wire::AssetKind::Device | wire::AssetKind::PhysicalComponent)
        ) {
            return Ok(());
        }
        let mut keys = Vec::with_capacity(4);
        for name in ["width", "height", "depth", "weight"] {
            if context.field(item, kind, name, false) {
                keys.push(text(format!("{}/intent/{name}", asset_key(item))));
            }
        }
        let mut fields = Vec::with_capacity(keys.len() + 1);
        if !keys.is_empty() {
            for row in self.query(
                "UNWIND $keys AS key MATCH (p:OmRow {key:key}) RETURN properties(p) AS p LIMIT 4",
                params([("keys", Value::Array(keys))]),
            )? {
                fields.push(inventory::read_field(&row)?);
            }
        }
        if context.field(item, kind, "name", false)
            && let Some(name) = root.get("name").and_then(Value::as_str)
        {
            fields.push(wire::Field {
                name: "name".into(),
                value: Some(field::Value::Text(name.into())),
            });
        }
        view.items.push(api::Item {
            target: Some(target_wire(item)),
            kind,
            revision: number(&root, "revision")?,
            fields,
        });
        Ok(())
    }
}
