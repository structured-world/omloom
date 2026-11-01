use super::*;

// Large enclosures remain readable through bounded pages rather than failing the entire projection.
#[test]
fn occupancy_beyond_128_parts_returns_a_bounded_first_page() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let rack = create(&store, &actor, K::Rack, "Large enclosure")?;
    for index in 0..129 {
        let item = create(
            &store,
            &actor,
            K::Device,
            &format!("Installed unit {index}"),
        )?;
        call(
            &store,
            &actor,
            Command::Relation(api::Relationship {
                relation: Some(wire::Relation {
                    from: Some(item),
                    to: Some(rack.clone()),
                    kind: wire::RelationKind::InstalledIn as i32,
                    ..Default::default()
                }),
                ..Default::default()
            }),
        )?;
    }
    let before = store.usage()?;
    let page = physical(
        &store,
        &actor,
        api::physical::Action::Read(api::PhysicalRead {
            enclosure: Some(rack.clone()),
            occupants: true,
            after: None,
        }),
    )?;
    assert_eq!(page.occupancy.len(), 25);
    assert!(page.next.is_some());
    let mut seen: BTreeSet<_> = page
        .occupancy
        .iter()
        .filter_map(|v| v.binding.as_ref().map(|v| v.relation_id.clone()))
        .collect();
    let first_cursor = page.next.clone();
    let mut after = page.next;
    let mut sizes = vec![page.occupancy.len()];
    while let Some(cursor) = after {
        let page = physical(
            &store,
            &actor,
            api::physical::Action::Read(api::PhysicalRead {
                enclosure: Some(rack.clone()),
                occupants: true,
                after: Some(cursor),
            }),
        )?;
        sizes.push(page.occupancy.len());
        for value in page.occupancy {
            assert!(seen.insert(value.binding.ok_or(Error::Corrupt)?.relation_id));
        }
        after = page.next;
    }
    assert_eq!(sizes, vec![25, 25, 25, 25, 25, 4]);
    assert_eq!(seen.len(), 129);
    assert_eq!(store.usage()?, before);
    let mut changed = actor.clone();
    changed.authority_revision += 1;
    assert!(matches!(
        physical(
            &store,
            &changed,
            api::physical::Action::Read(api::PhysicalRead {
                enclosure: Some(rack.clone()),
                occupants: true,
                after: first_cursor.clone()
            })
        ),
        Err(Error::Expired)
    ));
    create(&store, &actor, K::Device, "Revision change")?;
    assert!(matches!(
        physical(
            &store,
            &actor,
            api::physical::Action::Read(api::PhysicalRead {
                enclosure: Some(rack),
                occupants: true,
                after: first_cursor
            })
        ),
        Err(Error::Expired)
    ));
    Ok(())
}
