//! Current layer-specific source claims. Equality supplies comparison evidence,
//! never an identity decision, a grant or permission to retarget an operation.
use super::*;

const MAX_CLAIMS: usize = 4;
const FIELD: &str = "identity_evidence";
fn lookup(claim: &api::HostIdentityClaim) -> String {
    format!("{}/{}/{}", claim.kind, claim.namespace, claim.digest)
}
pub(in crate::disk) fn upgrade(store: &Store) -> Result<()> {
    let mut after = String::new();
    loop {
        let mut tx = store.begin()?;
        let rows=tx.query("MATCH (n:OmRow {class:0}) WHERE n.key>$after AND n.parent ENDS WITH '/identity-evidence' RETURN properties(n) AS p ORDER BY n.key LIMIT 128",params([("after",text(&after))]))?;
        if rows.is_empty() {
            tx.commit()?;
            return Ok(());
        }
        for old in rows {
            let key = string(&old, "key")?;
            let parent = string(&old, "parent")?;
            let claim = api::HostIdentityClaim::decode(binary(&old, "payload")?)
                .map_err(|_| Error::Corrupt)?;
            let mut data = old.clone();
            operations::strip_envelope(&mut data);
            data.insert("claim".into(), text(lookup(&claim)));
            tx.put_projection(&key, &parent, Class::Inventory, data, Some(&old))?;
            after = key;
        }
        tx.commit()?;
    }
}

pub(super) fn parent(target: AssetRef) -> String {
    format!(
        "{}/{}/identity-evidence",
        asset_key(target),
        target.incarnation
    )
}

pub(super) struct Source<'a> {
    pub mapping: &'a str,
    pub name: &'a str,
    pub observed: u64,
    pub boot: &'a str,
}

pub(super) fn validate(node: &api::HostFinding) -> Result<()> {
    use api::HostIdentityKind as I;
    use wire::AssetKind as K;
    if node.identity.len() > MAX_CLAIMS {
        return Err(Error::Limit);
    }
    let mut keys = BTreeSet::new();
    for claim in &node.identity {
        let kind = I::try_from(claim.kind).map_err(|_| Error::Invalid)?;
        let subject = K::try_from(node.kind).map_err(|_| Error::Invalid)?;
        let applicable = match kind {
            I::HardwareUuid => {
                matches!(subject, K::Device | K::VirtualMachine)
                    || (subject == K::Unspecified && node.key == "hardware")
            }
            I::InstallationId => subject == K::SoftwareInstallation,
            I::InterfaceId | I::FactoryMac => subject == K::Interface,
            I::Unspecified => false,
        };
        if !applicable
            || claim.namespace.is_empty()
            || claim.namespace.len() > 80
            || !claim
                .namespace
                .bytes()
                .all(|v| v.is_ascii_alphanumeric() || b"/-_.".contains(&v))
            || claim.digest.len() != 64
            || !claim
                .digest
                .bytes()
                .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
            || !matches!(
                api::HostIdentityQuality::try_from(claim.quality),
                Ok(api::HostIdentityQuality::Reported
                    | api::HostIdentityQuality::Movable
                    | api::HostIdentityQuality::Unreliable)
            )
            || !keys.insert((claim.kind, claim.namespace.as_str()))
        {
            return Err(Error::Invalid);
        }
    }
    Ok(())
}

