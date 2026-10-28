use super::*;
use api::{
    AssignmentMode as Mode, ResponsibilityAction as Action, ResponsibilityFacet as Facet,
    ResponsibilityStatus as Status, reply::Result as Reply, request::Command,
};

fn responsibility(
    store: &Store,
    actor: &Context,
    target: &wire::AssetRef,
    action: Action,
    changes: Vec<api::ResponsibilityAssignment>,
) -> Result<api::ResponsibilityState, Error> {
    let expected = if action == Action::Read {
        None
    } else {
        Some(
            store
                .asset(AssetRef::try_from(target).map_err(|_| Error::Invalid)?)?
                .revision,
        )
    };
    let Reply::Responsibility(state) = store
        .workspace(
            actor,
            api::Request {
                command: Some(Command::Responsibility(api::Responsibility {
                    target: Some(target.clone()),
                    expected,
                    changes,
                    action: action as i32,
                    ..Default::default()
                })),
                working: Some(api::WorkingContext {
                    expected: Some(store.usage()?.revision),
                    ..Default::default()
                }),
            },
            100,
        )?
        .result
        .ok_or(Error::Corrupt)?
    else {
        return Err(Error::Corrupt);
    };
    Ok(state)
}
fn owner(subject: &str) -> api::ResponsibilityAssignment {
    api::ResponsibilityAssignment {
        facet: Facet::ClaimedOwner as i32,
        mode: Mode::Explicit as i32,
        party: Some(api::PartyRef {
            issuer: "company.example".into(),
            subject: subject.into(),
            kind: api::PartyKind::Team as i32,
            name: subject.into(),
        }),
        ..Default::default()
    }
}

// A redacted contact must not become an implicit clear in a replacement declaration.
#[test]
fn responsibility_replacement_cannot_clear_an_unreadable_contact() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let target = create(&store, &actor, K::Device, "Selected server")?;
    let mut assignment = owner("devops");
    assignment.contact = Some("mailto:private@example.test".into());
    responsibility(&store, &actor, &target, Action::Apply, vec![assignment])?;
    let mut limited = operator();
    limited.grants[0].read = Fields::Selected(vec!["name".into(), "claimed_owner".into()]);
    let before = store.usage()?;
    assert!(matches!(
        responsibility(
            &store,
            &limited,
            &target,
            Action::Apply,
            vec![owner("updated")]
        ),
        Err(Error::Denied)
    ));
    assert_eq!(store.usage()?, before);
    let state = responsibility(&store, &actor, &target, Action::Read, vec![])?;
    assert_eq!(
        state.assignments[0].contact.as_deref(),
        Some("mailto:private@example.test")
    );
    Ok(())
}

// A source picker needs ancestors, never the full set of a server's components.
#[test]
fn responsibility_source_picker_does_not_enumerate_children() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let server = create(&store, &actor, K::Device, "Server with many components")?;
    for index in 0..129 {
        call(
            &store,
            &actor,
            Command::Write(api::Write {
                kind: K::Interface as i32,
                parent: Some(server.clone()),
                changes: vec![text_field("name", &format!("NIC {index}"))],
                ..Default::default()
            }),
        )?;
    }
    let before = store.usage()?;
    let Reply::Responsibility(state) = call(
        &store,
        &actor,
        Command::Responsibility(api::Responsibility {
            target: Some(server.clone()),
            action: Action::Read as i32,
            include_sources: true,
            ..Default::default()
        }),
    )?
    else {
        panic!("responsibility");
    };
    assert!(state.sources.is_empty());
    assert_eq!(store.usage()?, before);
    // Applying the default source must use the same bounded parent lookup as the picker.
    let parent = create(&store, &actor, K::Device, "Owning chassis")?;
    responsibility(
        &store,
        &actor,
        &parent,
        Action::Apply,
        vec![owner("hardware")],
    )?;
    connect(&store, &parent, &server, wire::RelationKind::Contains)?;
    let state = responsibility(
        &store,
        &actor,
        &server,
        Action::Apply,
        vec![api::ResponsibilityAssignment {
            facet: Facet::ClaimedOwner as i32,
            mode: Mode::Inherit as i32,
            ..Default::default()
        }],
    )?;
    assert_eq!(
        state.effective[0].party.as_ref().ok_or("party")?.subject,
        "hardware"
    );
    Ok(())
}

