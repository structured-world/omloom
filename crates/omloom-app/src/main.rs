use omloom_app::{Application, fixture, policy, router};
use omloom_store::Store;
use std::{
    net::{Ipv4Addr, SocketAddr},
    path::PathBuf,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut data = None;
    let mut ui = None;
    let mut selected = None;
    let mut grants = None;
    let mut port = 0u16;
    let mut query_memory_mib = 128u64;
    while let Some(arg) = args.next() {
        let value = args.next().ok_or("Every option requires a value")?;
        match arg.as_str() {
            "--data" => data = Some(PathBuf::from(value)),
            "--ui" => ui = Some(PathBuf::from(value)),
            "--fixture" => selected = Some(value),
            "--grants" => grants = Some(PathBuf::from(value)),
            "--port" => port = value.parse()?,
            "--query-memory-mib" => query_memory_mib = value.parse()?,
            _ => {
                return Err(
                    "Usage: omloom-dev --data PATH --ui PATH --fixture NAME [--port PORT] [--grants PATH] [--query-memory-mib MIB]".into(),
                );
            }
        }
    }
    let data = data.ok_or("--data is required")?;
    let ui = ui.ok_or("--ui is required")?;
    if !ui.join("index.html").is_file() {
        return Err("Build the inventory UI before starting this host".into());
    }
    let mut context = fixture(
        selected
            .as_deref()
            .ok_or("--fixture is required; this host has no production login")?,
    )?;
    if let Some(path) = grants {
        omloom_app::fixtures::configure(&mut context, &path)?;
    }
    let query_memory = query_memory_mib
        .checked_mul(1_048_576)
        .ok_or("Query memory size overflow")?;
    let store = Store::open_with_query_memory(&data, policy(), query_memory)?;
    let listener =
        tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port))).await?;
    println!(
        "Omloom development host: http://{} (fixture {})",
        listener.local_addr()?,
        context.fixture
    );
    let application = Application::new(store, context);
    let (stop, shutdown) = tokio::sync::watch::channel(false);
    let maintenance = tokio::spawn(application.clone().maintain(shutdown));
    let signal_stop = stop.clone();
    let result = axum::serve(listener, router(application, &ui))
        .with_graceful_shutdown(async move {
            if let Err(error) = tokio::signal::ctrl_c().await {
                eprintln!("Shutdown signal failed: {error}");
            }
            signal_stop.send_replace(true);
        })
        .await;
    stop.send_replace(true);
    maintenance.await?;
    result?;
    Ok(())
}
