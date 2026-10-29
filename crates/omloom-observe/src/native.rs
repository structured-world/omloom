use super::command::Runner;
use super::{MAX_FACTS, Result, add, api, coverage, finding, local, wire};
use serde_json::Value;
#[cfg(test)]
mod tests;

pub(super) fn json(input: &str) -> Result<Value> {
    // Preflight limits both structural fan-out and stack use before serde builds
    // the tree; source bytes are separately bounded by the command runner.
    let (mut depth, mut tokens, mut quoted, mut escaped) = (0usize, 0usize, false, false);
    for byte in input.bytes() {
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else {
            match byte {
                b'"' => {
                    quoted = true;
                    tokens += 1;
                }
                b'[' | b'{' => {
                    depth += 1;
                    tokens += 1;
                }
                b']' | b'}' => {
                    depth = depth.checked_sub(1).ok_or("Invalid JSON nesting")?;
                }
                b',' | b':' => tokens += 1,
                _ => (),
            }
        }
        if depth > 16 || tokens > 8192 {
            return Err("Structured command output limit exceeded".into());
        }
    }
    if input.len() > 1024 * 1024 || quoted || depth != 0 {
        return Err("Invalid structured output".into());
    }
    // At most one Value per structural token: 512 bytes includes B-tree
    // entry overhead and doubling Vec capacity. Twice the source length covers
    // decoded string capacity and parser scratch. Refuse before allocation.
    let reservation = tokens
        .checked_mul(512)
        .and_then(|n| {
            input
                .len()
                .checked_mul(2)
                .and_then(|bytes| n.checked_add(bytes))
        })
        .and_then(|n| n.checked_add(1024))
        .ok_or("JSON allocation budget exceeded")?;
    if reservation > 16 * 1024 * 1024 {
        return Err("JSON allocation budget exceeded".into());
    }
    serde_json::from_str(input).map_err(|_| "Invalid structured command output".into())
}
fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    let text = value
        .get(key)
        .and_then(Value::as_str)
        .ok_or("Missing source field")?;
    if text.len() > 512 || text.chars().any(char::is_control) {
        return Err("Source string limit exceeded".into());
    }
    Ok(text)
}
fn array(value: &Value) -> Result<&[Value]> {
    let values = value.as_array().ok_or("Expected a source array")?;
    if values.len() > MAX_FACTS {
        return Err("Source row limit exceeded".into());
    }
    Ok(values)
}
fn known_address(
    value: &str,
    prefix: Option<u32>,
    deprecated: bool,
) -> Result<Option<api::CurrentAddress>> {
    let value = value.split('%').next().ok_or("Missing address")?;
    let address: std::net::IpAddr = value.parse().map_err(|_| "Invalid source address")?;
    if local(value) || address.is_unspecified() {
        return Ok(None);
    }
    if prefix.is_some_and(|n| n > if address.is_ipv4() { 32 } else { 128 }) {
        return Err("Invalid source prefix".into());
    }
    Ok(Some(api::CurrentAddress {
        address: address.to_string(),
        prefix_length: prefix,
        stability: api::AddressStability::Stable as i32,
        state: if deprecated {
            api::AddressState::Deprecated
        } else {
            api::AddressState::Preferred
        } as i32,
    }))
}
async fn ps(runner: &mut Runner, script: &str) -> Result<String> {
    runner
        .run(
            "powershell.exe",
            &[
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                script,
            ],
        )
        .await
        .map(|v| v.text)
}
pub(super) struct Identity {
    pub platform: &'static str,
    pub source: String,
    pub boot: String,
    pub name: String,
    pub hardware: Option<api::HostIdentityClaim>,
    pub installation: api::HostIdentityClaim,
}

fn identity_claim(
    kind: api::HostIdentityKind,
    namespace: &str,
    raw: &str,
) -> Result<api::HostIdentityClaim> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > 128 || raw.chars().any(char::is_control) {
        return Err("Invalid native identity claim".into());
    }
    Ok(api::HostIdentityClaim {
        kind: kind as i32,
        namespace: namespace.into(),
        digest: super::digest(&raw.to_ascii_lowercase()),
        quality: api::HostIdentityQuality::Reported as i32,
    })
}

fn hardware_claim(namespace: &str, raw: &str) -> Option<api::HostIdentityClaim> {
    let uuid = uuid::Uuid::parse_str(raw.trim()).ok()?;
    if uuid.is_nil() || uuid == uuid::Uuid::max() {
        return None;
    }
    identity_claim(
        api::HostIdentityKind::HardwareUuid,
        namespace,
        &uuid.to_string(),
    )
    .ok()
}

