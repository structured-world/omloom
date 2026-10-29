//! Current layer-specific source claims. Equality supplies comparison evidence,
//! never an identity decision, a grant or permission to retarget an operation.
use super::*;

const MAX_CLAIMS: usize = 4;
const FIELD: &str = "identity_evidence";

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
        })
    }
}

impl Store {
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
        let comparison = api::HostIdentityComparison {
            revision,
            left: Some(tx.identity_subject(context, left)?),
            right: Some(tx.identity_subject(context, right)?),
        };
        tx.commit()?;
        if self.usage()?.revision != revision {
            return Err(Error::Expired);
        }
        Ok(reply(api::reply::Result::HostIdentityComparison(
            comparison,
        )))
    }
}
