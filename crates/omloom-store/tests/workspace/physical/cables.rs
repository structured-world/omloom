use super::*;

fn edit(cable: &wire::AssetRef, key: &str) -> api::PhysicalCableEndEdit {
    api::PhysicalCableEndEdit {
        cable: Some(cable.clone()),
        end: Some(api::PhysicalCableEnd {
            key: key.into(),
            name: format!("Cable end {key}"),
            basis: "Operator-labelled physical cable".into(),
            connector: Some("LC/UPC duplex".into()),
            ..Default::default()
        }),
        expected: None,
    }
}
fn ends(
    store: &Store,
    actor: &Context,
    cable: &wire::AssetRef,
    after: Option<String>,
) -> Result<api::PhysicalView, Error> {
    physical(
        store,
        actor,
        api::physical::Action::CableEnds(api::PhysicalCableEnds {
            cable: Some(cable.clone()),
            after,
        }),
    )
}

// Defining ends never invents a live attachment; repeats and reads do not write history.
#[test]
fn cable_end_definitions_are_scoped_bounded_and_durable() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let a = create(&store, &actor, K::Cable, "Cable A").expect("Create cable A fixture");
    let b = create(&store, &actor, K::Cable, "Cable B").expect("Create cable B fixture");
    let mut value = edit(&a, "A");
    let saved = physical(
        &store,
        &actor,
        api::physical::Action::CableEnd(value.clone()),
    )
    .expect("Save named cable end")
    .cable_ends
    .pop()
    .ok_or(Error::Corrupt)?;
    let usage = store.usage()?;
    value.expected = Some(saved.revision);
    physical(&store, &actor, api::physical::Action::CableEnd(value))?;
    assert_eq!(
        ends(&store, &actor, &a, None)?.cable_ends,
        vec![saved.clone()]
    );
    assert!(ends(&store, &actor, &b, None)?.cable_ends.is_empty());
    assert_eq!(store.usage()?, usage);
    assert!(
        store
            .relations(AssetRef::try_from(&a).map_err(|_| Error::Invalid)?, 32)?
            .is_empty()
    );
    for i in 1..64 {
        physical(
            &store,
            &actor,
            api::physical::Action::CableEnd(edit(&a, &format!("end{i:02}"))),
        )?;
    }
    let usage = store.usage()?;
    assert!(matches!(
        physical(
            &store,
            &actor,
            api::physical::Action::CableEnd(edit(&a, "overflow"))
        ),
        Err(Error::Limit)
    ));
    assert_eq!(store.usage()?, usage);
    let first = ends(&store, &actor, &a, None)?;
    assert_eq!(first.cable_ends.len(), 25);
    let second = ends(&store, &actor, &a, first.next)?;
    assert_eq!(second.cable_ends.len(), 25);
    let last = ends(&store, &actor, &a, second.next)?;
    assert_eq!(last.cable_ends.len(), 14);
    assert!(last.next.is_none());
    assert_eq!(store.usage()?, usage);
    drop(store);
    let reopened = Store::open(dir.path(), policy())?;
    assert_eq!(ends(&reopened, &actor, &a, None)?.cable_ends[0], saved);
    Ok(())
}

// Scoped rights protect end definitions too; deleting an unlinked cable reclaims its own ends.
#[test]
fn cable_ends_follow_the_cables_rights_and_removal_lifecycle() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let cable = create(&store, &actor, K::Cable, "Unlinked cable")?;
    physical(
        &store,
        &actor,
        api::physical::Action::CableEnd(edit(&cable, "A")),
    )?;
    let usage = store.usage()?;
    let mut restricted = actor.clone();
    restricted.grants[0].read = Fields::Selected(vec!["name".into()]);
    assert!(matches!(
        ends(&store, &restricted, &cable, None),
        Err(Error::Denied)
    ));
    assert!(matches!(
        physical(
            &store,
            &restricted,
            api::physical::Action::CableEnd(edit(&cable, "B"))
        ),
        Err(Error::Denied)
    ));
    assert_eq!(store.usage()?, usage);
    call(
        &store,
        &actor,
        Command::Bulk(api::Bulk {
            targets: vec![cable],
            expected: usage.revision,
            remove: true,
            ..Default::default()
        }),
    )?;
    assert_eq!(
        store.usage()?.rows[omloom_store::Class::Inventory as usize],
        usage.rows[omloom_store::Class::Inventory as usize] - 3
    );
    Ok(())
}
