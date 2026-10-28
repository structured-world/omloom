use super::*;
use api::{AccountCapability as Capability, AccountLinkKind as LinkKind};

const METADATA_FIELDS: [&str; 8] = [
    "issuer",
    "subject",
    "account_kind",
    "delegation",
    "login",
    "sender",
    "credential_binding_id",
    "recovery_route",
];
const CAPABILITIES: [(Capability, &str); 8] = [
    (Capability::ShareReference, "account.share"),
    (Capability::WorkloadUse, "account.use"),
    (Capability::RevealSecret, "account.reveal"),
    (Capability::Rotate, "account.rotate"),
    (Capability::Delegate, "account.delegate"),
    (Capability::RequestChange, "account.request"),
    (Capability::ApproveChange, "account.approve"),
    (Capability::ExecuteChange, "account.execute"),
];
fn permitted(context: &Context, account: AssetRef, field: &str, write: bool) -> bool {
    context.field(
        account,
        wire::AssetKind::ProviderAccount as i32,
        field,
        write,
    )
}
fn account_key(account: AssetRef) -> String {
    format!("{}/account", asset_key(account))
}
fn link_key(account: AssetRef, kind: LinkKind, target: AssetRef) -> String {
    format!(
        "{}/account-link/{}/{}/{}",
        asset_key(account),
        kind as i32,
        target.asset,
        target.incarnation
    )
}
fn metadata_props(metadata: &api::AccountMetadata) -> Result<Properties> {
    responsibility::bounded_value(&metadata.issuer, 256, true)?;
    responsibility::bounded_value(&metadata.subject, 256, true)?;
    if !matches!(
        api::ExternalAccountKind::try_from(metadata.kind),
        Ok(api::ExternalAccountKind::Administration | api::ExternalAccountKind::Programmatic)
    ) || api::AccountDelegation::try_from(metadata.delegation).is_err()
    {
        return Err(Error::Invalid);
    }
    let mut data = props([
        ("issuer", text(&metadata.issuer)),
        ("subject", text(&metadata.subject)),
        ("account_kind", int(metadata.kind as u64)?),
        ("delegation", int(metadata.delegation as u64)?),
    ]);
    for (name, value, max) in [
        ("login", &metadata.login, 256),
        ("sender", &metadata.sender, 256),
        ("credential_binding_id", &metadata.credential_binding_id, 36),
        ("recovery_route", &metadata.recovery_route, 512),
    ] {
        if let Some(value) = value {
            responsibility::bounded_value(value, max, true)?;
            if name == "credential_binding_id" {
                value.parse::<AuthorityId>().map_err(|_| Error::Invalid)?;
            }
            data.insert(name.into(), text(value));
        }
    }
    Ok(data)
}
fn identity_key(metadata: &api::AccountMetadata) -> String {
    let mut hash = Sha256::new();
    for value in [&metadata.issuer, &metadata.subject] {
        hash.update((value.len() as u64).to_be_bytes());
        hash.update(value);
    }
    format!("account-identity/{:x}", hash.finalize())
}