impl Tx<'_> {
    pub(super) fn accept_host_identity(
        &mut self,
        context: &Context,
        target: AssetRef,
        kind: i32,
        source: Source<'_>,
        claims: &[api::HostIdentityClaim],
    ) -> Result<()> {
        // An empty old-format report supplies no statement about identity.
        // It cannot erase claims collected by a newer, selected source profile.
        if claims.is_empty() {
            return Ok(());
        }
        if !context.field(target, kind, FIELD, true) {
            return Err(Error::Denied);
        }
        let parent = parent(target);
        let rows = self.fact_rows(&parent, MAX_CLAIMS * current_facts::MAX_SOURCES)?;
        let mut count = rows.len();
        for claim in claims {
            let slot = format!("{}/{}/{}", source.mapping, claim.kind, claim.namespace);
            let key = format!("host-identity/{}", current_facts::digest(slot.as_bytes()));
            let old = self.get(&key)?;
            let payload = claim.encode_to_vec();
            if let Some(old) = &old {
                let observed = number(old, "observed")?;
                if observed > source.observed
                    || (observed == source.observed && binary(old, "payload")? != payload)
                {
                    return Err(Error::Conflict);
                }
            }
            if old.is_none() {
                if count == MAX_CLAIMS * current_facts::MAX_SOURCES {
                    return Err(Error::Limit);
                }
                count += 1;
            }
            let mut data = target_props(target);
            data.extend(props([
                ("source", text(source.name)),
                ("binding", text(source.mapping)),
                ("observed", int(source.observed)?),
                ("vtext", text(source.boot)),
                ("payload", Value::Binary(payload)),
                ("claim", text(lookup(claim))),
            ]));
            self.put_projection(&key, &parent, Class::Inventory, data, old.as_ref())?;
        }
        Ok(())
    }

    pub(super) fn host_identity(
        &self,
        context: &Context,
        target: AssetRef,
        kind: i32,
    ) -> Result<Vec<api::HostIdentityObservation>> {
        if !context.field(target, kind, FIELD, false) {
            return Ok(Vec::new());
        }
        self.fact_rows(&parent(target), MAX_CLAIMS * current_facts::MAX_SOURCES)?
            .into_iter()
            .map(|row| {
                Ok(api::HostIdentityObservation {
                    claim: Some(
                        api::HostIdentityClaim::decode(binary(&row, "payload")?)
                            .map_err(|_| Error::Corrupt)?,
                    ),
                    source: string(&row, "source")?,
                    observed_at: number(&row, "observed")?,
                    boot_basis: string(&row, "vtext")?,
                })
            })
            .collect()
    }

    fn identity_subject(
        &self,
        context: &Context,
        target: AssetRef,
    ) -> Result<api::HostIdentitySubject> {
        let row = self.visible_asset(context, target)?;
        let kind = number(&row, "kind")? as i32;
        if !matches!(
            wire::AssetKind::try_from(kind),
            Ok(wire::AssetKind::Device
                | wire::AssetKind::VirtualMachine
                | wire::AssetKind::SoftwareInstallation
                | wire::AssetKind::Interface)
        ) {
            return Err(Error::Invalid);
        }
        let rows = self.query(
            "MATCH (n:OmRow {parent:$parent}) WHERE n.layer='intent' RETURN properties(n) AS p LIMIT 33",
            params([("parent", text(asset_key(target)))]),
        )?;
        if rows.len() > MAX_FIELDS {
            return Err(Error::Limit);
        }
        let mut fields = Vec::with_capacity(rows.len());
        for row in rows {
            let name = string(&row, "field")?;
            if context.field(target, kind, &name, false) {
                fields.push(inventory::read_field(&row)?);
            }
        }
        Ok(api::HostIdentitySubject {
            item: Some(api::Item {
                target: Some(target_wire(target)),
                kind,
                revision: number(&row, "revision")?,
                fields,
            }),
            identity: self.host_identity(context, target, kind)?,
            sources: Vec::new(),
            relations: Vec::new(),
        })
    }
}

