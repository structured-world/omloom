use super::*;

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
                occupants: true
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
