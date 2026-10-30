use super::*;

impl Tx<'_> {
    pub(super) fn require_no_pending_operations(&self, target: AssetRef) -> Result<()> {
        let pending=self.query("MATCH (n:OmRow) WHERE n.parent='operations' AND n.class=$class AND n.asset=$asset AND n.incarnation=$incarnation RETURN properties(n) AS p LIMIT 1",params([("class",int(Class::Pending as u64)?),("asset",text(target.asset.to_string())),("incarnation",text(target.incarnation.to_string()))]))?;
        if !pending.is_empty() {
            return Err(Error::UnsafeTransition);
        }
        Ok(())
    }
}

enum Change<'a> {
    Dispatch,
    Evidence {
        state: OperationState,
        outcome: EffectOutcome,
        provider_task: Option<&'a str>,
    },
}

fn digest(plan: &Plan<'_>, target: &Properties, owner_request: bool) -> Result<[u8; 32]> {
    bounded(plan.caller)?;
    bounded(plan.retry_key)?;
    // The host adds this namespace; the external action retains its full token budget.
    let action = if owner_request {
        plan.action.strip_prefix("owner.").ok_or(Error::Invalid)?
    } else {
        plan.action
    };
    if !token(action) || plan.arguments.len() > 64 {
        return Err(Error::Invalid);
    }
    let mut sorted = BTreeMap::new();
    for field in plan.arguments {
        if !token(&field.name) || sorted.insert(&field.name, field).is_some() {
            return Err(Error::Invalid);
        }
        match &field.value {
            Some(field::Value::Text(v)) if v.len() <= 4096 => {}
            Some(field::Value::Integer(_) | field::Value::Flag(_) | field::Value::Clear(0)) => {}
            _ => return Err(Error::Invalid),
        }
    }
    let mut hash = Sha256::new();
    hash.update(b"omloom/accepted-plan/v1");
    for value in [
        target
            .get("asset")
            .and_then(Value::as_str)
            .ok_or(Error::Corrupt)?,
        target
            .get("incarnation")
            .and_then(Value::as_str)
            .ok_or(Error::Corrupt)?,
        plan.caller,
        plan.action,
    ] {
        hash_part(&mut hash, value.as_bytes());
    }
    hash.update(plan.inventory_revision.to_be_bytes());
    hash.update(plan.expires_at_ms.to_be_bytes());
    for (_, field) in sorted {
        hash_field(&mut hash, field)?;
    }
    Ok(hash.finalize().into())
}
fn checkpoint(plan: &Plan<'_>) -> String {
    let mut hash = Sha256::new();
    hash.update(b"omloom/retry/v1");
    hash.update((plan.caller.len() as u64).to_be_bytes());
    hash.update(plan.caller);
    hash.update(plan.retry_key);
    format!("retry/{}", hex(&hash.finalize()))
}
pub(super) fn state(p: &Properties) -> Result<OperationState> {
    OperationState::try_from(number(p, "state")? as i32).map_err(|_| Error::Corrupt)
}
fn outcome(p: &Properties) -> Result<EffectOutcome> {
    EffectOutcome::try_from(number(p, "outcome")? as i32).map_err(|_| Error::Corrupt)
}
pub(super) fn terminal(s: OperationState) -> bool {
    matches!(
        s,
        OperationState::Succeeded | OperationState::Failed | OperationState::Cancelled
    )
}
fn lease_key(target: AssetRef) -> String {
    format!("lease/{}/{}", target.asset, target.incarnation)
}

fn audit_capacity(key: &str) -> Result<u64> {
    let row = charged_row(
        &format!("audit/{key}/{}", i64::MAX),
        &format!("detail/{key}"),
        Class::Detail,
        audit_data(
            &"x".repeat(256),
            "operation.reconciling",
            i64::MAX as u64,
            0,
        )?,
    )?;
    number(&row, "size")
}

