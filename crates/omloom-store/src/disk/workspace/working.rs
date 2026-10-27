use super::*;

pub(super) enum NetworkScope {
    Space(AssetRef),
    Link(AssetRef),
    Prefix {
        space: AssetRef,
        block: ipnet::IpNet,
        link: Option<AssetRef>,
    },
}
pub(super) struct Selection<'a> {
    pub members: Option<BTreeSet<AssetRef>>,
    pub focus: Option<AssetRef>,
    pub networks: Vec<NetworkScope>,
    context: &'a Context,
}

fn writable_fields(
    context: &Context,
    target: AssetRef,
    kind: wire::AssetKind,
    definitions: &[api::Definition],
) -> Vec<String> {
    use wire::AssetKind as K;
    let builtins: &[&str] = if kind == K::AddressReference {
        &[
            "address",
            "prefix_length",
            "meaning",
            "zone",
            "vlan_tag",
            "description",
        ]
    } else if kind == K::SoftwareInstallation {
        &["name", "description", "virtualization_role"]
    } else if matches!(kind, K::Device | K::Rack | K::Port | K::Cable) {
        &["name", "description", "serial"]
    } else {
        &["name", "description"]
    };
    let mut fields: Vec<_> = builtins
        .iter()
        .filter(|name| context.field(target, kind as i32, name, true))
        .map(|s| s.to_string())
        .collect();
    fields.extend(
        definitions
            .iter()
            .filter(|d| {
                !d.retired
                    && d.kinds.contains(&(kind as i32))
                    && context.field(target, kind as i32, &d.id, true)
            })
            .map(|d| d.id.clone()),
    );
    fields
}
impl Selection<'_> {
    pub fn contains(&self, target: AssetRef, tx: &Tx<'_>) -> Result<()> {
        if let Some(focus) = self.focus {
            if target == focus || tx.direct_child(self.context, focus, target)? {
                return Ok(());
            }
        } else if let Some(roots) = &self.members {
            if roots.contains(&target) {
                return Ok(());
            }
            let roots = roots.iter().map(|t| target_wire(*t)).collect::<Vec<_>>();
            match tx.verify_owned_focus(self.context, &roots, target) {
                Ok(()) => return Ok(()),
                Err(Error::Denied) => {}
                Err(error) => return Err(error),
            }
        } else {
            return Ok(());
        }
        {
            if self.networks.is_empty() {
                return Err(Error::Denied);
            }
            let root = tx.visible_asset(self.context, target)?;
            let kind = wire::AssetKind::try_from(number(&root, "kind")? as i32)
                .map_err(|_| Error::Corrupt)?;
            if kind == wire::AssetKind::Vlan {
                if !self.context.field(target, kind as i32, "link_scope", false) {
                    return Err(Error::Denied);
                }
                let row = tx
                    .get(&format!("network/{}", network::identity(target)))?
                    .ok_or(Error::Missing)?;
                for scope in &self.networks {
                    if let NetworkScope::Link(link) = scope
                        && row.get("link_scope").and_then(Value::as_str)
                            == Some(network::identity(*link).as_str())
                    {
                        return Ok(());
                    }
                }
                return Err(Error::Denied);
            }
            if !matches!(kind, wire::AssetKind::IpAddress | wire::AssetKind::Prefix)
                || !self
                    .context
                    .field(target, kind as i32, "address_space", false)
            {
                return Err(Error::Denied);
            }
            let row = tx
                .get(&format!("network/{}", network::identity(target)))?
                .ok_or(Error::Missing)?;
            let space = network::groups::reference(&string(&row, "address_space")?)?;
            let cidr: ipnet::IpNet = string(&row, "cidr")?.parse().map_err(|_| Error::Corrupt)?;
            for scope in &self.networks {
                match scope {
                    NetworkScope::Space(expected) if space == *expected => return Ok(()),
                    NetworkScope::Prefix {
                        space: expected,
                        block,
                        link,
                    } if space == *expected
                        && self.context.field(target, kind as i32, "cidr", false)
                        && block.contains(&cidr.addr())
                        && (kind != wire::AssetKind::Prefix
                            || cidr.prefix_len() >= block.prefix_len())
                        && (link.is_none()
                            || (self.context.field(target, kind as i32, "link_scope", false)
                                && row.get("link_scope")
                                    == link.map(|l| text(network::identity(l))).as_ref())) =>
                    {
                        return Ok(());
                    }
                    _ => {}
                }
            }
            Err(Error::Denied)
        }
    }
}