pub(super) async fn identity(runner: &mut Runner) -> Result<Identity> {
    match std::env::consts::OS {
        "macos" => {
            let device = runner
                .run("/usr/sbin/ioreg", &["-rd1", "-c", "IOPlatformExpertDevice"])
                .await?
                .text;
            let hardware = device
                .lines()
                .find_map(|l| l.trim().strip_prefix("\"IOPlatformUUID\" = "))
                .ok_or("Hardware basis unavailable")?
                .trim_matches('"');
            let volume = runner.run("/usr/sbin/diskutil", &["info", "/"]).await?.text;
            let installation = volume
                .lines()
                .find_map(|l| l.trim().strip_prefix("Volume UUID:"))
                .ok_or("Installation basis unavailable")?
                .trim();
            let boot = runner
                .run("/usr/sbin/sysctl", &["-n", "kern.bootsessionuuid"])
                .await?
                .text;
            let model = runner
                .run("/usr/sbin/sysctl", &["-n", "hw.model"])
                .await?
                .text;
            Ok(Identity {
                platform: "macos",
                source: format!("{hardware}/{installation}"),
                boot: boot.trim().into(),
                name: model.trim().into(),
                hardware: hardware_claim("apple/platform-uuid", hardware),
                installation: identity_claim(
                    api::HostIdentityKind::InstallationId,
                    "macos/apfs-volume",
                    installation,
                )?,
            })
        }
        "linux" => {
            let source = runner.run("cat", &["/etc/machine-id"]).await?.text;
            let boot = runner
                .run("cat", &["/proc/sys/kernel/random/boot_id"])
                .await?
                .text;
            let name = runner.run("uname", &["-n"]).await?.text;
            // DMI may be unreadable without root. Missing hardware evidence
            // does not prevent collecting the installation's current facts.
            let hardware = match runner.run("cat", &["/sys/class/dmi/id/product_uuid"]).await {
                Ok(value) => hardware_claim("smbios/system-uuid", &value.text),
                Err(_) => None,
            };
            Ok(Identity {
                platform: "linux",
                source: source.trim().into(),
                boot: boot.trim().into(),
                name: name.trim().into(),
                hardware,
                installation: identity_claim(
                    api::HostIdentityKind::InstallationId,
                    "linux/machine-id",
                    &source,
                )?,
            })
        }
        "windows" => {
            let result = ps(runner, "$ErrorActionPreference='Stop'; [Console]::OutputEncoding=[Text.UTF8Encoding]::new($false); $os=Get-CimInstance Win32_OperatingSystem; $cs=Get-CimInstance Win32_ComputerSystemProduct; @{source=(Get-ItemProperty 'HKLM:\\SOFTWARE\\Microsoft\\Cryptography').MachineGuid; boot=$os.LastBootUpTime.ToUniversalTime().ToString('o'); name=$cs.Name; hardware=$cs.UUID} | ConvertTo-Json -Compress").await?;
            let value = json(&result)?;
            Ok(Identity {
                platform: "windows",
                source: string(&value, "source")?.into(),
                boot: string(&value, "boot")?.into(),
                name: string(&value, "name")?.into(),
                hardware: value
                    .get("hardware")
                    .and_then(Value::as_str)
                    .and_then(|v| hardware_claim("smbios/system-uuid", v)),
                installation: identity_claim(
                    api::HostIdentityKind::InstallationId,
                    "windows/machine-guid",
                    string(&value, "source")?,
                )?,
            })
        }
        _ => Err("Native platform is not supported".into()),
    }
}
fn result(report: &mut api::HostReport, module: &str, outcome: Result<()>, note: &str) {
    match outcome {
        Ok(()) => coverage(report, module, "verified", note),
        Err(error) => coverage(report, module, failure_status(&error), &error),
    }
}
pub(super) fn failure_status(error: &str) -> &'static str {
    if error == "Permission denied" {
        "blocked"
    } else if error.starts_with("Command unavailable (") {
        "unavailable"
    } else {
        "failed"
    }
}
pub(super) async fn collect(runner: &mut Runner, report: &mut api::HostReport) -> Result<()> {
    let mut excluded = std::collections::BTreeSet::new();
    let interfaces = match report.platform.as_str() {
        "macos" => mac_interfaces(runner, report, &mut excluded).await,
        "linux" => linux_interfaces(runner, report, &mut excluded).await,
        "windows" => windows_interfaces(runner, report, &mut excluded).await,
        _ => return Err("Unsupported platform".into()),
    };
    if let Err(error) = &interfaces {
        return Err(format!(
            "Interface collection incomplete: {error}; no report written"
        ));
    }
    result(
        report,
        "interfaces",
        interfaces,
        "Current selected interfaces; no cable or upstream identity inferred",
    );
    if report.platform == "macos" {
        let ports = mac_ports(runner, report).await;
        result(
            report,
            "physical-connectors",
            ports,
            "Reported receptacles; chassis mapping and negotiated throughput unknown",
        );
    }
    let sockets = sockets(runner, report).await;
    if let Err(error) = &sockets {
        report.binds.clear();
        report.findings.retain(|v| !v.key.starts_with("process/"));
        report
            .network
            .as_mut()
            .ok_or("Network missing")?
            .socket_coverage = error.clone();
    }
    result(
        report,
        "sockets",
        sockets,
        "Sequential kernel/process views; process visibility may be partial; no reachability claim",
    );
    let routes = routes(runner, report).await;
    if let Err(error) = &routes {
        report
            .network
            .as_mut()
            .ok_or("Network missing")?
            .route_coverage = error.clone();
    }
    result(
        report,
        "routes",
        routes,
        "Local IPv4/IPv6 configuration, not upstream topology",
    );
    firewall(runner, report).await?;
    redact_literals(report, &excluded);
    Ok(())
}

