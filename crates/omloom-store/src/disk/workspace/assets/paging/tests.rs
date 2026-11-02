use super::*;

// Continuation must not consume selected rows omitted by the reply byte budget.
#[test]
fn prefix_continuation_advances_only_emitted_rows() -> Result<()> {
    let selected = Selection {
        total: 40,
        rows: vec![Properties::new(); 3],
        position: Position::Prefix([7, 11]),
        origins: Some(vec![1, 0, 1]),
    };
    assert_eq!(selected.advance(0)?.to_string(), "7,11");
    assert_eq!(selected.advance(1)?.to_string(), "7,12");
    assert_eq!(selected.advance(2)?.to_string(), "8,12");
    assert_eq!(selected.advance(3)?.to_string(), "8,13");
    Ok(())
}

// Both source positions share the existing pagination ceiling and reject malformed input.
#[test]
fn positions_reject_invalid_or_unbounded_offsets() -> Result<()> {
    let position: Position = "1,2".parse()?;
    assert_eq!(position.rank()?, 3);
    assert_eq!(position.to_string(), "1,2");
    assert_eq!("100000".parse::<Position>()?.rank()?, 100_000);
    for invalid in ["", "-1", "1,2,3", "1,100000", "18446744073709551615,1"] {
        assert!(invalid.parse::<Position>().is_err(), "{invalid}");
    }
    Ok(())
}
