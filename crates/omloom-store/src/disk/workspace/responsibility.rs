use super::*;
use api::{AssignmentMode as Mode, ResponsibilityFacet as Facet, ResponsibilityStatus as Status};
mod graph;

const MAX_ANCESTORS: usize = 64;
const FACETS: [Facet; 5] = [
    Facet::ClaimedOwner,
    Facet::Operator,
    Facet::Beneficiary,
    Facet::AccountController,
    Facet::Creator,
];

impl Tx<'_> {
    pub(super) fn workload_ownership(
        &self,
        context: &Context,
        rows: &[Properties],
        hints: &mut [api::ItemContext],
    ) -> Result<()> {
        if rows.len() != hints.len() || rows.len() > MAX_PAGE {
            return Err(Error::Limit);
        }
        let mut masks = BTreeMap::new();
        for (row, hint) in rows.iter().zip(hints.iter()) {
            let kind = number(row, "kind")? as i32;
            if !matches!(
                wire::AssetKind::try_from(kind),
                Ok(wire::AssetKind::Workload | wire::AssetKind::Container)
            ) {
                continue;
            }
            let target = parse_target(hint.target.as_ref())?;
            if context.field(target, kind, "claimed_owner", false) {
                masks.insert(target, 1);
            }
        }
        if masks.is_empty() {
            return Ok(());
        }
        let graph = graph::Graph::load_owner_summary(self, masks)?;
        for (row, hint) in rows.iter().zip(hints.iter_mut()) {
            let kind = number(row, "kind")? as i32;
            if !matches!(
                wire::AssetKind::try_from(kind),
                Ok(wire::AssetKind::Workload | wire::AssetKind::Container)
            ) {
                continue;
            }
            let target = parse_target(hint.target.as_ref())?;
            if context.field(target, kind, "claimed_owner", false) {
                hint.responsibility =
                    Some(graph.resolve_summary(context, target, Facet::ClaimedOwner)?);
            }
        }
        Ok(())
    }
}

fn qualified(
    child: AssetRef,
    child_kind: wire::AssetKind,
    parent: AssetRef,
    parent_kind: wire::AssetKind,
    relation: &wire::Relation,
    administrative: bool,
) -> Result<bool> {
    use wire::{AssetKind as K, RelationKind as R};
    if child == parent {
        return Ok(false);
    }
    let from = parse_target(relation.from.as_ref())?;
    let to = parse_target(relation.to.as_ref())?;
    let kind = R::try_from(relation.kind).map_err(|_| Error::Corrupt)?;
    Ok(match kind {
        R::Contains if from == parent && to == child => matches!(
            (parent_kind, child_kind),
            (
                K::Device,
                K::Device | K::Interface | K::SoftwareInstallation | K::AddressReference
            ) | (
                K::VirtualMachine,
                K::Interface | K::SoftwareInstallation | K::AddressReference
            ) | (K::Interface, K::Interface | K::AddressReference)
                | (
                    K::SoftwareInstallation,
                    K::Interface | K::Service | K::AddressReference | K::SoftwareInstallation
                )
                | (
                    K::Service,
                    K::Interface
                        | K::SoftwareInstallation
                        | K::ProviderAccount
                        | K::AddressReference
                )
                | (K::Workload, K::Container | K::Service)
                | (
                    K::Container,
                    K::Service | K::Interface | K::SoftwareInstallation
                )
        ),
        R::HasPort if from == parent && to == child => {
            parent_kind == K::Device && child_kind == K::Port
        }
        R::Carries if from == parent && to == child => {
            parent_kind == K::Port && child_kind == K::Interface
        }
        R::RunsOn if administrative && from == child && to == parent => matches!(
            (parent_kind, child_kind),
            (K::Device | K::SoftwareInstallation, K::VirtualMachine)
                | (K::Device | K::VirtualMachine, K::Service)
        ),
        _ => false,
    })
}