pub(super) fn redact_literals(
    report: &mut api::HostReport,
    excluded: &std::collections::BTreeSet<String>,
) {
    fn is_excluded(value: &str, excluded: &std::collections::BTreeSet<String>) -> bool {
        let value = value.split(['/', '%']).next().unwrap_or(value);
        match value.parse::<std::net::IpAddr>() {
            Ok(value) => excluded.contains(&value.to_string()),
            Err(_) => false,
        }
    }
    report.binds.retain(|v| {
        v.value
            .as_ref()
            .is_some_and(|v| !is_excluded(&v.bind, excluded))
    });
    if let Some(network) = &mut report.network {
        network.routes.retain(|v| {
            !is_excluded(&v.destination, excluded) && !is_excluded(&v.gateway, excluded)
        });
    }
    report.findings.retain(|v| {
        !v.key.starts_with("process/") || report.binds.iter().any(|b| b.owner == v.key)
    });
}

pub(super) fn parse_mac_interfaces(input: &str) -> Result<Vec<api::HostFinding>> {
    let mut rows = Vec::new();
    let mut selected = false;
    for line in input.lines() {
        if !line.starts_with(char::is_whitespace) {
            let Some((name, flags)) = line.split_once(": flags=") else {
                continue;
            };
            if rows.len() == super::MAX_FINDINGS {
                return Err("Interface count limit exceeded".into());
            }
            if flags.contains("LOOPBACK") {
                selected = false;
                continue;
            }
            selected = true;
            rows.push(finding(
                &format!("interface/{name}"),
                wire::AssetKind::Interface,
                Some("installation"),
                name,
            ));
        } else if let Some(row) = rows.last_mut().filter(|_| selected) {
            let words: Vec<_> = line.split_whitespace().take(32).collect();
            if matches!(words.first(), Some(&"inet" | &"inet6")) && words.len() > 1 {
                if words.contains(&"temporary") {
                    continue;
                }
                let prefix = if let Some(i) = words.iter().position(|v| *v == "prefixlen") {
                    Some(
                        words
                            .get(i + 1)
                            .ok_or("Missing prefix")?
                            .parse()
                            .map_err(|_| "Invalid prefix")?,
                    )
                } else if let Some(i) = words.iter().position(|v| *v == "netmask") {
                    let mask = u32::from_str_radix(
                        words
                            .get(i + 1)
                            .ok_or("Missing netmask")?
                            .trim_start_matches("0x"),
                        16,
                    )
                    .map_err(|_| "Invalid netmask")?;
                    if mask != u32::MAX.checked_shl(mask.trailing_zeros()).unwrap_or(0) {
                        return Err("Non-contiguous netmask".into());
                    }
                    Some(mask.count_ones())
                } else {
                    None
                };
                if let Some(address) =
                    known_address(words[1], prefix, words.contains(&"deprecated"))?
                {
                    if row.addresses.len() == 64 {
                        return Err("Interface address limit exceeded".into());
                    }
                    row.addresses.push(address);
                }
            }
        }
    }
    if rows.is_empty() {
        return Err("No interface table was recognized".into());
    }
    Ok(rows)
}
async fn mac_interfaces(
    runner: &mut Runner,
    report: &mut api::HostReport,
    excluded: &mut std::collections::BTreeSet<String>,
) -> Result<()> {
    let source = runner.run("/sbin/ifconfig", &["-a", "-v"]).await?;
    for line in source.text.lines() {
        let words: Vec<_> = line.split_whitespace().take(32).collect();
        if words.first() == Some(&"inet6") && words.contains(&"temporary") {
            let value = words
                .get(1)
                .ok_or("Missing temporary literal")?
                .split('%')
                .next()
                .ok_or("Missing literal")?;
            excluded.insert(
                value
                    .parse::<std::net::Ipv6Addr>()
                    .map_err(|_| "Invalid temporary literal")?
                    .to_string(),
            );
        }
    }
    let values = parse_mac_interfaces(&source.text)?;
    let hardware = runner
        .run("/usr/sbin/networksetup", &["-listallhardwareports"])
        .await?
        .text;
    let mut selected = std::collections::BTreeSet::new();
    for line in hardware.lines() {
        if let Some(name) = line.strip_prefix("Device: ") {
            selected.insert(name);
        }
    }
    for mut value in values {
        let name = value
            .key
            .strip_prefix("interface/")
            .ok_or("Interface key")?;
        // Hardware service registration is useful configuration evidence, but
        // auxiliary unaddressed kernel interfaces are not automatically adopted.
        if !selected.contains(name) && value.addresses.is_empty() {
            continue;
        }
        value.addresses_complete = !source.warnings;
        add(report, value)?;
    }
    Ok(())
}
async fn linux_interfaces(
    runner: &mut Runner,
    report: &mut api::HostReport,
    excluded: &mut std::collections::BTreeSet<String>,
) -> Result<()> {
    let output = runner
        .run("ip", &["-j", "-details", "address", "show"])
        .await?;
    let value = json(&output.text)?;
    // Unaddressed bridges, veth links and tunnels can carry L2 connectivity.
    // Keep them within the report limits rather than silently losing topology.
    for item in array(&value)? {
        let name = string(item, "ifname")?;
        if item
            .get("flags")
            .and_then(Value::as_array)
            .is_some_and(|f| f.iter().any(|v| v.as_str() == Some("LOOPBACK")))
        {
            continue;
        }
        let mut row = finding(
            &format!("interface/{name}"),
            wire::AssetKind::Interface,
            Some("installation"),
            name,
        );
        for address in array(item.get("addr_info").ok_or("Missing address table")?)? {
            if address.get("temporary").and_then(Value::as_bool) == Some(true)
                || address
                    .get("flags")
                    .and_then(Value::as_array)
                    .is_some_and(|v| v.iter().any(|v| v.as_str() == Some("temporary")))
            {
                excluded.insert(
                    string(address, "local")?
                        .parse::<std::net::IpAddr>()
                        .map_err(|_| "Invalid temporary address")?
                        .to_string(),
                );
                continue;
            }
            let prefix = address
                .get("prefixlen")
                .and_then(Value::as_u64)
                .map(u32::try_from)
                .transpose()
                .map_err(|_| "Invalid prefix")?;
            if let Some(address) = known_address(
                string(address, "local")?,
                prefix,
                address.get("preferred_life_time").and_then(Value::as_u64) == Some(0),
            )? {
                row.addresses.push(address);
            }
        }
        row.addresses_complete = !output.warnings;
        add(report, row)?;
    }
    Ok(())
}
async fn windows_interfaces(
    runner: &mut Runner,
    report: &mut api::HostReport,
    excluded: &mut std::collections::BTreeSet<String>,
) -> Result<()> {
    let output = ps(runner,"$ErrorActionPreference='Stop'; [Console]::OutputEncoding=[Text.UTF8Encoding]::new($false); ConvertTo-Json -InputObject @(Get-NetIPAddress | Select-Object InterfaceIndex,InterfaceAlias,IPAddress,PrefixLength,@{n='State';e={$_.AddressState.ToString()}},@{n='Origin';e={$_.SuffixOrigin.ToString()}}) -Depth 4 -Compress").await?;
    let value = json(&output)?;
    let mut rows = std::collections::BTreeMap::<u64, api::HostFinding>::new();
    for item in array(&value)? {
        let ip = string(item, "IPAddress")?;
        let index = item
            .get("InterfaceIndex")
            .and_then(Value::as_u64)
            .ok_or("Missing interface index")?;
        let prefix = item
            .get("PrefixLength")
            .and_then(Value::as_u64)
            .ok_or("Missing prefix")?
            .try_into()
            .map_err(|_| "Invalid prefix")?;
        // Random is not proof of temporary versus stable privacy generation.
        // Exclude unqualified random literals rather than serialize a temporary IP.
        if ip.contains(':') && string(item, "Origin")? == "Random" {
            excluded.insert(
                ip.parse::<std::net::IpAddr>()
                    .map_err(|_| "Invalid random-origin address")?
                    .to_string(),
            );
            continue;
        }
        if let Some(address) =
            known_address(ip, Some(prefix), string(item, "State")? == "Deprecated")?
        {
            if let std::collections::btree_map::Entry::Vacant(entry) = rows.entry(index) {
                entry.insert(finding(
                    &format!("interface/{index}"),
                    wire::AssetKind::Interface,
                    Some("installation"),
                    string(item, "InterfaceAlias")?,
                ));
            }
            rows.get_mut(&index)
                .ok_or("Interface allocation")?
                .addresses
                .push(address);
        }
    }
    for (_, row) in rows {
        add(report, row)?;
    }
    coverage(
        report,
        "ipv6-generation",
        "partial",
        "Random-origin Windows IPv6 literals excluded until temporary/public classification is qualified",
    );
    Ok(())
}

