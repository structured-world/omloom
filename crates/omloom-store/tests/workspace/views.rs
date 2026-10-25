use super::*;

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