// Ambiguous attachment and cycles must be refused atomically; adjacency cannot become authority.
#[test]
fn responsibility_rejects_ambiguous_cycles_and_sideways_sources() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let server = create(&store, &actor, K::Device, "Server")?;
    let Reply::Mutation(port) = call(
        &store,
        &actor,
        Command::Write(api::Write {
            kind: K::Port as i32,
            parent: Some(server.clone()),
            changes: vec![text_field("name", "Port")],
            ..Default::default()
        }),
    )?
    else {
        panic!("port");
    };
    let port = port.targets[0].clone();
    let nic = create(&store, &actor, K::Interface, "NIC")?;
    connect(&store, &server, &nic, wire::RelationKind::Contains)
        .map_err(|e| format!("server NIC: {e:?}"))?;
    connect(&store, &port, &nic, wire::RelationKind::Carries)
        .map_err(|e| format!("port NIC: {e:?}"))?;
    let inherited = api::ResponsibilityAssignment {
        facet: Facet::ClaimedOwner as i32,
        mode: Mode::Inherit as i32,
        ..Default::default()
    };
    let before = store.usage()?;
    assert!(matches!(
        responsibility(&store, &actor, &nic, Action::Apply, vec![inherited.clone()]),
        Err(Error::Incomplete)
    ));
    assert_eq!(store.usage()?, before);
    let software = create(&store, &actor, K::SoftwareInstallation, "Runtime")?;
    let service = create(&store, &actor, K::Service, "Service")?;
    connect(&store, &software, &service, wire::RelationKind::Contains)
        .map_err(|e| format!("runtime service: {e:?}"))?;
    let before = store.usage()?;
    assert!(matches!(
        connect(&store, &service, &software, wire::RelationKind::Contains),
        Err(Error::Invalid)
    ));
    assert_eq!(store.usage()?, before);
    let peer = create(&store, &actor, K::Device, "Gateway dependency")?;
    let peer_link = connect(&store, &service, &peer, wire::RelationKind::DependsOn)
        .map_err(|e| format!("peer link: {e:?}"))?;
    let before = store.usage()?;
    assert!(matches!(
        responsibility(
            &store,
            &actor,
            &peer,
            Action::Apply,
            vec![api::ResponsibilityAssignment {
                source_relation: Some(peer_link.relation_id),
                source_revision: Some(1),
                ..inherited
            }]
        ),
        Err(Error::Invalid)
    ));
    assert_eq!(store.usage()?, before);
    Ok(())
}
fn connect(
    store: &Store,
    from: &wire::AssetRef,
    to: &wire::AssetRef,
    kind: wire::RelationKind,
) -> Result<wire::Relation, Error> {
    let relation = wire::Relation {
        relation_id: omloom_contracts::ids::RelationId::generate().to_string(),
        from: Some(from.clone()),
        to: Some(to.clone()),
        kind: kind as i32,
        revision: 1,
        authority_id: omloom_contracts::ids::AuthorityId::generate().to_string(),
    };
    store.save_relation(
        &admit(wire::record::Body::Relation(relation.clone())),
        None,
        "fixture",
        100,
    )?;
    if matches!(
        kind,
        wire::RelationKind::Contains | wire::RelationKind::HasPort | wire::RelationKind::Carries
    ) {
        responsibility(
            store,
            &operator(),
            to,
            Action::Apply,
            vec![api::ResponsibilityAssignment {
                facet: Facet::ClaimedOwner as i32,
                mode: Mode::Inherit as i32,
                source_relation: Some(relation.relation_id.clone()),
                source_revision: Some(1),
                ..Default::default()
            }],
        )?;
    }
    Ok(relation)
}

// A forged hidden relation cannot select another team's responsibility source.
#[test]
fn responsibility_rejects_hidden_administrative_source() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let parent = create(&store, &actor, K::Device, "Private server")?;
    let child = create(&store, &actor, K::Interface, "Interface")?;
    let relation = connect(&store, &parent, &child, wire::RelationKind::Contains)?;
    responsibility(
        &store,
        &actor,
        &parent,
        Action::Apply,
        vec![owner("network-team")],
    )?;
    let mut limited = actor.clone();
    limited.grants[0].scope = Scope::Targets(vec![AssetRef::try_from(&child)?]);
    let before = store.usage()?;
    assert!(matches!(
        responsibility(
            &store,
            &limited,
            &child,
            Action::Apply,
            vec![api::ResponsibilityAssignment {
                facet: Facet::ClaimedOwner as i32,
                mode: Mode::Inherit as i32,
                source_relation: Some(relation.relation_id),
                source_revision: Some(1),
                ..Default::default()
            }]
        ),
        Err(Error::Denied)
    ));
    assert_eq!(store.usage()?, before);
    Ok(())
}

