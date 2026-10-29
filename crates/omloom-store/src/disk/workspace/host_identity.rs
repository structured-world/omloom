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
                (
                    "identity_lookup",
                    text(format!(
                        "{}/{}/{}",
                        claim.kind, claim.namespace, claim.digest
                    )),
                ),
                ("source", text(source.name)),
                ("mapping", text(source.mapping)),
                ("observed", int(source.observed)?),
                ("boot", text(source.boot)),
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
                    boot_basis: string(&row, "boot")?,
                })
            })
            .collect()
    }
}