async fn mac_ports(runner: &mut Runner, report: &mut api::HostReport) -> Result<()> {
    let output = runner
        .run("/usr/sbin/system_profiler", &["SPThunderboltDataType"])
        .await?
        .text;
    let mut bus = None;
    for line in output.lines() {
        let value = line.trim();
        if value.starts_with("Thunderbolt/USB4 Bus ") && value.ends_with(':') {
            bus = Some(value.trim_end_matches(':'));
        }
        if let Some(port) = value.strip_prefix("Receptacle: ") {
            let bus = bus.ok_or("Receptacle without a bus")?;
            let number: u32 = port.parse().map_err(|_| "Invalid receptacle")?;
            add(
                report,
                finding(
                    &format!("connector/{bus}/{number}"),
                    wire::AssetKind::Port,
                    Some("hardware"),
                    &format!("{bus} receptacle {number}"),
                ),
            )?;
        }
    }
    Ok(())
}

async fn sockets(runner: &mut Runner, report: &mut api::HostReport) -> Result<()> {
    match report.platform.as_str() {
        "macos" => {
            let output = runner
                .run(
                    "/usr/sbin/lsof",
                    &["-nP", "-iTCP", "-sTCP:LISTEN", "-iUDP", "-FpcftnT"],
                )
                .await?;
            parse_lsof(&output.text, report)?;
            for protocol in ["tcp", "udp"] {
                match runner
                    .run("/usr/sbin/netstat", &["-anlW", "-p", protocol])
                    .await
                {
                    Ok(kernel) if kernel.text.trim().is_empty() => coverage(
                        report,
                        &format!("kernel-{protocol}"),
                        "unavailable",
                        "Command returned no table; absence and complete visibility are not established",
                    ),
                    Ok(kernel) => {
                        parse_mac_sockets(&kernel.text, protocol, report)?;
                        coverage(
                            report,
                            &format!("kernel-{protocol}"),
                            if kernel.warnings {
                                "partial"
                            } else {
                                "verified"
                            },
                            "Readable kernel source table; sampled separately from process evidence",
                        );
                    }
                    Err(error) => result(report, &format!("kernel-{protocol}"), Err(error), ""),
                }
            }
            let network = report.network.as_mut().ok_or("Missing network")?;
            network.socket_coverage =
                "Sequential kernel table and accessible process sockets; unowned/disagreeing binds remain unattributed; non-root omission is not absence".into();
            network.sockets_complete = false;
        }
        "linux" => {
            let mut complete = true;
            for (flag, family) in [("-4", "IPv4"), ("-6", "IPv6")] {
                let output = runner.run("ss", &[flag, "-H", "-lntup"]).await?;
                parse_ss(&output.text, family, report)?;
                complete &= !output.warnings;
            }
            let network = report.network.as_mut().ok_or("Missing network")?;
            network.socket_coverage =
                "Kernel TCP listeners and UDP binds; inaccessible owners remain unresolved".into();
            network.sockets_complete = complete;
        }
        "windows" => {
            let output=ps(runner,"$ErrorActionPreference='Stop'; [Console]::OutputEncoding=[Text.UTF8Encoding]::new($false); $rows=@(); foreach($s in Get-NetTCPConnection -State Listen){$rows+=@{protocol='tcp';bind=$s.LocalAddress;port=$s.LocalPort;pid=$s.OwningProcess}}; foreach($s in Get-NetUDPEndpoint){$rows+=@{protocol='udp';bind=$s.LocalAddress;port=$s.LocalPort;pid=$s.OwningProcess}}; ConvertTo-Json -InputObject @($rows) -Depth 4 -Compress").await?;
            let values = json(&output)?;
            for row in array(&values)? {
                let ip = string(row, "bind")?;
                let pid = row
                    .get("pid")
                    .and_then(Value::as_u64)
                    .ok_or("Missing socket owner")?;
                let port = row
                    .get("port")
                    .and_then(Value::as_u64)
                    .ok_or("Missing socket port")?
                    .try_into()
                    .map_err(|_| "Invalid port")?;
                bind(
                    report,
                    pid,
                    &format!("Process {pid}"),
                    string(row, "protocol")?,
                    if ip.contains(':') { "IPv6" } else { "IPv4" },
                    ip,
                    port,
                )?;
            }
            let network = report.network.as_mut().ok_or("Missing network")?;
            network.socket_coverage =
                "Windows kernel endpoint tables; process names and executable attribution unknown"
                    .into();
            network.sockets_complete = true;
        }
        _ => return Err("Unsupported socket platform".into()),
    }
    Ok(())
}
fn bind(
    report: &mut api::HostReport,
    pid: u64,
    name: &str,
    protocol: &str,
    family: &str,
    address: &str,
    port: u32,
) -> Result<()> {
    if local(address) {
        return Ok(());
    }
    if !(1..=65535).contains(&port) {
        return Err("Invalid socket port".into());
    }
    let owner = format!("process/{}/{}/{pid}", report.boot_basis, report.observed_at);
    if !report.findings.iter().any(|f| f.key == owner) {
        add(
            report,
            finding(&owner, wire::AssetKind::Service, Some("installation"), name),
        )?;
    }
    if report.binds.len() == MAX_FACTS {
        return Err("Socket count limit exceeded".into());
    }
    if report.binds.iter().any(|v| {
        v.owner == owner
            && v.value.as_ref().is_some_and(|v| {
                v.protocol == protocol && v.family == family && v.bind == address && v.port == port
            })
    }) {
        return Ok(());
    }
    report.binds.push(api::HostBind {
        owner,
        value: Some(api::SocketBind {
            protocol: protocol.into(),
            family: family.into(),
            bind: address.into(),
            port,
            ..Default::default()
        }),
    });
    Ok(())
}
pub(super) fn parse_lsof(input: &str, report: &mut api::HostReport) -> Result<()> {
    let (mut pid, mut name, mut family, mut endpoint, mut state) = (None, "", "", None, false);
    fn finish(
        report: &mut api::HostReport,
        pid: Option<u64>,
        name: &str,
        family: &str,
        endpoint: Option<&str>,
        state: bool,
    ) -> Result<()> {
        let Some(endpoint) = endpoint else {
            return Ok(());
        };
        if endpoint.contains("->") {
            return Ok(());
        }
        let (ip, port) = endpoint
            .rsplit_once(':')
            .ok_or("Unrecognized socket endpoint")?;
        if port == "*" && !state {
            return Ok(());
        }
        bind(
            report,
            pid.ok_or("Missing process")?,
            name,
            if state { "tcp" } else { "udp" },
            family,
            ip.trim_matches(['[', ']']),
            port.parse().map_err(|_| "Invalid socket port")?,
        )
    }
    for line in input.lines() {
        if let Some(value) = line.strip_prefix('p') {
            finish(report, pid, name, family, endpoint, state)?;
            pid = Some(value.parse().map_err(|_| "Invalid process")?);
            endpoint = None;
            state = false;
        } else if let Some(value) = line.strip_prefix('c') {
            name = value;
        } else if line.starts_with('f') {
            finish(report, pid, name, family, endpoint, state)?;
            endpoint = None;
            state = false;
        } else if let Some(value) = line.strip_prefix('t') {
            family = value;
        } else if let Some(value) = line.strip_prefix('n') {
            endpoint = Some(value);
        } else if line == "TST=LISTEN" {
            state = true;
        }
    }
    finish(report, pid, name, family, endpoint, state)
}

