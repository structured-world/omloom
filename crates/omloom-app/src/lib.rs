//! Explicit loopback development host. Fixture authority is selected at startup,
//! not inferred from an unauthenticated request or used as a production fallback.
#![deny(unsafe_code, clippy::unwrap_used)]

use omloom_contracts::{wire::AssetKind, workspace as api};
use omloom_store::{
    Budget, CLASSES, Error, Policy, Store,
    workspace::{Context, Fields, Grant, Scope},
};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::Semaphore;
use tonic::{Code, Request, Response, Status};
use tonic_types::{ErrorDetails, StatusExt};
pub mod codec;
pub mod fixtures;

pub mod transport {
    tonic::include_proto!("omloom.workspace.v1");
}
pub const FRAME_LIMIT: usize = 512 * 1024;

pub fn policy() -> Policy {
    Policy {
        classes: [Budget {
            rows: 200_000,
            bytes: 128 * 1_048_576,
        }; CLASSES],
        total_bytes: 512 * 1_048_576,
        detail_age_ms: 7 * 86_400_000,
        review_age_ms: 30 * 60_000,
        review_per_source: Budget {
            rows: 512,
            bytes: 2 * 1_048_576,
        },
        batch_rows: 128,
    }
}
pub fn fixture(name: &str) -> Result<Context, &'static str> {
    let (scope, writable, schema_admin, teams) = match name {
        "operator" => (Scope::All, true, true, vec!["operations", "engineering"]),
        "developer" => (
            Scope::Kinds(vec![AssetKind::VirtualMachine]),
            true,
            false,
            vec!["development"],
        ),
        "devops" => (Scope::All, true, false, vec!["operations"]),
        "engineer" => (
            Scope::Kinds(vec![
                AssetKind::Device,
                AssetKind::Storage,
                AssetKind::Interface,
            ]),
            true,
            false,
            vec!["engineering"],
        ),
        "viewer" => (Scope::All, false, false, vec!["operations"]),
        _ => return Err("Select operator, developer, devops, engineer or viewer"),
    };
    Ok(Context {
        actor: format!("fixture-{name}"),
        fixture: name.into(),
        grants: vec![Grant {
            scope,
            read: Fields::All,
            write: Fields::All,
            writable,
        }],
        teams: teams.into_iter().map(str::to_owned).collect(),
        recipients: ["operator", "developer", "devops", "engineer", "viewer"]
            .map(|n| format!("fixture-{n}"))
            .into(),
        schema_admin,
        authority_revision: 1,
    })
}
fn status(code: Code, reason: &str, message: &str) -> Status {
    let mut details = ErrorDetails::new();
    details.set_error_info(reason, "omloom.com", HashMap::new());
    Status::with_error_details(code, message, details)
}
fn failure(error: Error) -> Status {
    let (code, reason, message) = match error {
        Error::Invalid => (
            Code::InvalidArgument,
            "INVALID_INPUT",
            "Check the selected types, fields and required values.",
        ),
        Error::Denied => (
            Code::PermissionDenied,
            "SCOPE_DENIED",
            "This action is outside the current fixture scope.",
        ),
        Error::Missing => (
            Code::NotFound,
            "NOT_FOUND",
            "The selected object is unavailable.",
        ),
        Error::Conflict => (
            Code::Aborted,
            "REVISION_CONFLICT",
            "State changed or the allocation is already held. Reload and review your changes.",
        ),
        Error::Expired => (
            Code::FailedPrecondition,
            "EXPIRED",
            "The selection or preview expired. Refresh it before applying.",
        ),
        Error::Incomplete => (
            Code::FailedPrecondition,
            "UNAVAILABLE_FIELD",
            "A selected field or predicate is unavailable. Repair it explicitly.",
        ),
        Error::Limit => (
            Code::ResourceExhausted,
            "RESOURCE_LIMIT",
            "The bounded operation or storage limit was reached. Narrow the selection.",
        ),
        Error::RecoveryRequired => (
            Code::FailedPrecondition,
            "RESTORE_REVIEW",
            "This restored store needs recovery review before accepting changes.",
        ),
        Error::UnsafeTransition => (
            Code::FailedPrecondition,
            "UNRESOLVED_WORK",
            "Unresolved work prevents this change.",
        ),
        _ => (
            Code::Internal,
            "STORAGE_FAILURE",
            "The store could not complete this request. No partial result is accepted.",
        ),
    };
    status(code, reason, message)
}
#[derive(Clone)]
pub struct Application {
    store: Arc<Store>,
    context: Context,
    work: Arc<Semaphore>,
}
impl Application {
    pub fn new(store: Store, context: Context) -> Self {
        Self {
            store: Arc::new(store),
            context,
            work: Arc::new(Semaphore::new(8)),
        }
    }
    /// One bounded page per interval. Cursors advance only after a successful
    /// transaction; restarting safely rescans rather than losing cleanup work.
    pub async fn maintain(self, mut shutdown: tokio::sync::watch::Receiver<bool>) {
        let mut detail = None;
        let mut review = None;
        let mut definitions = None;
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            tokio::select! {
                result = shutdown.changed() => { if result.is_err() || *shutdown.borrow() { break; } }
                _ = interval.tick() => {
                    let Ok(permit) = Arc::clone(&self.work).try_acquire_owned() else { continue };
                    let store = Arc::clone(&self.store);
                    let previous_detail = detail.clone();
                    let previous_review = review.clone();
                    let previous_definitions = definitions.clone();
                    let result = tokio::task::spawn_blocking(move || {
                        let _permit = permit;
                        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| Error::Invalid)?.as_millis();
                        let now = u64::try_from(now).map_err(|_| Error::Invalid)?;
                        let d = store.cleanup(now, previous_detail.as_deref())?;
                        let r = store.cleanup_reviews(now, previous_review.as_deref())?;
                        let definitions = store.workspace_cleanup_definitions(16, previous_definitions.as_deref())?;
                        Ok::<_, Error>((d.cursor, r.cursor, definitions.cursor))
                    }).await;
                    match result {
                        Ok(Ok((d, r, f))) => { detail = d; review = r; definitions = f; }
                        Ok(Err(error)) => eprintln!("Cleanup did not complete; cursor retained: {error}"),
                        Err(error) => eprintln!("Cleanup task failed; cursor retained: {error}"),
                    }
                }
            }
        }
    }
}
#[tonic::async_trait]
impl transport::workspace_server::Workspace for Application {
    async fn call(
        &self,
        request: Request<codec::Received>,
    ) -> Result<Response<api::Reply>, Status> {
        let permit = Arc::clone(&self.work).try_acquire_owned().map_err(|_| {
            status(
                Code::ResourceExhausted,
                "BUSY",
                "The local host is busy. Retry this request.",
            )
        })?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| status(Code::Internal, "CLOCK", "The host clock is invalid."))?
            .as_millis();
        let now = u64::try_from(now)
            .map_err(|_| status(Code::Internal, "CLOCK", "The host clock is out of range."))?;
        let store = Arc::clone(&self.store);
        let context = self.context.clone();
        let result = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let mut received = request.into_inner();
            store.workspace(&context, received.admitted.take(), now)
        })
        .await
        .map_err(|_| {
            status(
                Code::Internal,
                "WORKER_FAILURE",
                "The local operation failed.",
            )
        })?
        .map_err(failure)?;
        Ok(Response::new(result))
    }
}
pub fn router(application: Application, ui: &std::path::Path) -> axum::Router {
    let service = transport::workspace_server::WorkspaceServer::new(application)
        .max_decoding_message_size(FRAME_LIMIT)
        .max_encoding_message_size(FRAME_LIMIT);
    tonic::service::Routes::new(service)
        .into_axum_router()
        .layer(tonic_web::GrpcWebLayer::new())
        .fallback_service(tower_http::services::ServeDir::new(ui))
        .layer(tower_http::limit::RequestBodyLimitLayer::new(
            FRAME_LIMIT + 5,
        ))
        .layer(tower_http::set_header::SetResponseHeaderLayer::overriding(
            axum::http::header::CACHE_CONTROL,
            axum::http::HeaderValue::from_static("no-store"),
        ))
}
