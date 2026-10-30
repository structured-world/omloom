use super::*;

const FIELD: &str = "current_addresses";
const LIMIT: usize = 64;
fn source_parent(target: AssetRef) -> String {
    format!(
        "{}/{}/address-sources",
        asset_key(target),
        target.incarnation
    )
}
pub(super) fn source_key(target: AssetRef, source: &str) -> String {
    current_facts::source_key(&source_parent(target), source)
}
pub(super) fn address_layer(target: AssetRef) -> String {
    format!(
        "interface-address/{}/{}",
        asset_key(target),
        target.incarnation
    )
}

pub(super) fn validate_address(value: &api::CurrentAddress) -> Result<()> {
    if value.address.len() > 45 {
        return Err(Error::Invalid);
    }
    let ip: std::net::IpAddr = value.address.parse().map_err(|_| Error::Invalid)?;
    if value.address != ip.to_string()
        || projection::local_address(ip)
        || value
            .prefix_length
            .is_some_and(|n| n > if ip.is_ipv4() { 32 } else { 128 })
        || !matches!(
            api::AddressStability::try_from(value.stability),
            Ok(api::AddressStability::Unspecified | api::AddressStability::Stable)
        )
        || api::AddressState::try_from(value.state).is_err()
    {
        return Err(Error::Invalid);
    }
    Ok(())
}

