use super::*;
use api::{reply::Result as Reply, request::Command};

fn account_call(
    store: &Store,
    actor: &Context,
    request: api::ExternalAccount,
) -> Result<api::ExternalAccountState, Error> {
    let result = store
        .workspace(
            actor,
            api::Request {
                working: Some(api::WorkingContext {
                    expected: Some(store.usage()?.revision),
                    ..Default::default()
                }),
                command: Some(Command::ExternalAccount(request)),
            },
            100,
        )?
        .result
        .ok_or(Error::Corrupt)?;
    let Reply::ExternalAccount(result) = result else {
        return Err(Error::Corrupt);
    };
    Ok(result)
}
fn metadata(kind: api::ExternalAccountKind) -> api::AccountMetadata {
    api::AccountMetadata {
        issuer: "registrar.example".into(),
        subject: "personal-7".into(),
        kind: kind as i32,
        delegation: api::AccountDelegation::OwnerOnly as i32,
        login: Some("founder".into()),
        ..Default::default()
    }
}

// A replacement cannot implicitly delete a redacted credential-provider reference.
#[test]
fn account_metadata_rejects_omitted_unreadable_values() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let account = create(&store, &operator(), K::ProviderAccount, "Selected account")?;
    let mut saved = metadata(api::ExternalAccountKind::Administration);
    saved.credential_binding_id = Some(account.asset_id.clone());
    account_call(
        &store,
        &operator(),
        api::ExternalAccount {
            target: Some(account.clone()),
            expected: Some(1),
            metadata: Some(saved.clone()),
            ..Default::default()
        },
    )?;
    let mut limited = operator();
    limited.grants[0].read = Fields::Selected(vec![
        "name".into(),
        "issuer".into(),
        "subject".into(),
        "account_kind".into(),
        "delegation".into(),
        "login".into(),
    ]);
    let before = store.usage()?;
    assert!(matches!(
        account_call(
            &store,
            &limited,
            api::ExternalAccount {
                target: Some(account.clone()),
                expected: Some(2),
                metadata: Some(metadata(api::ExternalAccountKind::Administration)),
                ..Default::default()
            }
        ),
        Err(Error::Denied)
    ));
    assert_eq!(store.usage()?, before);
    assert_eq!(
        account_call(
            &store,
            &operator(),
            api::ExternalAccount {
                target: Some(account.clone()),
                ..Default::default()
            }
        )?
        .metadata,
        Some(saved.clone())
    );
    // An explicit write-only replacement remains permitted; omission is the ambiguous case.
    saved.credential_binding_id =
        Some(create(&store, &operator(), K::Domain, "binding.test")?.asset_id);
    account_call(
        &store,
        &limited,
        api::ExternalAccount {
            target: Some(account),
            expected: Some(2),
            metadata: Some(saved),
            ..Default::default()
        },
    )?;
    Ok(())
}
fn setup(store: &Store) -> Result<(wire::AssetRef, wire::AssetRef), Error> {
    let actor = operator();
    let domain = create(store, &actor, K::Domain, "example.test")?;
    let account = create(store, &actor, K::ProviderAccount, "Personal registrar")?;
    account_call(
        store,
        &actor,
        api::ExternalAccount {
            target: Some(account.clone()),
            expected: Some(1),
            metadata: Some(metadata(api::ExternalAccountKind::Administration)),
            add: vec![api::AccountLink {
                kind: api::AccountLinkKind::ManagedResource as i32,
                target: Some(domain.clone()),
            }],
            ..Default::default()
        },
    )?;
    Ok((domain, account))
}
fn owner_call(
    store: &Store,
    actor: &Context,
    request: api::OwnerRequest,
) -> Result<api::OwnerRequestState, Error> {
    let reply = store
        .workspace(
            actor,
            api::Request {
                command: Some(Command::OwnerRequest(request)),
                working: Some(api::WorkingContext {
                    expected: Some(store.usage()?.revision),
                    ..Default::default()
                }),
            },
            100,
        )?
        .result
        .ok_or(Error::Corrupt)?;
    let Reply::OwnerRequest(state) = reply else {
        return Err(Error::Corrupt);
    };
    Ok(state)
}