fn reserve_settlement(tx: &mut Tx<'_>, key: &str, operation: &mut Properties) -> Result<()> {
    // The same row encoder sizes the largest supported terminal projection.
    // No placeholder payload is persisted; only the reservation's count/bytes.
    let mut terminal = operation.clone();
    terminal.insert("provider_task".into(), text("x".repeat(256)));
    let summary = charged_row(key, "operations", Class::Detail, terminal)?;
    let bytes = number(&summary, "size")?
        .checked_add(audit_capacity(key)?.checked_mul(2).ok_or(Error::Limit)?)
        .ok_or(Error::Limit)?;
    tx.settlement_reservation(
        crate::Budget { rows: 0, bytes: 0 },
        crate::Budget { rows: 3, bytes },
    )?;
    operation.insert("settlement_rows".into(), int(3)?);
    operation.insert("settlement_bytes".into(), int(bytes)?);
    Ok(())
}

fn resize_settlement(
    tx: &mut Tx<'_>,
    key: &str,
    operation: &mut Properties,
    rows: u64,
) -> Result<()> {
    let old = crate::Budget {
        rows: number(operation, "settlement_rows")?,
        bytes: number(operation, "settlement_bytes")?,
    };
    if !(2..=3).contains(&old.rows) || !matches!(rows, 0 | 2 | 3) {
        return Err(Error::Corrupt);
    }
    if old.rows == rows {
        return Ok(());
    }
    let bytes = if rows == 0 {
        0
    } else if rows > old.rows {
        old.bytes
            .checked_add(audit_capacity(key)?)
            .ok_or(Error::Limit)?
    } else {
        old.bytes
            .checked_sub(audit_capacity(key)?)
            .ok_or(Error::Corrupt)?
    };
    tx.settlement_reservation(old, crate::Budget { rows, bytes })?;
    if rows == 0 {
        operation.remove("settlement_rows");
        operation.remove("settlement_bytes");
    } else {
        operation.insert("settlement_rows".into(), int(rows)?);
        operation.insert("settlement_bytes".into(), int(bytes)?);
    }
    Ok(())
}

impl Store {
    /// Persist confirmed arguments, retry checkpoint, initial work and audit in
    /// one transaction. Reusing a key with a changed plan never creates work.
    pub fn accept(&self, plan: &Plan<'_>, now: u64) -> Result<OperationId> {
        let mut tx = self.begin()?;
        let id = tx.accept_plan(plan, now, None)?;
        tx.commit()?;
        Ok(id)
    }

    pub fn operation(&self, id: OperationId) -> Result<DurableOperation> {
        let tx = self.begin()?;
        let result = tx.operation(id)?;
        tx.commit()?;
        Ok(result)
    }
}

