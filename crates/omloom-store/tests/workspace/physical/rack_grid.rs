use super::*;

// A cabinet's height cannot silently supply its rail offset, count or numbering.
#[test]
fn rack_grid_survives_reopen_without_per_unit_records_or_repeat_writes() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let rack = create(&store, &actor, K::Rack, "Offset rail rack")?;
    let mut facts = define(rack.clone());
    facts.rack_grid = Some(Box::new(api::PhysicalRackGrid {
        top_um: 30_000,
        unit_count: 42,
        first_unit: 10,
        direction: api::PhysicalRackDirection::BottomUp as i32,
    }));
    let save = api::physical::Action::Define(api::PhysicalDefinitionEdit {
        definition: Some(facts.clone()),
    });
    physical(&store, &actor, save.clone())?;
    let usage = store.usage()?;
    physical(&store, &actor, save)?;
    assert_eq!(store.usage()?, usage);
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    let read = physical(
        &store,
        &actor,
        api::physical::Action::Read(api::PhysicalRead {
            enclosure: Some(rack),
            occupants: false,
        }),
    )?;
    assert_eq!(read.definitions[0], facts);
    assert_eq!(store.usage()?, usage);
    Ok(())
}

// Invalid source grids are refused before replacing the current physical facts.
#[test]
fn rack_grid_rejects_invalid_numbering_overflow_and_outside_known_envelope() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let rack = create(&store, &actor, K::Rack, "Validated rail rack")?;
    let mut facts = define(rack);
    let valid = api::PhysicalRackGrid {
        top_um: 0,
        unit_count: 42,
        first_unit: 1,
        direction: api::PhysicalRackDirection::TopDown as i32,
    };
    for grid in [
        api::PhysicalRackGrid {
            unit_count: 0,
            ..valid
        },
        api::PhysicalRackGrid {
            first_unit: 0,
            ..valid
        },
        api::PhysicalRackGrid {
            direction: 0,
            ..valid
        },
        api::PhysicalRackGrid {
            direction: 99,
            ..valid
        },
        api::PhysicalRackGrid {
            first_unit: u32::MAX,
            ..valid
        },
        api::PhysicalRackGrid {
            top_um: i64::MAX,
            ..valid
        },
        api::PhysicalRackGrid {
            top_um: -1,
            ..valid
        },
        api::PhysicalRackGrid {
            unit_count: 46,
            ..valid
        },
    ] {
        facts.rack_grid = Some(Box::new(grid));
        let usage = store.usage()?;
        assert!(matches!(
            physical(
                &store,
                &actor,
                api::physical::Action::Define(api::PhysicalDefinitionEdit {
                    definition: Some(facts.clone())
                })
            ),
            Err(Error::Invalid)
        ));
        assert_eq!(store.usage()?, usage);
    }
    facts.target = Some(create(
        &store,
        &actor,
        K::Device,
        "A chassis is not a rack rail grid",
    )?);
    facts.rack_grid = Some(Box::new(valid));
    let usage = store.usage()?;
    assert!(matches!(
        physical(
            &store,
            &actor,
            api::physical::Action::Define(api::PhysicalDefinitionEdit {
                definition: Some(facts),
            })
        ),
        Err(Error::Invalid)
    ));
    assert_eq!(store.usage()?, usage);
    Ok(())
}
