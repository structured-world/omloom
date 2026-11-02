use super::*;
use core::{cmp::Ordering, fmt, str::FromStr};

#[cfg(test)]
mod tests;

#[derive(Clone, Copy)]
pub(super) enum Position {
    Offset(u64),
    Prefix([u64; 2]),
}

impl FromStr for Position {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        let position = if let Some((members, allocations)) = value.split_once(',') {
            Self::Prefix([
                members.parse().map_err(|_| Error::Invalid)?,
                allocations.parse().map_err(|_| Error::Invalid)?,
            ])
        } else {
            Self::Offset(value.parse().map_err(|_| Error::Invalid)?)
        };
        if position.rank()? > 100_000 {
            return Err(Error::Limit);
        }
        Ok(position)
    }
}

impl fmt::Display for Position {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Offset(offset) => write!(formatter, "{offset}"),
            Self::Prefix([members, allocations]) => write!(formatter, "{members},{allocations}"),
        }
    }
}

impl Position {
    pub(super) fn rank(self) -> Result<u64> {
        match self {
            Self::Offset(offset) => Ok(offset),
            Self::Prefix([members, allocations]) => {
                members.checked_add(allocations).ok_or(Error::Limit)
            }
        }
    }

    pub(super) fn max_bytes(self) -> usize {
        match self {
            Self::Offset(_) => 20,
            Self::Prefix(_) => 41,
        }
    }
}

pub(super) struct Selection {
    pub(super) total: u64,
    pub(super) rows: Vec<Properties>,
    pub(super) position: Position,
    origins: Option<Vec<u8>>,
}

impl Selection {
    pub(super) fn advance(&self, emitted: usize) -> Result<Position> {
        debug_assert!(emitted <= self.rows.len());
        Ok(match self.position {
            Position::Offset(offset) => {
                Position::Offset(offset.checked_add(emitted as u64).ok_or(Error::Limit)?)
            }
            Position::Prefix(mut offsets) => {
                let origins = self
                    .origins
                    .as_ref()
                    .expect("prefix pages retain their row origins");
                debug_assert_eq!(origins.len(), self.rows.len());
                for origin in &origins[..emitted] {
                    offsets[usize::from(*origin)] += 1;
                }
                Position::Prefix(offsets)
            }
        })
    }
}

fn compare(left: &Properties, right: &Properties, descending: bool) -> Result<Ordering> {
    let left_order = left.get("ordering").ok_or(Error::Corrupt)?;
    let right_order = right.get("ordering").ok_or(Error::Corrupt)?;
    // Match the database's string/integer ordering and NULL placement, including DESC.
    let order = match (left_order, right_order) {
        (Value::Null, Value::Null) => Ordering::Equal,
        (Value::Null, _) => Ordering::Greater,
        (_, Value::Null) => Ordering::Less,
        (Value::String(left), Value::String(right)) => left.cmp(right),
        (Value::Int(left), Value::Int(right)) => left.cmp(right),
        _ => return Err(Error::Corrupt),
    };
    let order = if descending { order.reverse() } else { order };
    if order != Ordering::Equal {
        return Ok(order);
    }
    let left_key = left
        .get("key")
        .and_then(Value::as_str)
        .ok_or(Error::Corrupt)?;
    let right_key = right
        .get("key")
        .and_then(Value::as_str)
        .ok_or(Error::Corrupt)?;
    Ok(left_key.cmp(right_key))
}

pub(super) fn select(
    tx: &Tx<'_>,
    bases: &[String],
    mut values: HashMap<String, Value>,
    order: &str,
    descending: bool,
    limit: u32,
    position: Position,
) -> Result<Selection> {
    let offsets = match (bases.len(), position) {
        (1, Position::Offset(offset)) => [offset, 0],
        (2, Position::Offset(0)) => [0, 0],
        (2, Position::Prefix(offsets)) => offsets,
        (1 | 2, _) => return Err(Error::Expired),
        _ => return Err(Error::Corrupt),
    };
    let mut totals = [0u64; 2];
    for (index, base) in bases.iter().enumerate() {
        let count = tx.raw(
            &format!("{base} RETURN count(DISTINCT a.key) AS total"),
            values.clone(),
        )?;
        totals[index] = count
            .first()
            .and_then(|row| row.get("total"))
            .and_then(Value::as_int)
            .and_then(|value| u64::try_from(value).ok())
            .ok_or(Error::Corrupt)?;
    }
    // Prefix cohorts have disjoint asset kinds; each count retains its own DISTINCT.
    let total = totals[0].checked_add(totals[1]).ok_or(Error::Limit)?;
    let direction = if descending { "DESC" } else { "ASC" };
    values.insert("limit".into(), int(u64::from(limit))?);
    let read_page = |base: &str, offset: u64, mut parameters: HashMap<String, Value>| {
        parameters.insert("offset".into(), int(offset)?);
        tx.raw(&format!("{base} RETURN DISTINCT a.key AS key, {order} AS ordering ORDER BY ordering {direction}, key ASC SKIP $offset LIMIT $limit"), parameters)
    };
    if bases.len() == 1 {
        let left = if offsets[0] < totals[0] {
            read_page(&bases[0], offsets[0], values)?
        } else {
            Vec::new()
        };
        return Ok(Selection {
            total,
            rows: left,
            position,
            origins: None,
        });
    }
    let left = if offsets[0] < totals[0] {
        read_page(&bases[0], offsets[0], values.clone())?
    } else {
        Vec::new()
    };
    let right = if offsets[1] < totals[1] {
        read_page(&bases[1], offsets[1], values)?
    } else {
        Vec::new()
    };
    let mut left = left.into_iter().peekable();
    let mut right = right.into_iter().peekable();
    let capacity = usize::try_from(u64::from(limit).min(total)).map_err(|_| Error::Limit)?;
    let mut rows = Vec::with_capacity(capacity);
    let mut origins = Vec::with_capacity(capacity);
    while rows.len() < limit as usize {
        let source = match (left.peek(), right.peek()) {
            (Some(left), Some(right)) => {
                if compare(left, right, descending)? == Ordering::Greater {
                    1
                } else {
                    0
                }
            }
            (Some(_), None) => 0,
            (None, Some(_)) => 1,
            (None, None) => break,
        };
        let row = if source == 0 {
            left.next()
        } else {
            right.next()
        };
        rows.push(row.expect("the selected source has a current row"));
        origins.push(source);
    }
    Ok(Selection {
        total,
        rows,
        position: Position::Prefix(offsets),
        origins: Some(origins),
    })
}