// Ownership direction is independent of the stored relation's arrow.
pub(super) fn attachment(
    parent: wire::AssetKind,
    child: wire::AssetKind,
) -> Option<(wire::RelationKind, bool)> {
    use wire::{AssetKind as K, RelationKind as R};
    match (parent, child) {
        (K::Site | K::Location, K::Location | K::Rack | K::Device) | (K::Rack, K::Device) => {
            Some((R::InstalledIn, true))
        }
        (K::Device, K::Device | K::Interface | K::AddressReference)
        | (K::VirtualMachine, K::Interface | K::AddressReference)
        | (K::Interface, K::Interface | K::AddressReference)
        | (K::SoftwareInstallation, K::Interface | K::AddressReference | K::Service)
        | (K::Service, K::Interface | K::AddressReference) => Some((R::Contains, false)),
        (K::Device | K::VirtualMachine, K::SoftwareInstallation) => Some((R::Contains, false)),
        (K::Service, K::SoftwareInstallation) => Some((R::Contains, false)),
        (K::Device, K::Port) => Some((R::HasPort, false)),
        (K::Port, K::Interface) => Some((R::Carries, false)),
        (K::Device, K::VirtualMachine | K::Service) | (K::VirtualMachine, K::Service) => {
            Some((R::RunsOn, true))
        }
        (K::SoftwareInstallation, K::VirtualMachine) => Some((R::RunsOn, true)),
        (K::Device, K::Storage) => Some((R::On, true)),
        _ => None,
    }
}
pub(super) fn refines(
    parent: AssetRef,
    parent_kind: wire::AssetKind,
    child_kind: wire::AssetKind,
    kind: i32,
    from: AssetRef,
    to: AssetRef,
) -> bool {
    let Some((expected, reverse)) = attachment(parent_kind, child_kind) else {
        return false;
    };
    if expected as i32 != kind {
        return false;
    }
    if reverse {
        to == parent
    } else {
        from == parent
    }
}