// The durable owner-action namespace must not reduce the advertised 64-byte token range.
#[test]
fn owner_request_accepts_the_full_action_token_range() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let (domain, account) = setup(&store)?;
    for length in [58, 59, 64, 65] {
        let request = api::OwnerRequest {
            action: api::OwnerRequestAction::Create as i32,
            account: Some(account.clone()),
            account_revision: Some(2),
            target: Some(domain.clone()),
            target_revision: Some(1),
            change: "a".repeat(length),
            retry_key: format!("length-{length}"),
            expires_at_ms: 10_000,
            ..Default::default()
        };
        let before = store.usage()?;
        if length == 65 {
            assert!(matches!(
                owner_call(&store, &operator(), request),
                Err(Error::Invalid)
            ));
            assert_eq!(store.usage()?, before);
        } else {
            let saved = owner_call(&store, &operator(), request.clone())?;
            assert_eq!(saved.change, request.change);
            let replay = owner_call(&store, &operator(), request)?;
            assert_eq!(saved.operation, replay.operation);
        }
    }
    Ok(())
}

// Selected account identity and metadata persist once; hidden fields and capabilities remain independent.
#[test]
fn accounts_keep_selected_current_metadata_without_secret_or_grant_copies() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let (_, account) = setup(&store)?;
    let actor = operator();
    let state = account_call(
        &store,
        &actor,
        api::ExternalAccount {
            target: Some(account.clone()),
            ..Default::default()
        },
    )?;
    assert_eq!(state.links.len(), 1);
    assert!(state.metadata_fully_readable);
    let before = store.usage()?;
    account_call(
        &store,
        &actor,
        api::ExternalAccount {
            target: Some(account.clone()),
            expected: Some(2),
            metadata: Some(metadata(api::ExternalAccountKind::Administration)),
            ..Default::default()
        },
    )?;
    assert_eq!(store.usage()?, before);
    let mut auditor = actor;
    auditor.grants[0].writable = false;
    auditor.grants[0].read = Fields::Selected(vec!["name".into(), "account_kind".into()]);
    let state = account_call(
        &store,
        &auditor,
        api::ExternalAccount {
            target: Some(account.clone()),
            ..Default::default()
        },
    )?;
    let permitted = state.metadata.ok_or("metadata")?;
    assert!(permitted.issuer.is_empty());
    assert!(permitted.subject.is_empty());
    assert!(permitted.login.is_none());
    assert!(state.links.is_empty());
    assert!(state.authorized.is_empty());
    assert!(!state.metadata_fully_readable);
    assert!(matches!(
        account_call(
            &store,
            &auditor,
            api::ExternalAccount {
                target: Some(account.clone()),
                expected: Some(2),
                metadata: Some(metadata(api::ExternalAccountKind::Administration)),
                remove: vec![api::AccountLink {
                    kind: api::AccountLinkKind::ManagedResource as i32,
                    target: Some(create(&store, &operator(), K::Domain, "other.test")?)
                }],
                ..Default::default()
            }
        ),
        Err(Error::Denied)
    ));
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    assert_eq!(
        account_call(
            &store,
            &operator(),
            api::ExternalAccount {
                target: Some(account),
                ..Default::default()
            }
        )?
        .links
        .len(),
        1
    );
    Ok(())
}

