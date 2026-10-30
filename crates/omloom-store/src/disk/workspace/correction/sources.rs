use super::super::source_layout::{Family, Relocation};
use super::*;

#[cfg(test)]
mod tests;

fn renamed(mapping: &api::IdentityMapping, name: &str) -> String {
    if let Some(choice) = mapping.sources.iter().find(|choice| name == choice.before) {
        return choice.after.clone();
    }
    for choice in &mapping.sources {
        if let Some(part) = name.strip_prefix(&format!("{} / ", choice.before)) {
            return format!("{} / {part}", choice.after);
        }
    }
    name.into()
}
fn require(context: &Context, from: AssetRef, to: AssetRef, kind: i32, field: &str) -> Result<()> {
    for target in [from, to] {
        if !context.field(target, kind, field, false) || !context.field(target, kind, field, true) {
            return Err(Error::Denied);
        }
    }
    Ok(())
}
impl Tx<'_> {
    pub(super) fn move_correction_sources(
        &mut self,
        context: &Context,
        mapping: &api::IdentityMapping,
        from: AssetRef,
        to: AssetRef,
        kind: i32,
    ) -> Result<()> {
        if mapping.sources.len() > 8 {
            return Err(Error::Limit);
        }
        let mut choices = BTreeSet::new();
        for choice in &mapping.sources {
            if choice.before.is_empty()
                || choice.after.is_empty()
                || choice.before.len() > 100
                || choice.after.len() > 80
                || choice
                    .before
                    .chars()
                    .chain(choice.after.chars())
                    .any(char::is_control)
                || !choices.insert(choice.before.as_str())
            {
                return Err(Error::Invalid);
            }
        }
        let identities = self.fact_rows(&host_identity::parent(from), 32)?;
        if identities.len() + self.fact_rows(&host_identity::parent(to), 32)?.len() > 32 {
            return Err(Error::Limit);
        }
        for row in identities {
            require(context, from, to, kind, "identity_evidence")?;
            let key = string(&row, "key")?;
            let mut data = strip(row);
            data.extend(target_props(to));
            data.insert(
                "source".into(),
                text(renamed(mapping, &string(&data, "source")?)),
            );
            self.put(&key, &host_identity::parent(to), Class::Inventory, data)?;
        }
        let host_parent = format!("{}/host-mappings", asset_key(to));
        let mappings =
            self.fact_rows(&format!("{}/host-mappings", asset_key(from)), MAX_CHILDREN)?;
        if mappings.len() + self.fact_rows(&host_parent, MAX_CHILDREN)?.len() > MAX_CHILDREN {
            return Err(Error::Limit);
        }
        for row in mappings {
            require(context, from, to, kind, "identity_evidence")?;
            let key = string(&row, "key")?;
            let source = row
                .get("namespace")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or(string(&row, "source")?);
            let mut data = strip(row);
            data.extend(target_props(to));
            let label = renamed(mapping, &source);
            if label != string(&data, "source")? {
                data.insert("namespace".into(), text(label));
            } else {
                data.remove("namespace");
            }
            self.put(&key, &host_parent, Class::Inventory, data)?;
        }
        let csv=self.query("MATCH (n:OmRow {asset:$asset}) WHERE n.incarnation=$inc AND n.parent='csv-mappings' RETURN properties(n) AS p LIMIT 129",params([("asset",text(from.asset.to_string())),("inc",text(from.incarnation.to_string()))]))?;
        if csv.len() > 128 {
            return Err(Error::Limit);
        }
        for row in csv {
            let key = string(&row, "key")?;
            // Preserve the personal source's original target as a replay fence.
            // It neither authorizes the retained identity nor creates a duplicate.
            self.put(&key, "csv-mappings", Class::Checkpoint, target_props(from))?;
        }
        for (family, field) in [
            (Family::Addresses, "current_addresses"),
            (Family::Network, "network_evidence"),
        ] {
            let old_sources = self.fact_rows(&family.parent(from), 8)?;
            if old_sources.len() + self.fact_rows(&family.parent(to), 8)?.len() > 8 {
                return Err(Error::Limit);
            }
            for source in old_sources {
                require(context, from, to, kind, field)?;
                let old = string(&source, "key")?;
                let name = renamed(mapping, &string(&source, "source")?);
                let binding = source.get("binding").and_then(Value::as_str);
                let new = family.key(to, &name, binding);
                self.relocate_source(
                    Some(context),
                    Relocation {
                        from,
                        to,
                        old: &old,
                        new: &new,
                        name: &name,
                        binding,
                        family,
                    },
                )?;
            }
            let summaries = self.fact_rows(&family.parent(to), 8)?;
            let count = summaries.iter().try_fold(0u64, |sum, row| {
                sum.checked_add(number(row, "count")?).ok_or(Error::Limit)
            })?;
            if count
                > if family == Family::Addresses {
                    64
                } else {
                    current_facts::MAX_FACTS as u64
                }
            {
                return Err(Error::Limit);
            }
            if family == Family::Network {
                let bytes = summaries.iter().try_fold(0u64, |sum, row| {
                    sum.checked_add(number(row, "bytes")?).ok_or(Error::Limit)
                })?;
                if bytes > current_facts::MAX_BYTES as u64 {
                    return Err(Error::Limit);
                }
            }
        }
        Ok(())
    }
}
