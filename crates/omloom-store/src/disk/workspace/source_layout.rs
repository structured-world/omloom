//! Source identity is a qualified binding, independent of its display label.
//! Relocation preserves individual observation windows and current-only state.
use super::*;
#[cfg(test)]
mod tests;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Family {
    Addresses,
    Network,
}
impl Family {
    pub fn name(self) -> &'static str {
        match self {
            Self::Addresses => "address-sources",
            Self::Network => "network-sources",
        }
    }
    fn parts(self) -> &'static [&'static str] {
        match self {
            Self::Addresses => &["values"],
            Self::Network => &["sockets", "routes", "rules", "policies"],
        }
    }
    pub fn parent(self, target: AssetRef) -> String {
        format!(
            "{}/{}/{}",
            asset_key(target),
            target.incarnation,
            self.name()
        )
    }
    pub fn key(self, target: AssetRef, name: &str, binding: Option<&str>) -> String {
        match binding {
            Some(binding) => current_facts::source_key(&format!("native/{}", self.name()), binding),
            None => current_facts::source_key(&self.parent(target), name),
        }
    }
}
pub(super) struct Relocation<'a> {
    pub from: AssetRef,
    pub to: AssetRef,
    pub old: &'a str,
    pub new: &'a str,
    pub name: &'a str,
    pub binding: Option<&'a str>,
    pub family: Family,
}
fn data(mut row: Properties) -> Properties {
    operations::strip_envelope(&mut row);
    row
}