// An account owner assertion never becomes verified success, queued work or executor authority.
#[test]
fn owner_requests_reuse_durable_operations_without_dispatch() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let (domain, account) = setup(&store)?;
    let actor = operator();
    let controller = api::ResponsibilityAssignment {
        facet: api::ResponsibilityFacet::AccountController as i32,
        mode: api::AssignmentMode::Explicit as i32,
        party: Some(api::PartyRef {
            issuer: "people.example".into(),
            subject: "founder".into(),
            kind: api::PartyKind::Individual as i32,
            name: "Founder".into(),
        }),
        ..Default::default()
    };
    store.workspace(
        &actor,
        api::Request {
            working: Some(api::WorkingContext {
                expected: Some(store.usage()?.revision),
                ..Default::default()
            }),
            command: Some(Command::Responsibility(api::Responsibility {
                target: Some(account.clone()),
                expected: Some(2),
                changes: vec![controller],
                action: api::ResponsibilityAction::Apply as i32,
                ..Default::default()
            })),
        },
        100,
    )?;
    let account_revision = store.asset(AssetRef::try_from(&account)?)?.revision;
    let request = api::OwnerRequest {
        action: api::OwnerRequestAction::Create as i32,
        account: Some(account.clone()),
        target: Some(domain),
        account_revision: Some(account_revision),
        target_revision: Some(1),
        change: "set-description".into(),
        arguments: vec![text_field("description", "Company domain")],
        retry_key: "requested-change".into(),
        expires_at_ms: 10_000,
        ..Default::default()
    };
    let created = owner_call(&store, &actor, request.clone())?;
    let operation = created.operation.ok_or("operation")?;
    assert_eq!(operation.state, wire::OperationState::Blocked as i32);
    assert_eq!(operation.outcome, wire::EffectOutcome::NotSent as i32);
    let before = store.usage()?;
    let replay = owner_call(&store, &actor, request)?;
    assert_eq!(
        replay.operation.as_ref().ok_or("operation")?.operation_id,
        operation.operation_id
    );
    assert_eq!(store.usage()?, before);
    assert!(matches!(
        store.claim(operation.operation_id.parse()?, "executor", 100, 200),
        Err(Error::UnsafeTransition)
    ));
    let transition = api::OwnerRequest {
        action: api::OwnerRequestAction::Approve as i32,
        account: Some(account.clone()),
        account_revision: Some(account_revision),
        operation_id: Some(operation.operation_id.clone()),
        expected: Some(1),
        ..Default::default()
    };
    let mut auditor = actor.clone();
    auditor.grants[0].writable = false;
    assert!(matches!(
        owner_call(&store, &auditor, transition.clone()),
        Err(Error::Denied)
    ));
    let mut approver = actor.clone();
    approver.actor = "company-approver".into();
    approver.grants[0].write = Fields::Selected(vec!["account.approve".into()]);
    let approved = owner_call(&store, &approver, transition)?;
    assert!(approved.approved);
    let report = api::OwnerRequest {
        action: api::OwnerRequestAction::Report as i32,
        account: Some(account.clone()),
        account_revision: Some(account_revision),
        operation_id: Some(operation.operation_id),
        expected: Some(2),
        reported_outcome: wire::EffectOutcome::Confirmed as i32,
        ..Default::default()
    };
    assert!(matches!(
        owner_call(&store, &approver, report.clone()),
        Err(Error::Denied)
    ));
    assert_eq!(
        store.asset(AssetRef::try_from(&account)?)?.revision,
        account_revision
    );
    let reported = owner_call(&store, &actor, report)?;
    assert!(reported.owner_reported);
    assert_eq!(
        reported.operation.as_ref().ok_or("operation")?.state,
        wire::OperationState::NeedsAttention as i32
    );
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    let read = owner_call(
        &store,
        &actor,
        api::OwnerRequest {
            action: api::OwnerRequestAction::Read as i32,
            account: Some(account),
            operation_id: Some(reported.operation.ok_or("operation")?.operation_id),
            ..Default::default()
        },
    )?;
    assert!(read.approved && read.owner_reported);
    assert_eq!(
        read.arguments,
        vec![text_field("description", "Company domain")]
    );
    Ok(())
}

