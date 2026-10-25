use super::*;

// Migration and an inventory edit share one database CAS boundary; restore cannot expose half a schema change.
#[test]
fn concurrent_migration_and_edit_keep_one_interpretation_after_restore() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let context = operator();
    let host = create(&store, &context, K::Device, "host")?;
    let api::reply::Result::Definitions(created) = call(
        &store,
        &context,
        api::request::Command::Definition(api::Definition {
            name: "Capacity".into(),
            kinds: vec![K::Device as i32],
            ..Default::default()
        }),
    )?
    else {
        panic!("definition");
    };
    let mut definition = created.items[0].clone();
    call(
        &store,
        &context,
        api::request::Command::Write(api::Write {
            target: Some(host.clone()),
            expected: Some(1),
            kind: K::Device as i32,
            changes: vec![text_field(&definition.id, "42")],
        }),
    )?;
    definition.r#type = api::FieldType::Integer as i32;
    let migration = api::Migration {
        definition: Some(definition.clone()),
        expected: store.usage()?.revision,
        apply: true,
        convert_types: true,
        ..Default::default()
    };
    let edit = api::Write {
        target: Some(host.clone()),
        expected: Some(2),
        kind: K::Device as i32,
        changes: vec![text_field("name", "edited")],
    };
    let barrier = std::sync::Barrier::new(2);
    let outcomes = std::thread::scope(|scope| {
        let first = scope.spawn(|| {
            barrier.wait();
            call(
                &store,
                &context,
                api::request::Command::Migration(migration),
            )
        });
        let second = scope.spawn(|| {
            barrier.wait();
            call(&store, &context, api::request::Command::Write(edit))
        });
        [
            first.join().expect("migration worker"),
            second.join().expect("edit worker"),
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
    let api::reply::Result::Definitions(current) = call(
        &store,
        &context,
        api::request::Command::Definitions(api::Empty {}),
    )?
    else {
        panic!("definitions");
    };
    let current = current.items[0].clone();
    let asset = store.asset(AssetRef::try_from(&host)?)?;
    let field = asset
        .intended
        .iter()
        .find_map(|v| v.field.as_ref().filter(|f| f.name == definition.id))
        .expect("value");
    if outcomes[0].is_ok() {
        assert_eq!(current.r#type, api::FieldType::Integer as i32);
        assert_eq!(field.value, Some(wire::field::Value::Integer(42)));
    } else {
        assert_eq!(current.r#type, api::FieldType::Text as i32);
        assert_eq!(field.value, Some(wire::field::Value::Text("42".into())));
    }
    let mut bytes = Vec::new();
    store.export(&mut bytes)?;
    let destination = tempfile::tempdir()?;
    let restored = Store::restore(destination.path(), &bytes, policy(), bytes.len() as u64)?;
    assert_eq!(restored.asset(AssetRef::try_from(&host)?)?, asset);
    let api::reply::Result::Definitions(after) = call(
        &restored,
        &context,
        api::request::Command::Definitions(api::Empty {}),
    )?
    else {
        panic!("definitions");
    };
    assert_eq!(after.items[0], current);
    Ok(())
}

// Definition visibility is separate from data permission and publication is an explicit mutation.
#[test]
fn personal_publication_and_shared_edit_conflicts() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let mut owner = operator();
    owner.teams.push("engineering".into());
    create(&store, &owner, K::Device, "private-host")?;
    let result = call(
        &store,
        &owner,
        api::request::Command::SaveView(api::SaveView {
            view: Some(api::SavedView {
                name: "Private topology".into(),
                query: Some(query()),
                ..Default::default()
            }),
            ..Default::default()
        }),
    )?;
    let api::reply::Result::Views(saved) = result else {
        panic!("saved definition")
    };
    let mut view = saved.items[0].clone();
    let mut colleague = owner.clone();
    colleague.actor = "fixture-colleague".into();
    colleague.teams = vec!["operations".into()];
    colleague.grants[0].scope = Scope::Kinds(vec![K::VirtualMachine]);
    let api::reply::Result::Views(hidden) = call(
        &store,
        &colleague,
        api::request::Command::Views(api::Empty {}),
    )?
    else {
        panic!("views")
    };
    assert!(hidden.items.is_empty());
    let before = store.usage()?;
    call(
        &store,
        &owner,
        api::request::Command::SaveView(api::SaveView {
            view: Some(view.clone()),
            ..Default::default()
        }),
    )?;
    assert_eq!(store.usage()?, before);
    view.audiences = vec!["operations".into(), "engineering".into()];
    view.editors = vec!["operations".into()];
    call(
        &store,
        &owner,
        api::request::Command::SaveView(api::SaveView {
            view: Some(view.clone()),
            publish: false,
            remove: false,
        }),
    )?;
    assert_eq!(store.usage()?, before);
    let api::reply::Result::Views(published) = call(
        &store,
        &owner,
        api::request::Command::SaveView(api::SaveView {
            view: Some(view),
            publish: true,
            remove: false,
        }),
    )?
    else {
        panic!("published")
    };
    let mut view = published.items[0].clone();
    let api::reply::Result::Views(visible) = call(
        &store,
        &colleague,
        api::request::Command::Views(api::Empty {}),
    )?
    else {
        panic!("views")
    };
    assert_eq!(visible.items.len(), 1);
    let api::reply::Result::Page(page) = call(
        &store,
        &colleague,
        api::request::Command::Query(view.query.clone().expect("query")),
    )?
    else {
        panic!("page")
    };
    assert_eq!(
        page.total, 0,
        "A published view never grants its owner's host scope"
    );
    view.name = "Shared edit".into();
    call(
        &store,
        &colleague,
        api::request::Command::SaveView(api::SaveView {
            view: Some(view.clone()),
            publish: true,
            remove: false,
        }),
    )?;
    assert!(matches!(
        call(
            &store,
            &owner,
            api::request::Command::SaveView(api::SaveView {
                view: Some(view),
                publish: true,
                remove: false
            })
        ),
        Err(Error::Conflict)
    ));
    Ok(())
}

// A retired or retyped predicate fails explicitly, including after a reopen; it never becomes all inventory.
#[test]
fn invalid_saved_predicate_is_not_silently_removed() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let context = operator();
    create(&store, &context, K::Device, "host")?;
    let api::reply::Result::Definitions(created) = call(
        &store,
        &context,
        api::request::Command::Definition(api::Definition {
            name: "Purpose".into(),
            kinds: vec![K::Device as i32],
            ..Default::default()
        }),
    )?
    else {
        panic!("definition")
    };
    let mut d = created.items[0].clone();
    let mut q = query();
    q.predicates.push(api::Predicate {
        field: d.id.clone(),
        definition_generation: Some(d.generation),
        value: Some(text_field(&d.id, "backup")),
    });
    let api::reply::Result::Views(saved) = call(
        &store,
        &context,
        api::request::Command::SaveView(api::SaveView {
            view: Some(api::SavedView {
                name: "Backup hosts".into(),
                query: Some(q.clone()),
                ..Default::default()
            }),
            ..Default::default()
        }),
    )?
    else {
        panic!("view")
    };
    d.retired = true;
    call(&store, &context, api::request::Command::Definition(d))?;
    assert!(matches!(
        call(&store, &context, api::request::Command::Query(q.clone())),
        Err(Error::Incomplete)
    ));
    store.workspace_cleanup_definitions(32)?;
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    let api::reply::Result::Views(views) = call(
        &store,
        &context,
        api::request::Command::Views(api::Empty {}),
    )?
    else {
        panic!("views")
    };
    assert_eq!(views.items[0].id, saved.items[0].id);
    assert_eq!(views.items[0].query.as_ref(), Some(&q));
    assert!(matches!(
        call(&store, &context, api::request::Command::Query(q)),
        Err(Error::Incomplete)
    ));
    Ok(())
}

// A label-only rename must not break the meaning of an existing typed predicate.
#[test]
fn renamed_definition_keeps_filter_interpretation() -> TestResult {
    let directory = tempfile::tempdir()?;
    let store = Store::open(directory.path(), policy())?;
    let context = operator();
    let target = create(&store, &context, K::Device, "host")?;
    let api::reply::Result::Definitions(created) = call(
        &store,
        &context,
        api::request::Command::Definition(api::Definition {
            name: "Owner label".into(),
            kinds: vec![K::Device as i32],
            ..Default::default()
        }),
    )?
    else {
        panic!("definition")
    };
    let mut definition = created.items[0].clone();
    call(
        &store,
        &context,
        api::request::Command::Write(api::Write {
            target: Some(target),
            expected: Some(1),
            kind: K::Device as i32,
            changes: vec![text_field(&definition.id, "operations")],
        }),
    )?;
    let mut q = query();
    q.predicates.push(api::Predicate {
        field: definition.id.clone(),
        definition_generation: Some(definition.generation),
        value: Some(text_field(&definition.id, "operations")),
    });
    definition.name = "Responsible team".into();
    call(
        &store,
        &context,
        api::request::Command::Definition(definition),
    )?;
    let api::reply::Result::Page(page) = call(&store, &context, api::request::Command::Query(q))?
    else {
        panic!("page")
    };
    assert_eq!(page.total, 1);
    Ok(())
}

// Unchanged migrations need no new state or audit, and schema administration cannot grant data access.
#[test]
fn migration_requires_data_rights_and_does_not_write_noops() -> TestResult {
    let directory = tempfile::tempdir()?;
    let store = Store::open(directory.path(), policy())?;
    let context = operator();
    let target = create(&store, &context, K::Device, "host")?;
    let api::reply::Result::Definitions(created) = call(
        &store,
        &context,
        api::request::Command::Definition(api::Definition {
            name: "Purpose".into(),
            kinds: vec![K::Device as i32],
            ..Default::default()
        }),
    )?
    else {
        panic!("definition")
    };
    let definition = created.items[0].clone();
    call(
        &store,
        &context,
        api::request::Command::Write(api::Write {
            target: Some(target),
            expected: Some(1),
            kind: K::Device as i32,
            changes: vec![text_field(&definition.id, "backup")],
        }),
    )?;
    let before = store.usage()?;
    call(
        &store,
        &context,
        api::request::Command::Migration(api::Migration {
            definition: Some(definition.clone()),
            expected: before.revision,
            apply: true,
            ..Default::default()
        }),
    )?;
    assert_eq!(store.usage()?, before);
    let mut restricted = context.clone();
    restricted.grants[0].read = Fields::Selected(vec!["name".into()]);
    restricted.grants[0].write = Fields::Selected(vec!["name".into()]);
    let mut next = definition;
    next.r#type = api::FieldType::Choice as i32;
    next.choices = vec!["backup".into()];
    assert!(matches!(
        call(
            &store,
            &restricted,
            api::request::Command::Migration(api::Migration {
                definition: Some(next),
                expected: store.usage()?.revision,
                apply: true,
                ..Default::default()
            })
        ),
        Err(Error::Denied)
    ));
    Ok(())
}

// Narrowing applicability must show exactly what is removed and require explicit consent.
#[test]
fn applicability_migration_previews_removal_without_silent_data_loss() -> TestResult {
    let directory = tempfile::tempdir()?;
    let store = Store::open(directory.path(), policy())?;
    let context = operator();
    let host = create(&store, &context, K::Device, "host")?;
    let vm = create(&store, &context, K::VirtualMachine, "vm")?;
    let api::reply::Result::Definitions(created) = call(
        &store,
        &context,
        api::request::Command::Definition(api::Definition {
            name: "Purpose".into(),
            kinds: vec![K::Device as i32, K::VirtualMachine as i32],
            ..Default::default()
        }),
    )?
    else {
        panic!("definition");
    };
    let mut definition = created.items[0].clone();
    for (target, kind) in [(host.clone(), K::Device), (vm.clone(), K::VirtualMachine)] {
        call(
            &store,
            &context,
            api::request::Command::Write(api::Write {
                target: Some(target),
                expected: Some(1),
                kind: kind as i32,
                changes: vec![text_field(&definition.id, "backup")],
            }),
        )?;
    }
    definition.kinds = vec![K::Device as i32];
    let before = store.usage()?;
    let mut migration = api::Migration {
        definition: Some(definition.clone()),
        expected: before.revision,
        ..Default::default()
    };
    let api::reply::Result::MigrationPreview(preview) = call(
        &store,
        &context,
        api::request::Command::Migration(migration.clone()),
    )?
    else {
        panic!("preview");
    };
    assert_eq!(
        (preview.affected, preview.removed, preview.invalid),
        (2, 0, 1)
    );
    assert_eq!(preview.issues[0].target.as_ref(), Some(&vm));
    assert_eq!(store.usage()?, before);
    migration.remove_inapplicable = true;
    let api::reply::Result::MigrationPreview(preview) = call(
        &store,
        &context,
        api::request::Command::Migration(migration.clone()),
    )?
    else {
        panic!("preview");
    };
    assert_eq!(
        (preview.affected, preview.removed, preview.invalid),
        (2, 1, 0)
    );
    assert_eq!(store.usage()?, before);
    migration.apply = true;
    call(
        &store,
        &context,
        api::request::Command::Migration(migration),
    )?;
    assert!(
        store
            .asset(AssetRef::try_from(&host)?)?
            .intended
            .iter()
            .any(|v| v.field.as_ref().is_some_and(|f| f.name == definition.id))
    );
    assert!(
        store
            .asset(AssetRef::try_from(&vm)?)?
            .intended
            .iter()
            .all(|v| v.field.as_ref().is_none_or(|f| f.name != definition.id))
    );
    assert_eq!(store.asset(AssetRef::try_from(&vm)?)?.revision, 3);
    Ok(())
}