impl Tx<'_> {
    pub(super) fn native_source(
        &mut self,
        target: AssetRef,
        name: &str,
        binding: &str,
        base_binding: &str,
        family: Family,
    ) -> Result<String> {
        let key = family.key(target, name, Some(binding));
        if let Some(current) = self.get(&key)? {
            if string(&current, "parent")? != family.parent(target)
                || current.get("binding") != Some(&text(binding))
            {
                return Err(Error::Conflict);
            }
            return Ok(key);
        }
        let legacy = family.key(target, name, None);
        let Some(old) = self.get(&legacy)? else {
            return Ok(key);
        };
        let suffix = if family == Family::Network {
            Some(
                binding
                    .strip_prefix(&format!("{base_binding}/"))
                    .ok_or(Error::Invalid)?,
            )
        } else {
            None
        };
        let mappings = self
            .fact_rows(&format!("{}/host-mappings", asset_key(target)), 128)?
            .into_iter()
            .filter(|p| {
                p.get("namespace")
                    .or_else(|| p.get("source"))
                    .and_then(Value::as_str)
                    .is_some_and(|label| match suffix {
                        Some(suffix) => name == format!("{label} / {suffix}"),
                        None => name == label,
                    })
            })
            .collect::<Vec<_>>();
        if mappings.len() == 1
            && string(&mappings[0], "key")? == base_binding
            && number(&mappings[0], "observed")? == number(&old, "observed")?
        {
            self.relocate_source(
                None,
                Relocation {
                    from: target,
                    to: target,
                    old: &legacy,
                    new: &key,
                    name,
                    binding: Some(binding),
                    family,
                },
            )?;
        } else if !mappings.is_empty() {
            let mut marked = data(old);
            marked.insert("source_revision".into(), text("unresolved-native-binding"));
            self.put(&legacy, &family.parent(target), Class::Inventory, marked)?;
        }
        Ok(key)
    }
    pub(super) fn relocate_source(
        &mut self,
        context: Option<&Context>,
        move_: Relocation<'_>,
    ) -> Result<()> {
        let Relocation {
            from,
            to,
            old,
            new,
            name,
            binding,
            family,
        } = move_;
        let source = self.get(old)?.ok_or(Error::Missing)?;
        if string(&source, "parent")? != family.parent(from) {
            return Err(Error::Conflict);
        }
        if old != new && self.get(new)?.is_some() {
            return Err(Error::Conflict);
        }
        if context.is_none() && (from != to || name != string(&source, "source")?) {
            return Err(Error::Invalid);
        }
        let observed = number(&source, "observed")?;
        let mut input = api::HostNetwork::default();
        if family == Family::Network {
            let body = self
                .get(&format!("{old}/coverage"))?
                .ok_or(Error::Corrupt)?;
            input =
                api::HostNetwork::decode(binary(&body, "payload")?).map_err(|_| Error::Corrupt)?;
            input.installation = Some(target_wire(to));
            input.source = name.into();
            input.observed_at = observed;
            let mut coverage = network_facts::coverage(&input);
            coverage.observed_at = 0;
            self.put(
                &format!("{new}/coverage"),
                new,
                Class::Inventory,
                props([("payload", Value::Binary(coverage.encode_to_vec()))]),
            )?;
            if old != new {
                self.delete(&format!("{old}/coverage"))?;
            }
        }
        let mut addresses = Vec::new();
        for (index, part) in family.parts().iter().enumerate() {
            let parent = format!("{new}/{part}");
            for mut row in self.fact_rows(
                &format!("{old}/{part}"),
                if family == Family::Addresses {
                    64
                } else {
                    current_facts::MAX_FACTS
                },
            )? {
                let key = string(&row, "key")?;
                let window = self
                    .get(&format!("{key}/freshness"))?
                    .ok_or(Error::Corrupt)?;
                let latest = window.get("observed") == Some(&int(observed)?);
                let identity = if family == Family::Addresses {
                    let value = api::CurrentAddress::decode(binary(&row, "payload")?)
                        .map_err(|_| Error::Corrupt)?;
                    let id = value.address.clone();
                    if latest {
                        addresses.push(value);
                    }
                    id
                } else {
                    let mut one = api::HostNetwork {
                        installation: Some(target_wire(to)),
                        ..Default::default()
                    };
                    match index {
                        0 => {
                            let value = api::SocketBind::decode(binary(&row, "payload")?)
                                .map_err(|_| Error::Corrupt)?;
                            if let Some(context) = context {
                                let service = parse_target(value.service.as_ref())?;
                                if !context.field(
                                    service,
                                    wire::AssetKind::Service as i32,
                                    "network_evidence",
                                    false,
                                ) || !context.field(
                                    service,
                                    wire::AssetKind::Service as i32,
                                    "network_evidence",
                                    true,
                                ) {
                                    return Err(Error::Denied);
                                }
                            }
                            one.sockets.push(value.clone());
                            if latest {
                                input.sockets.push(value);
                            }
                            row.extend(target_props(to));
                        }
                        1 => {
                            let value = api::LocalRoute::decode(binary(&row, "payload")?)
                                .map_err(|_| Error::Corrupt)?;
                            one.routes.push(value.clone());
                            if latest {
                                input.routes.push(value);
                            }
                        }
                        2 => {
                            let value = api::ApplicationRule::decode(binary(&row, "payload")?)
                                .map_err(|_| Error::Corrupt)?;
                            one.rules.push(value.clone());
                            if latest {
                                input.rules.push(value);
                            }
                        }
                        3 => {
                            let value = api::FirewallSetting::decode(binary(&row, "payload")?)
                                .map_err(|_| Error::Corrupt)?;
                            match api::firewall_setting::Kind::try_from(value.kind) {
                                Ok(api::firewall_setting::Kind::Enabled) => {
                                    one.firewall_enabled = Some(value.enabled);
                                    if latest {
                                        input.firewall_enabled = Some(value.enabled);
                                    }
                                }
                                Ok(api::firewall_setting::Kind::BlockAll) => {
                                    one.block_all = Some(value.enabled);
                                    if latest {
                                        input.block_all = Some(value.enabled);
                                    }
                                }
                                Ok(api::firewall_setting::Kind::AllowBuiltinSigned) => {
                                    one.allow_builtin_signed = Some(value.enabled);
                                    if latest {
                                        input.allow_builtin_signed = Some(value.enabled);
                                    }
                                }
                                Ok(api::firewall_setting::Kind::AllowDownloadedSigned) => {
                                    one.allow_downloaded_signed = Some(value.enabled);
                                    if latest {
                                        input.allow_downloaded_signed = Some(value.enabled);
                                    }
                                }
                                _ => return Err(Error::Corrupt),
                            }
                        }
                        _ => return Err(Error::Corrupt),
                    }
                    network_facts::sections(&one)?[index]
                        .keys()
                        .next()
                        .cloned()
                        .ok_or(Error::Corrupt)?
                };
                let next = format!(
                    "fact/{}",
                    current_facts::digest(format!("{parent}/{identity}").as_bytes())
                );
                row = data(row);
                if family == Family::Addresses {
                    row.insert("owner".into(), text(asset_key(to)));
                    row.insert("layer".into(), text(interface_facts::address_layer(to)));
                }
                self.put(&next, &parent, Class::Inventory, row)?;
                self.put(
                    &format!("{next}/freshness"),
                    &next,
                    Class::Inventory,
                    data(window),
                )?;
                if next != key {
                    self.delete(&format!("{key}/freshness"))?;
                    self.delete(&key)?;
                }
            }
        }
        let mut source = data(source);
        source.insert("source".into(), text(name));
        if let Some(binding) = binding {
            source.insert("binding".into(), text(binding));
        }
        let digest = if family == Family::Addresses {
            addresses.sort_by(|a, b| a.address.cmp(&b.address));
            Sha256::digest(
                api::InterfaceAddresses {
                    interface: Some(target_wire(to)),
                    source: name.into(),
                    observed_at: observed,
                    addresses,
                    complete: flag(&source, "vflag")?,
                }
                .encode_to_vec(),
            )
            .to_vec()
        } else {
            network_facts::source_digest(
                &network_facts::coverage(&input),
                &network_facts::sections(&input)?,
            )?
        };
        source.insert("digest".into(), Value::Binary(digest));
        self.put(new, &family.parent(to), Class::Inventory, source)?;
        if old != new {
            self.delete(old)?;
        }
        Ok(())
    }
}
