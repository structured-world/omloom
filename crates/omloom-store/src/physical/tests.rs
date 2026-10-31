use super::*;

fn volume(x: i64, y: i64, z: i64, width: u64, height: u64, depth: u64) -> Volume {
    Volume {
        x: Span::new(x, width).unwrap(),
        y: Span::new(y, height).unwrap(),
        z: Span::new(z, depth).unwrap(),
    }
}

// A rear device may share U space only when its real depth does not intersect.
#[test]
fn opposing_faces_and_half_width_use_actual_volumes() {
    let rack = volume(0, 0, 0, 500_000, 2_000_000, 1_000_000);
    let front = volume(0, 0, 0, 250_000, 44_450, 400_000);
    let rear = volume(0, 0, 600_000, 500_000, 44_450, 400_000);
    let sibling = volume(250_000, 0, 0, 250_000, 44_450, 400_000);
    assert_eq!(placement(Some(rack), Some(rear), &[front]), Decision::PASS);
    assert_eq!(
        placement(Some(rack), Some(sibling), &[front]),
        Decision::PASS
    );
    let full = volume(0, 0, 0, 500_000, 44_450, 1_000_000);
    assert_eq!(placement(Some(rack), Some(rear), &[full]), Decision::FAIL);
}

// Service clearance and zero-U obstruction are volumes, not special exemptions.
#[test]
fn clearance_and_zero_u_obstructions_conflict() {
    let rack = volume(-50_000, 0, 0, 600_000, 2_000_000, 1_000_000);
    let pdu = volume(0, 0, 950_000, 50_000, 1_800_000, 50_000);
    let device = volume(0, 0, 0, 500_000, 44_450, 900_000);
    assert_eq!(placement(Some(rack), Some(device), &[pdu]), Decision::PASS);
    let service = Volume {
        z: device.z.expand(0, 100_000).unwrap(),
        ..device
    };
    assert_eq!(placement(Some(rack), Some(service), &[pdu]), Decision::FAIL);
}

// Unknown dimensions cannot become a positive fit or an assumed zero load.
#[test]
fn missing_geometry_and_uncertain_capacity_remain_unknown() {
    let rack = volume(0, 0, 0, 500_000, 2_000_000, 1_000_000);
    assert_eq!(
        placement(Some(rack), None, &[]),
        Decision::unknown(Reason::Missing)
    );
    assert_eq!(
        capacity(None, Some(Quantity::new(0, 10).unwrap())),
        Decision::unknown(Reason::Missing)
    );
    assert_eq!(
        capacity(
            Some(Quantity::new(100, 120).unwrap()),
            Some(Quantity::new(90, 110).unwrap())
        ),
        Decision::unknown(Reason::Incomplete)
    );
    assert_eq!(
        capacity(
            Some(Quantity::new(100, 120).unwrap()),
            Some(Quantity::new(121, 130).unwrap())
        ),
        Decision::FAIL
    );
    assert_eq!(
        capacity(
            Some(Quantity::new(100, 120).unwrap()),
            Some(Quantity::new(90, 100).unwrap())
        ),
        Decision::PASS
    );
}

// Invalid units/ranges and arithmetic overflow are rejected before geometry work.
#[test]
fn coordinate_and_capacity_boundaries_are_checked() {
    assert_eq!(Span::new(0, 0), Err(Invalid::Empty));
    assert_eq!(Span::new(i64::MAX, 1), Err(Invalid::Overflow));
    assert_eq!(Quantity::new(2, 1), Err(Invalid::Reversed));
    assert_eq!(
        Quantity::new(u64::MAX, u64::MAX)
            .unwrap()
            .checked_sum(Quantity::new(1, 1).unwrap()),
        Err(Invalid::Overflow)
    );
    let first = Span::new(-100, 100).unwrap();
    assert!(!first.overlaps(Span::new(0, 100).unwrap()));
    assert!(first.overlaps(Span::new(-1, 100).unwrap()));
}