impl Store {
    pub(super) fn workspace_resolve_identity(
        &self,
        context: &Context,
        request: api::ResolveIdentity,
        working: &api::WorkingContext,
    ) -> Result<api::Reply> {
        let original = parse_target(request.original.as_ref())?;
        let mut current = original;
        let tx = self.begin()?;
        let mut seen = BTreeSet::new();
        for _ in 0..8 {
            if !seen.insert(current) {
                return Err(Error::Corrupt);
            }
            if let Some(row) = tx.get(&asset_key(current))? {
                if super::super::target(&row)? != current {
                    return Err(Error::Conflict);
                }
                let mut selector = working.clone();
                for root in &mut selector.roots {
                    if root == &target_wire(original) {
                        *root = target_wire(current);
                    }
                }
                if selector.focus.as_ref() == Some(&target_wire(original)) {
                    selector.focus = Some(target_wire(current));
                }
                tx.selection(context, &selector, false)?
                    .contains(current, &tx)?;
                let item = tx.identity_subject(context, current)?.item;
                if original != current
                    && !context.readable(original, item.as_ref().ok_or(Error::Corrupt)?.kind)
                {
                    return Err(Error::Denied);
                }
                tx.commit()?;
                return Ok(reply(api::reply::Result::ResolvedIdentity(
                    api::ResolvedIdentity {
                        original: Some(target_wire(original)),
                        canonical: item,
                    },
                )));
            }
            let fence = tx
                .get(&format!("removed/{}", current.asset))?
                .ok_or(Error::Missing)?;
            if !fence.contains_key("to_asset") {
                return Err(Error::Missing);
            }
            if super::super::target(&fence)? != current {
                return Err(Error::Conflict);
            }
            current = AssetRef {
                asset: string(&fence, "to_asset")?
                    .parse()
                    .map_err(|_| Error::Corrupt)?,
                incarnation: string(&fence, "to_incarnation")?
                    .parse()
                    .map_err(|_| Error::Corrupt)?,
            };
        }
        Err(Error::Limit)
    }
    pub(super) fn workspace_identity_candidates(
        &self,
        context: &Context,
        request: api::FindIdentityCandidates,
        working: &api::WorkingContext,
    ) -> Result<api::Reply> {
        let tx = self.begin()?;
        let selection = tx.selection(context, working, false)?;
        let (target, kind, claims) = if let Some(reference) = &request.target {
            if !request.claims.is_empty() || request.kind != 0 {
                return Err(Error::Invalid);
            }
            let target = parse_target(Some(reference))?;
            selection.contains(target, &tx)?;
            let row = tx.visible_asset(context, target)?;
            let kind = number(&row, "kind")? as i32;
            (
                Some(target),
                kind,
                tx.host_identity(context, target, kind)?
                    .into_iter()
                    .filter_map(|v| v.claim)
                    .collect::<Vec<_>>(),
            )
        } else {
            if request.claims.is_empty()
                || !matches!(
                    wire::AssetKind::try_from(request.kind),
                    Ok(wire::AssetKind::Device
                        | wire::AssetKind::VirtualMachine
                        | wire::AssetKind::SoftwareInstallation
                        | wire::AssetKind::Interface)
                )
            {
                return Err(Error::Invalid);
            }
            validate(&api::HostFinding {
                kind: request.kind,
                identity: request.claims.clone(),
                ..Default::default()
            })?;
            (None, request.kind, request.claims)
        };
        let mut values = HashMap::from([
            (
                "claims".into(),
                Value::Array(claims.iter().map(|v| text(lookup(v))).collect()),
            ),
            (
                "excluded".into(),
                Value::Array(target.into_iter().map(|v| text(asset_key(v))).collect()),
            ),
            ("kind".into(), Value::Int(kind as i64)),
            (
                "adopted".into(),
                Value::Int(wire::Adoption::Adopted as i32 as i64),
            ),
        ]);
        let visible = assets::visible_clause_for(context, Some(FIELD), &mut values, "a");
        let readable = assets::visible_clause_for(context, None, &mut values, "a");
        let rows=tx.query(&format!("UNWIND $claims AS claim MATCH (n:OmRow {{claim:claim}}) MATCH (a:OmRow {{key:'asset/'+n.asset}}) WHERE NOT a.key IN $excluded AND a.kind=$kind AND a.incarnation=n.incarnation AND n.parent=a.key+'/'+a.incarnation+'/identity-evidence' AND a.adoption=$adopted AND {visible} AND {readable} RETURN DISTINCT properties(a) AS p LIMIT 129"),Some(values))?;
        if rows.len() > 128 {
            return Err(Error::Limit);
        }
        let mut items = Vec::new();
        for row in rows {
            let candidate = super::super::target(&row)?;
            match selection.contains(candidate, &tx) {
                Ok(()) => {}
                Err(Error::Denied) => continue,
                Err(error) => return Err(error),
            }
            let item = tx
                .identity_subject(context, candidate)?
                .item
                .ok_or(Error::Corrupt)?;
            if items.len() == 25 {
                return Err(Error::Limit);
            }
            items.push(item);
        }
        tx.commit()?;
        Ok(reply(api::reply::Result::IdentityCandidates(
            api::IdentityCandidates { items },
        )))
    }
    pub(super) fn workspace_compare_host_identity(
        &self,
        context: &Context,
        value: api::CompareHostIdentity,
        working: &api::WorkingContext,
    ) -> Result<api::Reply> {
        let left = parse_target(value.left.as_ref())?;
        let right = parse_target(value.right.as_ref())?;
        if left == right {
            return Err(Error::Invalid);
        }
        let tx = self.begin()?;
        let revision = number(&tx.root, "revision")?;
        let selection = tx.selection(context, working, false)?;
        selection.contains(left, &tx)?;
        selection.contains(right, &tx)?;
        let mut comparison = api::HostIdentityComparison {
            revision,
            left: Some(tx.identity_subject(context, left)?),
            right: Some(tx.identity_subject(context, right)?),
        };
        if value.include_relations {
            for subject in [&mut comparison.left, &mut comparison.right]
                .into_iter()
                .flatten()
            {
                let item = subject.item.as_ref().ok_or(Error::Corrupt)?;
                let target = parse_target(item.target.as_ref())?;
                let mut sources = BTreeSet::new();
                if context.field(target, item.kind, "identity_evidence", false) {
                    for row in tx.fact_rows(&format!("{}/host-mappings", asset_key(target)), 128)? {
                        sources.insert(
                            row.get("namespace")
                                .and_then(Value::as_str)
                                .unwrap_or(&string(&row, "source")?)
                                .to_owned(),
                        );
                    }
                }
                for (family, field) in [
                    ("address-sources", "current_addresses"),
                    ("network-sources", "network_evidence"),
                ] {
                    if context.field(target, item.kind, field, false) {
                        for row in tx.fact_rows(
                            &format!("{}/{}/{family}", asset_key(target), target.incarnation),
                            8,
                        )? {
                            sources.insert(string(&row, "source")?);
                        }
                    }
                }
                subject.sources = sources
                    .iter()
                    .filter(|source| {
                        !sources
                            .iter()
                            .any(|base| source.starts_with(&format!("{base} / ")))
                    })
                    .cloned()
                    .collect();
                if context.field(target, item.kind, "relations", false) {
                    for relation in tx.working_relations(target)? {
                        let from = parse_target(relation.from.as_ref())?;
                        let to = parse_target(relation.to.as_ref())?;
                        let a = tx.asset(from)?;
                        let b = tx.asset(to)?;
                        if !context.field(from, number(&a, "kind")? as i32, "relations", false)
                            || !context.field(to, number(&b, "kind")? as i32, "relations", false)
                            || !context.readable(from, number(&a, "kind")? as i32)
                            || !context.readable(to, number(&b, "kind")? as i32)
                        {
                            continue;
                        }
                        let mut in_scope = true;
                        for endpoint in [from, to] {
                            match selection.contains(endpoint, &tx) {
                                Ok(()) => {}
                                Err(Error::Denied) => {
                                    in_scope = false;
                                    break;
                                }
                                Err(error) => return Err(error),
                            }
                        }
                        if in_scope {
                            subject.relations.push(relation);
                        }
                    }
                }
            }
        }
        tx.commit()?;
        if self.usage()?.revision != revision {
            return Err(Error::Expired);
        }
        Ok(reply(api::reply::Result::HostIdentityComparison(
            comparison,
        )))
    }
}