// Owner assertions refine uncertainty but cannot contradict a previously definite effect.
#[test]
fn owner_reports_preserve_definite_effect_knowledge() -> TestResult {
    for definite in [
        wire::EffectOutcome::Confirmed,
        wire::EffectOutcome::ConfirmedNoEffect,
    ] {
        let dir = tempfile::tempdir()?;
        let store = Store::open(dir.path(), policy())?;
        let (domain, account) = setup(&store)?;
        let actor = operator();
        let created = owner_call(
            &store,
            &actor,
            api::OwnerRequest {
                action: api::OwnerRequestAction::Create as i32,
                account: Some(account.clone()),
                account_revision: Some(2),
                target: Some(domain),
                target_revision: Some(1),
                change: "set-description".into(),
                arguments: vec![text_field("description", "Requested description")],
                retry_key: "monotonic-report".into(),
                expires_at_ms: 10_000,
                ..Default::default()
            },
        )?
        .operation
        .ok_or("operation")?;
        let mut transition = api::OwnerRequest {
            action: api::OwnerRequestAction::Approve as i32,
            account: Some(account),
            account_revision: Some(2),
            operation_id: Some(created.operation_id.clone()),
            expected: Some(created.revision),
            ..Default::default()
        };
        let approved = owner_call(&store, &actor, transition.clone())?
            .operation
            .ok_or("operation")?;
        transition.action = api::OwnerRequestAction::Report as i32;
        transition.expected = Some(approved.revision);
        transition.reported_outcome = wire::EffectOutcome::Unknown as i32;
        let unknown = owner_call(&store, &actor, transition.clone())?
            .operation
            .ok_or("operation")?;
        transition.expected = Some(unknown.revision);
        transition.reported_outcome = definite as i32;
        let known = owner_call(&store, &actor, transition.clone())?
            .operation
            .ok_or("operation")?;
        transition.expected = Some(known.revision);
        let before = store.usage()?;
        owner_call(&store, &actor, transition.clone())?;
        assert_eq!(store.usage()?, before);
        for other in [
            wire::EffectOutcome::Unknown,
            wire::EffectOutcome::Confirmed,
            wire::EffectOutcome::ConfirmedNoEffect,
        ] {
            if other == definite {
                continue;
            }
            transition.reported_outcome = other as i32;
            assert!(matches!(
                owner_call(&store, &actor, transition.clone()),
                Err(Error::UnsafeTransition)
            ));
            assert_eq!(store.usage()?, before);
        }
        drop(store);
        let store = Store::open(dir.path(), policy())?;
        assert_eq!(
            store.operation(created.operation_id.parse()?)?.outcome,
            definite
        );
    }
    Ok(())
}

