use super::*;

// Plan material inspection uses current serial facts and never discloses a denied field or writes a snapshot.
#[test]
fn plan_material_serials_follow_current_field_authority() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let rack = create(&store, &actor, K::Rack, "Materials rack")?;
    let item = create(&store, &actor, K::Device, "Selected serialized server")?;
    let current = store.asset(AssetRef::try_from(&item).map_err(|_| Error::Invalid)?)?;
    call(
        &store,
        &actor,
        Command::Write(api::Write {
            target: Some(item.clone()),
            expected: Some(current.revision),
            kind: current.kind,
            changes: vec![text_field("serial", "manufacturer-serial-1")],
            ..Default::default()
        }),
    )?;
    let plan = draft(&store, &actor, item.clone(), rack)?;
    let load = api::PhysicalPlanKey {
        id: plan.id.ok_or(Error::Corrupt)?,
        revision: plan.revision,
    };
    let usage = store.usage()?;
    let view = physical(&store, &actor, api::physical::Action::Load(load.clone()))?;
    let material = view
        .items
        .iter()
        .find(|i| i.target.as_ref() == Some(&item))
        .ok_or(Error::Missing)?;
    assert!(
        material
            .fields
            .contains(&text_field("serial", "manufacturer-serial-1"))
    );
    assert_eq!(store.usage()?, usage);
    let mut restricted = actor;
    for grant in &mut restricted.grants {
        grant.read = Fields::Selected(vec!["name".into(), "physical".into(), "relations".into()]);
    }
    let view = physical(&store, &restricted, api::physical::Action::Load(load))?;
    assert!(
        view.items
            .iter()
            .all(|i| i.fields.iter().all(|f| f.name != "serial"))
    );
    assert_eq!(store.usage()?, usage);
    Ok(())
}
