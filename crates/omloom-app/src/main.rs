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
    let mut port = 0u16;
    while let Some(arg) = args.next() {
        let value = args.next().ok_or("Every option requires a value")?;
        match arg.as_str() {
            "--data" => data = Some(PathBuf::from(value)),
            "--ui" => ui = Some(PathBuf::from(value)),
            "--fixture" => selected = Some(value),
            "--port" => port = value.parse()?,
            _ => {
                return Err(
                    "Usage: omloom-dev --data PATH --ui PATH --fixture NAME [--port PORT]".into(),
                );
            }
        }
    }
    let data = data.ok_or("--data is required")?;
    let ui = ui.ok_or("--ui is required")?;
    if !ui.join("index.html").is_file() {
        return Err("Build the inventory UI before starting this host".into());
    }
    let context = fixture(
        selected
            .as_deref()
            .ok_or("--fixture is required; this host has no production login")?,
    )?;
    let store = Store::open(&data, policy())?;
    let listener =
        tokio::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port))).await?;
    println!(
        "Omloom development host: http://{} (fixture {})",
        listener.local_addr()?,
        context.fixture
    );
    axum::serve(listener, router(Application::new(store, context), &ui))
        .with_graceful_shutdown(async {
            if let Err(error) = tokio::signal::ctrl_c().await {
                eprintln!("Shutdown signal failed: {error}");
            }
        })
        .await?;
    Ok(())
}
