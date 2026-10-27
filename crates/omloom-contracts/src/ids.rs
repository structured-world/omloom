//! Omloom-owned nominal identifiers. No SID identifier is defined here.
use crate::Error;
use std::{fmt, str::FromStr};
use uuid::{Uuid, Variant};

fn canonical(text: &str) -> Result<Uuid, Error> {
    // RFC 9562 §§4, 5.7: this API selects lowercase hyphenated UUIDv7 text.
    if text.len() != 36 {
        return Err(Error::Identifier);
    }
    let id = Uuid::try_parse(text).map_err(|_| Error::Identifier)?;
    if id.get_variant() != Variant::RFC4122 || id.get_version_num() != 7 {
        return Err(Error::Identifier);
    }
    let mut buffer = Uuid::encode_buffer();
    if id.hyphenated().encode_lower(&mut buffer) != text {
        return Err(Error::Identifier);
    }
    Ok(id)
}

macro_rules! id {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(Uuid);
        impl $name {
            pub fn generate() -> Self {
                Self(Uuid::now_v7())
            }
            pub fn parse(text: &str) -> Result<Self, Error> {
                canonical(text).map(Self)
            }
        }
        impl FromStr for $name {
            type Err = Error;
            fn from_str(text: &str) -> Result<Self, Error> {
                Self::parse(text)
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}
id!(AssetId);
id!(IncarnationId);
id!(ConnectionId);
id!(CandidateId);
id!(SourceId);
id!(AuthorityId);
id!(RelationId);
id!(OperationId);
id!(ScanId);
id!(TrustId);
id!(CredentialBindingId);

pub(crate) fn check(text: &str) -> Result<(), Error> {
    canonical(text).map(|_| ())
}

/// Nominal identity used by host domain code; equal names/provider IDs do not join it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AssetRef {
    pub asset: AssetId,
    pub incarnation: IncarnationId,
}
impl TryFrom<&crate::wire::AssetRef> for AssetRef {
    type Error = Error;
    fn try_from(value: &crate::wire::AssetRef) -> Result<Self, Error> {
        Ok(Self {
            asset: value.asset_id.parse()?,
            incarnation: value.incarnation.parse()?,
        })
    }
}