pub(super) fn parse_mac_sockets(
    input: &str,
    protocol: &str,
    report: &mut api::HostReport,
) -> Result<()> {
    let mut header = false;
    for line in input.lines() {
        if line.starts_with("Proto ") {
            header = true;
            continue;
        }
        if !header || line.trim().is_empty() {
            continue;
        }
        let mut words = line.split_whitespace();
        let kind = words.next().ok_or("Missing kernel socket protocol")?;
        let family = match kind {
            "tcp4" | "udp4" => "IPv4",
            "tcp6" | "udp6" => "IPv6",
            "tcp46" | "udp46" => "IPv4+IPv6",
            _ => return Err("Unknown kernel socket family".into()),
        };
        words.next().ok_or("Missing receive queue")?;
        words.next().ok_or("Missing send queue")?;
        let endpoint = words.next().ok_or("Missing kernel bind")?;
        let remote = words.next().ok_or("Missing kernel peer column")?;
        let state = words.next();
        if (protocol == "tcp" && state != Some("LISTEN")) || (protocol == "udp" && remote != "*.*")
        {
            continue;
        }
        let (address, port) = endpoint
            .rsplit_once('.')
            .ok_or("Unrecognized kernel endpoint")?;
        if port == "*" || local(address) {
            continue;
        }
        let port: u32 = port.parse().map_err(|_| "Invalid kernel port")?;
        if !(1..=65535).contains(&port) {
            return Err("Invalid kernel port".into());
        }
        if address != "*" {
            address
                .split('%')
                .next()
                .ok_or("Missing bind")?
                .parse::<std::net::IpAddr>()
                .map_err(|_| "Invalid kernel IP")?;
        }
        let found = report.binds.iter().any(|v| {
            v.value.as_ref().is_some_and(|v| {
                v.protocol == protocol && v.family == family && v.bind == address && v.port == port
            })
        });
        if !found {
            if report.binds.len() == MAX_FACTS {
                return Err("Kernel bind count limit exceeded".into());
            }
            report.binds.push(api::HostBind {
                owner: String::new(),
                value: Some(api::SocketBind {
                    protocol: protocol.into(),
                    family: family.into(),
                    bind: address.into(),
                    port,
                    ..Default::default()
                }),
            });
        }
    }
    if !header {
        return Err(format!(
            "Kernel {protocol} socket table header missing ({} bytes)",
            input.len()
        ));
    }
    Ok(())
}
pub(super) fn parse_ss(input: &str, family: &str, report: &mut api::HostReport) -> Result<()> {
    for line in input.lines() {
        let row: Vec<_> = line.split_whitespace().take(8).collect();
        if row.len() < 5 {
            return Err("Unrecognized kernel socket row".into());
        }
        if !matches!(row[0], "tcp" | "udp") {
            return Err("Unrecognized socket protocol".into());
        }
        let Some((ip, port)) = row[4].rsplit_once(':') else {
            return Err("Unrecognized socket endpoint".into());
        };
        let port: u32 = port.parse().map_err(|_| "Invalid socket port")?;
        let ip = ip.trim_matches(['[', ']']);
        let Some((_, owner)) = line.split_once("pid=") else {
            if local(ip) {
                continue;
            }
            if report.binds.len() == MAX_FACTS {
                return Err("Socket count limit exceeded".into());
            }
            report.binds.push(api::HostBind {
                owner: String::new(),
                value: Some(api::SocketBind {
                    protocol: row[0].into(),
                    family: family.into(),
                    bind: ip.into(),
                    port,
                    ..Default::default()
                }),
            });
            continue;
        };
        let pid = owner
            .split([',', ')'])
            .next()
            .ok_or("Missing process")?
            .parse()
            .map_err(|_| "Invalid process")?;
        let name = line
            .split("((\"")
            .nth(1)
            .and_then(|v| v.split('"').next())
            .ok_or("Missing process name")?;
        bind(report, pid, name, row[0], family, ip, port)?;
    }
    Ok(())
}