// A controller edit invalidates the previously accepted account basis, even with a fresh client revision.
#[test]
fn owner_request_cannot_cross_controller_revision() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let (domain, account) = setup(&store)?;
    let actor = operator();
    let current = store.asset(AssetRef::try_from(&account)?)?.revision;
    let state = owner_call(
        &store,
        &actor,
        api::OwnerRequest {
            action: api::OwnerRequestAction::Create as i32,
            account: Some(account.clone()),
            target: Some(domain.clone()),
            account_revision: Some(current),
            target_revision: Some(store.asset(AssetRef::try_from(&domain)?)?.revision),
            change: "set-description".into(),
            arguments: vec![text_field("description", "Requested change")],
            retry_key: "controller-change".into(),
            expires_at_ms: 10_000,
            ..Default::default()
        },
    )?;
    store.workspace(
        &actor,
        api::Request {
            working: Some(api::WorkingContext {
                expected: Some(store.usage()?.revision),
                ..Default::default()
            }),
            command: Some(Command::Responsibility(api::Responsibility {
                target: Some(account.clone()),
                expected: Some(current),
                action: api::ResponsibilityAction::Apply as i32,
                changes: vec![api::ResponsibilityAssignment {
                    facet: api::ResponsibilityFacet::AccountController as i32,
                    mode: api::AssignmentMode::Explicit as i32,
                    party: Some(api::PartyRef {
                        issuer: "people.example".into(),
                        subject: "successor".into(),
                        kind: api::PartyKind::Individual as i32,
                        name: "Successor".into(),
                    }),
                    ..Default::default()
                }],
                ..Default::default()
            })),
        },
        100,
    )?;
    let operation = state.operation.ok_or("operation")?;
    let before = store.usage()?;
    assert!(matches!(
        owner_call(
            &store,
            &actor,
            api::OwnerRequest {
                action: api::OwnerRequestAction::Approve as i32,
                account: Some(account.clone()),
                account_revision: Some(store.asset(AssetRef::try_from(&account)?)?.revision),
                operation_id: Some(operation.operation_id),
                expected: Some(operation.revision),
                ..Default::default()
            }
        ),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    Ok(())
}

// Removing/retyping a referenced target or an account with pending owner work must not strand it.
#[test]
fn account_references_protect_resource_lifecycles() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let (domain, account) = setup(&store)?;
    let actor = operator();
    let before = store.usage()?;
    assert!(matches!(
        call(
            &store,
            &actor,
            Command::Bulk(api::Bulk {
                targets: vec![domain.clone()],
                expected: before.revision,
                remove: true,
                ..Default::default()
            })
        ),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    owner_call(
        &store,
        &actor,
        api::OwnerRequest {
            action: api::OwnerRequestAction::Create as i32,
            account: Some(account.clone()),
            target: Some(domain),
            account_revision: Some(2),
            target_revision: Some(1),
            change: "set-description".into(),
            arguments: vec![text_field("description", "Company domain")],
            retry_key: "pending-owner".into(),
            expires_at_ms: 10_000,
            ..Default::default()
        },
    )?;
    let before = store.usage()?;
    assert!(matches!(
        call(
            &store,
            &actor,
            Command::Bulk(api::Bulk {
                targets: vec![account],
                expected: before.revision,
                remove: true,
                ..Default::default()
            })
        ),
        Err(Error::UnsafeTransition)
    ));
    assert_eq!(store.usage()?, before);
    Ok(())
}

// Retyping cannot strand outgoing account-owned bindings behind a non-account kind.
#[test]
fn account_owned_bindings_block_retyping_without_requests() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let (domain, account) = setup(&store)?;
    let target = AssetRef::try_from(&account)?;
    let old = store.asset(target)?;
    let mut replacement = asset();
    replacement.target = Some(account.clone());
    replacement.kind = K::Device as i32;
    replacement.revision = old.revision;
    let before = store.usage()?;
    assert!(matches!(
        store.save_asset(
            &admit(wire::record::Body::Asset(replacement)),
            Some(old.revision),
            "editor",
            100
        ),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, before);
    let state = account_call(
        &store,
        &operator(),
        api::ExternalAccount {
            target: Some(account),
            ..Default::default()
        },
    )?;
    assert_eq!(state.links.len(), 1);
    assert_eq!(state.links[0].target.as_ref(), Some(&domain));
    Ok(())
}

// Account bindings are peer relationships and require both endpoints in the working selection.
#[test]
fn account_binding_requires_explicit_endpoint_context() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let account = create(&store, &actor, K::ProviderAccount, "Selected account")?;
    let domain = create(&store, &actor, K::Domain, "Outside selected account")?;
    let link = api::AccountLink {
        kind: api::AccountLinkKind::ManagedResource as i32,
        target: Some(domain.clone()),
    };
    let edit = |remove: bool, focused: bool| -> Result<api::Reply, Error> {
        store.workspace(
            &actor,
            api::Request {
                working: Some(api::WorkingContext {
                    expected: Some(store.usage()?.revision),
                    focus: focused.then(|| account.clone()),
                    roots: if focused {
                        vec![]
                    } else {
                        vec![account.clone(), domain.clone()]
                    },
                }),
                command: Some(Command::ExternalAccount(api::ExternalAccount {
                    target: Some(account.clone()),
                    expected: Some(
                        store
                            .asset(AssetRef::try_from(&account).map_err(|_| Error::Invalid)?)?
                            .revision,
                    ),
                    add: if remove { vec![] } else { vec![link.clone()] },
                    remove: if remove { vec![link.clone()] } else { vec![] },
                    ..Default::default()
                })),
            },
            100,
        )
    };
    let before = store.usage()?;
    assert!(matches!(edit(false, true), Err(Error::Denied)));
    assert_eq!(store.usage()?, before);
    edit(false, false)?;
    let before = store.usage()?;
    assert!(matches!(edit(true, true), Err(Error::Denied)));
    assert_eq!(store.usage()?, before);
    edit(true, false)?;
    assert!(
        account_call(
            &store,
            &actor,
            api::ExternalAccount {
                target: Some(account),
                ..Default::default()
            }
        )?
        .links
        .is_empty()
    );
    Ok(())
}

// Two teams share exactly two identities; separate SMTP accounts expose neither mail administration nor each other.
#[test]
fn overlapping_departments_and_smtp_consumers_keep_independent_authority() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let private_a = create(&store, &actor, K::Device, "Developer A private")?;
    let private_b = create(&store, &actor, K::Device, "Developer B private")?;
    let shared_one = create(&store, &actor, K::Device, "Shared one")?;
    let shared_two = create(&store, &actor, K::Device, "Shared two")?;
    let mail = create(&store, &actor, K::Service, "Mail service")?;
    let endpoint = create(&store, &actor, K::Interface, "SMTP endpoint")?;
    let product_a = create(&store, &actor, K::Service, "Product A")?;
    let product_b = create(&store, &actor, K::Service, "Product B")?;
    let account_a = create(&store, &actor, K::ProviderAccount, "SMTP product A")?;
    let account_b = create(&store, &actor, K::ProviderAccount, "SMTP product B")?;
    for (account, product, subject) in [
        (&account_a, &product_a, "product-a"),
        (&account_b, &product_b, "product-b"),
    ] {
        account_call(
            &store,
            &actor,
            api::ExternalAccount {
                target: Some(account.clone()),
                expected: Some(1),
                metadata: Some(api::AccountMetadata {
                    issuer: "mail.example".into(),
                    subject: subject.into(),
                    kind: api::ExternalAccountKind::Programmatic as i32,
                    delegation: api::AccountDelegation::Scoped as i32,
                    login: Some(subject.into()),
                    sender: Some(format!("{subject}@example.test")),
                    ..Default::default()
                }),
                add: vec![
                    api::AccountLink {
                        kind: api::AccountLinkKind::Service as i32,
                        target: Some(mail.clone()),
                    },
                    api::AccountLink {
                        kind: api::AccountLinkKind::Endpoint as i32,
                        target: Some(endpoint.clone()),
                    },
                    api::AccountLink {
                        kind: api::AccountLinkKind::Consumer as i32,
                        target: Some(product.clone()),
                    },
                ],
                ..Default::default()
            },
        )?;
    }
    let team = |targets: Vec<wire::AssetRef>| -> Result<Context, Box<dyn std::error::Error>> {
        let mut context = operator();
        context.grants[0].scope = Scope::Targets(
            targets
                .iter()
                .map(AssetRef::try_from)
                .collect::<Result<Vec<_>, _>>()?,
        );
        context.grants[0].write = Fields::Selected(vec!["account.use".into()]);
        Ok(context)
    };
    let mut a = team(vec![
        private_a.clone(),
        shared_one.clone(),
        shared_two.clone(),
        product_a,
        account_a.clone(),
    ])?;
    let b = team(vec![
        private_b,
        shared_one.clone(),
        shared_two.clone(),
        product_b,
        account_b.clone(),
    ])?;
    let devices = |context: &Context| -> Result<Vec<wire::AssetRef>, Error> {
        let Reply::Page(page) = call(
            &store,
            context,
            Command::Query(api::Query {
                kinds: vec![K::Device as i32],
                ..query()
            }),
        )?
        else {
            return Err(Error::Corrupt);
        };
        assert_eq!(page.total, page.items.len() as u64);
        page.items
            .into_iter()
            .map(|item| item.target.ok_or(Error::Corrupt))
            .collect::<Result<Vec<_>, _>>()
    };
    let devices_a = devices(&a)?;
    let devices_b = devices(&b)?;
    assert_eq!(devices_a.len(), 3);
    assert_eq!(devices_b.len(), 3);
    let overlap: Vec<_> = devices_a
        .iter()
        .filter(|target| devices_b.contains(target))
        .cloned()
        .collect();
    assert_eq!(overlap.len(), 2);
    assert!(overlap.contains(&shared_one) && overlap.contains(&shared_two));
    let state = account_call(
        &store,
        &a,
        api::ExternalAccount {
            target: Some(account_a.clone()),
            ..Default::default()
        },
    )?;
    assert_eq!(
        state.metadata.ok_or("metadata")?.kind,
        api::ExternalAccountKind::Programmatic as i32
    );
    assert_eq!(state.links.len(), 1);
    assert_eq!(state.links[0].kind, api::AccountLinkKind::Consumer as i32);
    assert!(
        state
            .authorized
            .contains(&(api::AccountCapability::WorkloadUse as i32))
    );
    assert!(
        !state
            .authorized
            .contains(&(api::AccountCapability::RevealSecret as i32))
    );
    assert!(
        !state
            .authorized
            .contains(&(api::AccountCapability::ShareReference as i32))
    );
    assert!(matches!(
        account_call(
            &store,
            &a,
            api::ExternalAccount {
                target: Some(account_b),
                ..Default::default()
            }
        ),
        Err(Error::Denied)
    ));
    assert!(matches!(
        account_call(
            &store,
            &a,
            api::ExternalAccount {
                target: Some(account_a),
                expected: Some(2),
                metadata: Some(metadata(api::ExternalAccountKind::Administration)),
                ..Default::default()
            }
        ),
        Err(Error::Denied)
    ));
    let mut auditor = actor;
    auditor.grants[0].writable = false;
    assert_eq!(devices(&auditor)?.len(), 4);
    let audited = account_call(
        &store,
        &auditor,
        api::ExternalAccount {
            target: Some(create(
                &store,
                &operator(),
                K::ProviderAccount,
                "Audit card",
            )?),
            ..Default::default()
        },
    )?;
    assert!(audited.authorized.is_empty());
    a.grants.push(Grant {
        scope: Scope::Targets(vec![AssetRef::try_from(&shared_one)?]),
        read: Fields::Selected(vec!["name".into()]),
        write: Fields::Selected(Vec::new()),
        writable: false,
    });
    a.grants[0].scope = Scope::Targets(vec![AssetRef::try_from(&private_a)?]);
    a.authority_revision += 1;
    assert_eq!(devices(&a)?.len(), 2);
    Ok(())
}

