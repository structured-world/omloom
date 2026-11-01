use super::*;

// Recording a rack's dimensions does not establish that its unrecorded space is empty.
#[test]
fn dimensional_definition_alone_does_not_qualify_free_capacity() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let rack = create(&store, &actor, K::Rack, "Partly documented rack")?;
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(api::PhysicalDefinition {
                target: Some(rack.clone()),
                volume: Some(volume()),
                basis: "Dimensions measured; current occupancy not surveyed".into(),
                ..Default::default()
            }),
        }),
    )?;
    let offer = offer(&store, &actor, &rack)?;
    let before = store.usage()?;
    let availability = read(&store, &actor, &offer)?;
    assert_eq!(availability.state, api::RequirementState::Unknown as i32);
    assert_eq!(availability.reason, api::RequirementReason::Missing as i32);
    assert_eq!(store.usage()?, before);
    let item = create(&store, &actor, K::Device, "Known loose equipment")?;
    equipment_dimensions(&store, &actor, &item)?;
    let plan = draft(&store, &actor, item, rack)?;
    assert!(matches!(
        reserve(&store, &actor, plan),
        Err(Error::Incomplete)
    ));
    Ok(())
}

// A limited survey qualifies only that region; withdrawing it cannot evict an admitted allocation.
#[test]
fn occupancy_coverage_is_explicit_scoped_and_not_a_reservation_lifetime() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let rack = create(&store, &actor, K::Rack, "Partly surveyed rack")?;
    let mut facts = define(rack.clone());
    let mut partial = volume();
    partial.z.as_mut().ok_or(Error::Corrupt)?.length_um = 500_000;
    facts.occupancy_coverage = Some(Box::new(partial));
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(facts.clone()),
        }),
    )?;
    let mut offer = offer(&store, &actor, &rack)?;
    assert_eq!(
        read(&store, &actor, &offer)?.reason,
        api::RequirementReason::Incomplete as i32
    );
    offer.region = Some(partial);
    offer = physical(
        &store,
        &actor,
        api::physical::Action::Offer(api::PhysicalOfferEdit { offer: Some(offer) }),
    )?
    .offers
    .pop()
    .ok_or(Error::Corrupt)?;
    assert_eq!(
        read(&store, &actor, &offer)?.state,
        api::RequirementState::Satisfied as i32
    );
    // No observed load is not a complete whole-rack power statement.
    offer.power = true;
    offer = physical(
        &store,
        &actor,
        api::physical::Action::Offer(api::PhysicalOfferEdit { offer: Some(offer) }),
    )?
    .offers
    .pop()
    .ok_or(Error::Corrupt)?;
    assert_eq!(
        read(&store, &actor, &offer)?.reason,
        api::RequirementReason::Incomplete as i32
    );
    let item = create(&store, &actor, K::Device, "Unplaced known equipment")?;
    equipment_dimensions(&store, &actor, &item)?;
    let plan = draft(&store, &actor, item, rack)?;
    assert!(matches!(
        reserve(&store, &actor, plan.clone()),
        Err(Error::Incomplete)
    ));
    facts.occupancy_coverage = Some(Box::new(volume()));
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(facts.clone()),
        }),
    )?;
    reserve(&store, &actor, plan)?;
    let held = store.usage()?.rows[omloom_store::Class::Pending as usize];
    facts.occupancy_coverage = None;
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(facts),
        }),
    )?;
    assert_eq!(
        store.usage()?.rows[omloom_store::Class::Pending as usize],
        held
    );
    // The retained allocation still proves this region is not offered, even after survey withdrawal.
    assert_eq!(
        read(&store, &actor, &offer)?.state,
        api::RequirementState::Failed as i32
    );
    Ok(())
}