impl Tx<'_> {
    pub(super) fn operation(&self, id: OperationId) -> Result<DurableOperation> {
        let key = operation_key(id);
        let p = self.get(&key)?.ok_or(Error::Missing)?;
        let mut arguments = Vec::new();
        for row in self.children(&key)? {
            if row.contains_key("field") {
                arguments.push(inventory::read_field(&row)?);
            }
        }
        Ok(DurableOperation {
            id,
            target: target(&p)?,
            state: state(&p)?,
            outcome: outcome(&p)?,
            revision: number(&p, "revision")?,
            provider_task: p
                .get("provider_task")
                .and_then(Value::as_str)
                .map(str::to_owned),
            action: string(&p, "action")?,
            caller: string(&p, "caller")?,
            inventory_revision: number(&p, "inventory_revision")?,
            plan_digest: stored_digest(&p)?,
            arguments,
        })
    }

    // Account-bound requests share admission/retry/recovery with operations,
    // but have no work item and cannot be claimed by an executor.
    pub(super) fn accept_plan(
        &mut self,
        plan: &Plan<'_>,
        now: u64,
        account: Option<(AssetRef, u64)>,
    ) -> Result<OperationId> {
        int(now)?;
        int(plan.expires_at_ms)?;
        if now >= plan.expires_at_ms {
            return Err(Error::Expired);
        }
        let target_fields = target_props(plan.target);
        let hash = digest(plan, &target_fields, account.is_some())?;
        let checkpoint = checkpoint(plan);
        if self.root.get("restored") == Some(&Value::Bool(true)) {
            return Err(Error::RecoveryRequired);
        }
        if let Some(saved) = self.get(&checkpoint)? {
            if now >= number(&saved, "expires")? {
                return Err(Error::Expired);
            }
            if stored_digest(&saved)? != hash {
                return Err(Error::Conflict);
            }
            let id: OperationId = string(&saved, "operation")?
                .parse()
                .map_err(|_| Error::Corrupt)?;
            let operation = self.get(&operation_key(id))?.ok_or(Error::Missing)?;
            match account {
                Some((target, revision))
                    if operation.get("request_account")
                        == Some(&text(format!("{}/{}", target.asset, target.incarnation)))
                        && operation.get("account_revision") == Some(&int(revision)?) => {}
                None if !operation.contains_key("request_account") => {}
                _ => return Err(Error::Conflict),
            }
            return Ok(id);
        }
        let asset = self.asset(plan.target)?;
        if number(&asset, "revision")? != plan.inventory_revision
            || number(&asset, "adoption")? != wire::Adoption::Adopted as u64
        {
            return Err(Error::Conflict);
        }
        let id = OperationId::generate();
        let key = operation_key(id);
        let mut operation = target_fields;
        operation.extend(props([
            ("operation", text(id.to_string())),
            ("caller", text(plan.caller)),
            ("action", text(plan.action)),
            ("inventory_revision", int(plan.inventory_revision)?),
            ("digest", digest_value(hash)),
            (
                "state",
                int(if account.is_some() {
                    OperationState::Blocked
                } else {
                    OperationState::Queued
                } as u64)?,
            ),
            ("outcome", int(EffectOutcome::NotSent as u64)?),
            ("revision", int(1)?),
            ("at", int(now)?),
        ]));
        if let Some((account, revision)) = account {
            operation.insert(
                "request_account".into(),
                text(format!("{}/{}", account.asset, account.incarnation)),
            );
            operation.insert("account_revision".into(), int(revision)?);
            operation.insert("approved".into(), Value::Bool(false));
            operation.insert("owner_reported".into(), Value::Bool(false));
            operation.insert("expires".into(), int(plan.expires_at_ms)?);
        }
        reserve_settlement(self, &key, &mut operation)?;
        self.put(&key, "operations", Class::Pending, operation)?;
        for field in plan.arguments {
            let p = inventory::field_props(field)?;
            self.put(
                &format!("{key}/argument/{}", field.name),
                &key,
                Class::Pending,
                p,
            )?;
        }
        self.put(
            &checkpoint,
            "checkpoints",
            Class::Checkpoint,
            props([
                ("operation", text(id.to_string())),
                ("digest", digest_value(hash)),
                ("expires", int(plan.expires_at_ms)?),
            ]),
        )?;
        // This outbox is the local durable work consumer, not future webhooks.
        if account.is_none() {
            self.put(
                &format!("work/{id}"),
                "work",
                Class::Work,
                props([("operation", text(id.to_string()))]),
            )?;
        }
        self.audit(&key, plan.caller, "operation.accepted", 1, now)?;
        Ok(id)
    }
}

impl Store {
    /// Claiming unknown in-flight work is reconciliation only. It cannot reset
    /// effect knowledge or turn an expired lease into permission to resend.
    pub fn claim(&self, id: OperationId, worker: &str, now: u64, until: u64) -> Result<Lease> {
        bounded(worker)?;
        int(now)?;
        int(until)?;
        if until <= now {
            return Err(Error::Invalid);
        }
        let mut tx = self.begin()?;
        if tx.root.get("restored") == Some(&Value::Bool(true)) {
            return Err(Error::RecoveryRequired);
        }
        let key = operation_key(id);
        let mut operation = tx.get(&key)?.ok_or(Error::Missing)?;
        if terminal(state(&operation)?) || operation.contains_key("request_account") {
            return Err(Error::UnsafeTransition);
        }
        let target = target(&operation)?;
        let lease_key = lease_key(target);
        let prior = tx.get(&lease_key)?;
        if let Some(p) = &prior {
            if p.get("settled") != Some(&Value::Bool(true)) && number(p, "until")? > now {
                return Err(Error::Conflict);
            }
            let previous = string(p, "operation")?;
            if previous != id.to_string() {
                if let Some(in_flight) = tx.get(&format!("operation/{previous}"))? {
                    if outcome(&in_flight)? == EffectOutcome::Unknown {
                        return Err(Error::Conflict);
                    }
                } else {
                    // Missing detail is not proof that a previously admitted
                    // effect resolved. Only a settled lease checkpoint is.
                    if p.get("settled") != Some(&Value::Bool(true)) {
                        return Err(Error::Conflict);
                    }
                }
            }
        }
        let epoch = prior
            .as_ref()
            .map(|p| number(p, "epoch"))
            .transpose()?
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(Error::Limit)?;
        tx.put(
            &lease_key,
            "leases",
            Class::Checkpoint,
            props([
                ("operation", text(id.to_string())),
                ("worker", text(worker)),
                ("epoch", int(epoch)?),
                ("until", int(until)?),
                ("settled", Value::Bool(false)),
            ]),
        )?;
        // An unchanged reconciliation state needs no duplicate event. The lease
        // itself durably records ownership, even when optional audit space is full.
        if outcome(&operation)? == EffectOutcome::Unknown
            && state(&operation)? != OperationState::Reconciling
        {
            resize_settlement(&mut tx, &key, &mut operation, 2)?;
            operation.insert("state".into(), int(OperationState::Reconciling as u64)?);
            let revision = number(&operation, "revision")?
                .checked_add(1)
                .ok_or(Error::Limit)?;
            operation.insert("revision".into(), int(revision)?);
            strip_envelope(&mut operation);
            tx.put(&key, "operations", Class::Pending, operation)?;
            tx.audit(&key, worker, "operation.reconciling", revision, now)?;
        }
        tx.commit()?;
        Ok(Lease {
            operation: id,
            worker: worker.into(),
            epoch,
            until_ms: until,
        })
    }

