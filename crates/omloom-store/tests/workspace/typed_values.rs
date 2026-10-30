use super::*;
use api::{LocatorKind as L, reply::Result as Reply, request::Command};
use omloom_store::Class;

fn request(store: &Store, actor: &Context, command: Command) -> Result<Reply, Error> {
    store
        .workspace(
            actor,
            api::Request {
                working: Some(api::WorkingContext {
                    expected: Some(store.usage()?.revision),
                    ..Default::default()
                }),
                command: Some(command),
            },
            100,
        )?
        .result
        .ok_or(Error::Corrupt)
}

// All writes and imports use the same DNS validator; labels stay independent of DNS syntax.
#[test]
fn domain_dns_validation_covers_write_bulk_and_csv() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let domain = create(&store, &actor, K::Domain, "Corporate domain label")?;
    let revision = store.asset(AssetRef::try_from(&domain)?)?.revision;
    for invalid in ["bad domain", "-bad.test", "192.0.2.1", "a..test"] {
        let before = store.usage()?;
        assert!(matches!(
            request(
                &store,
                &actor,
                Command::Write(api::Write {
                    target: Some(domain.clone()),
                    kind: K::Domain as i32,
                    expected: Some(revision),
                    changes: vec![text_field("dns_name", invalid)],
                    ..Default::default()
                })
            ),
            Err(Error::Invalid)
        ));
        assert!(matches!(
            request(
                &store,
                &actor,
                Command::Bulk(api::Bulk {
                    targets: vec![domain.clone()],
                    expected: before.revision,
                    changes: vec![text_field("dns_name", invalid)],
                    ..Default::default()
                })
            ),
            Err(Error::Invalid)
        ));
        assert_eq!(store.usage()?, before);
    }
    request(
        &store,
        &actor,
        Command::Write(api::Write {
            target: Some(domain.clone()),
            kind: K::Domain as i32,
            expected: Some(revision),
            changes: vec![text_field("dns_name", "münchen.de")],
            ..Default::default()
        }),
    )?;
    let Reply::Page(page) = request(
        &store,
        &actor,
        Command::Query(api::Query {
            search: "münchen.de".into(),
            columns: vec!["name".into(), "dns_name".into()],
            limit: 25,
            ..Default::default()
        }),
    )?
    else {
        return Err("page".into());
    };
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].target.as_ref(), Some(&domain));
    let before = store.usage()?;
    let Reply::Preview(preview) = request(
        &store,
        &actor,
        Command::Import(api::Import {
            source: "DNS fixture".into(),
            kind: K::Domain as i32,
            csv: "external_id,name,dns\n1,Separate label,bad domain\n".into(),
            mappings: vec![
                api::Mapping {
                    source: "name".into(),
                    field: "name".into(),
                },
                api::Mapping {
                    source: "dns".into(),
                    field: "dns_name".into(),
                },
            ],
            ..Default::default()
        }),
    )?
    else {
        return Err("preview".into());
    };
    assert!(preview.rows[0].change.is_none());
    assert!(!preview.rows[0].diagnostic.is_empty());
    assert_eq!(
        store.usage()?.rows[Class::Inventory as usize],
        before.rows[Class::Inventory as usize]
    );
    Ok(())
}

