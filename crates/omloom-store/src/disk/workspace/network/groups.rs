use super::*;

pub(in super::super) fn reference(value: &str) -> Result<AssetRef> {
    let (asset, incarnation) = value.split_once('/').ok_or(Error::Corrupt)?;
    Ok(AssetRef {
        asset: asset.parse().map_err(|_| Error::Corrupt)?,
        incarnation: incarnation.parse().map_err(|_| Error::Corrupt)?,
    })
}
pub(in super::super) fn group_of(row: &Properties) -> Result<Option<AssetRef>> {
    row.get("group")
        .map(|value| reference(value.as_str().ok_or(Error::Corrupt)?))
        .transpose()
}
pub(super) fn mechanism(row: &Properties) -> Result<api::SharedUse> {
    api::SharedUse::from_str_name(&string(row, "sharing")?).ok_or(Error::Corrupt)
}
fn protocol_key(link: AssetRef, family: u64, vrid: u32) -> String {
    format!("gateway/{}/{family}/{vrid}", identity(link))
}

impl Store {
    pub(in super::super) fn workspace_group_settings(
        &self,
        context: &Context,
        request: api::Network,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<api::Reply> {
        if !request.cidr.is_empty()
            || request.address_space.is_some()
            || request.routing.is_some()
            || request.vlan_id.is_some()
            || request.group.is_some()
            || !request.group_name.is_empty()
            || request.expected_group.is_some()
            || request.previous_group_revision.is_some()
            || request.next_available
        {
            return Err(Error::Invalid);
        }
        let target = parse_target(request.target.as_ref())?;
        let mut tx = self.begin()?;
        tx.check_network_context(context, working, &request)?;
        let old = tx.workspace_group_member_owner(context, target, request.expected)?;
        let sharing = api::SharedUse::try_from(request.sharing).map_err(|_| Error::Invalid)?;
        if !matches!(sharing, api::SharedUse::Vrrp | api::SharedUse::Anycast) {
            return Err(Error::Invalid);
        }
        let changed_context = old.get("sharing") != Some(&text(sharing.as_str_name()))
            || old.get("link_scope")
                != request
                    .link
                    .as_ref()
                    .map(|t| parse_target(Some(t)).map(|t| text(identity(t))))
                    .transpose()?
                    .as_ref();
        if changed_context {
            let addresses = tx.query("MATCH (n:OmRow {parent:'networks'}) WHERE n.group=$group RETURN properties(n) AS p LIMIT 1", params([("group", text(identity(target)))]))?;
            if !addresses.is_empty()
                || !tx
                    .children(&format!("members/{}", identity(target)))?
                    .is_empty()
            {
                return Err(Error::Conflict);
            }
        }
        let namespace = if sharing == api::SharedUse::Vrrp {
            let link = parse_target(request.link.as_ref())?;
            if number(&tx.visible_asset(context, link)?, "kind")?
                != wire::AssetKind::L2Domain as u64
            {
                return Err(Error::Invalid);
            }
            let Some(vrid @ 1..=255) = request.vrid else {
                return Err(Error::Invalid);
            };
            let namespace = protocol_key(link, number(&old, "family")?, vrid);
            if let Some(owner) = tx.get(&namespace)?
                && !same_target(&owner, target)
            {
                return Err(Error::Conflict);
            }
            Some(namespace)
        } else {
            if request.link.is_some() || request.vrid.is_some() {
                return Err(Error::Invalid);
            }
            None
        };
        tx.workspace_network_write(
            context,
            api::Write {
                target: Some(target_wire(target)),
                expected: request.expected,
                kind: wire::AssetKind::AddressGroup as i32,
                changes: vec![wire::Field {
                    name: "name".into(),
                    value: Some(field::Value::Text(request.name)),
                }],
                parent: None,
            },
            now,
        )?;
        let mut data = target_props(target);
        data.extend(props([
            ("sharing", text(sharing.as_str_name())),
            ("family", int(number(&old, "family")?)?),
        ]));
        if let Some(namespace) = &namespace {
            tx.put(
                namespace,
                "gateway-groups",
                Class::Inventory,
                target_props(target),
            )?;
            data.insert("namespace".into(), text(namespace));
            data.insert(
                "link_scope".into(),
                text(identity(parse_target(request.link.as_ref())?)),
            );
            data.insert(
                "vrid".into(),
                int(u64::from(request.vrid.ok_or(Error::Invalid)?))?,
            );
        }
        if let Some(previous) = old.get("namespace").and_then(Value::as_str)
            && namespace.as_deref() != Some(previous)
        {
            tx.delete(previous)?;
        }
        if tx.put_against(
            &format!("group/{}", identity(target)),
            "address-groups",
            Class::Inventory,
            data,
            Some(&old),
        )? {
            tx.workspace_advance(target, &context.actor, "group.settings", now)?;
        }
        Ok(mutation(tx.commit()?, vec![target_wire(target)]))
    }
}

impl Tx<'_> {
    pub(in super::super) fn workspace_shared_group(
        &mut self,
        context: &Context,
        request: &mut api::Network,
        family: u64,
        now: u64,
    ) -> Result<Option<AssetRef>> {
        if let Some(selected) = request.group.as_ref() {
            if !request.group_name.is_empty()
                || request.sharing != api::SharedUse::Unspecified as i32
            {
                return Err(Error::Invalid);
            }
            let target = parse_target(Some(selected))?;
            let root = self.visible_asset(context, target)?;
            if number(&root, "kind")? != wire::AssetKind::AddressGroup as u64 {
                return Err(Error::Invalid);
            }
            if request.expected_group != Some(number(&root, "revision")?) {
                return Err(Error::Conflict);
            }
            if !context.field(
                target,
                wire::AssetKind::AddressGroup as i32,
                "network",
                true,
            ) {
                return Err(Error::Denied);
            }
            let settings = self
                .get(&format!("group/{}", identity(target)))?
                .ok_or(Error::Corrupt)?;
            let sharing = mechanism(&settings)?;
            if sharing == api::SharedUse::Vrrp {
                if request.vrid.is_some() || number(&settings, "family")? != family {
                    return Err(Error::Invalid);
                }
                let link = reference(&string(&settings, "link_scope")?)?;
                // A link-local allocation parent already supplies this same L2 boundary.
                if let Some(provided) = request.link.as_ref()
                    && parse_target(Some(provided))? != link
                {
                    return Err(Error::Invalid);
                }
                request.link = Some(target_wire(link));
            } else if request.vrid.is_some() {
                return Err(Error::Invalid);
            }
            request.sharing = sharing as i32;
            return Ok(Some(target));
        }
        if request.expected_group.is_some() {
            return Err(Error::Invalid);
        }
        let sharing = api::SharedUse::try_from(request.sharing).map_err(|_| Error::Invalid)?;
        if sharing == api::SharedUse::Exclusive {
            if !request.group_name.is_empty() || request.vrid.is_some() {
                return Err(Error::Invalid);
            }
            return Ok(None);
        }
        if !matches!(sharing, api::SharedUse::Vrrp | api::SharedUse::Anycast)
            || request.group_name.trim().is_empty()
            || request.group_name.len() > 160
        {
            return Err(Error::Invalid);
        }
        let namespace = if sharing == api::SharedUse::Vrrp {
            let link = parse_target(request.link.as_ref())?;
            let Some(vrid @ 1..=255) = request.vrid else {
                return Err(Error::Invalid);
            };
            if number(&self.visible_asset(context, link)?, "kind")?
                != wire::AssetKind::L2Domain as u64
            {
                return Err(Error::Invalid);
            }
            Some(protocol_key(link, family, vrid))
        } else {
            if request.vrid.is_some() {
                return Err(Error::Invalid);
            }
            None
        };
        if let Some(namespace) = &namespace
            && self.get(namespace)?.is_some()
        {
            return Err(Error::Conflict);
        }
        let target = self.workspace_network_write(
            context,
            api::Write {
                kind: wire::AssetKind::AddressGroup as i32,
                changes: vec![wire::Field {
                    name: "name".into(),
                    value: Some(field::Value::Text(request.group_name.trim().into())),
                }],
                ..Default::default()
            },
            now,
        )?;
        if !context.field(
            target,
            wire::AssetKind::AddressGroup as i32,
            "network",
            true,
        ) {
            return Err(Error::Denied);
        }
        let mut data = target_props(target);
        data.extend(props([
            ("sharing", text(sharing.as_str_name())),
            ("family", int(family)?),
        ]));
        if let Some(namespace) = namespace {
            self.put(
                &namespace,
                "gateway-groups",
                Class::Inventory,
                target_props(target),
            )?;
            data.insert("namespace".into(), text(namespace));
            data.insert(
                "link_scope".into(),
                text(identity(parse_target(request.link.as_ref())?)),
            );
            data.insert(
                "vrid".into(),
                int(u64::from(request.vrid.ok_or(Error::Invalid)?))?,
            );
        }
        self.put(
            &format!("group/{}", identity(target)),
            "address-groups",
            Class::Inventory,
            data,
        )?;
        Ok(Some(target))
    }

    pub(in super::super) fn workspace_group_member_owner(
        &self,
        context: &Context,
        group: AssetRef,
        expected: Option<u64>,
    ) -> Result<Properties> {
        let root = self.visible_asset(context, group)?;
        if number(&root, "kind")? != wire::AssetKind::AddressGroup as u64 {
            return Err(Error::Invalid);
        }
        if expected != Some(number(&root, "revision")?) {
            return Err(Error::Conflict);
        }
        if !context.field(group, wire::AssetKind::AddressGroup as i32, "network", true) {
            return Err(Error::Denied);
        }
        self.get(&format!("group/{}", identity(group)))?
            .ok_or(Error::Corrupt)
    }
}