// Malformed ownership syntax is rejected before resolving any requested resource scope.
#[test]
fn responsibility_filter_validation_precedes_scope_lookup() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let missing = wire::AssetRef {
        asset_id: omloom_contracts::ids::AssetId::generate().to_string(),
        incarnation: omloom_contracts::ids::IncarnationId::generate().to_string(),
    };
    let before = store.usage()?;
    assert!(matches!(
        call(
            &store,
            &operator(),
            Command::Query(api::Query {
                roots: vec![missing],
                responsibility: Some(api::ResponsibilityFilter {
                    facet: Facet::ClaimedOwner as i32,
                    issuer: String::new(),
                    subject: "team".into()
                }),
                ..query()
            })
        ),
        Err(Error::Invalid)
    ));
    assert_eq!(store.usage()?, before);
    Ok(())
}

// Current assignments survive restart, stop inheritance explicitly and never grant access.
#[test]
fn responsibility_is_current_downward_and_independent_of_access() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let parent = create(&store, &actor, K::Device, "Server")?;
    let child = create(&store, &actor, K::Interface, "NIC")?;
    let peer = create(&store, &actor, K::Device, "Peer")?;
    connect(&store, &parent, &child, wire::RelationKind::Contains)?;
    responsibility(
        &store,
        &actor,
        &parent,
        Action::Apply,
        vec![owner("devops")],
    )?;
    let state = responsibility(&store, &actor, &child, Action::Read, vec![])?;
    let inherited = &state.effective[0];
    assert_eq!(inherited.status, Status::Inherited as i32);
    assert_eq!(inherited.party.as_ref().ok_or("party")?.subject, "devops");
    assert_eq!(
        responsibility(&store, &actor, &peer, Action::Read, vec![])?.effective[0].status,
        Status::Unknown as i32
    );
    let before = store.usage()?;
    responsibility(
        &store,
        &actor,
        &parent,
        Action::Apply,
        vec![owner("devops")],
    )?;
    responsibility(&store, &actor, &child, Action::Read, vec![])?;
    assert_eq!(store.usage()?, before);
    responsibility(
        &store,
        &actor,
        &child,
        Action::Apply,
        vec![api::ResponsibilityAssignment {
            facet: Facet::ClaimedOwner as i32,
            mode: Mode::Unassigned as i32,
            ..Default::default()
        }],
    )?;
    responsibility(
        &store,
        &actor,
        &parent,
        Action::Apply,
        vec![owner("engineering")],
    )?;
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    assert_eq!(
        responsibility(&store, &actor, &child, Action::Read, vec![])?.effective[0].status,
        Status::Unassigned as i32
    );
    let mut limited = actor;
    limited.grants[0].scope = Scope::Targets(vec![AssetRef::try_from(&child)?]);
    assert!(matches!(
        responsibility(&store, &limited, &parent, Action::Read, vec![]),
        Err(Error::Denied)
    ));
    Ok(())
}

// Child value disclosure must not reveal a hidden ancestor identity or contact.
#[test]
fn responsibility_redacts_hidden_source_and_contact() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let parent = create(&store, &actor, K::Device, "Private")?;
    let child = create(&store, &actor, K::Interface, "NIC")?;
    connect(&store, &parent, &child, wire::RelationKind::Contains)?;
    let mut assignment = owner("operators");
    assignment.contact = Some("mailto:private@example.test".into());
    responsibility(&store, &actor, &parent, Action::Apply, vec![assignment])?;
    let mut limited = actor;
    limited.grants[0].scope = Scope::Targets(vec![AssetRef::try_from(&child)?]);
    limited.grants[0].read = Fields::Selected(vec!["name".into(), "claimed_owner".into()]);
    let state = responsibility(&store, &limited, &child, Action::Read, vec![])?;
    assert_eq!(state.effective.len(), 1);
    assert_eq!(
        state.effective[0].party.as_ref().ok_or("party")?.subject,
        "operators"
    );
    assert!(state.effective[0].source.is_none());
    assert!(state.effective[0].contact.is_none());
    Ok(())
}

// Reading a parent's name must not disclose its restricted component association.
#[test]
fn responsibility_redacts_source_without_parent_relationship_rights() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let parent = create(&store, &actor, K::Device, "Named but restricted parent")?;
    let child = create(&store, &actor, K::Interface, "Visible child")?;
    connect(&store, &parent, &child, wire::RelationKind::Contains)?;
    responsibility(
        &store,
        &actor,
        &parent,
        Action::Apply,
        vec![owner("operators")],
    )?;
    let mut limited = actor;
    limited.grants[0].scope = Scope::Targets(vec![AssetRef::try_from(&child)?]);
    let mut parent_grant = limited.grants[0].clone();
    parent_grant.scope = Scope::Targets(vec![AssetRef::try_from(&parent)?]);
    parent_grant.read = Fields::Selected(vec!["name".into()]);
    parent_grant.writable = false;
    limited.grants.push(parent_grant);
    let before = store.usage()?;
    let state = responsibility(&store, &limited, &child, Action::Read, vec![])?;
    assert!(state.assignments[0].source_relation.is_none());
    assert_eq!(
        state.effective[0].party.as_ref().ok_or("party")?.subject,
        "operators"
    );
    assert!(state.effective[0].source.is_none());
    assert!(state.effective[0].source_revision.is_none());
    assert_eq!(store.usage()?, before);
    Ok(())
}