// Direct callers cannot bypass typed contacts, change their kind without authority,
// or discover a hidden contact's type. Reopen preserves the declared type and bytes.
#[test]
fn typed_contacts_share_value_permissions_and_survive_reopen() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let target = create(&store, &actor, K::Device, "Contact owner")?;
    let assignment = api::ResponsibilityAssignment {
        facet: api::ResponsibilityFacet::ClaimedOwner as i32,
        mode: api::AssignmentMode::Explicit as i32,
        party: Some(api::PartyRef {
            issuer: "opaque namespace".into(),
            subject: "team".into(),
            name: "Team".into(),
            kind: api::PartyKind::Team as i32,
        }),
        contact: Some("User+tag@example.test".into()),
        contact_kind: L::Email as i32,
        ..Default::default()
    };
    let apply = |actor: &Context, change| {
        request(
            &store,
            actor,
            Command::Responsibility(api::Responsibility {
                target: Some(target.clone()),
                expected: Some(
                    store
                        .asset(AssetRef::try_from(&target).map_err(|_| Error::Invalid)?)?
                        .revision,
                ),
                action: api::ResponsibilityAction::Apply as i32,
                changes: vec![change],
                ..Default::default()
            }),
        )
    };
    for (kind, value) in [
        (L::Email, "bad email"),
        (L::Phone, "+123"),
        (L::Domain, "bad domain"),
        (L::Web, "javascript:alert(1)"),
    ] {
        let before = store.usage()?;
        let mut change = assignment.clone();
        change.contact_kind = kind as i32;
        change.contact = Some(value.into());
        assert!(matches!(apply(&actor, change), Err(Error::Invalid)));
        assert_eq!(store.usage()?, before);
    }
    apply(&actor, assignment.clone())?;
    let before = store.usage()?;
    apply(&actor, assignment.clone())?;
    assert_eq!(store.usage()?, before);
    let mut limited = actor.clone();
    limited.grants[0].write = Fields::Selected(vec!["claimed_owner".into()]);
    let mut changed = assignment.clone();
    changed.contact_kind = L::Unspecified as i32;
    assert!(matches!(apply(&limited, changed), Err(Error::Denied)));
    assert_eq!(store.usage()?, before);
    limited.grants[0].read = Fields::Selected(vec!["name".into(), "claimed_owner".into()]);
    let Reply::Responsibility(hidden) = request(
        &store,
        &limited,
        Command::Responsibility(api::Responsibility {
            target: Some(target.clone()),
            action: api::ResponsibilityAction::Read as i32,
            ..Default::default()
        }),
    )?
    else {
        return Err("hidden state".into());
    };
    assert!(hidden.assignments[0].contact.is_none());
    assert_eq!(hidden.assignments[0].contact_kind, L::Unspecified as i32);
    assert!(
        hidden
            .effective
            .iter()
            .all(|v| v.contact.is_none() && v.contact_kind == L::Unspecified as i32)
    );
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    let Reply::Responsibility(saved) = request(
        &store,
        &actor,
        Command::Responsibility(api::Responsibility {
            target: Some(target),
            action: api::ResponsibilityAction::Read as i32,
            ..Default::default()
        }),
    )?
    else {
        return Err("saved state".into());
    };
    assert_eq!(saved.assignments[0].contact, assignment.contact);
    assert_eq!(saved.assignments[0].contact_kind, L::Email as i32);
    Ok(())
}

// Account IDs remain opaque; sender/recovery syntax and discriminator changes
// share their existing field permissions, including redaction and repeat ingestion.
#[test]
fn typed_account_values_preserve_provider_identity_and_field_authority() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let account = create(&store, &actor, K::ProviderAccount, "Registrar account")?;
    let identity = AssetRef::try_from(&account)?;
    let metadata = api::AccountMetadata {
        issuer: "opaque provider namespace".into(),
        subject: "personal ID / 7".into(),
        kind: api::ExternalAccountKind::Programmatic as i32,
        sender: Some("User+tag@example.test".into()),
        sender_kind: L::Email as i32,
        recovery_route: Some("+442079460018".into()),
        recovery_route_kind: L::Phone as i32,
        ..Default::default()
    };
    let save = |actor: &Context, value| {
        request(
            &store,
            actor,
            Command::ExternalAccount(api::ExternalAccount {
                target: Some(account.clone()),
                expected: Some(store.asset(identity)?.revision),
                metadata: Some(value),
                ..Default::default()
            }),
        )
    };
    for (kind, value) in [
        (L::Email, "not email"),
        (L::Domain, "bad domain"),
        (L::Phone, "+442079460018"),
    ] {
        let before = store.usage()?;
        let mut invalid = metadata.clone();
        invalid.sender = Some(value.into());
        invalid.sender_kind = kind as i32;
        assert!(matches!(save(&actor, invalid), Err(Error::Invalid)));
        assert_eq!(store.usage()?, before);
    }
    save(&actor, metadata.clone())?;
    let before = store.usage()?;
    save(&actor, metadata.clone())?;
    assert_eq!(store.usage()?, before);
    let mut limited = actor.clone();
    limited.grants[0].write = Fields::Selected(vec![
        "issuer".into(),
        "subject".into(),
        "account_kind".into(),
        "delegation".into(),
        "login".into(),
        "recovery_route".into(),
    ]);
    let mut changed = metadata.clone();
    changed.sender_kind = L::Unspecified as i32;
    assert!(matches!(save(&limited, changed), Err(Error::Denied)));
    assert_eq!(store.usage()?, before);
    limited.grants[0].read = Fields::Selected(vec![
        "name".into(),
        "issuer".into(),
        "subject".into(),
        "account_kind".into(),
        "delegation".into(),
        "recovery_route".into(),
    ]);
    let Reply::ExternalAccount(hidden) = request(
        &store,
        &limited,
        Command::ExternalAccount(api::ExternalAccount {
            target: Some(account),
            ..Default::default()
        }),
    )?
    else {
        return Err("account".into());
    };
    let saved = hidden.metadata.ok_or("metadata")?;
    assert!(saved.sender.is_none());
    assert_eq!(saved.sender_kind, L::Unspecified as i32);
    assert_eq!(saved.issuer, metadata.issuer);
    assert_eq!(saved.subject, metadata.subject);
    assert_eq!(saved.recovery_route, metadata.recovery_route);
    assert_eq!(saved.recovery_route_kind, L::Phone as i32);
    Ok(())
}