impl Tx<'_> {
    pub(super) fn direct_child(
        &self,
        context: &Context,
        parent: AssetRef,
        child: AssetRef,
    ) -> Result<bool> {
        let p = self.visible_asset(context, parent)?;
        let c = self.visible_asset(context, child)?;
        let pk =
            wire::AssetKind::try_from(number(&p, "kind")? as i32).map_err(|_| Error::Corrupt)?;
        let ck =
            wire::AssetKind::try_from(number(&c, "kind")? as i32).map_err(|_| Error::Corrupt)?;
        if !context.field(parent, pk as i32, "relations", false)
            || !context.field(child, ck as i32, "relations", false)
        {
            return Ok(false);
        }
        let Some((kind, reverse)) = attachment(pk, ck) else {
            return Ok(false);
        };
        let path = assets::ownership_path(false, reverse);
        let rows=self.query(&format!("UNWIND [$child] AS key MATCH {path} WHERE p.key=$parent AND r.kind=$kind RETURN properties(r) AS p LIMIT 129"),params([("parent",text(asset_key(parent))),("child",text(asset_key(child))),("kind",Value::Int(kind as i64))]))?;
        if rows.len() > 128 {
            return Err(Error::Limit);
        }
        for row in rows {
            let relation = super::super::inventory::relation_projection(&row)?;
            let (from, to) = if reverse {
                (child, parent)
            } else {
                (parent, child)
            };
            if parse_target(relation.from.as_ref())? == from
                && parse_target(relation.to.as_ref())? == to
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
    pub(super) fn check_relationship(
        &self,
        context: &Context,
        selection: &Selection,
        relation: &wire::Relation,
    ) -> Result<(AssetRef, AssetRef)> {
        let from = parse_target(relation.from.as_ref())?;
        let to = parse_target(relation.to.as_ref())?;
        selection.contains(from, self)?;
        selection.contains(to, self)?;
        let from_row = self.visible_asset(context, from)?;
        let to_row = self.visible_asset(context, to)?;
        let from_kind = wire::AssetKind::try_from(number(&from_row, "kind")? as i32)
            .map_err(|_| Error::Corrupt)?;
        let to_kind = wire::AssetKind::try_from(number(&to_row, "kind")? as i32)
            .map_err(|_| Error::Corrupt)?;
        if !context.field(from, from_kind as i32, "relations", true)
            || !context.field(to, to_kind as i32, "relations", true)
        {
            return Err(Error::Denied);
        }
        if let Some(focus) = selection.focus {
            let permitted = if focus == from {
                attachment(from_kind, to_kind).filter(|(_, rev)| !rev)
            } else if focus == to {
                attachment(to_kind, from_kind).filter(|(_, rev)| *rev)
            } else {
                None
            };
            if permitted.is_none_or(|(kind, _)| kind as i32 != relation.kind) {
                return Err(Error::Denied);
            }
        }
        Ok((from, to))
    }

    pub(super) fn attach_created(
        &mut self,
        context: &Context,
        child: AssetRef,
        kind: wire::AssetKind,
        parent: Option<(AssetRef, wire::RelationKind, bool)>,
        now: u64,
    ) -> Result<()> {
        let Some((parent, relation_kind, reverse)) = parent else {
            return Ok(());
        };
        if !context.field(child, kind as i32, "relations", true) {
            return Err(Error::Denied);
        }
        let (from, to) = if reverse {
            (child, parent)
        } else {
            (parent, child)
        };
        let relation = wire::Relation {
            relation_id: RelationId::generate().to_string(),
            from: Some(target_wire(from)),
            to: Some(target_wire(to)),
            kind: relation_kind as i32,
            revision: 1,
            authority_id: self.workspace_authority()?,
        };
        self.save_typed_relation(&relation, None, &context.actor, now)?;
        Ok(())
    }

    pub(super) fn working_relations(&self, target: AssetRef) -> Result<Vec<wire::Relation>> {
        let rows = self.query(
            "MATCH (a:OmRow {key:$key})-[:OmRelationFrom|OmRelationTo]-(r:OmRow) RETURN properties(r) AS p LIMIT 129",
            params([("key", text(asset_key(target)))]),
        )?;
        if rows.len() > 128 {
            return Err(Error::Limit);
        }
        rows.iter()
            .map(super::super::inventory::relation_projection)
            .collect()
    }

    pub(super) fn local_children(
        &self,
        context: &Context,
        parent: AssetRef,
    ) -> Result<Vec<AssetRef>> {
        let parent_row = self.visible_asset(context, parent)?;
        let parent_kind = wire::AssetKind::try_from(number(&parent_row, "kind")? as i32)
            .map_err(|_| Error::Corrupt)?;
        if !context.field(parent, parent_kind as i32, "relations", false) {
            return Ok(Vec::new());
        }
        let mut children = Vec::new();
        for (_, child_row, relation) in
            self.context_edges(context, &BTreeSet::from([asset_key(parent)]), true)?
        {
            let from = parse_target(relation.from.as_ref())?;
            let to = parse_target(relation.to.as_ref())?;
            let child = if from == parent { to } else { from };
            let child_kind = wire::AssetKind::try_from(number(&child_row, "kind")? as i32)
                .map_err(|_| Error::Corrupt)?;
            if number(&child_row, "adoption")? != wire::Adoption::Adopted as u64
                || !context.readable(child, child_kind as i32)
                || !context.field(child, child_kind as i32, "relations", false)
            {
                continue;
            }
            if refines(parent, parent_kind, child_kind, relation.kind, from, to) {
                children.push(child);
            }
        }
        Ok(children)
    }

    pub(super) fn working_scope(
        &self,
        context: &Context,
        roots: &[wire::AssetRef],
    ) -> Result<BTreeSet<AssetRef>> {
        if roots.len() > MAX_BULK {
            return Err(Error::Limit);
        }
        let mut selected = BTreeSet::new();
        let mut pending = Vec::new();
        for root in roots {
            let target = parse_target(Some(root))?;
            self.visible_asset(context, target)?;
            if selected.insert(target) {
                pending.push(target);
            }
        }
        while let Some(parent) = pending.pop() {
            for child in self.local_children(context, parent)? {
                if !selected.contains(&child) {
                    if selected.len() >= 1024 {
                        return Err(Error::Limit);
                    }
                    selected.insert(child);
                    pending.push(child);
                }
            }
        }
        Ok(selected)
    }

    fn network_scope(&self, context: &Context, target: AssetRef) -> Result<Option<NetworkScope>> {
        let root = self.visible_asset(context, target)?;
        let kind = number(&root, "kind")? as i32;
        if !context.field(target, kind, "relations", false) {
            return Ok(None);
        }
        if kind == wire::AssetKind::AddressSpace as i32 {
            return Ok(Some(NetworkScope::Space(target)));
        }
        if kind == wire::AssetKind::L2Domain as i32 {
            return Ok(Some(NetworkScope::Link(target)));
        }
        if kind != wire::AssetKind::Prefix as i32 {
            return Ok(None);
        }
        if !context.field(target, kind, "cidr", false)
            || !context.field(target, kind, "address_space", false)
        {
            return Err(Error::Denied);
        }
        let row = self
            .get(&format!("network/{}", network::identity(target)))?
            .ok_or(Error::Missing)?;
        let block: ipnet::IpNet = string(&row, "cidr")?.parse().map_err(|_| Error::Corrupt)?;
        let link = if network::link_local(block.addr()) {
            if !context.field(target, kind, "link_scope", false) {
                return Err(Error::Denied);
            }
            Some(network::groups::reference(&string(&row, "link_scope")?)?)
        } else {
            None
        };
        Ok(Some(NetworkScope::Prefix {
            space: network::groups::reference(&string(&row, "address_space")?)?,
            block,
            link,
        }))
    }

    pub(super) fn selection<'a>(
        &self,
        context: &'a Context,
        working: &api::WorkingContext,
        mutation: bool,
    ) -> Result<Selection<'a>> {
        if working.roots.len() > MAX_BULK {
            return Err(Error::Limit);
        }
        if mutation {
            match working.expected {
                Some(expected) if expected != number(&self.root, "revision")? => {
                    return Err(Error::Conflict);
                }
                None if !working.roots.is_empty() || working.focus.is_some() => {
                    return Err(Error::Conflict);
                }
                _ => {}
            }
        }
        let members = if working.roots.is_empty() {
            None
        } else {
            let mut roots = BTreeSet::new();
            for root in &working.roots {
                let target = parse_target(Some(root))?;
                self.visible_asset(context, target)?;
                roots.insert(target);
            }
            Some(roots)
        };
        let mut networks = Vec::new();
        for root in &working.roots {
            if let Some(scope) = self.network_scope(context, parse_target(Some(root))?)? {
                networks.push(scope);
            }
        }
        let focus = working
            .focus
            .as_ref()
            .map(|t| parse_target(Some(t)))
            .transpose()?;
        let mut selection = Selection {
            members,
            focus,
            networks,
            context,
        };
        if let Some(focus) = focus {
            self.visible_asset(context, focus)?;
            selection.focus = None;
            match selection.contains(focus, self) {
                Ok(()) => {}
                Err(Error::Denied) => self.verify_owned_focus(context, &working.roots, focus)?,
                Err(error) => return Err(error),
            }
            selection.networks = self.network_scope(context, focus)?.into_iter().collect();
            selection.focus = Some(focus);
            selection.members = Some(BTreeSet::from([focus]));
        }
        Ok(selection)
    }

    pub(super) fn creation_parent(
        &self,
        context: &Context,
        working: &api::WorkingContext,
        parent: Option<&wire::AssetRef>,
        child: wire::AssetKind,
    ) -> Result<Option<(AssetRef, wire::RelationKind, bool)>> {
        let selection = self.selection(context, working, true)?;
        let parent = parent.map(|p| parse_target(Some(p))).transpose()?;
        if selection.focus.is_some() && selection.focus != parent {
            return Err(Error::Invalid);
        }
        if !working.roots.is_empty() && parent.is_none() {
            return Err(Error::Invalid);
        }
        let Some(parent) = parent else {
            return Ok(None);
        };
        selection.contains(parent, self)?;
        let row = self.visible_asset(context, parent)?;
        let kind =
            wire::AssetKind::try_from(number(&row, "kind")? as i32).map_err(|_| Error::Corrupt)?;
        if !context.field(parent, kind as i32, "relations", true) {
            return Err(Error::Denied);
        }
        let (relation, reverse) = attachment(kind, child).ok_or(Error::Invalid)?;
        if kind == wire::AssetKind::SoftwareInstallation && child == wire::AssetKind::VirtualMachine
        {
            if !context.field(parent, kind as i32, "virtualization_role", false) {
                return Err(Error::Denied);
            }
            if row.get("virtualization_role").and_then(Value::as_str)
                != Some(api::VirtualizationRole::Hypervisor.as_str_name())
            {
                return Err(Error::Invalid);
            }
        }
        Ok(Some((parent, relation, reverse)))
    }
}

