use super::*;

pub(super) fn declaration(value: &api::PhysicalMating) -> Result<crate::physical::Mating<'_>> {
    use crate::physical::MatingRole;
    let profile = value.profile.as_ref().ok_or(Error::Invalid)?;
    let role = match api::PhysicalMatingRole::try_from(value.role).map_err(|_| Error::Invalid)? {
        api::PhysicalMatingRole::SideA => MatingRole::SideA,
        api::PhysicalMatingRole::SideB => MatingRole::SideB,
        api::PhysicalMatingRole::SelfMating => MatingRole::SelfMating,
        _ => return Err(Error::Invalid),
    };
    crate::physical::Mating::new(&profile.issuer, &profile.key, &profile.basis, role)
        .map_err(|_| Error::Invalid)
}
pub(super) fn store(row: &mut Properties, value: Option<&api::PhysicalMating>) -> Result<()> {
    if let Some(value) = value {
        declaration(value)?;
        if value.encoded_len() > 512 {
            return Err(Error::Limit);
        }
        // The whole small mating declaration is consumed together by one primary-key read.
        // It has no independently indexed fields, samples or version stream.
        row.insert("payload".into(), Value::Binary(value.encode_to_vec()));
    } else {
        row.remove("payload");
    }
    Ok(())
}
pub(super) fn read(row: &Properties) -> Result<Option<api::PhysicalMating>> {
    if !row.contains_key("payload") {
        return Ok(None);
    }
    let bytes = binary(row, "payload")?;
    if bytes.len() > 512 {
        return Err(Error::Corrupt);
    }
    let value = api::PhysicalMating::decode(bytes).map_err(|_| Error::Corrupt)?;
    declaration(&value).map_err(|_| Error::Corrupt)?;
    Ok(Some(value))
}