fn field_name(facet: Facet) -> Result<&'static str> {
    match facet {
        Facet::ClaimedOwner => Ok("claimed_owner"),
        Facet::Operator => Ok("operator"),
        Facet::Beneficiary => Ok("beneficiary"),
        Facet::AccountController => Ok("account_controller"),
        Facet::Creator => Ok("creator"),
        Facet::Unspecified => Err(Error::Invalid),
    }
}
fn inheritable(facet: Facet) -> bool {
    matches!(facet, Facet::ClaimedOwner | Facet::Operator)
}
pub(super) fn validate_filter(filter: &api::ResponsibilityFilter) -> Result<Facet> {
    let facet = Facet::try_from(filter.facet).map_err(|_| Error::Invalid)?;
    field_name(facet)?;
    bounded_value(&filter.issuer, 256, true)?;
    bounded_value(&filter.subject, 256, true)?;
    Ok(facet)
}
fn assignment_key(target: AssetRef, facet: Facet) -> String {
    format!("{}/responsibility/{}", asset_key(target), facet as i32)
}
fn party_lookup(facet: Facet, issuer: &str, subject: &str) -> String {
    let mut hash = Sha256::new();
    hash.update((facet as i32).to_be_bytes());
    for value in [issuer, subject] {
        hash.update((value.len() as u64).to_be_bytes());
        hash.update(value);
    }
    format!("{:x}", hash.finalize())
}
pub(super) fn bounded_value(value: &str, max: usize, required: bool) -> Result<()> {
    if value.len() > max
        || (required && value.trim().is_empty())
        || value.chars().any(char::is_control)
    {
        return Err(Error::Invalid);
    }
    Ok(())
}
fn assignment_props(value: &api::ResponsibilityAssignment) -> Result<Properties> {
    let facet = Facet::try_from(value.facet).map_err(|_| Error::Invalid)?;
    field_name(facet)?;
    let mode = Mode::try_from(value.mode).map_err(|_| Error::Invalid)?;
    let mut row = props([
        ("facet", int(value.facet as u64)?),
        ("mode", int(value.mode as u64)?),
    ]);
    match mode {
        Mode::Explicit => {
            if value.source_relation.is_some() || value.source_revision.is_some() {
                return Err(Error::Invalid);
            }
            let party = value.party.as_ref().ok_or(Error::Invalid)?;
            bounded_value(&party.issuer, 256, true)?;
            bounded_value(&party.subject, 256, true)?;
            bounded_value(&party.name, 160, true)?;
            if !matches!(
                api::PartyKind::try_from(party.kind),
                Ok(api::PartyKind::Individual
                    | api::PartyKind::Organization
                    | api::PartyKind::Team)
            ) {
                return Err(Error::Invalid);
            }
            row.extend(props([
                ("issuer", text(&party.issuer)),
                ("subject", text(&party.subject)),
                ("party_kind", int(party.kind as u64)?),
                ("name", text(&party.name)),
            ]));
            row.insert(
                "party_lookup".into(),
                text(party_lookup(facet, &party.issuer, &party.subject)),
            );
        }
        Mode::Inherit if inheritable(facet) && value.party.is_none() && value.contact.is_none() => {
            match (&value.source_relation, value.source_revision) {
                (Some(id), Some(revision)) if revision > 0 => {
                    id.parse::<RelationId>().map_err(|_| Error::Invalid)?;
                    row.insert("source_relation".into(), text(id));
                    row.insert("source_binding_revision".into(), int(revision)?);
                }
                (None, None) => {}
                _ => return Err(Error::Invalid),
            }
        }
        Mode::Unassigned
            if value.party.is_none()
                && value.source_relation.is_none()
                && value.source_revision.is_none()
                && value.contact.is_none() => {}
        _ => return Err(Error::Invalid),
    }
    if let Some(contact) = &value.contact {
        bounded_value(contact, 512, true)?;
        row.insert("contact".into(), text(contact));
    }
    Ok(row)
}
fn read_assignment(row: &Properties) -> Result<api::ResponsibilityAssignment> {
    let mode = number(row, "mode")? as i32;
    Ok(api::ResponsibilityAssignment {
        facet: number(row, "facet")? as i32,
        mode,
        party: if mode == Mode::Explicit as i32 {
            Some(api::PartyRef {
                issuer: string(row, "issuer")?,
                subject: string(row, "subject")?,
                kind: number(row, "party_kind")? as i32,
                name: string(row, "name")?,
            })
        } else {
            None
        },
        source_relation: row
            .get("source_relation")
            .and_then(Value::as_str)
            .map(str::to_owned),
        source_revision: row
            .get("source_binding_revision")
            .and_then(Value::as_int)
            .map(|v| v as u64),
        contact: row
            .get("contact")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

impl Store {
    pub(super) fn workspace_responsibility(
        &self,
        context: &Context,
        request: api::Responsibility,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<api::Reply> {
        let mut transfer = self.responsibility_transfer(
            context,
            api::ResponsibilityTransfer {
                entries: vec![request],
            },
            working,
            now,
        )?;
        Ok(reply(api::reply::Result::Responsibility(
            transfer.entries.pop().ok_or(Error::Corrupt)?,
        )))
    }
    pub(super) fn responsibility_transfer(
        &self,
        context: &Context,
        request: api::ResponsibilityTransfer,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<api::ResponsibilityTransferState> {
        if request.entries.is_empty() || request.entries.len() > MAX_PAGE {
            return Err(Error::Limit);
        }
        let action = api::ResponsibilityAction::try_from(request.entries[0].action)
            .map_err(|_| Error::Invalid)?;
        if action == api::ResponsibilityAction::Unspecified {
            return Err(Error::Invalid);
        }
        let mut tx = self.begin()?;
        let mut targets = BTreeSet::new();
        let mut masks = BTreeMap::new();
        for entry in &request.entries {
            if action == api::ResponsibilityAction::Read
                && (!entry.changes.is_empty() || entry.expected.is_some())
            {
                return Err(Error::Invalid);
            }
            if entry.action != action as i32
                || !targets.insert(parse_target(entry.target.as_ref())?)
            {
                return Err(Error::Invalid);
            }
        }
        let root_rows = if action == api::ResponsibilityAction::Read {
            let keys = targets
                .iter()
                .map(|target| text(asset_key(*target)))
                .collect();
            tx.query(
                "UNWIND $keys AS key MATCH (n:OmRow {key:key}) RETURN {asset:n.asset,incarnation:n.incarnation,kind:n.kind,adoption:n.adoption,revision:n.revision} AS p",
                params([("keys", Value::Array(keys))]),
            )?
            .into_iter()
            .map(|row| super::super::target(&row).map(|target| (target, row)))
            .collect::<Result<BTreeMap<_, _>>>()?
        } else {
            BTreeMap::new()
        };
        let selection =
            tx.selection(context, working, action != api::ResponsibilityAction::Read)?;
        for entry in &request.entries {
            let target = parse_target(entry.target.as_ref())?;
            let kind = if action == api::ResponsibilityAction::Read {
                if !entry.changes.is_empty() || entry.expected.is_some() {
                    return Err(Error::Invalid);
                }
                let row = root_rows.get(&target).ok_or(Error::Missing)?;
                let kind = number(row, "kind")? as i32;
                if number(row, "adoption")? != wire::Adoption::Adopted as u64 {
                    return Err(Error::Missing);
                }
                if !context.readable(target, kind) {
                    return Err(Error::Denied);
                }
                selection.contains(target, &tx)?;
                kind
            } else {
                tx.change_responsibility(context, entry, working, now)?
            };
            let mut mask = 0;
            for facet in [Facet::ClaimedOwner, Facet::Operator] {
                if context.field(target, kind, field_name(facet)?, false)
                    || entry
                        .changes
                        .iter()
                        .any(|change| change.facet == facet as i32)
                {
                    mask |= 1 << (facet as i32 - 1);
                }
            }
            masks.insert(target, mask);
        }
        drop(root_rows);
        let targets: Vec<_> = targets.into_iter().collect();
        let graph = graph::Graph::load(&tx, masks)?;
        let mut entries = Vec::with_capacity(targets.len());
        let mut reply_bytes = 64usize;
        for entry in &request.entries {
            for change in &entry.changes {
                graph.resolve(
                    context,
                    parse_target(entry.target.as_ref())?,
                    Facet::try_from(change.facet).map_err(|_| Error::Invalid)?,
                )?;
            }
            let state = tx.responsibility_state(
                context,
                parse_target(entry.target.as_ref())?,
                &graph,
                entry.include_sources,
            )?;
            reply_bytes = reply_bytes
                .checked_add(state.encoded_len())
                .and_then(|bytes| bytes.checked_add(16))
                .ok_or(Error::Limit)?;
            if reply_bytes > MAX_REPLY_BYTES {
                return Err(Error::Limit);
            }
            entries.push(state);
        }
        let revision = if action == api::ResponsibilityAction::Preview {
            number(&tx.root, "revision")?
        } else {
            tx.commit()?
        };
        for entry in &mut entries {
            entry.revision = revision;
        }
        Ok(api::ResponsibilityTransferState { entries, revision })
    }
}

impl Tx<'_> {
    fn change_responsibility(
        &mut self,
        context: &Context,
        request: &api::Responsibility,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<i32> {
        let target = parse_target(request.target.as_ref())?;
        let action =
            api::ResponsibilityAction::try_from(request.action).map_err(|_| Error::Invalid)?;
        if action == api::ResponsibilityAction::Unspecified || request.changes.len() > FACETS.len()
        {
            return Err(Error::Invalid);
        }
        let writing = action != api::ResponsibilityAction::Read;
        if !writing && (!request.changes.is_empty() || request.expected.is_some()) {
            return Err(Error::Invalid);
        }
        let tx = self;
        tx.selection(context, working, writing)?
            .contains(target, tx)?;
        let row = tx.visible_asset(context, target)?;
        let kind = number(&row, "kind")? as i32;
        let mut changed = false;
        if writing {
            if request.expected != Some(number(&row, "revision")?)
                || request.changes.is_empty()
                || working.expected != Some(number(&tx.root, "revision")?)
            {
                return Err(Error::Conflict);
            }
            let mut seen = BTreeSet::new();
            for requested in &request.changes {
                let mut assignment = requested.clone();
                let facet = Facet::try_from(assignment.facet).map_err(|_| Error::Invalid)?;
                let field = field_name(facet)?;
                if !seen.insert(assignment.facet) {
                    return Err(Error::Invalid);
                }
                if !context.field(target, kind, field, true) {
                    return Err(Error::Denied);
                }
                if assignment.mode == Mode::Inherit as i32 && assignment.source_relation.is_none() {
                    let mut source = None;
                    for properties in tx.responsibility_relations(target, false)? {
                        match tx.responsibility_source(target, &properties, false) {
                            Ok(_) if source.is_some() => return Err(Error::Incomplete),
                            Ok(_) => source = Some(inventory::relation_projection(&properties)?),
                            Err(Error::Invalid) => {}
                            Err(error) => return Err(error),
                        }
                    }
                    if let Some(source) = source {
                        assignment.source_relation = Some(source.relation_id);
                        assignment.source_revision = Some(source.revision);
                    }
                }
                let data = assignment_props(&assignment)?;
                if let Some(id) = &assignment.source_relation {
                    let relation = tx.get(&format!("relation/{id}"))?.ok_or(Error::Missing)?;
                    if assignment.source_revision != Some(number(&relation, "revision")?) {
                        return Err(Error::Conflict);
                    }
                    let parent = tx.responsibility_source(target, &relation, true)?;
                    let parent_row = tx.visible_asset(context, parent)?;
                    let parent_kind = number(&parent_row, "kind")? as i32;
                    if !context.field(target, kind, "relations", false)
                        || !context.field(parent, parent_kind, "relations", false)
                        || !context.field(parent, parent_kind, field, false)
                    {
                        return Err(Error::Denied);
                    }
                }
                let key = assignment_key(target, facet);
                let old = tx.get(&key)?;
                if !data.contains_key("contact")
                    && old.as_ref().is_some_and(|row| row.contains_key("contact"))
                    && !context.field(target, kind, &format!("{field}.contact"), false)
                {
                    return Err(Error::Denied);
                }
                if old.as_ref().and_then(|row| row.get("contact")) != data.get("contact")
                    && !context.field(target, kind, &format!("{field}.contact"), true)
                {
                    return Err(Error::Denied);
                }
                changed |= tx.put_against(
                    &key,
                    &asset_key(target),
                    Class::Inventory,
                    data,
                    old.as_ref(),
                )?;
            }
            if changed {
                tx.workspace_advance(target, &context.actor, "responsibility.accepted", now)?;
            }
        }
        Ok(kind)
    }
    pub(super) fn placement_inheritance(
        &self,
        context: &Context,
        target: AssetRef,
        relation: &wire::Relation,
    ) -> Result<Vec<api::EffectiveResponsibility>> {
        let mut mask = 0;
        for facet in [Facet::ClaimedOwner, Facet::Operator] {
            if context.field(
                target,
                wire::AssetKind::VirtualMachine as i32,
                field_name(facet)?,
                false,
            ) {
                mask |= 1 << (facet as i32 - 1);
            }
        }
        let graph = graph::Graph::load(self, [(target, mask)].into())?;
        let mut affected = Vec::new();
        for facet in [Facet::ClaimedOwner, Facet::Operator] {
            if mask & (1 << (facet as i32 - 1)) != 0
                && let Some(assignment) = graph.assignment(target, facet)
                && assignment.mode == Mode::Inherit as i32
                && assignment.source_relation.as_deref() == Some(relation.relation_id.as_str())
                && assignment.source_revision == Some(relation.revision)
            {
                let mut value = graph.resolve(context, target, facet)?;
                // Placement review needs the affected party, not its contact
                // route or independently addressable source metadata.
                value.contact = None;
                value.source = None;
                value.source_revision = None;
                affected.push(value);
            }
        }
        Ok(affected)
    }

    fn responsibility_state(
        &self,
        context: &Context,
        target: AssetRef,
        graph: &graph::Graph,
        include_sources: bool,
    ) -> Result<api::ResponsibilityState> {
        let tx = self;
        let row = graph.asset(target)?;
        let kind = number(row, "kind")? as i32;
        let item = api::Item {
            target: Some(target_wire(target)),
            kind,
            revision: number(row, "revision")?,
            fields: Vec::new(),
        };
        let mut result = api::ResponsibilityState {
            item: Some(item),
            revision: number(&tx.root, "revision")?,
            ..Default::default()
        };
        for facet in FACETS {
            let field = field_name(facet)?;
            if context.field(target, kind, field, true) {
                result.writable.push(facet as i32);
            }
            if !context.field(target, kind, field, false) {
                continue;
            }
            if context.field(target, kind, &format!("{field}.contact"), false) {
                result.fully_readable.push(facet as i32);
            }
            if let Some(local) = graph.assignment(target, facet) {
                let mut declaration = local.clone();
                if !context.field(target, kind, &format!("{field}.contact"), false) {
                    declaration.contact = None;
                }
                if let Some(id) = &declaration.source_relation
                    && (!context.field(target, kind, "relations", false)
                        || !graph.source_disclosable(context, target, id)?)
                {
                    declaration.source_relation = None;
                    declaration.source_revision = None;
                }
                result.assignments.push(declaration);
            }
            result
                .effective
                .push(graph.resolve(context, target, facet)?);
        }
        if include_sources && context.field(target, kind, "relations", false) {
            let rows = tx.responsibility_relations(
                target,
                matches!(
                    wire::AssetKind::try_from(kind),
                    Ok(wire::AssetKind::VirtualMachine | wire::AssetKind::Service)
                ),
            )?;
            let mut candidates = Vec::with_capacity(rows.len());
            let mut parents = BTreeSet::new();
            for row in rows {
                let relation = inventory::relation_projection(&row)?;
                let parent = if relation.kind == wire::RelationKind::RunsOn as i32 {
                    parse_target(relation.to.as_ref())?
                } else {
                    parse_target(relation.from.as_ref())?
                };
                parents.insert(parent);
                candidates.push((relation, parent));
            }
            if candidates.is_empty() {
                return Ok(result);
            }
            let keys = parents
                .iter()
                .map(|parent| text(asset_key(*parent)))
                .collect();
            let parent_rows = tx
                .query(
                    "UNWIND $keys AS key MATCH (p:OmRow {key:key}) RETURN properties(p) AS p",
                    params([("keys", Value::Array(keys))]),
                )?
                .into_iter()
                .map(|row| super::super::target(&row).map(|target| (target, row)))
                .collect::<Result<BTreeMap<_, _>>>()?;
            for (relation, parent) in candidates {
                let Some(parent_row) = parent_rows.get(&parent) else {
                    continue;
                };
                let pk = wire::AssetKind::try_from(number(parent_row, "kind")? as i32)
                    .map_err(|_| Error::Corrupt)?;
                let ck = wire::AssetKind::try_from(kind).map_err(|_| Error::Corrupt)?;
                if !qualified(target, ck, parent, pk, &relation, true)?
                    || number(parent_row, "adoption")? != wire::Adoption::Adopted as u64
                    || !context.field(parent, pk as i32, "name", false)
                    || !context.field(parent, pk as i32, "relations", false)
                {
                    continue;
                }
                let facets: Vec<_> = [Facet::ClaimedOwner, Facet::Operator]
                    .into_iter()
                    .filter(|facet| {
                        context.field(
                            parent,
                            pk as i32,
                            field_name(*facet).expect("known facet"),
                            false,
                        ) && context.field(
                            target,
                            kind,
                            field_name(*facet).expect("known facet"),
                            true,
                        )
                    })
                    .map(|f| f as i32)
                    .collect();
                if facets.is_empty() {
                    continue;
                }
                let fields = parent_row
                    .get("name")
                    .and_then(Value::as_str)
                    .map(|name| wire::Field {
                        name: "name".into(),
                        value: Some(field::Value::Text(name.into())),
                    })
                    .into_iter()
                    .collect();
                result.sources.push(api::ResponsibilitySource {
                    relation: Some(relation),
                    parent: Some(api::Item {
                        target: Some(target_wire(parent)),
                        kind: pk as i32,
                        revision: number(parent_row, "revision")?,
                        fields,
                    }),
                    facets,
                });
            }
        }
        Ok(result)
    }
}

impl Tx<'_> {
    // Both source discovery and editing inspect parent bindings, never all children.
    fn responsibility_relations(
        &self,
        target: AssetRef,
        include_hosting: bool,
    ) -> Result<Vec<Properties>> {
        let mut rows = self.query("MATCH (r:OmRow {to_asset:$asset}) WHERE r.parent='relations' AND r.to_incarnation=$incarnation AND r.kind IN $kinds RETURN properties(r) AS p LIMIT 129", params([
            ("asset", text(target.asset.to_string())), ("incarnation", text(target.incarnation.to_string())),
            ("kinds", Value::Array([wire::RelationKind::Contains, wire::RelationKind::HasPort, wire::RelationKind::Carries].into_iter().map(|kind| Value::Int(kind as i64)).collect()))
        ]))?;
        if include_hosting {
            rows.extend(self.query("MATCH (r:OmRow {asset:$asset}) WHERE r.parent='relations' AND r.incarnation=$incarnation AND r.kind=$kind RETURN properties(r) AS p LIMIT 129", params([
                ("asset", text(target.asset.to_string())), ("incarnation", text(target.incarnation.to_string())), ("kind", Value::Int(wire::RelationKind::RunsOn as i64))
            ]))?);
        }
        if rows.len() > 128 {
            return Err(Error::Limit);
        }
        Ok(rows)
    }

    pub(in super::super) fn initialize_responsibility(
        &mut self,
        child: AssetRef,
        relation: &wire::Relation,
    ) -> Result<()> {
        let row = self
            .get(&format!("relation/{}", relation.relation_id))?
            .ok_or(Error::Corrupt)?;
        match self.responsibility_source(child, &row, false) {
            Ok(_) => {}
            Err(Error::Invalid) => return Ok(()),
            Err(error) => return Err(error),
        }
        for facet in [Facet::ClaimedOwner, Facet::Operator] {
            let assignment = api::ResponsibilityAssignment {
                facet: facet as i32,
                mode: Mode::Inherit as i32,
                source_relation: Some(relation.relation_id.clone()),
                source_revision: Some(relation.revision),
                ..Default::default()
            };
            let key = assignment_key(child, facet);
            if self.get(&key)?.is_some() {
                return Err(Error::Conflict);
            }
            self.put(
                &key,
                &asset_key(child),
                Class::Inventory,
                assignment_props(&assignment)?,
            )?;
        }
        Ok(())
    }
    fn responsibility_source(
        &self,
        child: AssetRef,
        relation: &Properties,
        administrative: bool,
    ) -> Result<AssetRef> {
        let relation = inventory::relation_projection(relation)?;
        let from = parse_target(relation.from.as_ref())?;
        let to = parse_target(relation.to.as_ref())?;
        let parent =
            match wire::RelationKind::try_from(relation.kind).map_err(|_| Error::Corrupt)? {
                wire::RelationKind::Contains | wire::RelationKind::HasPort if to == child => from,
                wire::RelationKind::Carries if to == child => from,
                wire::RelationKind::RunsOn if administrative && from == child => to,
                _ => return Err(Error::Invalid),
            };
        let parent_kind = wire::AssetKind::try_from(number(&self.asset(parent)?, "kind")? as i32)
            .map_err(|_| Error::Corrupt)?;
        let child_kind = wire::AssetKind::try_from(number(&self.asset(child)?, "kind")? as i32)
            .map_err(|_| Error::Corrupt)?;
        if !qualified(
            child,
            child_kind,
            parent,
            parent_kind,
            &relation,
            administrative,
        )? {
            return Err(Error::Invalid);
        }
        Ok(parent)
    }
}