impl Store {
    pub(super) fn workspace_prepare(
        &self,
        context: &Context,
        working: &api::WorkingContext,
        request: api::Prepare,
    ) -> Result<api::Reply> {
        let view = api::View::try_from(request.view).map_err(|_| Error::Invalid)?;
        let definitions = if request.from.is_some() {
            Vec::new()
        } else {
            self.workspace_definitions(context)?
        };
        let tx = self.begin()?;
        let unscoped = api::WorkingContext::default();
        let selection = if request.from.is_none() && working.focus.is_none() {
            if working.roots.len() > MAX_BULK {
                return Err(Error::Limit);
            }
            for root in &working.roots {
                tx.visible_asset(context, parse_target(Some(root))?)?;
            }
            // Creation choices use only the explicitly selected parents, not descendants.
            tx.selection(context, &unscoped, false)?
        } else {
            tx.selection(context, working, false)?
        };
        let parents = if request.from.is_some() {
            Vec::new()
        } else {
            match selection.focus {
                Some(focus) => vec![Some(focus)],
                None if !working.roots.is_empty() => working
                    .roots
                    .iter()
                    .map(|r| parse_target(Some(r)).map(Some))
                    .collect::<Result<_>>()?,
                None => vec![None],
            }
        };
        let mut creations = Vec::new();
        let mut allocations = Vec::new();
        for parent in parents {
            let parent_item = if let Some(parent) = parent {
                let row = tx.visible_asset(context, parent)?;
                let kind = number(&row, "kind")? as i32;
                if !context.field(parent, kind, "relations", true) {
                    continue;
                }
                let fields = if context.field(parent, kind, "name", false) {
                    tx.get(&format!("{}/intent/name", asset_key(parent)))?
                        .as_ref()
                        .map(super::super::inventory::read_field)
                        .transpose()?
                        .into_iter()
                        .collect()
                } else {
                    Vec::new()
                };
                Some(api::Item {
                    target: Some(target_wire(parent)),
                    kind,
                    revision: number(&row, "revision")?,
                    fields,
                })
            } else {
                None
            };
            use wire::AssetKind as K;
            if request.from.is_none() {
                let available: &[K] = match parent_item
                    .as_ref()
                    .map(|p| K::try_from(p.kind))
                    .transpose()
                    .map_err(|_| Error::Corrupt)?
                {
                    None => &[K::IpAddress, K::Prefix, K::Vlan],
                    Some(K::AddressSpace | K::Prefix) => &[K::IpAddress, K::Prefix],
                    Some(K::L2Domain) => &[K::Vlan],
                    _ => &[],
                };
                for kind in available {
                    let target = new_target();
                    if context.field(target, *kind as i32, "name", true)
                        && context.field(target, *kind as i32, "network", true)
                    {
                        allocations.push(api::Creation {
                            kind: *kind as i32,
                            parent: parent_item.clone(),
                            fields: Vec::new(),
                        });
                    }
                }
            }
            for kind in [
                K::Site,
                K::Rack,
                K::Device,
                K::VirtualMachine,
                K::Port,
                K::Interface,
                K::Vrf,
                K::Cable,
                K::PowerFeed,
                K::Service,
                K::Storage,
                K::Cluster,
                K::Lag,
                K::Location,
                K::AddressSpace,
                K::L2Domain,
                K::AddressReference,
                K::SoftwareInstallation,
            ] {
                let raw = kind as i32;
                if let Some(parent) = &parent_item {
                    let parent_kind =
                        wire::AssetKind::try_from(parent.kind).map_err(|_| Error::Corrupt)?;
                    if attachment(parent_kind, kind).is_none() {
                        continue;
                    }
                    if parent_kind == K::SoftwareInstallation && kind == K::VirtualMachine {
                        let target = parse_target(parent.target.as_ref())?;
                        if !context.field(target, parent.kind, "virtualization_role", false)
                            || tx
                                .asset(target)?
                                .get("virtualization_role")
                                .and_then(Value::as_str)
                                != Some(api::VirtualizationRole::Hypervisor.as_str_name())
                        {
                            continue;
                        }
                    }
                }
                let view_kinds = super::assets::kind_for_view(view);
                if parent_item.is_none() && !view_kinds.is_empty() && !view_kinds.contains(&kind) {
                    continue;
                }
                let prospective = new_target();
                let fields = writable_fields(context, prospective, kind, &definitions);
                if parent.is_some() && !context.field(prospective, raw, "relations", true) {
                    continue;
                }
                if !context.field(
                    prospective,
                    raw,
                    if kind == wire::AssetKind::AddressReference {
                        "address"
                    } else {
                        "name"
                    },
                    true,
                ) {
                    continue;
                }
                if creations.len() >= MAX_BULK {
                    return Err(Error::Limit);
                }
                creations.push(api::Creation {
                    kind: raw,
                    parent: parent_item.clone(),
                    fields,
                });
            }
        }
        let mut relationships = Vec::new();
        match (request.from.as_ref(), request.to.as_ref()) {
            (None, None) => {}
            (Some(from), Some(to)) => {
                let from_ref = parse_target(Some(from))?;
                let to_ref = parse_target(Some(to))?;
                selection.contains(from_ref, &tx)?;
                selection.contains(to_ref, &tx)?;
                let from_kind = wire::AssetKind::try_from(number(
                    &tx.visible_asset(context, parse_target(Some(from))?)?,
                    "kind",
                )? as i32)
                .map_err(|_| Error::Corrupt)?;
                let to_kind = wire::AssetKind::try_from(number(
                    &tx.visible_asset(context, parse_target(Some(to))?)?,
                    "kind",
                )? as i32)
                .map_err(|_| Error::Corrupt)?;
                if !context.field(from_ref, from_kind as i32, "relations", true)
                    || !context.field(to_ref, to_kind as i32, "relations", true)
                {
                    return Err(Error::Denied);
                }
                use wire::RelationKind as R;
                for kind in [
                    R::InstalledIn,
                    R::Contains,
                    R::HasPort,
                    R::Carries,
                    R::ConnectedTo,
                    R::RunsOn,
                    R::StoredOn,
                    R::On,
                    R::Uses,
                    R::PoweredBy,
                    R::Manages,
                    R::MemberOf,
                    R::DependsOn,
                ] {
                    if !super::super::inventory::compatible(from_kind, to_kind, kind) {
                        continue;
                    }
                    let relation = wire::Relation {
                        from: Some(from.clone()),
                        to: Some(to.clone()),
                        kind: kind as i32,
                        ..Default::default()
                    };
                    match tx.check_relationship(context, &selection, &relation) {
                        Ok(_) => relationships.push(kind as i32),
                        Err(Error::Denied) => {}
                        Err(error) => return Err(error),
                    }
                }
            }
            _ => return Err(Error::Invalid),
        }
        let fields = if let Some(focus) = selection.focus {
            let kind = wire::AssetKind::try_from(number(&tx.asset(focus)?, "kind")? as i32)
                .map_err(|_| Error::Corrupt)?;
            writable_fields(context, focus, kind, &definitions)
        } else {
            Vec::new()
        };
        let revision = number(&tx.root, "revision")?;
        tx.commit()?;
        Ok(reply(api::reply::Result::Actions(api::Actions {
            creations,
            revision,
            relationships,
            fields,
            allocations,
        })))
    }
}