async fn routes(runner: &mut Runner, report: &mut api::HostReport) -> Result<()> {
    let mut rows = Vec::new();
    match report.platform.as_str() {
        "macos" => {
            for (family, arg) in [("IPv4", "inet"), ("IPv6", "inet6")] {
                let output = runner
                    .run("/usr/sbin/netstat", &["-rnW", "-f", arg])
                    .await?;
                let mut header = false;
                for line in output.text.lines() {
                    let parts: Vec<_> = line.split_whitespace().take(8).collect();
                    if parts.starts_with(&["Destination", "Gateway", "Flags", "Netif"]) {
                        header = true;
                        continue;
                    }
                    if !header || parts.is_empty() {
                        continue;
                    }
                    if parts.len() < 4 {
                        return Err("Unrecognized route columns".into());
                    }
                    if rows.len() == MAX_FACTS {
                        return Err("Route count limit exceeded".into());
                    }
                    rows.push(api::LocalRoute {
                        destination: parts[0].into(),
                        gateway: parts[1].into(),
                        flags: parts[2].into(),
                        interface: parts[3].into(),
                        family: family.into(),
                        link_layer: parts[2].contains('L'),
                    });
                }
                if !header {
                    return Err("Routing table header missing".into());
                }
            }
        }
        "linux" => {
            for (family, arg) in [("IPv4", "-4"), ("IPv6", "-6")] {
                let output = runner
                    .run("ip", &["-j", arg, "route", "show", "table", "all"])
                    .await?;
                let values = json(&output.text)?;
                for value in array(&values)? {
                    if rows.len() == MAX_FACTS {
                        return Err("Route count limit exceeded".into());
                    }
                    rows.push(api::LocalRoute {
                        destination: string(value, "dst")?.into(),
                        gateway: value
                            .get("gateway")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .into(),
                        flags: route_qualifiers(value)?,
                        interface: value
                            .get("dev")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .into(),
                        family: family.into(),
                        link_layer: value.get("type").and_then(Value::as_str) == Some("local"),
                    });
                }
            }
        }
        "windows" => {
            let output=ps(runner,"$ErrorActionPreference='Stop'; [Console]::OutputEncoding=[Text.UTF8Encoding]::new($false); ConvertTo-Json -InputObject @(Get-NetRoute -IncludeAllCompartments | Select-Object DestinationPrefix,NextHop,InterfaceIndex,CompartmentId,@{n='Family';e={$_.AddressFamily.ToString()}},@{n='Protocol';e={$_.Protocol.ToString()}}) -Depth 4 -Compress").await?;
            let values = json(&output)?;
            for value in array(&values)? {
                rows.push(api::LocalRoute {
                    destination: string(value, "DestinationPrefix")?.into(),
                    gateway: string(value, "NextHop")?.into(),
                    interface: value
                        .get("InterfaceIndex")
                        .and_then(Value::as_u64)
                        .ok_or("Route interface missing")?
                        .to_string(),
                    family: string(value, "Family")?.into(),
                    flags: format!(
                        "protocol={};compartment={}",
                        string(value, "Protocol")?,
                        value
                            .get("CompartmentId")
                            .and_then(Value::as_u64)
                            .ok_or("Route compartment missing")?
                    ),
                    link_layer: false,
                });
            }
        }
        _ => return Err("Unsupported route platform".into()),
    }
    let network = report.network.as_mut().ok_or("Network missing")?;
    network.routes = rows;
    network.routes_complete = true;
    network.route_coverage =
        "Native local IPv4/IPv6 tables; scopes and source notation preserved".into();
    Ok(())
}

