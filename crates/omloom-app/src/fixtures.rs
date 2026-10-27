//! Trusted development-host configuration, not a browser permission assertion.
use omloom_contracts::{
    ids::AssetRef,
    wire::{self, AssetKind},
};
use omloom_store::workspace::{Context, Fields, Grant, Scope};
use serde::Deserialize;
use std::{error::Error, io::Read, path::Path};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reference {
    asset_id: String,
    incarnation: String,
}
#[derive(Deserialize)]
#[serde(
    tag = "mode",
    content = "fields",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum FieldSelection {
    All,
    Selected(Vec<String>),
}
impl FieldSelection {
    fn into_fields(self) -> Result<Fields, Box<dyn Error>> {
        match self {
            Self::All => Ok(Fields::All),
            Self::Selected(fields) => {
                if fields.len() > 32 || fields.iter().any(|s| s.is_empty() || s.len() > 100) {
                    return Err("Invalid fixture field selection".into());
                }
                Ok(Fields::Selected(fields))
            }
        }
    }
}
#[derive(Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case", deny_unknown_fields)]
enum Selection {
    All,
    Kinds { kinds: Vec<String> },
    Targets { targets: Vec<Reference> },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigGrant {
    selection: Selection,
    read: FieldSelection,
    write: FieldSelection,
    writable: bool,
}

/// Replaces the compiled fixture's grant list from an explicit host-owned file.
/// It cannot change the synthetic actor identity or turn this host into SID.
pub fn configure(context: &mut Context, path: &Path) -> Result<(), Box<dyn Error>> {
    let file = std::fs::File::open(path)?;
    let mut bytes = Vec::with_capacity(65_537);
    file.take(65_537).read_to_end(&mut bytes)?;
    if bytes.len() > 65_536 {
        return Err("Fixture grant file exceeds 64 KiB".into());
    }
    let entries: Vec<ConfigGrant> = serde_json::from_slice(&bytes)?;
    if entries.is_empty() || entries.len() > 32 {
        return Err("Fixture needs 1-32 grants".into());
    }
    let mut grants = Vec::with_capacity(entries.len());
    for entry in entries {
        let scope = match entry.selection {
            Selection::All => Scope::All,
            Selection::Kinds { kinds } => {
                if kinds.is_empty() || kinds.len() > 32 {
                    return Err("Invalid fixture kind count".into());
                }
                let kinds = kinds
                    .iter()
                    .map(|name| {
                        AssetKind::from_str_name(name)
                            .filter(|k| *k != AssetKind::Unspecified)
                            .ok_or("Unknown fixture resource kind")
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Scope::Kinds(kinds)
            }
            Selection::Targets { targets } => {
                if targets.is_empty() || targets.len() > 1024 {
                    return Err("Invalid fixture target count".into());
                }
                let targets = targets
                    .into_iter()
                    .map(|r| {
                        AssetRef::try_from(&wire::AssetRef {
                            asset_id: r.asset_id,
                            incarnation: r.incarnation,
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Scope::Targets(targets)
            }
        };
        grants.push(Grant {
            scope,
            read: entry.read.into_fields()?,
            write: entry.write.into_fields()?,
            writable: entry.writable,
        });
    }
    context.grants = grants;
    Ok(())
}
