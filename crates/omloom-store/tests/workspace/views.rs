use super::*;

// Definition visibility is separate from data permission and publication is an explicit mutation.
#[test]
fn personal_publication_and_shared_edit_conflicts() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let owner = operator();
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
    view.audiences = vec!["operations".into()];
    view.editors = view.audiences.clone();
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
        definition_revision: Some(d.revision),
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