// Reading/using a selected account cannot publish its identity through a shared view definition.
#[test]
fn account_card_sharing_requires_its_own_capability() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let (_, account) = setup(&store)?;
    let mut actor = operator();
    actor.grants[0].write = Fields::Selected(vec!["account.use".into()]);
    let publish = api::SaveView {
        view: Some(api::SavedView {
            name: "Selected account".into(),
            query: Some(api::Query {
                focus: Some(account),
                ..query()
            }),
            audiences: vec!["operations".into()],
            ..Default::default()
        }),
        publish: true,
        ..Default::default()
    };
    let before = store.usage()?;
    assert!(matches!(
        call(&store, &actor, Command::SaveView(publish.clone())),
        Err(Error::Denied)
    ));
    assert_eq!(store.usage()?, before);
    actor.grants[0].write = Fields::Selected(vec!["account.share".into()]);
    let Reply::Views(published) = call(&store, &actor, Command::SaveView(publish))? else {
        panic!("view");
    };
    assert_eq!(published.items[0].audiences, vec!["operations"]);
    Ok(())
}

// Denying binding disclosure must not bypass the account's hard selected-binding admission limit.
#[test]
fn hidden_binding_projection_cannot_bypass_account_limits() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let (_, account) = setup(&store)?;
    let actor = operator();
    let mut links = Vec::new();
    for index in 0..32 {
        links.push(api::AccountLink {
            kind: api::AccountLinkKind::ManagedResource as i32,
            target: Some(create(
                &store,
                &actor,
                K::Domain,
                &format!("domain-{index}.test"),
            )?),
        });
    }
    let extra = links.pop().ok_or("extra")?;
    let mut restricted = actor;
    restricted.grants[0].read = Fields::Selected(vec!["name".into()]);
    restricted.grants[0].write = Fields::Selected(vec!["account.bindings".into()]);
    account_call(
        &store,
        &restricted,
        api::ExternalAccount {
            target: Some(account.clone()),
            expected: Some(2),
            add: links,
            ..Default::default()
        },
    )?;
    let before = store.usage()?;
    assert!(matches!(
        account_call(
            &store,
            &restricted,
            api::ExternalAccount {
                target: Some(account),
                expected: Some(3),
                add: vec![extra],
                ..Default::default()
            }
        ),
        Err(Error::Limit)
    ));
    assert_eq!(store.usage()?, before);
    Ok(())
}
