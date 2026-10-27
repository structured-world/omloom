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
mod observe;
#[cfg(test)]
mod tests;

pub mod transport {
    tonic::include_proto!("omloom.workspace.v1");
}
pub const FRAME_LIMIT: usize = omloom_store::workspace::MAX_REPLY_BYTES;

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
    observations: Arc<Semaphore>,
}
impl Application {
    pub fn new(store: Store, context: Context) -> Self {
        Self {
            store: Arc::new(store),
            context,
            work: Arc::new(Semaphore::new(8)),
            observations: Arc::new(Semaphore::new(2)),
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
        let mut received = request.into_inner();
        let mut decoded = received.admitted.take();
        if matches!(
            decoded.command,
            Some(api::request::Command::ObserveProxmox(_))
        ) {
            let _observation = Arc::clone(&self.observations).try_acquire_owned().map_err(
                |_| {
                    status(
                        Code::ResourceExhausted,
                        "OBSERVE_BUSY",
                        "Two observations are already running. Inventory reads remain available.",
                    )
                },
            )?;
            if !context.grants.iter().any(|g| g.writable) {
                return Err(status(
                    Code::PermissionDenied,
                    "OBSERVE_DENIED",
                    "This fixture cannot start provider observations.",
                ));
            }
            let Some(api::request::Command::ObserveProxmox(probe)) = decoded.command.take() else {
                return Err(status(
                    Code::Internal,
                    "OBSERVE_COMMAND",
                    "Observation request is unavailable.",
                ));
            };
            let observation = observe::probe(probe).await?;
            let result = api::Reply {
                result: Some(api::reply::Result::Observation(observation)),
            };
            if prost::Message::encoded_len(&result) > FRAME_LIMIT {
                return Err(status(
                    Code::ResourceExhausted,
                    "OBSERVE_LIMIT",
                    "Narrow the provider observation scope.",
                ));
            }
            drop(permit);
            return Ok(Response::new(result));
        }
        let result = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let _received = received;
            store.workspace(&context, decoded, now)
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
fn loopback_authority(value: &str, port: u16) -> bool {
    if value.contains('@') {
        return false;
    }
    let Ok(authority) = value.parse::<axum::http::uri::Authority>() else {
        return false;
    };
    authority.port_u16() == Some(port)
        && (authority.host() == "127.0.0.1" || authority.host().eq_ignore_ascii_case("localhost"))
}

fn loopback_request(request: &axum::extract::Request, port: u16) -> bool {
    let authority = request.uri().authority();
    if let Some(authority) = authority
        && !loopback_authority(authority.as_str(), port)
    {
        return false;
    }
    let mut hosts = request.headers().get_all(axum::http::header::HOST).iter();
    if let Some(host) = hosts.next() {
        hosts.next().is_none()
            && host
                .to_str()
                .is_ok_and(|host| loopback_authority(host, port))
    } else {
        authority.is_some()
    }
}

pub fn router(application: Application, ui: &std::path::Path, port: u16) -> axum::Router {
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
        // A remote website rebinding its DNS must not acquire the local fixture's authority.
        .layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| async move {
                if loopback_request(&request, port) {
                    next.run(request).await
                } else {
                    axum::response::IntoResponse::into_response(
                        axum::http::StatusCode::MISDIRECTED_REQUEST,
                    )
                }
            },
        ))
}
