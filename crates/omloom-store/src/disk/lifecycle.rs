use super::*;
use coordinode_embed::backup::{BackupFormat, export, restore::RestoreOptions};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cleanup {
    pub removed: u16,
    pub pinned: u16,
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityEntry {
    pub actor: String,
    pub action: String,
    pub revision: u64,
    pub at_ms: u64,
}

impl Store {
    /// Read a bounded recent activity slice. The host supplies an authorized
    /// owner key, never an arbitrary query; current-field reads do not use it.
    pub fn activity(&self, owner: &str, limit: u16) -> Result<Vec<ActivityEntry>> {
        bounded(owner)?;
        if limit == 0 || limit > 128 {
            return Err(Error::Invalid);
        }
        let tx = self.begin()?;
        let rows=tx.query("MATCH (n:OmRow) WHERE n.parent=$parent RETURN properties(n) AS p, n.revision AS ordering ORDER BY ordering DESC LIMIT $limit",params([("parent",text(format!("detail/{owner}"))),("limit",int(u64::from(limit))?)]))?;
        let output = rows
            .iter()
            .map(|p| {
                Ok(ActivityEntry {
                    actor: string(p, "actor")?,
                    action: string(p, "action")?,
                    revision: number(p, "revision")?,
                    at_ms: number(p, "at")?,
                })
            })
            .collect::<Result<_>>()?;
        tx.commit()?;
        Ok(output)
    }
    /// Privileged host database backup, including every namespace of an injected
    /// database. Never expose this as an authorized inventory export endpoint.
    pub fn export<W: std::io::Write>(&self, writer: &mut W) -> Result<()> {
        let snapshot = self.db.engine().snapshot();
        export::export_binary(self.db.engine(), &self.db.interner()?, 1, &snapshot, writer)
            .map_err(|e| Error::Export(Box::new(e)))?;
        Ok(())
    }
    /// Import into a fresh database with dispatch durably disabled. A logical
    /// backup cannot prove current authority or completed external effects.
    /// The host supplies a separate encoded-input ceiling for its whole backup,
    /// enforcing that ceiling before buffering the input as well.
    pub fn restore(
        path: &Path,
        image: &[u8],
        policy: Policy,
        max_backup_bytes: u64,
    ) -> Result<Self> {
        if !policy.valid() || max_backup_bytes == 0 || image.len() as u64 > max_backup_bytes {
            return Err(Error::Limit);
        }
        let mut db = Database::open(path)?;
        let rows = db.execute_cypher("MATCH (n) RETURN id(n) AS id LIMIT 1")?;
        if !rows.is_empty() {
            return Err(Error::Conflict);
        }
        // The marker must precede import: a crash between import and the root
        // update must not turn an old snapshot into a dispatchable installation.
        use std::io::Write;
        let mut marker = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path.join("RESTORE_REQUIRED"))
            .map_err(Error::Io)?;
        marker
            .write_all(b"Omloom logical restore requires reconciliation\n")
            .map_err(Error::Io)?;
        marker.sync_all().map_err(Error::Io)?;
        #[cfg(unix)]
        std::fs::File::open(path)
            .and_then(|directory| directory.sync_all())
            .map_err(Error::Io)?;
        db.restore(BackupFormat::Binary, &image, &RestoreOptions::default())
            .map_err(|e| Error::Restore(Box::new(e)))?;
        let store = Self::from_database(db, policy)?;
        store.disable_restored_dispatch()?;
        Ok(store)
    }
    pub(super) fn disable_restored_dispatch(&self) -> Result<()> {
        let mut tx = self.begin()?;
        if tx.root.get("restored") == Some(&Value::Bool(true)) {
            tx.commit()?;
            return Ok(());
        }
        tx.root.insert("restored".into(), Value::Bool(true));
        tx.changed = true;
        tx.commit()?;
        Ok(())
    }
    pub fn recovery_required(&self) -> Result<bool> {
        Ok(self.read(ROOT)?.ok_or(Error::Corrupt)?.get("restored") == Some(&Value::Bool(true)))
    }

    /// Pin recent audit/detail for an admitted recovery consumer. The whole
    /// transaction conflicts with concurrent cleanup through the root CAS guard.
    pub fn pin_detail(&self, key: &str, pin: bool, expected_revision: u64) -> Result<()> {
        bounded(key)?;
        let mut tx = self.begin()?;
        if number(&tx.root, "revision")? != expected_revision {
            return Err(Error::Conflict);
        }
        let mut p = tx.get(key)?.ok_or(Error::Missing)?;
        if number(&p, "class")? != Class::Detail as u64 {
            return Err(Error::Invalid);
        }
        let parent = string(&p, "parent")?;
        operations::strip_envelope(&mut p);
        p.insert("pinned".into(), Value::Bool(pin));
        tx.put(key, &parent, Class::Detail, p)?;
        tx.commit()?;
        Ok(())
    }

    /// A bounded, restartable pass. The worker persists/echoes the returned
    /// cursor; CAS makes concurrent workers and pin changes safe. This reports
    /// logical deletion only; engine physical reclamation is qualified separately.
    pub fn cleanup(&self, now: u64, after: Option<&str>) -> Result<Cleanup> {
        self.cleanup_class(now, after, Class::Detail)
    }
    pub fn cleanup_reviews(&self, now: u64, after: Option<&str>) -> Result<Cleanup> {
        self.cleanup_class(now, after, Class::Review)
    }
    fn cleanup_class(&self, now: u64, after: Option<&str>, class: Class) -> Result<Cleanup> {
        int(now)?;
        let prefix = if class == Class::Review { "r:" } else { "d:" };
        let resume = after
            .map(|cursor| decode_cursor(cursor, prefix))
            .transpose()?;
        const QUERY: &str = "MATCH (n:OmRow) WHERE n.class IN $classes RETURN n.key AS key";
        if !self.db.keyset_pageable(QUERY) {
            return Err(Error::SchemaVersion);
        }
        let mut tx = self.begin()?;
        // The native scan bounds inspected rows, including ineligible rows.
        if tx.root.get("restored") == Some(&Value::Bool(true)) {
            return Err(Error::RecoveryRequired);
        }
        // Re-read each selected key under the transaction and recheck pins;
        // any intervening application mutation invalidates the root CAS.
        let page = self.db.execute_cypher_paged(
            QUERY,
            params([("classes", Value::Array(vec![int(class as u64)?]))]),
            None,
            resume,
            usize::from(self.policy.batch_rows),
        )?;
        let mut removed = 0;
        let mut pinned = 0;
        let mut scopes: BTreeMap<String, (u64, u64)> = BTreeMap::new();
        for row in &page.rows {
            let key = row
                .get("key")
                .and_then(Value::as_str)
                .ok_or(Error::Corrupt)?;
            let Some(p) = tx.get(key)? else { continue };
            if number(&p, "class")? != class as u64 {
                continue;
            }
            if class == Class::Review {
                if number(&p, "expires")? > now {
                    continue;
                }
                let parent = p
                    .get("parent")
                    .and_then(Value::as_str)
                    .ok_or(Error::Corrupt)?;
                if parent.starts_with("candidate/")
                    && let Some(current) = tx.get(parent)?
                    && number(&current, "expires")? > now
                {
                    continue;
                }
                if p.contains_key("connection") {
                    let scope = scopes.entry(string(&p, "connection")?).or_default();
                    scope.0 += 1;
                    scope.1 = scope
                        .1
                        .checked_add(number(&p, "size")?)
                        .ok_or(Error::Limit)?;
                } else if parent != "workspace-previews" && !parent.starts_with("preview/") {
                    return Err(Error::Corrupt);
                }
                tx.delete(key)?;
                removed += 1;
                continue;
            }
            let at = number(&p, "at")?;
            if now < at || now - at < self.policy.detail_age_ms {
                continue;
            }
            if p.get("pinned") == Some(&Value::Bool(true)) {
                pinned += 1;
                continue;
            }
            let parent = string(&p, "parent")?;
            if let Some(owner) = parent.strip_prefix("detail/operation/")
                && let Some(operation) = tx.get(&format!("operation/{owner}"))?
                && !operations::terminal(operations::state(&operation)?)
            {
                pinned += 1;
                continue;
            }
            tx.delete(key)?;
            removed += 1;
        }
        for (scope, (rows, bytes)) in scopes {
            tx.review_removed(&scope, rows, bytes)?;
        }
        let cursor = if page.exhausted {
            None
        } else {
            page.last_key.map(|key| format!("{prefix}{}", hex(&key)))
        };
        tx.commit()?;
        Ok(Cleanup {
            removed,
            pinned,
            cursor,
        })
    }
}

fn decode_cursor(cursor: &str, prefix: &str) -> Result<Vec<u8>> {
    bounded(cursor)?;
    let encoded = cursor.strip_prefix(prefix).ok_or(Error::Invalid)?;
    if encoded.is_empty() || !encoded.len().is_multiple_of(2) {
        return Err(Error::Invalid);
    }
    let mut bytes = Vec::with_capacity(encoded.len() / 2);
    for chunk in encoded.as_bytes().as_chunks::<2>().0 {
        fn digit(b: u8) -> Result<u8> {
            match b {
                b'0'..=b'9' => Ok(b - b'0'),
                b'a'..=b'f' => Ok(b - b'a' + 10),
                _ => Err(Error::Invalid),
            }
        }
        bytes.push((digit(chunk[0])? << 4) | digit(chunk[1])?);
    }
    Ok(bytes)
}
