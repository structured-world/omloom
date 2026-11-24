//! Trusted host composition for inventory and the embedded identity engine.

use std::sync::Arc;

use coordinode_embed::Database;
use omloom_store::{Policy, Store};
use sid::{Installation, InstallationSettings};
use sid_storage::coordinode::CoordinodeBackend;

/// Initializes both schemas on the host's configured database handle. The
/// returned installation is for host lifecycle management before serving.
pub async fn attach(
    db: Arc<Database>,
    policy: Policy,
    settings: InstallationSettings,
) -> Result<(Store, Installation), Box<dyn std::error::Error + Send + Sync>> {
    let inventory = {
        let db = Arc::clone(&db);
        tokio::task::spawn_blocking(move || Store::from_database(db, policy)).await??
    };
    let backend = CoordinodeBackend::new(db).await?;
    let installation = Installation::open(Arc::new(backend), settings).await?;
    Ok((inventory, installation))
}
