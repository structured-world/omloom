use super::{Result, add, api, coverage, digest, finding, text, wire};
use super::{command::Runner, native::json};
use serde_json::Value;
#[cfg(test)]
mod tests;

fn local_docker_endpoint(platform: &str, endpoint: &str) -> Result<()> {
    let path = endpoint.strip_prefix("unix:///").or_else(|| {
        (platform == "windows")
            .then(|| endpoint.strip_prefix("npipe:////./pipe/"))
            .flatten()
    });
    if path.is_none_or(|v| v.is_empty() || v.chars().any(char::is_control)) {
        return Err("Selected Docker context is not a local daemon".into());
    }
    Ok(())
}

fn docker_environment(name: &str) -> Result<Option<String>> {
    let Some(value) = std::env::var_os(name) else {
        return Ok(None);
    };
    if value.is_empty() {
        return Ok(None);
    }
    let value = value
        .into_string()
        .map_err(|_| "Docker connection setting is not UTF-8")?;
    if value.len() > 512 || value.chars().any(char::is_control) {
        return Err("Docker connection setting exceeds contract".into());
    }
    Ok(Some(value))
}

enum DockerSelection {
    Context(String),
    Host(String),
    Current,
}
fn docker_selection(read: impl Fn(&str) -> Result<Option<String>>) -> Result<DockerSelection> {
    if let Some(value) = read("DOCKER_CONTEXT")? {
        return Ok(DockerSelection::Context(value));
    }
    Ok(match read("DOCKER_HOST")? {
        Some(value) => DockerSelection::Host(value),
        None => DockerSelection::Current,
    })
}

fn docker_step(step: &str, error: String) -> String {
    // Keep the generic permission/unavailable classification prefix intact.
    format!("{error}: Docker {step}")
}

fn docker_status(error: &str) -> &'static str {
    let base = if let Some((base, _)) = error.rsplit_once(": Docker ") {
        base
    } else {
        error
    };
    super::native::failure_status(base)
}

fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    let result = value
        .get(key)
        .and_then(Value::as_str)
        .ok_or("Runtime field unavailable")?;
    if result.is_empty() || result.len() > 512 || result.chars().any(char::is_control) {
        return Err("Runtime field exceeds contract".into());
    }
    Ok(result)
}
trait DockerCommands {
    async fn read(&mut self, args: &[&str]) -> Result<String>;
}
impl DockerCommands for Runner {
    async fn read(&mut self, args: &[&str]) -> Result<String> {
        self.run("docker", args).await.map(|output| output.text)
    }
}
pub(super) async fn collect(runner: &mut Runner, report: &mut api::HostReport) -> Result<()> {
    let before = report.findings.len();
    match docker(runner, report).await {
        Ok(()) => coverage(
            report,
            "docker",
            "verified",
            "Local daemon configuration and endpoint attachments; publication does not prove a listener; labels do not prove human ownership",
        ),
        Err(error) => {
            report.findings.truncate(before);
            coverage(report, "docker", docker_status(&error), &error);
        }
    }
    match netbird(runner, report).await {
        Ok(()) => coverage(
            report,
            "netbird",
            "verified",
            "Only local daemon IP/interface evidence; no peer, key or management endpoint retained",
        ),
        Err(error) => coverage(
            report,
            "netbird",
            super::native::failure_status(&error),
            &error,
        ),
    }
    Ok(())
}
async fn docker(runner: &mut Runner, report: &mut api::HostReport) -> Result<()> {
    let selection = docker_selection(docker_environment)?;
    // Docker documents context > host > configured current context. Resolve
    // once, then pin every daemon read to the checked endpoint.
    let endpoint = if let DockerSelection::Host(host) = selection {
        host
    } else {
        let context = match selection {
            DockerSelection::Context(value) => value,
            DockerSelection::Current => runner
                .run("docker", &["context", "show"])
                .await
                .map_err(|e| docker_step("context selection", e))?
                .text
                .trim()
                .to_owned(),
            DockerSelection::Host(_) => unreachable!("host handled above"),
        };
        if context.is_empty() || context.len() > 256 || context.chars().any(char::is_control) {
            return Err("Docker context name exceeds contract".into());
        }
        let output = runner
            .run(
                "docker",
                &[
                    "context",
                    "inspect",
                    &context,
                    "--format",
                    "{{json .Endpoints.docker.Host}}",
                ],
            )
            .await
            .map_err(|e| docker_step("context inspection", e))?
            .text;
        let value = json(output.trim())?;
        value
            .as_str()
            .ok_or("Docker endpoint unavailable")?
            .to_owned()
    };
    docker_at(runner, &endpoint, report).await
}

