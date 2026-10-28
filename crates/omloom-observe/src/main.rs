//! Explicit native one-shot collection. No enrollment, daemon or remote effects.
#![deny(unsafe_code, clippy::unwrap_used)]

use omloom_contracts::{wire, workspace as api};
use prost::Message;
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
mod command;
mod native;
mod runtime;
#[cfg(test)]
mod tests;

const MAX_REPORT: usize = 256 * 1024;
const MAX_FINDINGS: usize = 128;
const MAX_FACTS: usize = 256;
type Result<T> = std::result::Result<T, String>;

fn text(name: &str, value: impl Into<String>) -> wire::Field {
    wire::Field {
        name: name.into(),
        value: Some(wire::field::Value::Text(value.into())),
    }
}
fn digest(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}
fn local(value: &str) -> bool {
    let Ok(ip) = value
        .split('%')
        .next()
        .unwrap_or(value)
        .parse::<std::net::IpAddr>()
    else {
        return false;
    };
    match ip {
        std::net::IpAddr::V4(v) => v.is_loopback() || v.is_link_local(),
        std::net::IpAddr::V6(v) => {
            v.is_loopback()
                || v.is_unicast_link_local()
                || v.to_ipv4_mapped()
                    .is_some_and(|v| v.is_loopback() || v.is_link_local())
        }
    }
}
fn finding(key: &str, kind: wire::AssetKind, parent: Option<&str>, name: &str) -> api::HostFinding {
    api::HostFinding {
        key: key.into(),
        kind: kind as i32,
        parent: parent.map(str::to_owned),
        fields: vec![text("name", name)],
        ..Default::default()
    }
}
fn coverage(report: &mut api::HostReport, module: &str, status: &str, detail: &str) {
    report.coverage.push(api::CollectionCoverage {
        module: module.into(),
        status: status.into(),
        detail: detail.into(),
    });
}
fn add(report: &mut api::HostReport, value: api::HostFinding) -> Result<()> {
    if report.findings.len() == MAX_FINDINGS {
        return Err("Finding count limit exceeded".into());
    }
    report.findings.push(value);
    Ok(())
}

async fn collect() -> Result<api::HostReport> {
    let mut runner = command::Runner::new();
    let (platform, source, boot, name) = native::identity(&mut runner).await?;
    let observed_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "Clock predates Unix epoch")?
        .as_millis()
        .try_into()
        .map_err(|_| "Clock limit")?;
    let mut report = api::HostReport {
        version: 1,
        platform: platform.into(),
        source_basis: digest(&format!("omloom-native/1/{platform}/{source}")),
        boot_basis: digest(&boot),
        observed_at,
        ..Default::default()
    };
    add(
        &mut report,
        finding("hardware", wire::AssetKind::Device, None, &name),
    )?;
    let mut installation = finding(
        "installation",
        wire::AssetKind::SoftwareInstallation,
        Some("hardware"),
        platform,
    );
    installation.fields.push(text(
        "installation_kind",
        wire::InstallationKind::OperatingSystem.as_str_name(),
    ));
    add(&mut report, installation)?;
    coverage(
        &mut report,
        "host",
        "verified",
        "Native hardware and installation evidence; not authenticated identity",
    );
    report.network = Some(api::HostNetwork {
        observed_at,
        basis: report.boot_basis.clone(),
        source: "native".into(),
        ..Default::default()
    });
    native::collect(&mut runner, &mut report).await?;
    runtime::collect(&mut runner, &mut report).await?;
    let (_, after_source, after_boot, _) = native::identity(&mut runner).await?;
    if source != after_source || boot != after_boot {
        return Err("Installation or boot changed during collection; output not written".into());
    }
    Ok(report)
}

fn encode(report: api::HostReport) -> Result<Vec<u8>> {
    if report
        .findings
        .iter()
        .flat_map(|v| &v.addresses)
        .any(|v| v.stability == api::AddressStability::Temporary as i32)
        || report
            .binds
            .iter()
            .filter_map(|v| v.value.as_ref())
            .any(|v| v.address_stability == api::AddressStability::Temporary as i32)
    {
        return Err("Temporary address reached report serialization".into());
    }
    let request = api::Request {
        command: Some(api::request::Command::HostImport(api::HostImport {
            report: Some(Box::new(report)),
            ..Default::default()
        })),
        ..Default::default()
    };
    if request.encoded_len() > MAX_REPORT {
        return Err("Report byte limit exceeded".into());
    }
    let bytes = request.encode_to_vec();
    // The same structural preflight used at the server bounds decoder allocation.
    let budget = omloom_contracts::MemoryBudget::new(16 * 1024 * 1024);
    omloom_contracts::decode_workspace(&bytes, Default::default(), &budget)
        .map_err(|e| format!("Report structure exceeds contract: {e}"))?;
    Ok(bytes)
}
fn save(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().ok_or("Report path has no parent")?;
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    let mut file =
        tempfile::NamedTempFile::new_in(parent).map_err(|e| format!("Cannot stage report: {e}"))?;
    file.write_all(bytes)
        .and_then(|()| file.as_file().sync_all())
        .map_err(|e| format!("Report staging failed: {e}"))?;
    // Publish only a complete synced frame, without replacing another file or
    // following its symlink. Interrupted staging is never an importable report.
    file.persist_noclobber(path)
        .map_err(|e| format!("Cannot publish report: {}", e.error))?;
    #[cfg(unix)]
    std::fs::File::open(parent)
        .and_then(|f| f.sync_all())
        .map_err(|e| format!("Report directory sync failed: {e}"))?;
    Ok(())
}
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 || (args[0] != "--output" && args[0] != "--check") {
        return Err("Usage: omloom-observe --output NEW_REPORT_FILE | --check REPORT_FILE".into());
    }
    if args[0] == "--check" {
        use std::io::Read;
        let file = std::fs::File::open(&args[1]).map_err(|e| format!("Cannot open report: {e}"))?;
        let mut bytes = Vec::with_capacity(MAX_REPORT + 1);
        file.take((MAX_REPORT + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|e| format!("Cannot read report: {e}"))?;
        if bytes.len() > MAX_REPORT {
            return Err("Report byte limit exceeded".into());
        }
        let budget = omloom_contracts::MemoryBudget::new(16 * 1024 * 1024);
        let mut decoded = omloom_contracts::decode_workspace(&bytes, Default::default(), &budget)
            .map_err(|e| format!("Invalid report frame: {e}"))?;
        let Some(api::request::Command::HostImport(value)) = decoded.take().command else {
            return Err("Not a host report".into());
        };
        let report = value.report.ok_or("Missing native report")?;
        println!(
            "Format {}: {}, {} candidate resources, {} binds",
            report.version,
            report.platform,
            report.findings.len(),
            report.binds.len()
        );
        for item in &report.coverage {
            println!("{}: {} ({})", item.module, item.status, item.detail);
        }
        return Ok(());
    }
    let report = collect().await?;
    let bytes = encode(report)?;
    save(Path::new(&args[1]), &bytes)?;
    println!("Report written. Import it in Omloom to review selected current facts.");
    Ok(())
}