fn route_qualifiers(value: &Value) -> Result<String> {
    let mut flags = Vec::new();
    for name in ["table", "type", "protocol", "scope", "metric"] {
        if let Some(value) = value.get(name) {
            let text = match value {
                Value::String(v) => v.clone(),
                Value::Number(v) => v.to_string(),
                _ => return Err("Invalid route qualifier".into()),
            };
            if text.len() > 64 || text.chars().any(char::is_control) {
                return Err("Route qualifier limit exceeded".into());
            }
            flags.push(format!("{name}={text}"));
        }
    }
    let result = flags.join(";");
    if result.len() > 128 {
        return Err("Route qualifier limit exceeded".into());
    }
    Ok(result)
}

pub(super) fn parse_alf_apps(input: &str) -> Result<Vec<api::ApplicationRule>> {
    let mut lines = input.lines();
    let count: usize = lines
        .next()
        .and_then(|v| v.strip_prefix("Total number of apps = "))
        .ok_or("Unrecognized ALF header")?
        .trim()
        .parse()
        .map_err(|_| "Invalid ALF count")?;
    if count > MAX_FACTS {
        return Err("Application rule limit exceeded".into());
    }
    let mut rows = Vec::with_capacity(count);
    let mut subject = None;
    for line in lines {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some((number, path)) = line.split_once(" : ") {
            let number: usize = number.parse().map_err(|_| "Invalid ALF rule number")?;
            if subject.is_some()
                || number != rows.len() + 1
                || !(path.starts_with('/')
                    || (path.contains('.')
                        && path
                            .bytes()
                            .all(|v| v.is_ascii_alphanumeric() || matches!(v, b'.' | b'-' | b'_'))))
                || path.len() > 1024
            {
                return Err("Invalid ALF subject".into());
            }
            subject = Some(path);
        } else {
            let action = match line {
                "(Allow incoming connections)" => "allow incoming",
                "(Block incoming connections)" => "block incoming",
                _ => return Err("Unrecognized ALF action".into()),
            };
            if rows.len() == count {
                return Err("ALF count changed during read".into());
            }
            rows.push(api::ApplicationRule {
                subject: subject.take().ok_or("ALF action without subject")?.into(),
                action: action.into(),
            });
        }
    }
    if subject.is_some() || rows.len() != count {
        return Err("Incomplete ALF rules".into());
    }
    Ok(rows)
}