impl Store {
    pub(super) fn workspace_account(
        &self,
        context: &Context,
        request: api::ExternalAccount,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<api::Reply> {
        let account = parse_target(request.target.as_ref())?;
        let writing =
            request.metadata.is_some() || !request.add.is_empty() || !request.remove.is_empty();
        if request.add.len() + request.remove.len() > 32
            || (!writing && request.expected.is_some())
            || (writing && request.requests_cursor.is_some())
        {
            return Err(Error::Invalid);
        }
        let mut tx = self.begin()?;
        let selection = tx.selection(context, working, writing)?;
        selection.contains(account, &tx)?;
        let row = tx.visible_asset(context, account)?;
        if number(&row, "kind")? != wire::AssetKind::ProviderAccount as u64 {
            return Err(Error::Invalid);
        }
        if writing
            && (request.expected != Some(number(&row, "revision")?)
                || working.expected != Some(number(&tx.root, "revision")?))
        {
            return Err(Error::Conflict);
        }
        if (!request.add.is_empty() || !request.remove.is_empty())
            && !permitted(context, account, "account.bindings", true)
        {
            return Err(Error::Denied);
        }
        if !request.add.is_empty() {
            let rows=tx.query("MATCH (n:OmRow {parent:$key}) WHERE n.link_kind IS NOT NULL RETURN {key:n.key} AS p LIMIT 33",params([("key",text(asset_key(account)))]))?;
            if rows.len() > 32 {
                return Err(Error::Limit);
            }
            let mut keys = rows
                .iter()
                .map(|row| string(row, "key"))
                .collect::<Result<BTreeSet<_>>>()?;
            for link in &request.remove {
                let kind = LinkKind::try_from(link.kind).map_err(|_| Error::Invalid)?;
                if kind == LinkKind::Unspecified {
                    return Err(Error::Invalid);
                }
                keys.remove(&link_key(
                    account,
                    kind,
                    parse_target(link.target.as_ref())?,
                ));
            }
            for link in &request.add {
                let kind = LinkKind::try_from(link.kind).map_err(|_| Error::Invalid)?;
                if kind == LinkKind::Unspecified {
                    return Err(Error::Invalid);
                }
                keys.insert(link_key(account, kind, parse_target(link.target.as_ref())?));
            }
            if keys.len() > 32 {
                return Err(Error::Limit);
            }
        }
        let mut changed = false;
        if let Some(metadata) = &request.metadata {
            let data = metadata_props(metadata)?;
            let key = account_key(account);
            let old = tx.get(&key)?;
            for field in METADATA_FIELDS {
                if !data.contains_key(field)
                    && old.as_ref().is_some_and(|row| row.contains_key(field))
                    && !permitted(context, account, field, false)
                {
                    return Err(Error::Denied);
                }
                if old.as_ref().and_then(|r| r.get(field)) != data.get(field)
                    && !permitted(context, account, field, true)
                {
                    return Err(Error::Denied);
                }
            }
            if let Some(old) = &old
                && (old.get("issuer") != data.get("issuer")
                    || old.get("subject") != data.get("subject"))
            {
                return Err(Error::Conflict);
            }
            let identity = identity_key(metadata);
            if let Some(existing) = tx.get(&identity)? {
                if super::super::target(&existing)? != account {
                    return Err(Error::Conflict);
                }
            } else {
                tx.put(
                    &identity,
                    &asset_key(account),
                    Class::Inventory,
                    target_props(account),
                )?;
            }
            changed |= tx.put_against(
                &key,
                &asset_key(account),
                Class::Inventory,
                data,
                old.as_ref(),
            )?;
        }
        let mut edited = BTreeSet::new();
        for (links, removing) in [(&request.add, false), (&request.remove, true)] {
            if !links.is_empty() && !permitted(context, account, "account.bindings", true) {
                return Err(Error::Denied);
            }
            for link in links {
                let target = parse_target(link.target.as_ref())?;
                selection.contains(target, &tx)?;
                let kind = LinkKind::try_from(link.kind).map_err(|_| Error::Invalid)?;
                if kind == LinkKind::Unspecified
                    || !edited.insert((link.kind, target))
                    || target == account
                {
                    return Err(Error::Invalid);
                }
                let endpoint = tx.visible_asset(context, target)?;
                let endpoint_kind = number(&endpoint, "kind")? as i32;
                if !context.field(target, endpoint_kind, "account.bindings", true) {
                    return Err(Error::Denied);
                }
                let valid = match kind {
                    LinkKind::Service => endpoint_kind == wire::AssetKind::Service as i32,
                    LinkKind::Endpoint => endpoint_kind == wire::AssetKind::Interface as i32,
                    LinkKind::Consumer => matches!(
                        wire::AssetKind::try_from(endpoint_kind),
                        Ok(wire::AssetKind::Service
                            | wire::AssetKind::VirtualMachine
                            | wire::AssetKind::Device
                            | wire::AssetKind::SoftwareInstallation
                            | wire::AssetKind::Workload
                            | wire::AssetKind::Container)
                    ),
                    LinkKind::ManagedResource => {
                        endpoint_kind != wire::AssetKind::ProviderAccount as i32
                    }
                    LinkKind::Unspecified => false,
                };
                if !valid {
                    return Err(Error::Invalid);
                }
                let key = link_key(account, kind, target);
                let old = tx.get(&key)?;
                if removing {
                    if old.is_some() {
                        tx.delete(&key)?;
                        changed = true;
                    }
                } else {
                    let mut data = target_props(target);
                    data.insert("link_kind".into(), int(link.kind as u64)?);
                    changed |= tx.put_against(
                        &key,
                        &asset_key(account),
                        Class::Inventory,
                        data,
                        old.as_ref(),
                    )?;
                }
            }
        }
        if changed {
            tx.workspace_advance(account, &context.actor, "account.accepted", now)?;
        }
        let mut state = tx.account_state(context, account, request.requests_cursor.as_deref())?;
        state.revision = tx.commit()?;
        Ok(reply(api::reply::Result::ExternalAccount(state)))
    }

    pub(super) fn workspace_owner_request(
        &self,
        context: &Context,
        request: api::OwnerRequest,
        working: &api::WorkingContext,
        now: u64,
    ) -> Result<api::Reply> {
        use api::OwnerRequestAction as Action;
        let action = Action::try_from(request.action).map_err(|_| Error::Invalid)?;
        if action == Action::Unspecified {
            return Err(Error::Invalid);
        }
        let account = parse_target(request.account.as_ref())?;
        let mut tx = self.begin()?;
        tx.selection(context, working, action != Action::Read)?
            .contains(account, &tx)?;
        let account_row = tx.visible_asset(context, account)?;
        if number(&account_row, "kind")? != wire::AssetKind::ProviderAccount as u64 {
            return Err(Error::Invalid);
        }
        if !permitted(context, account, "account.requests", false) {
            return Err(Error::Denied);
        }
        if action != Action::Read
            && (request.account_revision != Some(number(&account_row, "revision")?)
                || working.expected != Some(number(&tx.root, "revision")?))
        {
            return Err(Error::Conflict);
        }
        let id = if action == Action::Create {
            if request.operation_id.is_some()
                || request.expected.is_some()
                || request.reported_outcome != wire::EffectOutcome::Unspecified as i32
            {
                return Err(Error::Invalid);
            }
            if !permitted(context, account, "account.request", true) {
                return Err(Error::Denied);
            }
            let target = parse_target(request.target.as_ref())?;
            let target_row = tx.visible_asset(context, target)?;
            let target_kind = number(&target_row, "kind")? as i32;
            if !context.field(target, target_kind, "account.request", true) {
                return Err(Error::Denied);
            }
            let binding = tx
                .get(&link_key(account, LinkKind::ManagedResource, target))?
                .ok_or(Error::Missing)?;
            if super::super::target(&binding)? != target {
                return Err(Error::Conflict);
            }
            let metadata = tx.get(&account_key(account))?.ok_or(Error::Incomplete)?;
            if number(&metadata, "delegation")? != api::AccountDelegation::OwnerOnly as u64 {
                return Err(Error::Incomplete);
            }
            responsibility::bounded_value(&request.change, 64, true)?;
            if !token(&request.change) || request.arguments.len() > 32 {
                return Err(Error::Invalid);
            }
            for argument in &request.arguments {
                if !context.field(target, target_kind, &argument.name, true) {
                    return Err(Error::Denied);
                }
                tx.validate_field(target_kind, argument)?;
            }
            let change = format!("owner.{}", request.change);
            let plan = Plan {
                target,
                inventory_revision: request.target_revision.ok_or(Error::Invalid)?,
                caller: &context.actor,
                action: &change,
                arguments: &request.arguments,
                retry_key: &request.retry_key,
                expires_at_ms: request.expires_at_ms,
            };
            tx.accept_plan(
                &plan,
                now,
                Some((account, number(&account_row, "revision")?)),
            )?
        } else {
            if request.target.is_some()
                || request.target_revision.is_some()
                || !request.change.is_empty()
                || !request.arguments.is_empty()
                || !request.retry_key.is_empty()
                || request.expires_at_ms != 0
            {
                return Err(Error::Invalid);
            }
            request
                .operation_id
                .as_ref()
                .ok_or(Error::Invalid)?
                .parse::<OperationId>()
                .map_err(|_| Error::Invalid)?
        };
        let key = operation_key(id);
        let mut operation = tx.get(&key)?.ok_or(Error::Missing)?;
        if operation.get("request_account") != Some(&text(network::identity(account))) {
            return Err(Error::Denied);
        }
        let target = super::super::target(&operation)?;
        let target_row = tx.visible_asset(context, target)?;
        if !context.field(
            target,
            number(&target_row, "kind")? as i32,
            "account.requests",
            false,
        ) {
            return Err(Error::Denied);
        }
        let saved = tx.operation(id)?;
        let target_kind = number(&target_row, "kind")? as i32;
        let arguments_complete = saved
            .arguments
            .iter()
            .all(|field| context.field(target, target_kind, &field.name, false));
        if !arguments_complete && matches!(action, Action::Approve | Action::Report) {
            return Err(Error::Denied);
        }
        if action == Action::Approve || action == Action::Report {
            if request.expected != Some(number(&operation, "revision")?)
                || request.account_revision != Some(number(&operation, "account_revision")?)
                || number(&target_row, "revision")? != number(&operation, "inventory_revision")?
            {
                return Err(Error::Conflict);
            }
            let field = if action == Action::Approve {
                "account.approve"
            } else {
                "account.execute"
            };
            if !permitted(context, account, field, true)
                || !context.field(target, number(&target_row, "kind")? as i32, field, true)
            {
                return Err(Error::Denied);
            }
            if action == Action::Approve
                && request.reported_outcome != wire::EffectOutcome::Unspecified as i32
            {
                return Err(Error::Invalid);
            }
            if action == Action::Approve && now >= number(&operation, "expires")? {
                return Err(Error::Expired);
            }
            let mut data = operation.clone();
            operations::strip_envelope(&mut data);
            let event = if action == Action::Approve {
                data.insert("approved".into(), Value::Bool(true));
                "owner-request.approved"
            } else {
                if operation.get("approved") != Some(&Value::Bool(true)) {
                    return Err(Error::UnsafeTransition);
                }
                if !matches!(
                    wire::EffectOutcome::try_from(request.reported_outcome),
                    Ok(wire::EffectOutcome::Unknown
                        | wire::EffectOutcome::Confirmed
                        | wire::EffectOutcome::ConfirmedNoEffect)
                ) {
                    return Err(Error::Invalid);
                }
                if operation.get("owner_reported") == Some(&Value::Bool(true)) {
                    let previous =
                        wire::EffectOutcome::try_from(number(&operation, "outcome")? as i32)
                            .map_err(|_| Error::Corrupt)?;
                    // An uncertain owner assertion may be refined, but definite effects cannot be contradicted.
                    if previous != wire::EffectOutcome::Unknown
                        && previous as i32 != request.reported_outcome
                    {
                        return Err(Error::UnsafeTransition);
                    }
                }
                data.insert("owner_reported".into(), Value::Bool(true));
                data.insert("state".into(), int(OperationState::NeedsAttention as u64)?);
                data.insert("outcome".into(), int(request.reported_outcome as u64)?);
                "owner-request.reported"
            };
            let changed = data
                .iter()
                .any(|(name, value)| operation.get(name) != Some(value));
            if changed {
                let revision = number(&operation, "revision")?
                    .checked_add(1)
                    .ok_or(Error::Limit)?;
                data.insert("revision".into(), int(revision)?);
                tx.put_against(&key, "operations", Class::Pending, data, Some(&operation))?;
                tx.audit(&key, &context.actor, event, revision, now)?;
                operation = tx.get(&key)?.ok_or(Error::Corrupt)?;
            }
        } else if action == Action::Read
            && (request.expected.is_some()
                || request.account_revision.is_some()
                || request.reported_outcome != wire::EffectOutcome::Unspecified as i32)
        {
            return Err(Error::Invalid);
        }
        let saved = tx.operation(id)?;
        let arguments = saved
            .arguments
            .into_iter()
            .filter(|field| context.field(target, target_kind, &field.name, false))
            .collect();
        let result = api::OwnerRequestState {
            operation: Some(wire::Operation {
                operation_id: id.to_string(),
                target: Some(target_wire(saved.target)),
                inventory_revision: saved.inventory_revision,
                plan_digest: saved.plan_digest.to_vec(),
                state: saved.state as i32,
                outcome: saved.outcome as i32,
                provider_task_id: saved.provider_task,
                revision: saved.revision,
            }),
            arguments,
            change: saved
                .action
                .strip_prefix("owner.")
                .ok_or(Error::Corrupt)?
                .to_owned(),
            approved: operation.get("approved") == Some(&Value::Bool(true)),
            owner_reported: operation.get("owner_reported") == Some(&Value::Bool(true)),
            arguments_complete,
        };
        tx.commit()?;
        Ok(reply(api::reply::Result::OwnerRequest(result)))
    }
}

impl Tx<'_> {
    pub(in super::super) fn require_no_account_references(&self, target: AssetRef) -> Result<()> {
        let pending=self.query("MATCH (n:OmRow {request_account:$account}) WHERE n.class=$class RETURN properties(n) AS p LIMIT 1",params([("account",text(network::identity(target))),("class",int(Class::Pending as u64)?)]))?;
        if !pending.is_empty() {
            return Err(Error::UnsafeTransition);
        }
        let references=self.query("MATCH (n:OmRow {asset:$asset}) WHERE n.incarnation=$incarnation AND n.link_kind IS NOT NULL RETURN properties(n) AS p LIMIT 1",params([("asset",text(target.asset.to_string())),("incarnation",text(target.incarnation.to_string()))]))?;
        if !references.is_empty() {
            return Err(Error::Conflict);
        }
        Ok(())
    }
    fn account_state(
        &self,
        context: &Context,
        account: AssetRef,
        cursor: Option<&str>,
    ) -> Result<api::ExternalAccountState> {
        let row = self.asset(account)?;
        let mut metadata = None;
        if let Some(data) = self.get(&account_key(account))? {
            let mut visible = api::AccountMetadata::default();
            if permitted(context, account, "issuer", false) {
                visible.issuer = string(&data, "issuer")?;
            }
            if permitted(context, account, "subject", false) {
                visible.subject = string(&data, "subject")?;
            }
            if permitted(context, account, "account_kind", false) {
                visible.kind = number(&data, "account_kind")? as i32;
            }
            if permitted(context, account, "delegation", false) {
                visible.delegation = number(&data, "delegation")? as i32;
            }
            for (name, value) in [
                ("login", &mut visible.login),
                ("sender", &mut visible.sender),
                ("credential_binding_id", &mut visible.credential_binding_id),
                ("recovery_route", &mut visible.recovery_route),
            ] {
                if permitted(context, account, name, false) {
                    *value = data.get(name).and_then(Value::as_str).map(str::to_owned);
                }
            }
            if METADATA_FIELDS
                .iter()
                .any(|field| permitted(context, account, field, false))
            {
                metadata = Some(visible);
            }
        }
        let mut links = Vec::new();
        let mut related = Vec::new();
        if permitted(context, account, "account.bindings", false) {
            let rows = self.query("MATCH (n:OmRow {parent:$key}) WHERE n.link_kind IS NOT NULL RETURN properties(n) AS p LIMIT 33", params([("key", text(asset_key(account)))]))?;
            if rows.len() > 32 {
                return Err(Error::Limit);
            }
            let keys = rows
                .iter()
                .map(|row| super::super::target(row).map(|t| text(asset_key(t))))
                .collect::<Result<Vec<_>>>()?;
            let targets = self.query(
                "UNWIND $keys AS key MATCH (n:OmRow {key:key}) RETURN properties(n) AS p",
                params([("keys", Value::Array(keys))]),
            )?;
            let targets = targets
                .into_iter()
                .map(|row| super::super::target(&row).map(|t| (t, row)))
                .collect::<Result<BTreeMap<_, _>>>()?;
            for link in rows {
                let target = super::super::target(&link)?;
                let Some(row) = targets.get(&target) else {
                    continue;
                };
                if number(row, "adoption")? == wire::Adoption::Adopted as u64
                    && context.field(
                        target,
                        number(row, "kind")? as i32,
                        "account.bindings",
                        false,
                    )
                {
                    links.push(api::AccountLink {
                        kind: number(&link, "link_kind")? as i32,
                        target: Some(target_wire(target)),
                    });
                    let fields =
                        if context.field(target, number(row, "kind")? as i32, "name", false) {
                            row.get("name")
                                .and_then(Value::as_str)
                                .map(|name| wire::Field {
                                    name: "name".into(),
                                    value: Some(field::Value::Text(name.into())),
                                })
                                .into_iter()
                                .collect()
                        } else {
                            Vec::new()
                        };
                    related.push(api::Item {
                        target: Some(target_wire(target)),
                        kind: number(row, "kind")? as i32,
                        revision: number(row, "revision")?,
                        fields,
                    });
                }
            }
        }
        let revision = number(&self.root, "revision")?;
        let mut requests = Vec::new();
        let mut next = None;
        if permitted(context, account, "account.requests", false) {
            let mut after = String::new();
            let fingerprint = assets::query_fingerprint(
                &api::Query {
                    roots: vec![target_wire(account)],
                    ..Default::default()
                },
                context,
            );
            if let Some(cursor) = cursor {
                if cursor.len() > 256 {
                    return Err(Error::Invalid);
                }
                let parts: Vec<_> = cursor.split(':').collect();
                if parts.len() != 4
                    || parts[0].parse::<u64>().ok() != Some(revision)
                    || parts[1] != fingerprint
                    || parts[2].parse::<u64>().ok() != Some(context.authority_revision)
                {
                    return Err(Error::Expired);
                }
                after = parts[3].into();
            }
            let mut values = HashMap::from([
                ("account".into(), text(network::identity(account))),
                ("after".into(), text(&after)),
            ]);
            let visible =
                assets::visible_clause_for(context, Some("account.requests"), &mut values, "a");
            let rows = self.query(&format!("MATCH (n:OmRow {{request_account:$account}}) MATCH (a:OmRow {{key:'asset/'+n.asset}}) WHERE a.incarnation=n.incarnation AND a.adoption={} AND n.key>$after AND {visible} RETURN properties(n) AS p ORDER BY n.key LIMIT 21", wire::Adoption::Adopted as i32), Some(values))?;
            for operation in rows {
                let key = string(&operation, "key")?;
                if requests.len() == 20 {
                    next = Some(format!(
                        "{revision}:{fingerprint}:{}:{after}",
                        context.authority_revision
                    ));
                    break;
                }
                after = key;
                let target = super::super::target(&operation)?;
                requests.push(wire::Operation {
                    operation_id: string(&operation, "operation")?,
                    target: Some(target_wire(target)),
                    inventory_revision: number(&operation, "inventory_revision")?,
                    plan_digest: stored_digest(&operation)?.to_vec(),
                    state: number(&operation, "state")? as i32,
                    outcome: number(&operation, "outcome")? as i32,
                    provider_task_id: None,
                    revision: number(&operation, "revision")?,
                });
            }
        } else if cursor.is_some() {
            return Err(Error::Denied);
        }
        let writable = METADATA_FIELDS
            .into_iter()
            .chain(["account.bindings"])
            .filter(|field| permitted(context, account, field, true))
            .map(str::to_owned)
            .collect();
        Ok(api::ExternalAccountState {
            item: Some(api::Item {
                target: Some(target_wire(account)),
                kind: wire::AssetKind::ProviderAccount as i32,
                revision: number(&row, "revision")?,
                fields: Vec::new(),
            }),
            metadata,
            links,
            related,
            requests,
            requests_next: next,
            authorized: CAPABILITIES
                .into_iter()
                .filter(|(_, field)| permitted(context, account, field, true))
                .map(|(cap, _)| cap as i32)
                .collect(),
            writable,
            metadata_fully_readable: METADATA_FIELDS
                .into_iter()
                .all(|field| permitted(context, account, field, false)),
            revision,
        })
    }
}
