use std::{path::Path, sync::Arc};

use coordinode_embed::Database;
use omloom_app::{fixture, identity, policy};
use omloom_contracts::{wire, workspace as api};
use omloom_store::Store;
use sid::{Installation, InstallationSettings};
use sid_plugin::cache::InMemoryCacheBackend;

async fn open(
    path: &Path,
    master_key: &Path,
) -> Result<(Store, Installation), Box<dyn std::error::Error + Send + Sync>> {
    let db = Arc::new(Database::open(path)?);
    identity::attach(
        db,
        policy(),
        InstallationSettings {
            issuer: "https://inventory.example.com".parse()?,
            master_key: master_key.to_owned(),
            crypto: Arc::new(sid_crypto::RustCryptoPrimitives::new()),
            cache: Arc::new(InMemoryCacheBackend::new()),
            access_token_ttl: Default::default(),
            management: None,
            sod_warnings: Vec::new(),
        },
    )
    .await
}

/// Identity and inventory share one disk database and retain their state on reopen.
#[tokio::test]
async fn embedded_sid_reopens_alongside_inventory()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    sid_crypto::jwt::rustcrypto::provider()
        .install_default()
        .map_err(|_| "A JWT provider was already installed")?;
    sid_crypto::tls::rustcrypto::provider()
        .install_default()
        .map_err(|_| "A TLS provider was already installed")?;
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("db");
    let master_key = directory.path().join("sid-master");
    let (inventory, installation) = open(&path, &master_key).await?;
    let context = fixture("operator")?;
    let created = inventory.workspace(
        &context,
        api::Request {
            command: Some(api::request::Command::Write(api::Write {
                kind: wire::AssetKind::Device as i32,
                changes: vec![wire::Field {
                    name: "name".into(),
                    value: Some(wire::field::Value::Text("Shared host".into())),
                }],
                ..Default::default()
            })),
            working: Some(api::WorkingContext::default()),
        },
        1,
    )?;
    let Some(api::reply::Result::Mutation(created)) = created.result else {
        return Err("Inventory creation returned no mutation".into());
    };
    let target = created.targets.into_iter().next().ok_or("Missing device")?;
    let usage = inventory.usage()?;
    let organization = installation.organization.id;
    let issuer = installation.issuer.id;
    assert!(installation.admin_claim().await?.is_some());
    installation.close().await;
    drop(inventory);

    let (inventory, installation) = open(&path, &master_key).await?;
    assert_eq!(inventory.usage()?, usage);
    assert_eq!(installation.organization.id, organization);
    assert_eq!(installation.issuer.id, issuer);
    assert!(installation.admin_claim().await?.is_some());
    let restored = inventory.workspace(
        &context,
        api::Request {
            command: Some(api::request::Command::Read(api::Read {
                target: Some(target.clone()),
                ..Default::default()
            })),
            working: Some(api::WorkingContext::default()),
        },
        2,
    )?;
    let Some(api::reply::Result::Detail(restored)) = restored.result else {
        return Err("Inventory read returned no detail".into());
    };
    assert_eq!(restored.item.and_then(|item| item.target), Some(target));
    installation.close().await;
    Ok(())
}