// Hosting alone never transfers ownership; an explicit administrative source is revision-bound.
#[test]
fn responsibility_administrative_binding_does_not_follow_vm_migration() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let first = create(&store, &actor, K::Device, "First host")?;
    let second = create(&store, &actor, K::Device, "Second host")?;
    let guest = create(&store, &actor, K::VirtualMachine, "Guest")?;
    responsibility(
        &store,
        &actor,
        &first,
        Action::Apply,
        vec![owner("team-one")],
    )?;
    responsibility(
        &store,
        &actor,
        &second,
        Action::Apply,
        vec![owner("team-two")],
    )?;
    let mut relation = connect(&store, &guest, &first, wire::RelationKind::RunsOn)?;
    assert_eq!(
        responsibility(&store, &actor, &guest, Action::Read, vec![])?.effective[0].status,
        Status::Unknown as i32
    );
    let selected = responsibility(
        &store,
        &actor,
        &guest,
        Action::Apply,
        vec![api::ResponsibilityAssignment {
            facet: Facet::ClaimedOwner as i32,
            mode: Mode::Inherit as i32,
            source_relation: Some(relation.relation_id.clone()),
            source_revision: Some(1),
            ..Default::default()
        }],
    )?;
    assert_eq!(
        selected.effective[0].party.as_ref().ok_or("party")?.subject,
        "team-one"
    );
    relation.to = Some(second);
    relation.revision = 2;
    store.save_relation(
        &admit(wire::record::Body::Relation(relation)),
        Some(1),
        "migration",
        100,
    )?;
    let state = responsibility(&store, &actor, &guest, Action::Read, vec![])?;
    assert_eq!(state.effective[0].status, Status::SourceUnavailable as i32);
    assert!(state.effective[0].party.is_none());
    Ok(())
}

// Filtering resolves the selected facet before totals/pages and cannot use denied owner fields.
#[test]
fn responsibility_filter_precedes_pagination_and_disclosure() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let server = create(&store, &actor, K::Device, "Server")?;
    let child = create(&store, &actor, K::Interface, "NIC")?;
    let other = create(&store, &actor, K::Device, "Other")?;
    connect(&store, &server, &child, wire::RelationKind::Contains)?;
    responsibility(
        &store,
        &actor,
        &server,
        Action::Apply,
        vec![owner("devops")],
    )?;
    let mut same_label = owner("devops");
    same_label.party.as_mut().ok_or("party")?.issuer = "another-company.example".into();
    responsibility(&store, &actor, &other, Action::Apply, vec![same_label])?;
    let mut query = api::Query {
        responsibility: Some(api::ResponsibilityFilter {
            facet: Facet::ClaimedOwner as i32,
            issuer: "company.example".into(),
            subject: "devops".into(),
        }),
        limit: 1,
        ..query()
    };
    let Reply::Page(first) = call(&store, &actor, Command::Query(query.clone()))? else {
        panic!("page");
    };
    assert_eq!(first.total, 2);
    assert_eq!(first.items.len(), 1);
    query.cursor = Some(first.next.ok_or("next")?);
    let Reply::Page(second) = call(&store, &actor, Command::Query(query.clone()))? else {
        panic!("page");
    };
    assert_eq!(second.total, 2);
    assert_ne!(first.items[0].target, second.items[0].target);
    let mut hidden = actor;
    hidden.grants[0].read = Fields::Selected(vec!["name".into()]);
    query.cursor = None;
    let Reply::Page(page) = call(&store, &hidden, Command::Query(query))? else {
        panic!("page");
    };
    assert_eq!(page.total, 0);
    assert!(page.items.is_empty());
    Ok(())
}