async fn firewall(runner: &mut Runner, report: &mut api::HostReport) -> Result<()> {
    let mut packet = Vec::new();
    match report.platform.as_str() {
        "macos" => {
            let alf = mac_alf(runner, report).await;
            if let Err(error) = &alf {
                let network = report.network.as_mut().ok_or("Network missing")?;
                network.application_coverage = error.clone();
                network.rules_complete = false;
            }
            result(
                report,
                "application-firewall",
                alf,
                "ALF source app paths and settings; not an effective ingress verdict",
            );
            for (name, args) in [
                ("status", &["-s", "info"][..]),
                ("filter", &["-a", "*", "-sr"]),
                ("NAT", &["-a", "*", "-sn"]),
                ("anchors", &["-s", "Anchors"]),
                ("tables", &["-s", "Tables"]),
                ("states", &["-s", "states"]),
            ] {
                let outcome = runner.run("/sbin/pfctl", args).await;
                let (status, detail) = match outcome {
                    Ok(value) => (
                        if value.warnings {
                            "partial"
                        } else {
                            "verified"
                        },
                        format!(
                            "{name}: {} readable source rows; no effective packet decision inferred",
                            value.text.lines().count()
                        ),
                    ),
                    Err(error) => (
                        if error == "Permission denied" {
                            "blocked"
                        } else {
                            "failed"
                        },
                        format!("{name}: {error}"),
                    ),
                };
                coverage(report, &format!("packet-filter-{name}"), status, &detail);
                packet.push(format!("{name}: {status}"));
            }
        }
        "linux" => {
            let outcome: Result<String> = async {
                let output = runner.run("nft", &["-j", "list", "ruleset"]).await?;
                let value = json(&output.text)?;
                let rows = array(value.get("nftables").ok_or("Missing nftables result")?)?;
                Ok(format!(
                    "nftables: {} source entries; effective ingress unknown",
                    rows.len()
                ))
            }
            .await;
            match outcome {
                Ok(detail) => {
                    coverage(report, "packet-filter", "verified", &detail);
                    packet.push(detail);
                }
                Err(error) => {
                    result(report, "packet-filter", Err(error.clone()), "");
                    packet.push(error);
                }
            }
            report
                .network
                .as_mut()
                .ok_or("Network missing")?
                .application_coverage =
                "No separate application firewall source selected on Linux".into();
        }
        "windows" => {
            let outcome: Result<String> = async {
                let output=ps(runner,"$ErrorActionPreference='Stop'; [Console]::OutputEncoding=[Text.UTF8Encoding]::new($false); ConvertTo-Json -InputObject @(Get-NetFirewallProfile -PolicyStore ActiveStore | Select-Object Name,@{n='Enabled';e={$_.Enabled.ToString()}},@{n='Inbound';e={$_.DefaultInboundAction.ToString()}},@{n='Outbound';e={$_.DefaultOutboundAction.ToString()}}) -Depth 4 -Compress").await?;
                let value = json(&output)?;
                let mut profiles = Vec::new();
                for row in array(&value)? {
                    profiles.push(format!("{}: enabled={}, inbound={}, outbound={}", string(row,"Name")?, string(row,"Enabled")?, string(row,"Inbound")?, string(row,"Outbound")?));
                }
                if profiles.len() > 3 { return Err("Unexpected firewall profile count".into()); }
                Ok(format!("ActiveStore: {}; effective ingress unknown", profiles.join("; ")))
            }.await;
            match outcome {
                Ok(detail) => {
                    coverage(report, "packet-filter", "verified", &detail);
                    packet.push(detail);
                }
                Err(error) => {
                    result(report, "packet-filter", Err(error.clone()), "");
                    packet.push(error);
                }
            }
            report
                .network
                .as_mut()
                .ok_or("Network missing")?
                .application_coverage =
                "ActiveStore profile configuration only; per-application matching unavailable"
                    .into();
        }
        _ => return Err("Unsupported firewall platform".into()),
    }
    let network = report.network.as_mut().ok_or("Network missing")?;
    network.packet_filter_coverage = packet.join("; ");
    Ok(())
}

async fn mac_alf(runner: &mut Runner, report: &mut api::HostReport) -> Result<()> {
    let mut values = Vec::new();
    for arg in [
        "--getglobalstate",
        "--getblockall",
        "--getallowsigned",
        "--listapps",
    ] {
        let output = runner
            .run("/usr/libexec/ApplicationFirewall/socketfilterfw", &[arg])
            .await?;
        if output.warnings {
            return Err("Application firewall source reported warnings".into());
        }
        values.push(output.text);
    }
    let enabled = match values[0].trim() {
        "Firewall is enabled. (State = 1)" => true,
        "Firewall is disabled. (State = 0)" => false,
        _ => return Err("Unrecognized ALF global setting".into()),
    };
    let block_all = match values[1].trim() {
        "Firewall has block all state set to enabled." => true,
        "Firewall has block all state set to disabled." => false,
        _ => return Err("Unrecognized ALF block-all setting".into()),
    };
    let mut builtin = None;
    let mut downloaded = None;
    for line in values[2].lines() {
        if let Some(value) = line.strip_prefix("Automatically allow built-in signed software ") {
            builtin = Some(alf_bool(value)?);
        } else if let Some(value) =
            line.strip_prefix("Automatically allow downloaded signed software ")
        {
            downloaded = Some(alf_bool(value)?);
        } else {
            return Err("Unrecognized ALF signed-app setting".into());
        }
    }
    let rules = parse_alf_apps(&values[3])?;
    let network = report.network.as_mut().ok_or("Network missing")?;
    network.firewall_enabled = Some(enabled);
    network.block_all = Some(block_all);
    network.allow_builtin_signed = Some(builtin.ok_or("Missing ALF builtin setting")?);
    network.allow_downloaded_signed = Some(downloaded.ok_or("Missing ALF downloaded setting")?);
    network.rules = rules;
    network.rules_complete = true;
    network.application_coverage = "Verified ALF settings and app identities; executable-to-rule and effective packet decisions unknown".into();
    Ok(())
}
fn alf_bool(value: &str) -> Result<bool> {
    match value {
        "ENABLED." => Ok(true),
        "DISABLED." => Ok(false),
        _ => Err("Unrecognized ALF boolean".into()),
    }
}
