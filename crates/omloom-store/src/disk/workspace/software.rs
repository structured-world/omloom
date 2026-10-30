//! Current software facts use the ordinary field authority, CAS, CSV and audit
//! pipeline. Catalog references are relations, not copied installation snapshots.
use super::*;
use alloc::borrow::Cow;
use wire::AssetKind as K;

const RELEASE_IDENTITY: [&str; 4] = ["version", "edition", "architecture", "artifact_digest"];
const CONTAINER_IDENTITY: [&str; 2] = ["runtime_object_id", "runtime_lifetime"];

fn stored_reference(row: &Properties, asset: &str, incarnation: &str) -> Result<AssetRef> {
    Ok(AssetRef {
        asset: row
            .get(asset)
            .and_then(Value::as_str)
            .ok_or(Error::Corrupt)?
            .parse()
            .map_err(|_| Error::Corrupt)?,
        incarnation: row
            .get(incarnation)
            .and_then(Value::as_str)
            .ok_or(Error::Corrupt)?
            .parse()
            .map_err(|_| Error::Corrupt)?,
    })
}

#[derive(Clone, Copy)]
enum Changes<'a> {
    Partial(&'a [wire::Field]),
    Full(&'a [wire::IntendedField]),
}
impl<'a> Changes<'a> {
    fn get(self, name: &str) -> Option<&'a wire::Field> {
        match self {
            Self::Partial(fields) => fields.iter().find(|field| field.name == name),
            Self::Full(fields) => fields
                .iter()
                .filter_map(|value| value.field.as_ref())
                .find(|field| field.name == name),
        }
    }
    fn touches(self, name: &str) -> bool {
        matches!(self, Self::Full(_)) || self.get(name).is_some()
    }
    fn preserves(self, name: &str, prior: &Properties) -> bool {
        match self.get(name) {
            Some(field) => inventory::field_matches(field, prior),
            None => matches!(self, Self::Partial(_)),
        }
    }
}

pub(super) fn fields(kind: K) -> &'static [&'static str] {
    match kind {
        K::SoftwareProduct => &["publisher", "raw_name", "normalization_basis"],
        K::SoftwareRelease => &[
            "version",
            "raw_version",
            "edition",
            "architecture",
            "artifact_digest",
            "normalization_basis",
        ],
        K::SoftwareInstallation => &[
            "installation_kind",
            "installation_scope",
            "lifecycle",
            "warranty_until",
            "support_until",
            "contract_reference",
            "purpose",
            "supplier",
            "model",
            "purchase_reference",
            "lifecycle_date",
            "disposal_reference",
        ],
        K::Workload => &[
            "workload_kind",
            "compose_project",
            "compose_service",
            "compose_files",
            "working_directory",
            "repository_reference",
            "source_revision",
            "dockerfile_path",
            "invocation_reference",
            "image_digest",
            "lifecycle",
            "contract_reference",
        ],
        K::Container => &[
            "runtime_object_id",
            "runtime_lifetime",
            "image_digest",
            "lifecycle",
        ],
        K::Device | K::VirtualMachine => &[
            "lifecycle",
            "warranty_until",
            "support_until",
            "contract_reference",
            "purpose",
            "supplier",
            "model",
            "purchase_reference",
            "lifecycle_date",
            "disposal_reference",
        ],
        _ => &[],
    }
}

pub(super) fn known_field(name: &str) -> bool {
    [
        K::SoftwareProduct,
        K::SoftwareRelease,
        K::SoftwareInstallation,
        K::Workload,
        K::Container,
    ]
    .into_iter()
    .any(|kind| fields(kind).contains(&name))
}

pub(super) fn validate_field(kind: i32, field: &wire::Field) -> Result<()> {
    let kind = K::try_from(kind).map_err(|_| Error::Invalid)?;
    if !fields(kind).contains(&field.name.as_str()) {
        return Err(Error::Invalid);
    }
    let value = match field.value.as_ref() {
        Some(field::Value::Clear(EXPLICIT_CLEAR)) => return Ok(()),
        Some(field::Value::Text(value)) => value,
        _ => return Err(Error::Invalid),
    };
    let max = match field.name.as_str() {
        "compose_files" => 2048,
        "runtime_object_id" | "runtime_lifetime" => 256,
        _ => 512,
    };
    if value.is_empty()
        || value.len() > max
        || value
            .chars()
            .any(|c| c.is_control() && !(field.name == "compose_files" && c == '\n'))
    {
        return Err(Error::Invalid);
    }
    let valid = match field.name.as_str() {
        "installation_kind" => {
            matches!(wire::InstallationKind::from_str_name(value), Some(v) if v != wire::InstallationKind::Unspecified)
        }
        "workload_kind" => matches!(
            wire::WorkloadKind::from_str_name(value),
            Some(wire::WorkloadKind::Standalone | wire::WorkloadKind::Compose)
        ),
        "lifecycle" => {
            matches!(wire::AssetLifecycle::from_str_name(value), Some(v) if v != wire::AssetLifecycle::Unspecified)
        }
        "warranty_until" | "support_until" | "lifecycle_date" => super::definitions::date(value),
        "compose_files" => {
            let mut seen = [""; 8];
            for (count, path) in value.split('\n').enumerate() {
                if count == seen.len() || path.trim().is_empty() || path.len() > 256 {
                    return Err(Error::Invalid);
                }
                for prior in &seen[..count] {
                    if *prior == path {
                        return Err(Error::Invalid);
                    }
                }
                seen[count] = path;
            }
            true
        }
        // References describe selected locations, not executable commands or
        // embedded credentials. Omloom does not fetch them or mirror source files.
        "repository_reference" | "invocation_reference" => {
            !value.contains('@') && !value.contains('?') && !value.contains('#')
        }
        "artifact_digest" | "image_digest" => value.strip_prefix("sha256:").is_some_and(|digest| {
            digest.len() == 64 && digest.bytes().all(|c| c.is_ascii_hexdigit())
        }),
        _ => true,
    };
    if valid { Ok(()) } else { Err(Error::Invalid) }
}