// Typed import previews roll back and validation failure cannot partly apply a multi-resource change.
#[test]
fn responsibility_import_is_atomic_and_revision_bound() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let a = create(&store, &actor, K::Device, "First")?;
    let b = create(&store, &actor, K::VirtualMachine, "Second")?;
    let entry = |target: wire::AssetRef,
                 action: Action,
                 assignment: api::ResponsibilityAssignment| api::Responsibility {
        target: Some(target),
        expected: Some(1),
        changes: vec![assignment],
        action: action as i32,
        ..Default::default()
    };
    let transfer = |entries: Vec<api::Responsibility>, basis: u64| {
        store.workspace(
            &actor,
            api::Request {
                command: Some(Command::ResponsibilityTransfer(
                    api::ResponsibilityTransfer { entries },
                )),
                working: Some(api::WorkingContext {
                    expected: Some(basis),
                    ..Default::default()
                }),
            },
            100,
        )
    };
    let before = store.usage()?;
    transfer(
        vec![
            entry(a.clone(), Action::Preview, owner("a")),
            entry(b.clone(), Action::Preview, owner("b")),
        ],
        before.revision,
    )?;
    assert_eq!(store.usage()?, before);
    let mut invalid = owner("b");
    invalid.party.as_mut().ok_or("party")?.kind = 999;
    assert!(matches!(
        transfer(
            vec![
                entry(a.clone(), Action::Apply, owner("a")),
                entry(b.clone(), Action::Apply, invalid)
            ],
            before.revision
        ),
        Err(Error::Invalid)
    ));
    assert_eq!(store.usage()?, before);
    transfer(
        vec![
            entry(a.clone(), Action::Apply, owner("a")),
            entry(b, Action::Apply, owner("b")),
        ],
        before.revision,
    )?;
    assert!(matches!(
        transfer(
            vec![entry(a.clone(), Action::Apply, owner("different"))],
            before.revision
        ),
        Err(Error::Conflict)
    ));
    assert_eq!(
        responsibility(&store, &actor, &a, Action::Read, vec![])?.effective[0]
            .party
            .as_ref()
            .ok_or("party")?
            .subject,
        "a"
    );
    Ok(())
}

// Competing assignments share the database CAS; backup/restore preserves the winner without authority widening.
#[test]
fn responsibility_concurrent_edits_and_restore_preserve_one_assignment() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let resource = create(&store, &actor, K::Device, "Shared server")?;
    let basis = store.usage()?.revision;
    let barrier = std::sync::Barrier::new(2);
    let change = |subject: &str| {
        barrier.wait();
        store.workspace(
            &actor,
            api::Request {
                command: Some(Command::Responsibility(api::Responsibility {
                    target: Some(resource.clone()),
                    expected: Some(1),
                    changes: vec![owner(subject)],
                    action: Action::Apply as i32,
                    ..Default::default()
                })),
                working: Some(api::WorkingContext {
                    expected: Some(basis),
                    ..Default::default()
                }),
            },
            100,
        )
    };
    let outcomes = std::thread::scope(|scope| {
        let a = scope.spawn(|| change("team-a"));
        let b = scope.spawn(|| change("team-b"));
        [
            a.join().expect("assignment writer"),
            b.join().expect("assignment writer"),
        ]
    });
    assert_eq!(outcomes.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        outcomes
            .iter()
            .filter(|r| matches!(r, Err(Error::Conflict)))
            .count(),
        1
    );
    let expected = responsibility(&store, &actor, &resource, Action::Read, vec![])?;
    let mut bytes = Vec::new();
    store.export(&mut bytes)?;
    let restored_dir = tempfile::tempdir()?;
    let restored = Store::restore(restored_dir.path(), &bytes, policy(), bytes.len() as u64)?;
    assert_eq!(
        responsibility(&restored, &actor, &resource, Action::Read, vec![])?.effective,
        expected.effective
    );
    let mut denied = actor;
    denied.grants.clear();
    assert!(matches!(
        responsibility(&restored, &denied, &resource, Action::Read, vec![]),
        Err(Error::Denied)
    ));
    Ok(())
}

// Independent fixed and dynamic fixture decisions do not change when ownership is edited.
#[test]
fn responsibility_cannot_create_or_revoke_descendant_grants() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let server = create(&store, &actor, K::Device, "Server")?;
    let parent = AssetRef::try_from(&server)?;
    let mut fixed = actor.clone();
    fixed.grants[0].scope = Scope::Targets(vec![parent]);
    let child = call(
        &store,
        &actor,
        Command::Write(api::Write {
            kind: K::Interface as i32,
            parent: Some(server.clone()),
            changes: vec![text_field("name", "NIC")],
            ..Default::default()
        }),
    )?;
    let Reply::Mutation(child) = child else {
        panic!("creation");
    };
    let child = child.targets[0].clone();
    let mut dynamic = fixed.clone();
    dynamic.authority_revision += 1;
    dynamic.grants[0].scope = Scope::Targets(vec![parent, AssetRef::try_from(&child)?]);
    assert!(matches!(
        responsibility(&store, &fixed, &child, Action::Read, vec![]),
        Err(Error::Denied)
    ));
    responsibility(
        &store,
        &actor,
        &server,
        Action::Apply,
        vec![owner("new-team")],
    )?;
    assert!(matches!(
        responsibility(&store, &fixed, &child, Action::Read, vec![]),
        Err(Error::Denied)
    ));
    assert_eq!(
        responsibility(&store, &dynamic, &child, Action::Read, vec![])?.effective[0]
            .party
            .as_ref()
            .ok_or("party")?
            .subject,
        "new-team"
    );
    dynamic.grants[0].read = Fields::Selected(vec!["name".into()]);
    assert!(
        responsibility(&store, &dynamic, &child, Action::Read, vec![])?
            .effective
            .is_empty()
    );
    Ok(())
}

