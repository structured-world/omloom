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
    /// Logical export to a caller-owned bounded destination. No credential or
    /// SID state is part of this store. The destination owns staging/reclamation.
    pub fn export<W: std::io::Write>(&self, writer: &mut W) -> Result<()> {
        let snapshot = self.db.engine().snapshot();
        export::export_binary(self.db.engine(), &self.db.interner()?, 1, &snapshot, writer)
            .map_err(|e| Error::Export(Box::new(e)))?;
        Ok(())
    }
    /// Import into a fresh database with dispatch durably disabled. A logical
    /// backup cannot prove current authority or completed external effects.
    pub fn restore(path: &Path, image: &[u8], policy: Policy) -> Result<Self> {
        if !policy.valid() || image.len() as u64 > policy.total_bytes {
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
        int(now)?;
        if let Some(cursor) = after {
            bounded(cursor)?;
        }
        let tx = self.begin()?;
        let rows=tx.query("MATCH (n:OmRow) WHERE n.class=$class AND n.key>$after RETURN properties(n) AS p, n.key AS ordering ORDER BY ordering LIMIT $limit",params([("class",int(Class::Detail as u64)?),("after",text(after.unwrap_or(""))),("limit",int(u64::from(self.policy.batch_rows))?)]))?;
        let mut tx = tx;
        let mut removed = 0;
        let mut pinned = 0;
        for p in &rows {
            let at = number(p, "at")?;
            if now < at || now - at < self.policy.detail_age_ms {
                continue;
            }
            if p.get("pinned") == Some(&Value::Bool(true)) {
                pinned += 1;
                continue;
            }
            let parent = string(p, "parent")?;
            if let Some(owner) = parent.strip_prefix("detail/operation/")
                && let Some(operation) = tx.get(&format!("operation/{owner}"))?
                && !operations::terminal(operations::state(&operation)?)
            {
                pinned += 1;
                continue;
            }
            tx.delete(&string(p, "key")?)?;
            removed += 1;
        }
        let cursor = if rows.len() == usize::from(self.policy.batch_rows) {
            rows.last().map(|p| string(p, "key")).transpose()?
        } else {
            None
        };
        tx.commit()?;
        Ok(Cleanup {
            removed,
            pinned,
            cursor,
        })
    }
}
