use super::*;

fn volume(x: i64, y: i64, z: i64, width: u64, height: u64, depth: u64) -> Result<Volume, Invalid> {
    Ok(Volume {
        x: Span::new(x, width)?,
        y: Span::new(y, height)?,
        z: Span::new(z, depth)?,
    })
}
fn body(value: Volume) -> Result<Region, Invalid> {
    Region::new(value, value)
}

// A rear device may share U space only when its real depth does not intersect.
#[test]
fn opposing_faces_and_half_width_use_actual_volumes() -> Result<(), Invalid> {
    let rack = volume(0, 0, 0, 500_000, 2_000_000, 1_000_000)?;
    let front = volume(0, 0, 0, 250_000, 44_450, 400_000)?;
    let rear = volume(0, 0, 600_000, 500_000, 44_450, 400_000)?;
    let sibling = volume(250_000, 0, 0, 250_000, 44_450, 400_000)?;
    assert_eq!(
        placement(Some(rack), Some(body(rear)?), &[body(front)?]),
        Decision::PASS
    );
    assert_eq!(
        placement(Some(rack), Some(body(sibling)?), &[body(front)?]),
        Decision::PASS
    );
    let full = volume(0, 0, 0, 500_000, 44_450, 1_000_000)?;
    assert_eq!(
        placement(Some(rack), Some(body(rear)?), &[body(full)?]),
        Decision::FAIL
    );
    Ok(())
}

// Service clearance and zero-U obstruction are volumes, not special exemptions.
#[test]
fn clearance_and_zero_u_obstructions_conflict() -> Result<(), Invalid> {
    let rack = volume(-50_000, 0, 0, 600_000, 2_000_000, 1_000_000)?;
    let pdu = volume(0, 0, 950_000, 50_000, 1_800_000, 50_000)?;
    let device = volume(0, 0, 0, 500_000, 44_450, 900_000)?;
    assert_eq!(
        placement(Some(rack), Some(body(device)?), &[body(pdu)?]),
        Decision::PASS
    );
    let service = Volume {
        z: device.z.expand(0, 100_000)?,
        ..device
    };
    assert_eq!(
        placement(
            Some(rack),
            Some(Region::new(device, service)?),
            &[body(pdu)?]
        ),
        Decision::FAIL
    );
    Ok(())
}

// Two service envelopes may share empty maintenance space; neither may intersect the other material body.
#[test]
fn service_envelopes_are_not_duplicate_material_occupancy() -> Result<(), Invalid> {
    let room = volume(0, 0, 0, 100, 100, 100)?;
    let first = Region::new(volume(0, 0, 0, 10, 10, 10)?, volume(0, 0, 0, 20, 10, 10)?)?;
    let second = Region::new(volume(20, 0, 0, 10, 10, 10)?, volume(10, 0, 0, 20, 10, 10)?)?;
    assert_eq!(
        placement(Some(room), Some(second), &[first]),
        Decision::PASS
    );
    assert!(matches!(
        Region::new(room, volume(0, 0, 0, 10, 10, 10)?),
        Err(Invalid::Reversed)
    ));
    Ok(())
}

// Unknown dimensions cannot become a positive fit or an assumed zero load.
#[test]
fn missing_geometry_and_uncertain_capacity_remain_unknown() -> Result<(), Invalid> {
    let rack = volume(0, 0, 0, 500_000, 2_000_000, 1_000_000)?;
    assert_eq!(
        placement(Some(rack), None, &[]),
        Decision::unknown(Reason::Missing)
    );
    assert_eq!(
        capacity(None, Some(Quantity::new(0, 10)?)),
        Decision::unknown(Reason::Missing)
    );
    assert_eq!(
        capacity(
            Some(Quantity::new(100, 120)?),
            Some(Quantity::new(90, 110)?)
        ),
        Decision::unknown(Reason::Incomplete)
    );
    assert_eq!(
        capacity(
            Some(Quantity::new(100, 120)?),
            Some(Quantity::new(121, 130)?)
        ),
        Decision::FAIL
    );
    assert_eq!(
        capacity(
            Some(Quantity::new(100, 120)?),
            Some(Quantity::new(90, 100)?)
        ),
        Decision::PASS
    );
    Ok(())
}

// Invalid units/ranges and arithmetic overflow are rejected before geometry work.
#[test]
fn coordinate_and_capacity_boundaries_are_checked() -> Result<(), Invalid> {
    assert_eq!(Span::new(0, 0), Err(Invalid::Empty));
    assert_eq!(Span::new(i64::MAX, 1), Err(Invalid::Overflow));
    assert_eq!(Quantity::new(2, 1), Err(Invalid::Reversed));
    assert_eq!(
        Quantity::new(u64::MAX, u64::MAX)?.checked_sum(Quantity::new(1, 1)?),
        Err(Invalid::Overflow)
    );
    let first = Span::new(-100, 100)?;
    assert!(!first.overlaps(Span::new(0, 100)?));
    assert!(first.overlaps(Span::new(-1, 100)?));
    Ok(())
}

// Labels cannot establish fit: only an exact declaration with complementary sides can satisfy it.
#[test]
fn mating_contract_keeps_unknown_and_incompatible_distinct() -> Result<(), Invalid> {
    let a = Mating::new("private:test", "pair", "revision-1", MatingRole::SideA)?;
    let b = Mating::new("private:test", "pair", "revision-1", MatingRole::SideB)?;
    let changed = Mating::new("private:test", "pair", "revision-2", MatingRole::SideB)?;
    assert_eq!(mating(Some(a), Some(b)), Decision::PASS);
    assert_eq!(mating(Some(a), Some(a)), Decision::FAIL);
    assert_eq!(mating(Some(a), None), Decision::unknown(Reason::Missing));
    assert_eq!(
        mating(Some(a), Some(changed)),
        Decision::unknown(Reason::Unconfigured)
    );
    assert!(matches!(
        Mating::new("", "pair", "revision-1", MatingRole::SideA),
        Err(Invalid::Empty)
    ));
    Ok(())
}