// A shared ancestor can first need one facet, then another, without skipping or disclosing either chain.
#[test]
fn responsibility_batch_keeps_independent_ancestor_field_masks() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let server = create(&store, &actor, K::Device, "Hidden server")?;
    let mut operational = owner("operations");
    operational.facet = Facet::Operator as i32;
    responsibility(&store, &actor, &server, Action::Apply, vec![operational])?;
    let make = |parent: &wire::AssetRef, name: &str| -> Result<wire::AssetRef, Error> {
        let Reply::Mutation(child) = call(
            &store,
            &actor,
            Command::Write(api::Write {
                kind: K::Interface as i32,
                parent: Some(parent.clone()),
                changes: vec![text_field("name", name)],
                ..Default::default()
            }),
        )?
        else {
            return Err(Error::Corrupt);
        };
        Ok(child.targets[0].clone())
    };
    let middle = make(&server, "Middle")?;
    let child = make(&middle, "Child")?;
    responsibility(
        &store,
        &actor,
        &middle,
        Action::Apply,
        vec![owner("local-team")],
    )?;
    let mut limited = actor;
    limited.grants = vec![
        Grant {
            scope: Scope::Targets(vec![AssetRef::try_from(&middle)?]),
            read: Fields::Selected(vec!["name".into(), "claimed_owner".into()]),
            write: Fields::Selected(Vec::new()),
            writable: false,
        },
        Grant {
            scope: Scope::Targets(vec![AssetRef::try_from(&child)?]),
            read: Fields::Selected(vec!["name".into(), "operator".into()]),
            write: Fields::Selected(Vec::new()),
            writable: false,
        },
    ];
    let Reply::ResponsibilityTransfer(export) = call(
        &store,
        &limited,
        Command::ResponsibilityTransfer(api::ResponsibilityTransfer {
            entries: vec![
                api::Responsibility {
                    target: Some(middle),
                    action: Action::Read as i32,
                    ..Default::default()
                },
                api::Responsibility {
                    target: Some(child),
                    action: Action::Read as i32,
                    ..Default::default()
                },
            ],
        }),
    )?
    else {
        panic!("transfer");
    };
    assert_eq!(export.entries[0].effective.len(), 1);
    assert_eq!(export.entries[1].effective.len(), 1);
    assert_eq!(
        export.entries[1].effective[0]
            .party
            .as_ref()
            .ok_or("operator")?
            .subject,
        "operations"
    );
    assert!(export.entries[1].effective[0].source.is_none());
    Ok(())
}

// A scoped owner query must not resolve unrelated infrastructure or fail on its stale bindings.
#[test]
fn responsibility_filter_resolves_only_its_selected_scope() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let local = create(&store, &actor, K::Device, "Selected server")?;
    let nic = create(&store, &actor, K::Interface, "NIC")?;
    connect(&store, &local, &nic, wire::RelationKind::Contains)?;
    let first = create(&store, &actor, K::Device, "Other first")?;
    let second = create(&store, &actor, K::Device, "Other second")?;
    let guest = create(&store, &actor, K::VirtualMachine, "Other guest")?;
    for server in [&local, &first, &second] {
        responsibility(&store, &actor, server, Action::Apply, vec![owner("team")])?;
    }
    let mut placement = connect(&store, &guest, &first, wire::RelationKind::RunsOn)?;
    responsibility(
        &store,
        &actor,
        &guest,
        Action::Apply,
        vec![api::ResponsibilityAssignment {
            facet: Facet::ClaimedOwner as i32,
            mode: Mode::Inherit as i32,
            source_relation: Some(placement.relation_id.clone()),
            source_revision: Some(1),
            ..Default::default()
        }],
    )?;
    placement.to = Some(second);
    placement.revision = 2;
    store.save_relation(
        &admit(wire::record::Body::Relation(placement)),
        Some(1),
        "migration",
        100,
    )?;
    let Reply::Page(page) = call(
        &store,
        &actor,
        Command::Query(api::Query {
            roots: vec![local],
            responsibility: Some(api::ResponsibilityFilter {
                facet: Facet::ClaimedOwner as i32,
                issuer: "company.example".into(),
                subject: "team".into(),
            }),
            ..query()
        }),
    )?
    else {
        panic!("page");
    };
    assert_eq!(page.total, 2);
    Ok(())
}

