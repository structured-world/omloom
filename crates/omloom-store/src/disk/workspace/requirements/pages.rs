use super::*;

fn hash_message(hash: &mut Sha256, message: &impl Message, bytes: &mut Vec<u8>) -> Result<()> {
    bytes.clear();
    message.encode(bytes).map_err(|_| Error::Corrupt)?;
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes.as_slice());
    Ok(())
}

/// Stateless continuation rechecks authority and content, without retaining database snapshots.
pub(super) fn page(
    mut value: api::RequirementEvaluation,
    request: &api::EvaluateRequirements,
    working: &api::WorkingContext,
    cursor: Option<&str>,
) -> Result<api::RequirementEvaluation> {
    if cursor.is_none() && value.encoded_len() + 6 <= MAX_REPLY_BYTES {
        return Ok(value);
    }
    let mut hash = Sha256::new();
    for bytes in [request.encode_to_vec(), working.encode_to_vec()] {
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    hash.update(value.revision.to_le_bytes());
    hash.update(value.state.to_le_bytes());
    hash.update(value.reason.to_le_bytes());
    let mut evidence = Vec::with_capacity(value.results.len());
    let mut encoded = Vec::new();
    for node in &mut value.results {
        node.evidence.sort_by(|a, b| a.source.cmp(&b.source));
        let sources = std::mem::take(&mut node.evidence);
        hash_message(&mut hash, node, &mut encoded)?;
        hash.update((sources.len() as u32).to_le_bytes());
        for source in &sources {
            hash_message(&mut hash, source, &mut encoded)?;
        }
        evidence.push(sources);
    }
    let basis = format!("{:x}", hash.finalize());
    let offset = if let Some(cursor) = cursor {
        if cursor.len() > 70 {
            return Err(Error::Invalid);
        }
        let (old, offset) = cursor.split_once(':').ok_or(Error::Invalid)?;
        if old != basis {
            return Err(Error::Conflict);
        }
        let offset: usize = offset.parse().map_err(|_| Error::Invalid)?;
        if offset == 0 || offset >= evidence.iter().map(Vec::len).sum() {
            return Err(Error::Invalid);
        }
        offset
    } else {
        0
    };
    // 128 covers the bounded cursor and outer Reply envelope. Twelve bytes per
    // source cover its tag/length and growth of the enclosing node's length prefix.
    let mut remaining = MAX_REPLY_BYTES
        .checked_sub(value.encoded_len() + 128)
        .ok_or(Error::Limit)?;
    let mut index = 0;
    let mut full = false;
    for (node, sources) in value.results.iter_mut().zip(evidence) {
        for source in sources {
            if index < offset {
                index += 1;
                continue;
            }
            let bytes = source.encoded_len() + 12;
            if full || bytes > remaining {
                full = true;
                continue;
            }
            remaining -= bytes;
            node.evidence.push(source);
            index += 1;
        }
    }
    if full {
        if index == offset {
            return Err(Error::Limit);
        }
        value.next = Some(format!("{basis}:{index}"));
    }
    Ok(value)
}