fn offer(
    store: &Store,
    actor: &Context,
    rack: &wire::AssetRef,
) -> Result<api::PhysicalOffer, Error> {
    physical(
        store,
        actor,
        api::physical::Action::Offer(api::PhysicalOfferEdit {
            offer: Some(api::PhysicalOffer {
                enclosure: Some(rack.clone()),
                region: Some(volume()),
                recipient: "regional-reader".into(),
                name: "Reviewed customer region".into(),
                ..Default::default()
            }),
        }),
    )?
    .offers
    .pop()
    .ok_or(Error::Corrupt)
}
fn read(
    store: &Store,
    actor: &Context,
    offer: &api::PhysicalOffer,
) -> Result<api::PhysicalView, Error> {
    physical(
        store,
        actor,
        api::physical::Action::Availability(api::PhysicalOfferRead {
            id: offer.id.clone().ok_or(Error::Corrupt)?,
        }),
    )
}

// The separate offer grant reveals one approved region result, never hidden occupants or their geometry.
#[test]
fn availability_offer_hides_occupants_and_obeys_revoke() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let rack = create(&store, &actor, K::Rack, "Private rack")?;
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(define(rack.clone())),
        }),
    )?;
    let mut offer = offer(&store, &actor, &rack)?;
    let mut recipient = actor.clone();
    recipient.actor = "regional-reader".into();
    recipient.grants = vec![Grant {
        scope: Scope::Targets(vec![AssetRef::try_from(&rack).map_err(|_| Error::Invalid)?]),
        read: Fields::Selected(vec!["physical_availability".into()]),
        write: Fields::Selected(vec![]),
        writable: false,
    }];
    let usage = store.usage()?;
    let available = read(&store, &recipient, &offer)?;
    assert_eq!(available.state, api::RequirementState::Satisfied as i32);
    assert!(
        available.occupancy.is_empty()
            && available.items.is_empty()
            && available.definitions.is_empty()
    );
    assert_eq!(available.offers, vec![offer.clone()]);
    assert_eq!(store.usage()?, usage);
    let hidden = create(&store, &actor, K::Device, "Secret occupant")?;
    equipment_dimensions(&store, &actor, &hidden)?;
    let plan = draft(&store, &actor, hidden, rack.clone())?;
    reserve(&store, &actor, plan)?;
    let unavailable = read(&store, &recipient, &offer)?;
    assert_eq!(unavailable.state, api::RequirementState::Failed as i32);
    assert!(
        unavailable.occupancy.is_empty()
            && unavailable.items.is_empty()
            && unavailable.definitions.is_empty()
    );
    assert_eq!(unavailable.offers, vec![offer.clone()]);
    let usage = store.usage()?;
    assert!(matches!(
        physical(
            &store,
            &recipient,
            api::physical::Action::Read(api::PhysicalRead {
                enclosure: Some(rack),
                occupants: true,
                after: None,
            })
        ),
        Err(Error::Denied)
    ));
    for _ in 0..3 {
        read(&store, &recipient, &offer)?;
    }
    assert_eq!(store.usage()?, usage);
    // Loss of geometry cannot prevent withdrawing an already disclosed region.
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(api::PhysicalDefinition {
                target: offer.enclosure.clone(),
                basis: "Geometry withdrawn after review".into(),
                ..Default::default()
            }),
        }),
    )?;
    offer.revoked = true;
    physical(
        &store,
        &actor,
        api::physical::Action::Offer(api::PhysicalOfferEdit {
            offer: Some(offer.clone()),
        }),
    )?;
    assert!(matches!(
        read(&store, &recipient, &offer),
        Err(Error::Denied)
    ));
    Ok(())
}