impl Tx<'_> {
    // One immutable namespace row and, when fully qualified, one uniqueness
    // claim prevent relabelling after unlink/restart. Both belong to the
    // container lifecycle and disappear with its explicit removal, not a TTL.
    pub(in crate::disk) fn bind_container_runtime(
        &mut self,
        container: AssetRef,
        runtime: Option<AssetRef>,
    ) -> Result<()> {
        let parent = asset_key(container);
        let key = format!("{parent}/runtime-basis");
        let old = self.get(&key)?;
        let runtime = match (runtime, &old) {
            (Some(runtime), Some(row))
                if stored_reference(row, "asset", "incarnation")? != runtime =>
            {
                return Err(Error::Conflict);
            }
            (Some(runtime), _) => runtime,
            (None, Some(row)) => stored_reference(row, "asset", "incarnation")?,
            (None, None) => return Ok(()),
        };
        let new_basis = if old.is_none() {
            let mut basis = target_props(runtime);
            basis.insert("layer".into(), text("runtime-identity"));
            Some(basis)
        } else {
            None
        };
        let basis = match &old {
            Some(row) => row,
            None => new_basis.as_ref().ok_or(Error::Corrupt)?,
        };
        if let (Some(id), Some(lifetime)) = (
            self.software_value(container, Changes::Partial(&[]), "runtime_object_id")?,
            self.software_value(container, Changes::Partial(&[]), "runtime_lifetime")?,
        ) {
            let mut digest = Sha256::new();
            let runtime_asset = basis
                .get("asset")
                .and_then(Value::as_str)
                .ok_or(Error::Corrupt)?;
            let runtime_incarnation = basis
                .get("incarnation")
                .and_then(Value::as_str)
                .ok_or(Error::Corrupt)?;
            for part in [
                runtime_asset,
                runtime_incarnation,
                id.as_ref(),
                lifetime.as_ref(),
            ] {
                digest.update((part.len() as u64).to_be_bytes());
                digest.update(part);
            }
            let claim_key = format!("runtime-instance/{:x}", digest.finalize());
            let prior = self.get(&claim_key)?;
            if let Some(prior) = &prior
                && stored_reference(prior, "asset", "incarnation")? != container
            {
                return Err(Error::Conflict);
            }
            if prior.is_none() {
                let mut claim = target_props(container);
                claim.insert("layer".into(), text("runtime-claim"));
                self.put_against(&claim_key, &parent, Class::Inventory, claim, None)?;
            }
        }
        if let Some(basis) = new_basis {
            self.put_against(&key, &parent, Class::Inventory, basis, None)?;
        }
        Ok(())
    }

    pub(in crate::disk) fn validate_full_software_asset(
        &self,
        target: AssetRef,
        asset: &wire::Asset,
    ) -> Result<()> {
        let kind = K::try_from(asset.kind).map_err(|_| Error::Invalid)?;
        for field in asset
            .intended
            .iter()
            .filter_map(|v| v.field.as_ref())
            .filter(|v| known_field(&v.name))
        {
            validate_field(asset.kind, field)?;
        }
        self.validate_software_changes(target, kind, Changes::Full(&asset.intended))
    }
    pub(super) fn release_is_installed(&self, target: AssetRef) -> Result<bool> {
        Ok(!self.query("MATCH (r:OmRow {to_asset:$asset}) WHERE r.parent='relations' AND r.to_incarnation=$inc AND r.kind=$kind RETURN {} AS p LIMIT 1", params([("asset", text(target.asset.to_string())), ("inc", text(target.incarnation.to_string())), ("kind", int(wire::RelationKind::InstalledRelease as u64)?)]))?.is_empty())
    }
    pub(in crate::disk) fn check_software_relation(
        &self,
        from: AssetRef,
        from_kind: K,
        kind: wire::RelationKind,
        id: &str,
    ) -> Result<()> {
        use wire::RelationKind as R;
        if !matches!(kind, R::ReleaseOf | R::InstalledRelease)
            && !(kind == R::RunsOn
                && matches!(from_kind, K::Container | K::Workload | K::VirtualMachine))
        {
            return Ok(());
        }
        let key = format!("relation/{id}");
        let rows = self.query("MATCH (r:OmRow {asset:$asset}) WHERE r.parent='relations' AND r.incarnation=$inc AND r.kind=$kind AND r.key<>$key RETURN {} AS p LIMIT 1", params([("asset", text(from.asset.to_string())), ("inc", text(from.incarnation.to_string())), ("kind", int(kind as u64)?), ("key", text(&key))]))?;
        if !rows.is_empty() {
            return Err(Error::Conflict);
        }
        Ok(())
    }

    pub(in crate::disk) fn check_software_relation_replacement(
        &self,
        old: &Properties,
        from: AssetRef,
        from_kind: K,
        to: AssetRef,
        kind: wire::RelationKind,
        rebound: bool,
    ) -> Result<()> {
        use wire::RelationKind as R;
        let old_kind = R::try_from(number(old, "kind")? as i32).map_err(|_| Error::Corrupt)?;
        if !matches!(old_kind, R::ReleaseOf | R::InstalledRelease | R::RunsOn) {
            return Ok(());
        }
        let old_from = stored_reference(old, "asset", "incarnation")?;
        let old_from_kind = if old_from == from {
            from_kind
        } else {
            K::try_from(number(&self.asset(old_from)?, "kind")? as i32)
                .map_err(|_| Error::Corrupt)?
        };
        if old_kind == R::RunsOn && old_from_kind == K::VirtualMachine {
            if !rebound && (from != old_from || kind != old_kind) {
                return Err(Error::Conflict);
            }
            if from != old_from || kind != old_kind {
                self.require_no_pending_operations(old_from)?;
            }
        }
        let container = old_kind == R::RunsOn && old_from_kind == K::Container;
        if matches!(old_kind, R::ReleaseOf | R::InstalledRelease) || container {
            if from != old_from || kind != old_kind {
                return Err(Error::Conflict);
            }
            if (old_kind == R::ReleaseOf || container)
                && to != stored_reference(old, "to_asset", "to_incarnation")?
            {
                return Err(Error::Conflict);
            }
        }
        Ok(())
    }
    fn software_value<'a>(
        &self,
        target: AssetRef,
        changes: Changes<'a>,
        name: &str,
    ) -> Result<Option<Cow<'a, str>>> {
        if let Some(field) = changes.get(name) {
            return match field.value.as_ref() {
                Some(field::Value::Text(value)) => Ok(Some(Cow::Borrowed(value))),
                Some(field::Value::Clear(EXPLICIT_CLEAR)) => Ok(None),
                _ => Err(Error::Corrupt),
            };
        }
        if matches!(changes, Changes::Full(_)) {
            return Ok(None);
        }
        let Some(mut row) = self.get(&format!("{}/intent/{name}", asset_key(target)))? else {
            return Ok(None);
        };
        match number(&row, "vtype")? {
            1 => match row.remove("vtext") {
                Some(Value::String(value)) => Ok(Some(Cow::Owned(value))),
                _ => Err(Error::Corrupt),
            },
            4 => Ok(None),
            _ => Err(Error::Corrupt),
        }
    }

    pub(super) fn validate_software_state(
        &self,
        target: AssetRef,
        kind: K,
        changes: &[wire::Field],
    ) -> Result<()> {
        self.validate_software_changes(target, kind, Changes::Partial(changes))
    }

    fn validate_software_changes(
        &self,
        target: AssetRef,
        kind: K,
        changes: Changes<'_>,
    ) -> Result<()> {
        if kind == K::SoftwareRelease
            && RELEASE_IDENTITY.iter().any(|name| changes.touches(name))
            && self.release_is_installed(target)?
        {
            for name in RELEASE_IDENTITY {
                if changes.touches(name)
                    && let Some(prior) =
                        self.get(&format!("{}/intent/{name}", asset_key(target)))?
                    && prior.get("vtext").and_then(Value::as_str).is_some()
                    && !changes.preserves(name, &prior)
                {
                    return Err(Error::Conflict);
                }
            }
        }
        if kind == K::Container {
            // A concrete runtime ID/lifetime cannot mutate into a replacement.
            // A deployment keeps continuity; a new container gets a new identity.
            for name in CONTAINER_IDENTITY {
                if changes.touches(name)
                    && let Some(prior) =
                        self.get(&format!("{}/intent/{name}", asset_key(target)))?
                    && prior.get("vtext").and_then(Value::as_str).is_some()
                    && !changes.preserves(name, &prior)
                {
                    return Err(Error::Conflict);
                }
            }
        }
        Ok(())
    }
}