// A second hosting edge is not the guest's accepted administrative owner source.
#[test]
fn responsibility_global_filter_excludes_unselected_hosting_sources() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let first = create(&store, &actor, K::Device, "Administrative host")?;
    let second = create(&store, &actor, K::Device, "Placement host")?;
    let guest = create(&store, &actor, K::VirtualMachine, "Unrelated guest")?;
    responsibility(&store, &actor, &first, Action::Apply, vec![owner("other")])?;
    responsibility(
        &store,
        &actor,
        &second,
        Action::Apply,
        vec![owner("selected")],
    )?;
    let mut source = connect(&store, &guest, &first, wire::RelationKind::RunsOn)?;
    responsibility(
        &store,
        &actor,
        &guest,
        Action::Apply,
        vec![api::ResponsibilityAssignment {
            facet: Facet::ClaimedOwner as i32,
            mode: Mode::Inherit as i32,
            source_relation: Some(source.relation_id.clone()),
            source_revision: Some(source.revision),
            ..Default::default()
        }],
    )?;
    source.revision += 1;
    source.authority_id = omloom_contracts::ids::AuthorityId::generate().to_string();
    store.save_relation(
        &admit(wire::record::Body::Relation(source)),
        Some(1),
        "source-changed",
        100,
    )?;
    connect(&store, &guest, &second, wire::RelationKind::RunsOn)?;
    let before = store.usage()?;
    let start = std::time::Instant::now();
    let result = call(
        &store,
        &actor,
        Command::Query(api::Query {
            responsibility: Some(api::ResponsibilityFilter {
                facet: Facet::ClaimedOwner as i32,
                issuer: "company.example".into(),
                subject: "selected".into(),
            }),
            ..query()
        }),
    );
    eprintln!(
        "global selected-source query: {:?}, logical bytes {}",
        start.elapsed(),
        before.bytes.iter().sum::<u64>()
    );
    let Reply::Page(page) = result? else {
        panic!("page");
    };
    assert_eq!(page.total, 1);
    assert_eq!(page.items[0].target.as_ref(), Some(&second));
    assert_eq!(store.usage()?, before);
    // Excluded stale guests cannot invalidate a complete kind/view-restricted result.
    for restricted in [
        api::Query {
            kinds: vec![K::Domain as i32],
            ..query()
        },
        api::Query {
            view: api::View::Services as i32,
            ..query()
        },
    ] {
        let Reply::Page(page) = call(
            &store,
            &actor,
            Command::Query(api::Query {
                responsibility: Some(api::ResponsibilityFilter {
                    facet: Facet::ClaimedOwner as i32,
                    issuer: "company.example".into(),
                    subject: "other".into(),
                }),
                ..restricted
            }),
        )?
        else {
            panic!("page");
        };
        assert_eq!(page.total, 0);
    }
    // The actually selected source is still stale and must not become a complete count.
    assert!(matches!(
        call(
            &store,
            &actor,
            Command::Query(api::Query {
                responsibility: Some(api::ResponsibilityFilter {
                    facet: Facet::ClaimedOwner as i32,
                    issuer: "company.example".into(),
                    subject: "other".into(),
                }),
                ..query()
            })
        ),
        Err(Error::Incomplete)
    ));
    assert_eq!(store.usage()?, before);
    Ok(())
}

fn physical_bytes(
    root: &std::path::Path,
) -> Result<(u64, Option<u64>), Box<dyn std::error::Error>> {
    let mut pending = vec![root.to_path_buf()];
    let mut apparent = 0;
    #[cfg(unix)]
    let mut allocated = 0;
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.is_dir() {
                pending.push(entry.path());
            } else {
                assert!(metadata.is_file(), "fixture contains only regular files");
                apparent += metadata.len();
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    allocated += metadata.blocks() * 512;
                }
            }
        }
    }
    #[cfg(unix)]
    let allocated = Some(allocated);
    #[cfg(not(unix))]
    let allocated = None;
    Ok((apparent, allocated))
}