// Offer IDs and region profiles grant nothing without current recipient and field authority.
#[test]
fn availability_offer_is_revisioned_bounded_and_not_a_bearer_grant() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let rack = create(&store, &actor, K::Rack, "Rack")?;
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(define(rack.clone())),
        }),
    )?;
    let offer = offer(&store, &actor, &rack)?;
    let usage = store.usage()?;
    physical(
        &store,
        &actor,
        api::physical::Action::Offer(api::PhysicalOfferEdit {
            offer: Some(offer.clone()),
        }),
    )?;
    assert_eq!(store.usage()?, usage);
    let mut other = actor.clone();
    other.actor = "other-recipient".into();
    assert!(matches!(read(&store, &other, &offer), Err(Error::Denied)));
    other.actor = "regional-reader".into();
    other.grants.clear();
    assert!(matches!(read(&store, &other, &offer), Err(Error::Denied)));
    let mut invalid = offer.clone();
    invalid
        .region
        .as_mut()
        .ok_or(Error::Corrupt)?
        .z
        .as_mut()
        .ok_or(Error::Corrupt)?
        .length_um += 1;
    assert!(matches!(
        physical(
            &store,
            &actor,
            api::physical::Action::Offer(api::PhysicalOfferEdit {
                offer: Some(invalid)
            })
        ),
        Err(Error::Invalid)
    ));
    let mut stale = offer.clone();
    stale.revision += 1;
    assert!(matches!(
        physical(
            &store,
            &actor,
            api::physical::Action::Offer(api::PhysicalOfferEdit { offer: Some(stale) })
        ),
        Err(Error::Conflict)
    ));
    assert_eq!(store.usage()?, usage);
    drop(store);
    let store = Store::open(dir.path(), policy())?;
    assert_eq!(read(&store, &actor, &offer)?.offers, vec![offer.clone()]);
    // A backup preserves the profile, not fresh proof that its formerly free region is still available.
    let mut image = Vec::new();
    store.export(&mut image)?;
    let restored_dir = tempfile::tempdir()?;
    let restored = Store::restore(restored_dir.path(), &image, policy(), image.len() as u64)?;
    let stale = read(&restored, &actor, &offer)?;
    assert_eq!(stale.offers, vec![offer]);
    assert_eq!(stale.state, api::RequirementState::Unknown as i32);
    assert_eq!(stale.reason, api::RequirementReason::Stale as i32);
    Ok(())
}

// Continuations cover every offer; revocation never frees the finite profile admission budget.
#[test]
fn offer_pages_and_creation_limit_include_revoked_profiles() -> TestResult {
    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), policy())?;
    let actor = operator();
    let rack = create(&store, &actor, K::Rack, "Bounded profiles")?;
    physical(
        &store,
        &actor,
        api::physical::Action::Define(api::PhysicalDefinitionEdit {
            definition: Some(define(rack.clone())),
        }),
    )?;
    let mut created = Vec::new();
    for _ in 0..26 {
        created.push(offer(&store, &actor, &rack)?);
    }
    let first = physical(
        &store,
        &actor,
        api::physical::Action::Offers(api::PhysicalOffers {
            enclosure: Some(rack.clone()),
            after: None,
        }),
    )?;
    assert_eq!(first.offers.len(), 25);
    assert_eq!(
        first.next,
        first.offers.last().and_then(|value| value.id.clone())
    );
    let second = physical(
        &store,
        &actor,
        api::physical::Action::Offers(api::PhysicalOffers {
            enclosure: Some(rack.clone()),
            after: first.next,
        }),
    )?;
    assert_eq!(second.offers.len(), 1);
    assert!(second.next.is_none());
    let mut ids: Vec<_> = first
        .offers
        .iter()
        .chain(&second.offers)
        .map(|value| value.id.clone())
        .collect();
    ids.sort();
    let mut expected: Vec<_> = created.iter().map(|value| value.id.clone()).collect();
    expected.sort();
    assert_eq!(ids, expected);
    for _ in 26..64 {
        offer(&store, &actor, &rack)?;
    }
    let mut revoked = created.remove(0);
    revoked.revoked = true;
    physical(
        &store,
        &actor,
        api::physical::Action::Offer(api::PhysicalOfferEdit {
            offer: Some(revoked),
        }),
    )?;
    let usage = store.usage()?;
    assert!(matches!(offer(&store, &actor, &rack), Err(Error::Limit)));
    assert_eq!(store.usage()?, usage);
    Ok(())
}
