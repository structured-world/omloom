use super::*;
#[cfg(test)]
mod tests;

pub(super) const MAX_SOURCES: usize = 8;
pub(super) const MAX_FACTS: usize = 2048;
pub(super) const MAX_BYTES: usize = 256 * 1024;

pub(super) fn digest(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}
pub(super) fn source_key(parent: &str, source: &str) -> String {
    format!("source/{}", digest(format!("{parent}/{source}").as_bytes()))
}

// Values and exact evidence windows have separate lifetimes on the write path:
// reattesting a value changes only its small freshness row, never its payload.
impl Tx<'_> {
    pub(in crate::disk) fn retire_current_facts(&mut self, target: AssetRef) -> Result<()> {
        for row in self.fact_rows(
            &format!("{}/host-mappings", asset_key(target)),
            MAX_CHILDREN,
        )? {
            self.delete(&string(&row, "key")?)?;
        }
        for (family, parts) in [
            (
                "network-sources",
                &["sockets", "routes", "rules", "policies"][..],
            ),
            ("address-sources", &["values"][..]),
        ] {
            for source in self.fact_rows(
                &format!("{}/{}/{family}", asset_key(target), target.incarnation),
                MAX_SOURCES,
            )? {
                let key = string(&source, "key")?;
                for part in parts {
                    for row in self.fact_rows(&format!("{key}/{part}"), MAX_FACTS)? {
                        let key = string(&row, "key")?;
                        self.delete(&format!("{key}/freshness"))?;
                        self.delete(&key)?;
                    }
                }
                self.delete(&format!("{key}/coverage"))?;
                self.delete(&key)?;
            }
        }
        let sockets = self.query(
            "MATCH (n:OmRow {layer:$layer}) RETURN properties(n) AS p LIMIT 2049",
            params([("layer", text(network_facts::socket_layer(target)))]),
        )?;
        if sockets.len() > MAX_FACTS {
            return Err(Error::Limit);
        }
        for row in sockets {
            let key = string(&row, "key")?;
            let parent = string(&row, "parent")?;
            let source_key = parent.strip_suffix("/sockets").ok_or(Error::Corrupt)?;
            let mut source = self.get(source_key)?.ok_or(Error::Corrupt)?;
            let count = number(&source, "count")?
                .checked_sub(1)
                .ok_or(Error::Corrupt)?;
            let bytes = number(&source, "bytes")?
                .checked_sub(binary(&row, "payload")?.len() as u64)
                .ok_or(Error::Corrupt)?;
            let owner = string(&source, "parent")?;
            super::super::operations::strip_envelope(&mut source);
            source.insert("count".into(), int(count)?);
            source.insert("bytes".into(), int(bytes)?);
            self.put(source_key, &owner, Class::Inventory, source)?;
            self.delete(&format!("{key}/freshness"))?;
            self.delete(&key)?;
        }
        Ok(())
    }
    pub(super) fn fact_rows(&self, parent: &str, limit: usize) -> Result<Vec<Properties>> {
        let rows = self.query(
            "MATCH (n:OmRow {parent:$parent}) RETURN properties(n) AS p LIMIT $limit",
            params([
                ("parent", text(parent)),
                ("limit", int((limit + 1) as u64)?),
            ]),
        )?;
        if rows.len() > limit {
            return Err(Error::Limit);
        }
        Ok(rows)
    }

    pub(super) fn fact_windows(&self, rows: &[Properties]) -> Result<BTreeMap<String, Properties>> {
        if rows.is_empty() {
            return Ok(BTreeMap::new());
        }
        let keys = rows
            .iter()
            .map(|r| string(r, "key").map(|k| text(format!("{k}/freshness"))))
            .collect::<Result<Vec<_>>>()?;
        let windows = self.query(
            "UNWIND $keys AS key MATCH (n:OmRow {key:key}) RETURN properties(n) AS p LIMIT 2049",
            params([("keys", Value::Array(keys))]),
        )?;
        if windows.len() != rows.len() {
            return Err(Error::Corrupt);
        }
        windows
            .into_iter()
            .map(|r| Ok((string(&r, "parent")?, r)))
            .collect()
    }

    pub(super) fn replace_facts(
        &mut self,
        parent: &str,
        facts: BTreeMap<String, Properties>,
        complete: bool,
        observed: u64,
        basis: &str,
        limit: usize,
    ) -> Result<(usize, usize)> {
        let prior = self.fact_rows(parent, limit)?;
        let mut existing: BTreeMap<_, _> = prior
            .into_iter()
            .map(|row| Ok((string(&row, "key")?, row)))
            .collect::<Result<_>>()?;
        for (identity, data) in facts {
            let key = format!("fact/{}", digest(format!("{parent}/{identity}").as_bytes()));
            let previous = existing.remove(&key);
            self.put_projection(&key, parent, Class::Inventory, data, previous.as_ref())?;
            let fresh = format!("{key}/freshness");
            let old = self.get(&fresh)?;
            self.put_projection(
                &fresh,
                &key,
                Class::Inventory,
                props([("observed", int(observed)?), ("scan", text(basis))]),
                old.as_ref(),
            )?;
        }
        if complete {
            for key in existing.keys() {
                self.delete_projection(&format!("{key}/freshness"))?;
                self.delete_projection(key)?;
            }
        }
        let current = self.fact_rows(parent, limit)?;
        let bytes = current.iter().try_fold(0usize, |sum, row| {
            sum.checked_add(binary(row, "payload")?.len())
                .ok_or(Error::Limit)
        })?;
        Ok((current.len(), bytes))
    }
}