    /// Commit the uncertainty marker before the executor can perform external I/O.
    /// The caller still owns current SID authorization and provider preconditions.
    pub fn prepare_dispatch(&self, lease: &Lease, expected: u64, now: u64) -> Result<u64> {
        self.transition(lease, expected, Change::Dispatch, now)
    }
    /// Record provider evidence under the current fence. This never sends I/O.
    pub fn reconcile(
        &self,
        lease: &Lease,
        expected: u64,
        next: OperationState,
        effect: EffectOutcome,
        provider_task: Option<&str>,
        now: u64,
    ) -> Result<u64> {
        self.transition(
            lease,
            expected,
            Change::Evidence {
                state: next,
                outcome: effect,
                provider_task,
            },
            now,
        )
    }
    fn transition(
        &self,
        lease: &Lease,
        expected: u64,
        change: Change<'_>,
        now: u64,
    ) -> Result<u64> {
        let (next, effect, provider_task, dispatch) = match change {
            Change::Dispatch => (OperationState::Running, EffectOutcome::Unknown, None, true),
            Change::Evidence {
                state,
                outcome,
                provider_task,
            } => (state, outcome, provider_task, false),
        };
        int(now)?;
        if let Some(task) = provider_task {
            bounded(task)?;
        }
        let mut tx = self.begin()?;
        if tx.root.get("restored") == Some(&Value::Bool(true)) {
            return Err(Error::RecoveryRequired);
        }
        let key = operation_key(lease.operation);
        let mut operation = tx.get(&key)?.ok_or(Error::Missing)?;
        let lease_key = lease_key(target(&operation)?);
        let persisted = tx.get(&lease_key)?.ok_or(Error::Conflict)?;
        if number(&persisted, "epoch")? != lease.epoch
            || persisted.get("operation") != Some(&text(lease.operation.to_string()))
            || persisted.get("worker") != Some(&text(&lease.worker))
            || number(&persisted, "until")? <= now
            || number(&operation, "revision")? != expected
        {
            return Err(Error::Conflict);
        }
        let old = state(&operation)?;
        let old_effect = outcome(&operation)?;
        if let Some(task) = provider_task
            && let Some(saved) = operation.get("provider_task")
            && saved.as_str().ok_or(Error::Corrupt)? != task
        {
            return Err(Error::Conflict);
        }
        if terminal(old) {
            return Err(Error::UnsafeTransition);
        }
        if dispatch {
            if old != OperationState::Queued || old_effect != EffectOutcome::NotSent {
                return Err(Error::UnsafeTransition);
            }
            let asset = tx.asset(target(&operation)?)?;
            if number(&asset, "revision")? != number(&operation, "inventory_revision")? {
                return Err(Error::Conflict);
            }
        } else {
            // Effect knowledge is monotonic and separate from execution success.
            let knowledge = match old_effect {
                EffectOutcome::Unknown => matches!(
                    effect,
                    EffectOutcome::Unknown
                        | EffectOutcome::Confirmed
                        | EffectOutcome::ConfirmedNoEffect
                ),
                EffectOutcome::NotSent
                | EffectOutcome::Confirmed
                | EffectOutcome::ConfirmedNoEffect => effect == old_effect,
                EffectOutcome::Unspecified => false,
            };
            let valid = knowledge
                && match next {
                    OperationState::Reconciling | OperationState::NeedsAttention => {
                        matches!(
                            effect,
                            EffectOutcome::Unknown
                                | EffectOutcome::Confirmed
                                | EffectOutcome::ConfirmedNoEffect
                        )
                    }
                    OperationState::Succeeded => effect == EffectOutcome::Confirmed,
                    OperationState::Failed => {
                        matches!(
                            effect,
                            EffectOutcome::Confirmed | EffectOutcome::ConfirmedNoEffect
                        ) || (effect == EffectOutcome::NotSent && old == OperationState::Queued)
                    }
                    OperationState::Cancelled => {
                        effect == EffectOutcome::ConfirmedNoEffect
                            || (effect == EffectOutcome::NotSent && old == OperationState::Queued)
                    }
                    _ => false,
                };
            if !valid
                || (effect == EffectOutcome::NotSent
                    && (provider_task.is_some() || operation.contains_key("provider_task")))
            {
                return Err(Error::UnsafeTransition);
            }
        }
        let revision = expected.checked_add(1).ok_or(Error::Limit)?;
        if terminal(next) {
            resize_settlement(&mut tx, &key, &mut operation, 0)?;
        } else if next == OperationState::Reconciling
            || (next == OperationState::NeedsAttention && effect != EffectOutcome::Unknown)
        {
            resize_settlement(&mut tx, &key, &mut operation, 2)?;
        } else if next == OperationState::NeedsAttention {
            resize_settlement(&mut tx, &key, &mut operation, 3)?;
        }
        operation.insert("state".into(), int(next as u64)?);
        operation.insert("outcome".into(), int(effect as u64)?);
        operation.insert("revision".into(), int(revision)?);
        operation.insert("at".into(), int(now)?);
        if let Some(task) = provider_task {
            operation.insert("provider_task".into(), text(task));
        }
        strip_envelope(&mut operation);
        tx.put(
            &key,
            "operations",
            if terminal(next) {
                Class::Detail
            } else {
                Class::Pending
            },
            operation,
        )?;
        tx.audit(&key, &lease.worker, "operation.transition", revision, now)?;
        if terminal(next) {
            let mut settled = persisted;
            settled.insert("settled".into(), Value::Bool(true));
            strip_envelope(&mut settled);
            tx.put(&lease_key, "leases", Class::Checkpoint, settled)?;
            tx.delete(&format!("work/{}", lease.operation))?;
            // Terminal arguments no longer serve recovery; keep the summary and
            // retry checkpoint, rather than retaining the whole accepted request.
            for p in tx.children(&key)? {
                if p.contains_key("field") {
                    tx.delete(&string(&p, "key")?)?;
                }
            }
        }
        tx.commit()?;
        Ok(revision)
    }

    /// One bounded durable-work page, ordered by operation ID. Resume after the
    /// last returned ID even if that operation remains unresolved or is removed.
    /// Restart at None after reaching an empty page to discover newly queued work.
    /// A returned ID is not a dispatch grant.
    pub fn pending(&self, after: Option<OperationId>) -> Result<Vec<OperationId>> {
        let tx = self.begin()?;
        let after = after.map_or_else(String::new, |id| format!("work/{id}"));
        let rows=tx.query("MATCH (n:OmRow) WHERE n.parent='work' AND n.key>$after RETURN properties(n) AS p, n.key AS ordering ORDER BY ordering LIMIT $limit",params([("after",text(after)),("limit",int(u64::from(self.policy.batch_rows))?)]))?;
        let result = rows
            .iter()
            .map(|p| string(p, "operation")?.parse().map_err(|_| Error::Corrupt))
            .collect::<Result<_>>()?;
        tx.commit()?;
        Ok(result)
    }
}

pub(super) fn strip_envelope(p: &mut Properties) {
    for key in ["key", "parent", "class", "size"] {
        p.remove(key);
    }
}
