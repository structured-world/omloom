//! Explicit disclosure profiles derive availability from current authoritative bindings.
use super::*;
use crate::requirements::{Decision, Reason, State};

fn offer_key(id: &str) -> Result<String> {
    id.parse::<AuthorityId>().map_err(|_| Error::Invalid)?;
    Ok(format!("physical-offer/{id}"))
}
fn offer_parent(enclosure: AssetRef) -> String {
    format!("physical-offers/{}", network::identity(enclosure))
}
fn offer(row: &Properties) -> Result<api::PhysicalOffer> {
    let flags = number(row, "vint")?;
    if flags > 7 {
        return Err(Error::Corrupt);
    }
    let key = string(row, "key")?;
    Ok(api::PhysicalOffer {
        id: Some(
            key.strip_prefix("physical-offer/")
                .ok_or(Error::Corrupt)?
                .into(),
        ),
        enclosure: Some(target_wire(target(row)?)),
        region: read_volume(row)?,
        recipient: string(row, "source")?,
        name: string(row, "name")?,
        revision: number(row, "revision")?,
        revoked: row.get("vflag") == Some(&Value::Bool(true)),
        mass: flags & 1 != 0,
        power: flags & 2 != 0,
        cooling: flags & 4 != 0,
    })
}
impl Tx<'_> {
    fn offer_publisher(
        &self,
        context: &Context,
        enclosure: AssetRef,
        working: &api::WorkingContext,
        write: bool,
    ) -> Result<()> {
        self.selection(context, working, write)?
            .contains(enclosure, self)?;
        let row = self.visible_asset(context, enclosure)?;
        let kind = number(&row, "kind")? as i32;
        if !context.field(enclosure, kind, "physical_availability", write) {
            return Err(Error::Denied);
        }
        if !matches!(
            wire::AssetKind::try_from(kind),
            Ok(wire::AssetKind::Rack | wire::AssetKind::PhysicalPosition | wire::AssetKind::Device)
        ) {
            return Err(Error::Invalid);
        }
        Ok(())
    }
    pub(super) fn save_physical_offer(
        &mut self,
        context: &Context,
        value: api::PhysicalOfferEdit,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<api::PhysicalOffer> {
        let mut value = value.offer.ok_or(Error::Invalid)?;
        let enclosure = parse_target(value.enclosure.as_ref())?;
        self.offer_publisher(context, enclosure, working, true)?;
        for (value, max) in [(&value.name, 160), (&value.recipient, 128)] {
            if value.trim().is_empty() || value.len() > max || value.chars().any(char::is_control) {
                return Err(Error::Invalid);
            }
        }
        let region = volume(value.region.as_ref().ok_or(Error::Invalid)?)?;
        if !value.revoked {
            let root = self.asset(enclosure)?;
            if !context.field(enclosure, number(&root, "kind")? as i32, "physical", false) {
                return Err(Error::Denied);
            }
            let definition = self
                .physical_definition(enclosure)?
                .ok_or(Error::Incomplete)?;
            let envelope = definition
                .volume
                .as_ref()
                .map(volume)
                .transpose()?
                .ok_or(Error::Incomplete)?;
            if !envelope.contains(region) {
                return Err(Error::Invalid);
            }
        }
        let key = match &value.id {
            Some(id) => offer_key(id)?,
            None => format!("physical-offer/{}", AuthorityId::generate()),
        };
        if let Some(prior) = self.get(&key)? {
            if string(&prior, "owner")? != context.actor {
                return Err(Error::Denied);
            }
            if target(&prior)? != enclosure || number(&prior, "revision")? != value.revision {
                return Err(Error::Conflict);
            }
            if offer(&prior)? == value {
                return Ok(value);
            }
        } else {
            if value.id.is_some() || value.revision != 0 {
                return Err(Error::Missing);
            }
            let rows = self.query(
                "MATCH (n:OmRow {parent:$parent}) RETURN {key:n.key} AS p LIMIT 65",
                params([("parent", text(offer_parent(enclosure)))]),
            )?;
            if rows.len() >= 64 {
                return Err(Error::Limit);
            }
        }
        value.revision = number(&self.root, "revision")?
            .checked_add(1)
            .ok_or(Error::Limit)?;
        let mut row = target_props(enclosure);
        row.extend(props([
            ("owner", text(&context.actor)),
            ("name", text(&value.name)),
            ("source", text(&value.recipient)),
            ("vflag", Value::Bool(value.revoked)),
            (
                "vint",
                int(u64::from(value.mass)
                    | u64::from(value.power) << 1
                    | u64::from(value.cooling) << 2)?,
            ),
            ("revision", int(value.revision)?),
        ]));
        write_volume(&mut row, value.region.as_ref().ok_or(Error::Invalid)?)?;
        self.put(&key, &offer_parent(enclosure), Class::Inventory, row)?;
        self.audit(&key, &context.actor, "physical-offer", value.revision, now)?;
        value.id = Some(
            key.strip_prefix("physical-offer/")
                .ok_or(Error::Corrupt)?
                .into(),
        );
        Ok(value)
    }
    pub(super) fn physical_offers(
        &self,
        context: &Context,
        request: api::PhysicalOffers,
        working: &api::WorkingContext,
    ) -> Result<(Vec<api::PhysicalOffer>, Option<String>)> {
        let enclosure = parse_target(request.enclosure.as_ref())?;
        self.offer_publisher(context, enclosure, working, false)?;
        let after = request
            .after
            .as_deref()
            .map(offer_key)
            .transpose()?
            .unwrap_or_default();
        let rows=self.query("MATCH (n:OmRow {parent:$parent}) WHERE n.owner=$owner AND n.key>$after RETURN properties(n) AS p ORDER BY n.key LIMIT 26",params([("parent",text(offer_parent(enclosure))),("owner",text(&context.actor)),("after",text(after))]))?;
        let offers = rows
            .iter()
            .take(25)
            .map(offer)
            .collect::<Result<Vec<_>>>()?;
        let next = if rows.len() > 25 {
            offers.last().and_then(|o| o.id.clone())
        } else {
            None
        };
        Ok((offers, next))
    }
    pub(super) fn offered_availability(
        &self,
        context: &Context,
        request: api::PhysicalOfferRead,
    ) -> Result<(api::PhysicalOffer, Decision)> {
        let row = self.get(&offer_key(&request.id)?)?.ok_or(Error::Denied)?;
        let enclosure = target(&row)?;
        let root = self.asset(enclosure)?;
        if (string(&row, "source")? != context.actor && string(&row, "owner")? != context.actor)
            || !context.field(
                enclosure,
                number(&root, "kind")? as i32,
                "physical_availability",
                false,
            )
            || row.get("vflag") == Some(&Value::Bool(true))
        {
            return Err(Error::Denied);
        }
        let offer = offer(&row)?;
        if self.root.get("restored") == Some(&Value::Bool(true)) {
            return Ok((offer, Decision::unknown(Reason::Stale)));
        }
        let assessment = (|| -> Result<Decision> {
            let occupied = self.physical_use(
                enclosure,
                &api::PhysicalPlan::default(),
                &[0; crate::plans::MAX_STEPS],
                2048,
            )?;
            let definition = &occupied.definition;
            let region =
                clearance::region(volume(offer.region.as_ref().ok_or(Error::Corrupt)?)?, None)?;
            let state = crate::physical::placement(
                definition.volume.as_ref().map(volume).transpose()?,
                Some(region),
                &occupied.volumes,
            );
            if state.state != State::Satisfied {
                return Ok(state);
            }
            let knowledge = coverage::assess(definition.occupancy_coverage.as_deref(), region)?;
            if knowledge.state != State::Satisfied {
                return Ok(knowledge);
            }
            let selected = [offer.mass, offer.power, offer.cooling];
            if selected.iter().any(|&v| v) {
                let envelope = definition
                    .volume
                    .as_ref()
                    .map(volume)
                    .transpose()?
                    .ok_or(Error::Incomplete)?;
                let knowledge = coverage::assess(
                    definition.occupancy_coverage.as_deref(),
                    clearance::region(envelope, None)?,
                )?;
                if knowledge.state != State::Satisfied {
                    return Ok(knowledge);
                }
                let mut demand = self.physical_loads(&occupied.items, selected)?;
                for future in &occupied.future_loads {
                    equipment::add_loads(&mut demand, *future)?;
                }
                for ((selected, limit), demand) in selected
                    .into_iter()
                    .zip([
                        definition.max_mass_g,
                        definition.capacity_mw,
                        definition.cooling_capacity_mw,
                    ])
                    .zip(demand)
                {
                    if !selected {
                        continue;
                    }
                    let (Some(limit), Some(demand)) = (limit, demand) else {
                        return Ok(Decision::unknown(Reason::Missing));
                    };
                    if demand > limit {
                        return Ok(Decision::FAIL);
                    }
                }
            }
            Ok(Decision::PASS)
        })();
        let result = match assessment {
            Ok(result) => result,
            Err(Error::Incomplete | Error::Missing) => Decision::unknown(Reason::Missing),
            Err(Error::Limit) => Decision::unknown(Reason::Incomplete),
            Err(error) => return Err(error),
        };
        Ok((offer, result))
    }
    pub(in crate::disk::workspace) fn remove_physical_offers(
        &mut self,
        enclosure: AssetRef,
    ) -> Result<()> {
        let rows = self.query(
            "MATCH (n:OmRow {parent:$parent}) RETURN {key:n.key} AS p LIMIT 65",
            params([("parent", text(offer_parent(enclosure)))]),
        )?;
        if rows.len() > 64 {
            return Err(Error::Corrupt);
        }
        for row in rows {
            self.delete(&string(&row, "key")?)?;
        }
        Ok(())
    }
}