// Qualify a real 100-server estate: current reads write nothing and a parent edit never rewrites descendants.
#[test]
fn responsibility_reads_and_parent_changes_are_bounded() -> TestResult {
    let qualification_started = std::time::Instant::now();
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let mut parents = Vec::new();
    let mut children = Vec::new();
    for index in 0..100 {
        let server = create(&store, &actor, K::Device, &format!("Server {index:03}"))?;
        responsibility(
            &store,
            &actor,
            &server,
            Action::Apply,
            vec![owner(if index % 2 == 0 { "even" } else { "odd" })],
        )?;
        for port in 0..4 {
            let Reply::Mutation(created) = call(
                &store,
                &actor,
                Command::Write(api::Write {
                    kind: K::Interface as i32,
                    parent: Some(server.clone()),
                    changes: vec![text_field("name", &format!("NIC {index:03}-{port}"))],
                    ..Default::default()
                }),
            )?
            else {
                panic!("child");
            };
            children.push(created.targets[0].clone());
        }
        parents.push(server);
    }
    store.diagnostic_database().persist()?;
    eprintln!(
        "responsibility fixture ready after {:?}",
        qualification_started.elapsed()
    );
    let mut taps = Vec::new();
    for partition in coordinode_storage::engine::partition::Partition::all() {
        let (tap, _) = store
            .diagnostic_database()
            .engine()
            .tap_writes(*partition)?;
        taps.push(tap);
    }
    let before = store.usage()?;
    let physical_before = physical_bytes(dir.path())?;
    assert!(
        physical_before.0 > 0,
        "qualification needs actual disk files"
    );
    let syncs = store.diagnostic_database().engine().journal_syncs();
    let filtered = api::Query {
        limit: 25,
        include_context: true,
        responsibility: Some(api::ResponsibilityFilter {
            facet: Facet::ClaimedOwner as i32,
            issuer: "company.example".into(),
            subject: "even".into(),
        }),
        ..query()
    };
    let mut samples = Vec::new();
    for sample in 0..100 {
        let start = std::time::Instant::now();
        let Reply::Page(page) = call(&store, &actor, Command::Query(filtered.clone()))? else {
            panic!("page");
        };
        samples.push(start.elapsed());
        assert_eq!(page.total, 250);
        assert_eq!(page.items.len(), 25);
        if sample % 10 == 0 {
            eprintln!(
                "responsibility sample {sample}: {:?}; total elapsed {:?}",
                samples.last().ok_or("sample")?,
                qualification_started.elapsed()
            );
        }
    }
    let Reply::ResponsibilityTransfer(page) = call(
        &store,
        &actor,
        Command::ResponsibilityTransfer(api::ResponsibilityTransfer {
            entries: children
                .iter()
                .take(100)
                .map(|target| api::Responsibility {
                    target: Some(target.clone()),
                    action: Action::Read as i32,
                    ..Default::default()
                })
                .collect(),
        }),
    )?
    else {
        panic!("transfer");
    };
    assert_eq!(page.entries.len(), 100);
    assert_eq!(store.usage()?, before);
    let after_syncs = store.diagnostic_database().engine().journal_syncs();
    eprintln!("read journal syncs: {syncs:?} -> {after_syncs:?}");
    for tap in &taps {
        let writes = tap.take();
        eprintln!("read partition {:?}: {writes:?}", tap.partition());
        assert_eq!(
            writes,
            coordinode_storage::engine::tap::Tapped::Keys(Vec::new()),
            "read wrote partition {:?}",
            tap.partition()
        );
    }
    assert_eq!(after_syncs, syncs);
    let physical_after_reads = physical_bytes(dir.path())?;
    eprintln!(
        "database files including indexes/journal (apparent, allocated): before={physical_before:?}; after reads={physical_after_reads:?}"
    );
    samples.sort_unstable();
    eprintln!(
        "responsibility qualification: 100 servers + 400 interfaces, owner filter+context p50={:?} p95={:?} p99={:?}; logical bytes={}; read partition writes=0, journal syncs=0",
        samples[49],
        samples[94],
        samples[98],
        before.bytes.iter().sum::<u64>()
    );
    assert!(
        samples[98] < std::time::Duration::from_secs(2),
        "owner-filter p99 {:?} exceeds the retrieval gate",
        samples[98]
    );
    responsibility(
        &store,
        &actor,
        &parents[0],
        Action::Apply,
        vec![owner("changed")],
    )?;
    assert_eq!(
        store.usage()?.rows[omloom_store::Class::Inventory as usize],
        before.rows[omloom_store::Class::Inventory as usize]
    );
    let mut written = 0;
    for tap in &taps {
        let coordinode_storage::engine::tap::Tapped::Keys(keys) = tap.take() else {
            panic!("write observer exceeded its bound");
        };
        written += keys.len();
    }
    assert!(
        written < children.len(),
        "parent edit touched {written} partition keys for {} children",
        children.len()
    );
    eprintln!(
        "one parent change: partition key writes={written}; inventory row growth=0; inherited values change without descendant copies"
    );
    eprintln!(
        "database files after parent change (apparent, allocated): {:?}",
        physical_bytes(dir.path())?
    );
    let Reply::Page(changed) = call(
        &store,
        &actor,
        Command::Query(api::Query {
            responsibility: Some(api::ResponsibilityFilter {
                subject: "changed".into(),
                ..filtered.responsibility.ok_or("filter")?
            }),
            ..query()
        }),
    )?
    else {
        panic!("page");
    };
    assert_eq!(changed.total, 5);
    Ok(())
}
