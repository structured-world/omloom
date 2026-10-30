//! Explicit syntax validation never proves delivery, registration or account control.
use super::*;
#[cfg(test)]
mod tests;

pub(super) fn dns_name(value: &str) -> Result<()> {
    responsibility::bounded_value(value, 254, true)?;
    // RFC 1034 section 3.1: an absolute name's final dot denotes the root label.
    let dns = value.strip_suffix('.').unwrap_or(value);
    let ascii = idna::domain_to_ascii_strict(dns).map_err(|_| Error::Invalid)?;
    let name = ascii.as_str();
    if name.is_empty() || name.parse::<std::net::IpAddr>().is_ok() {
        return Err(Error::Invalid);
    }
    // PSL parsing must not turn DNS syntax into a registration requirement:
    // private zones and public suffix nodes are valid DNS names too.
    structured_public_domains::lookup(name).ok_or(Error::Invalid)?;
    Ok(())
}
pub(super) fn locator(kind: i32, value: &str) -> Result<()> {
    responsibility::bounded_value(value, 512, true)?;
    match api::LocatorKind::try_from(kind).map_err(|_| Error::Invalid)? {
        api::LocatorKind::Unspecified => Ok(()),
        api::LocatorKind::Email => {
            if structured_email_address::is_smtp_mailbox(value) {
                Ok(())
            } else {
                Err(Error::Invalid)
            }
        }
        api::LocatorKind::Phone => {
            let number = phonenumber::parse(None, value).map_err(|_| Error::Invalid)?;
            if phonenumber::is_valid(&number)
                && number.format().mode(phonenumber::Mode::E164).to_string() == value
            {
                Ok(())
            } else {
                Err(Error::Invalid)
            }
        }
        api::LocatorKind::Web => {
            let url = url::Url::parse(value).map_err(|_| Error::Invalid)?;
            if matches!(url.scheme(), "http" | "https")
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
            {
                Ok(())
            } else {
                Err(Error::Invalid)
            }
        }
        api::LocatorKind::Domain => dns_name(value),
    }
}

pub(super) fn read_kind(row: &Properties, name: &str) -> Result<i32> {
    let kind = match row.get(name) {
        None | Some(Value::Null) => api::LocatorKind::Unspecified as i32,
        Some(Value::Int(value)) => i32::try_from(*value).map_err(|_| Error::Corrupt)?,
        _ => return Err(Error::Corrupt),
    };
    api::LocatorKind::try_from(kind).map_err(|_| Error::Corrupt)?;
    Ok(kind)
}