async fn docker_at(
    runner: &mut impl DockerCommands,
    endpoint: &str,
    report: &mut api::HostReport,
) -> Result<()> {
    local_docker_endpoint(&report.platform, endpoint)?;
    let info=runner.read(&["--host",endpoint,"info","--format","{\"id\":{{json .ID}},\"os\":{{json .OperatingSystem}},\"type\":{{json .OSType}},\"version\":{{json .ServerVersion}}}"]).await.map_err(|e|docker_step("daemon inspection",e))?;
    let info = json(&info)?;
    let daemon = string(&info, "id")?;
    let runtime = format!("docker/{}", digest(daemon));
    let mut installation = finding(
        &runtime,
        wire::AssetKind::SoftwareInstallation,
        Some("installation"),
        "Docker runtime",
    );
    installation.fields.push(text(
        "installation_kind",
        wire::InstallationKind::ContainerRuntime.as_str_name(),
    ));
    installation.fields.push(text(
        "installation_scope",
        format!("{}; {}", string(&info, "os")?, string(&info, "type")?),
    ));
    add(report, installation)?;
    let ids = runner
        .read(&["--host", endpoint, "ps", "-q", "--no-trunc"])
        .await
        .map_err(|e| docker_step("container listing", e))?;
    if ids.lines().count() > 32 {
        return Err("Selected Docker instance count limit exceeded".into());
    }
    for id in ids.lines() {
        if id.len() != 64 || !id.bytes().all(|v| v.is_ascii_hexdigit()) {
            return Err("Invalid Docker container identity".into());
        }
        let result=runner.read(&["--host",endpoint,"inspect",id,"--format","{\"id\":{{json .Id}},\"name\":{{json .Name}},\"created\":{{json .Created}},\"image\":{{json .Image}},\"project\":{{json (index .Config.Labels \"com.docker.compose.project\")}},\"service\":{{json (index .Config.Labels \"com.docker.compose.service\")}},\"working_directory\":{{json (index .Config.Labels \"com.docker.compose.project.working_dir\")}},\"networks\":{{json .NetworkSettings.Networks}}}"]).await.map_err(|e|docker_step("container inspection",e))?;
        let value = json(&result)?;
        if string(&value, "id")? != id {
            return Err("Container changed during inspection".into());
        }
        let name = string(&value, "name")?
            .strip_prefix('/')
            .ok_or("Unrecognized Docker name")?;
        let workload_key = format!("{runtime}/workload/{id}");
        let mut workload = finding(
            &workload_key,
            wire::AssetKind::Workload,
            Some(&runtime),
            name,
        );
        let project = value
            .get("project")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty());
        let service = value
            .get("service")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty());
        workload.fields.push(text(
            "workload_kind",
            if project.is_some() && service.is_some() {
                wire::WorkloadKind::Compose
            } else {
                wire::WorkloadKind::Standalone
            }
            .as_str_name(),
        ));
        if let Some(value) = project {
            workload.fields.push(text("compose_project", value));
        }
        if let Some(value) = service {
            workload.fields.push(text("compose_service", value));
        }
        if let Some(value) = value
            .get("working_directory")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
        {
            if value.len() > 512 || value.chars().any(char::is_control) {
                return Err("Compose source reference limit exceeded".into());
            }
            workload.fields.push(text("working_directory", value));
        }
        add(report, workload)?;
        let container_key = format!("{runtime}/container/{id}");
        let mut container = finding(
            &container_key,
            wire::AssetKind::Container,
            Some(&workload_key),
            name,
        );
        container.runtime = Some(runtime.clone());
        container.fields.extend([
            text("runtime_object_id", id),
            text("runtime_lifetime", string(&value, "created")?),
            text("image_digest", string(&value, "image")?),
        ]);
        add(report, container)?;
        let networks = value
            .get("networks")
            .and_then(Value::as_object)
            .ok_or("Container endpoint table unavailable")?;
        if networks.len() > 16 {
            return Err("Container attachment count limit exceeded".into());
        }
        for (network, attachment) in networks {
            let endpoint = attachment
                .get("EndpointID")
                .and_then(Value::as_str)
                .ok_or("Missing Docker endpoint identity")?;
            // Host/none network modes can have no separate endpoint. Network
            // configuration alone cannot fabricate a container interface.
            if endpoint.is_empty() {
                continue;
            }
            if endpoint.len() > 64 || !endpoint.bytes().all(|v| v.is_ascii_hexdigit()) {
                return Err("Invalid Docker endpoint identity".into());
            }
            let mut interface = finding(
                &format!("{container_key}/attachment/{endpoint}"),
                wire::AssetKind::Interface,
                Some(&container_key),
                network,
            );
            for (key, prefix) in [
                ("IPAddress", "IPPrefixLen"),
                ("GlobalIPv6Address", "GlobalIPv6PrefixLen"),
            ] {
                let address = attachment
                    .get(key)
                    .and_then(Value::as_str)
                    .ok_or("Missing attachment address field")?;
                if address.is_empty() {
                    continue;
                }
                if super::local(address) {
                    continue;
                }
                let mask = attachment
                    .get(prefix)
                    .and_then(Value::as_u64)
                    .ok_or("Missing attachment prefix")?
                    .try_into()
                    .map_err(|_| "Invalid attachment prefix")?;
                interface.addresses.push(api::CurrentAddress {
                    address: address.into(),
                    prefix_length: Some(mask),
                    stability: api::AddressStability::Stable as i32,
                    state: api::AddressState::Preferred as i32,
                });
            }
            interface.addresses_complete = true;
            add(report, interface)?;
        }
    }
    let after = runner
        .read(&["--host", endpoint, "info", "--format", "{{json .ID}}"])
        .await
        .map_err(|e| docker_step("daemon identity recheck", e))?;
    let after = json(after.trim())?;
    if after.as_str() != Some(daemon) {
        return Err("Docker daemon changed during inspection".into());
    }
    Ok(())
}
async fn netbird(runner: &mut Runner, report: &mut api::HostReport) -> Result<()> {
    let result = runner.run("netbird", &["status", "--json"]).await?.text;
    let value = json(&result)?;
    let literal = string(&value, "netbirdIp")?
        .split('/')
        .next()
        .ok_or("NetBird local address absent")?;
    let ip: std::net::IpAddr = literal
        .parse()
        .map_err(|_| "Invalid local NetBird address")?;
    let mut matches = report.findings.iter().enumerate().filter(|(_, f)| {
        f.kind == wire::AssetKind::Interface as i32
            && f.addresses.iter().any(|v| v.address == ip.to_string())
    });
    let index = matches
        .next()
        .ok_or("NetBird local interface was not observed")?
        .0;
    if matches.next().is_some() {
        return Err("NetBird local interface attribution is ambiguous".into());
    }
    add(
        report,
        finding(
            "netbird",
            wire::AssetKind::Service,
            Some("installation"),
            "NetBird",
        ),
    )?;
    report.findings[index].parent = Some("netbird".into());
    // Parent-before-child ordering keeps review/application deterministic.
    let service = report.findings.pop().ok_or("NetBird candidate missing")?;
    report.findings.insert(index, service);
    Ok(())
}