impl Tx<'_> {
    pub(super) fn native_address_delta(
        &self,
        context: &Context,
        value: &api::InterfaceAddresses,
        binding: &str,
    ) -> Result<Option<api::AddressDelta>> {
        self.address_delta_owned(
            context,
            parse_target(value.interface.as_ref())?,
            &value.source,
            &value.addresses,
            value.complete,
            Some(binding),
        )
    }
    fn address_delta_owned(
        &self,
        context: &Context,
        target: AssetRef,
        source: &str,
        incoming: &[api::CurrentAddress],
        complete: bool,
        binding: Option<&str>,
    ) -> Result<Option<api::AddressDelta>> {
        if !context.field(target, wire::AssetKind::Interface as i32, FIELD, false) {
            return Ok(None);
        }
        let key = source_layout::Family::Addresses.key(target, source, binding);
        let mut prior = BTreeMap::new();
        for row in self.fact_rows(&format!("{key}/values"), LIMIT)? {
            let value = api::CurrentAddress::decode(binary(&row, "payload")?)
                .map_err(|_| Error::Corrupt)?;
            prior.insert(value.address.clone(), value);
        }
        let mut delta = api::AddressDelta {
            source: source.into(),
            complete,
            ..Default::default()
        };
        for value in incoming {
            match prior.remove(&value.address) {
                Some(old) if old != *value => delta.updated.push(api::AddressAttributeChange {
                    before: Some(old),
                    after: Some(value.clone()),
                }),
                Some(_) => {}
                None => delta.added.push(value.clone()),
            }
        }
        if complete {
            delta.removed.extend(prior.into_values());
        }
        Ok(Some(delta))
    }

    pub(super) fn accept_addresses(&mut self, value: &api::InterfaceAddresses) -> Result<()> {
        self.accept_addresses_owned(value, None)
    }
    pub(super) fn accept_native_addresses(
        &mut self,
        context: &Context,
        value: &api::InterfaceAddresses,
        binding: &str,
    ) -> Result<Option<api::AddressDelta>> {
        self.native_source(
            parse_target(value.interface.as_ref())?,
            &value.source,
            binding,
            binding,
            source_layout::Family::Addresses,
        )?;
        let delta = self.native_address_delta(context, value, binding)?;
        self.accept_addresses_owned(value, Some(binding))?;
        Ok(delta)
    }
    fn accept_addresses_owned(
        &mut self,
        value: &api::InterfaceAddresses,
        binding: Option<&str>,
    ) -> Result<()> {
        let target = parse_target(value.interface.as_ref())?;
        let parent = source_parent(target);
        let key = source_layout::Family::Addresses.key(target, &value.source, binding);
        let sources = self.fact_rows(&parent, current_facts::MAX_SOURCES)?;
        let old = self.get(&key)?;
        if old.is_none() && sources.len() >= current_facts::MAX_SOURCES {
            return Err(Error::Limit);
        }
        let mut canonical = value.clone();
        canonical
            .addresses
            .sort_by(|a, b| a.address.cmp(&b.address));
        if canonical
            .addresses
            .windows(2)
            .any(|v| v[0].address == v[1].address)
        {
            return Err(Error::Invalid);
        }
        let digest = Sha256::digest(canonical.encode_to_vec()).to_vec();
        if let Some(old) = &old {
            let stamp = number(old, "observed")?;
            if stamp > value.observed_at
                || (stamp == value.observed_at && binary(old, "digest")? != digest)
            {
                return Err(Error::Conflict);
            }
        }
        let facts = canonical
            .addresses
            .iter()
            .map(|a| {
                (
                    a.address.clone(),
                    props([
                        ("layer", text(address_layer(target))),
                        ("payload", Value::Binary(a.encode_to_vec())),
                        ("owner", text(asset_key(target))),
                    ]),
                )
            })
            .collect();
        let (count, _) = self.replace_facts(
            &format!("{key}/values"),
            facts,
            value.complete,
            value.observed_at,
            "",
            LIMIT,
        )?;
        let other = sources
            .iter()
            .filter(|s| s.get("key") != Some(&text(&key)))
            .try_fold(0u64, |sum, s| {
                sum.checked_add(number(s, "count")?).ok_or(Error::Limit)
            })?;
        if other + count as u64 > LIMIT as u64 {
            return Err(Error::Limit);
        }
        let mut metadata = props([
            ("layer", text("address-source")),
            ("source", text(&value.source)),
            ("observed", int(value.observed_at)?),
            ("vflag", Value::Bool(value.complete)),
            ("count", int(count as u64)?),
            ("digest", Value::Binary(digest)),
        ]);
        if let Some(binding) = binding {
            metadata.insert("binding".into(), text(binding));
        }
        self.put_projection(&key, &parent, Class::Inventory, metadata, old.as_ref())?;
        Ok(())
    }

    pub(super) fn current_addresses(
        &self,
        context: &Context,
        target: AssetRef,
        kind: i32,
    ) -> Result<Vec<api::InterfaceAddressView>> {
        if kind != wire::AssetKind::Interface as i32 || !context.field(target, kind, FIELD, false) {
            return Ok(Vec::new());
        }
        let mut result = Vec::new();
        for meta in self.fact_rows(&source_parent(target), current_facts::MAX_SOURCES)? {
            let mut observations = Vec::new();
            let rows = self.fact_rows(&format!("{}/values", string(&meta, "key")?), LIMIT)?;
            let windows = self.fact_windows(&rows)?;
            for row in rows {
                let window = windows.get(&string(&row, "key")?).ok_or(Error::Corrupt)?;
                observations.push(api::AddressObservation {
                    value: Some(
                        api::CurrentAddress::decode(binary(&row, "payload")?)
                            .map_err(|_| Error::Corrupt)?,
                    ),
                    observed_at: if window.contains_key("observed") {
                        Some(number(window, "observed")?)
                    } else {
                        None
                    },
                });
            }
            observations.sort_by(|a, b| {
                a.value
                    .as_ref()
                    .map(|a| &a.address)
                    .cmp(&b.value.as_ref().map(|a| &a.address))
            });
            result.push(api::InterfaceAddressView {
                source_id: string(&meta, "key")?,
                binding_unresolved: meta.get("source_revision")
                    == Some(&text("unresolved-native-binding")),
                coverage: Some(api::InterfaceAddresses {
                    interface: Some(target_wire(target)),
                    source: string(&meta, "source")?,
                    observed_at: number(&meta, "observed")?,
                    addresses: Vec::new(),
                    complete: flag(&meta, "vflag")?,
                }),
                observations,
            });
        }
        Ok(result)
    }
}
impl Store {
    pub(super) fn workspace_interface_addresses(
        &self,
        context: &Context,
        value: api::InterfaceAddresses,
        working: &api::WorkingContext,
    ) -> Result<api::Reply> {
        if value.addresses.len() > LIMIT
            || value.source.is_empty()
            || value.source.len() > 100
            || value.source.chars().any(char::is_control)
            || value.observed_at == 0
            || value.observed_at > i64::MAX as u64
        {
            return Err(Error::Invalid);
        }
        for a in &value.addresses {
            validate_address(a)?;
        }
        let target = parse_target(value.interface.as_ref())?;
        let mut tx = self.begin()?;
        if tx.root.get("restored") == Some(&Value::Bool(true)) {
            return Err(Error::RecoveryRequired);
        }
        tx.selection(context, working, true)?
            .contains(target, &tx)?;
        let row = tx.visible_asset(context, target)?;
        if number(&row, "kind")? != wire::AssetKind::Interface as u64 {
            return Err(Error::Invalid);
        }
        if !context.field(target, wire::AssetKind::Interface as i32, FIELD, true) {
            return Err(Error::Denied);
        }
        tx.accept_addresses(&value)?;
        Ok(mutation(tx.commit()?, vec![target_wire(target)]))
    }
}
