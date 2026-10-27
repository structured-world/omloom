//! Read-only, request-scoped development observer. No provider payload or credential reaches storage.
use omloom_contracts::workspace as api;
use reqwest::{
    Client, Url,
    header::{AUTHORIZATION, COOKIE, HeaderValue},
};
use serde::{Deserialize, de::DeserializeOwned};
use std::{net::IpAddr, sync::Arc, time::Duration};
use tonic::{Code, Status};

const MAX_BODY: usize = 128 * 1024;
fn error(code: Code, reason: &str, message: &str) -> Status {
    super::status(code, reason, message)
}
fn invalid() -> Status {
    error(
        Code::InvalidArgument,
        "OBSERVE_INPUT",
        "Supply one HTTPS endpoint and one complete credential method.",
    )
}
fn format_error() -> Status {
    error(
        Code::FailedPrecondition,
        "PROVIDER_FORMAT",
        "The provider response exceeds the supported shape or limits.",
    )
}
#[derive(Deserialize)]
struct Envelope<T> {
    data: T,
}
#[derive(Deserialize)]
struct Version {
    version: String,
}
#[derive(Deserialize)]
struct Node {
    node: String,
}
#[derive(Deserialize)]
struct Ticket {
    ticket: String,
}
#[derive(Deserialize)]
struct Cpu {
    model: Option<String>,
}
#[derive(Deserialize)]
struct Memory {
    total: Option<u64>,
}
#[derive(Deserialize)]
struct NodeStatus {
    cpuinfo: Option<Cpu>,
    memory: Option<Memory>,
}
#[derive(Deserialize)]
struct Interface {
    iface: String,
    cidr: Option<String>,
    cidr6: Option<String>,
    address: Option<String>,
    address6: Option<String>,
    #[serde(rename = "vlan-id")]
    vlan_id: Option<u32>,
}

// Bound collection capacity before deserializing elements, including the first excess element.
struct Bounded<T, const N: usize>(Vec<T>);
impl<'de, T: Deserialize<'de>, const N: usize> Deserialize<'de> for Bounded<T, N> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Items<T, const N: usize>(std::marker::PhantomData<T>);
        impl<'de, T: Deserialize<'de>, const N: usize> serde::de::Visitor<'de> for Items<T, N> {
            type Value = Bounded<T, N>;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "at most {N} items")
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                let mut items = Vec::with_capacity(N);
                for _ in 0..N {
                    match seq.next_element()? {
                        Some(item) => items.push(item),
                        None => return Ok(Bounded(items)),
                    }
                }
                struct Excess;
                impl<'de> serde::de::DeserializeSeed<'de> for Excess {
                    type Value = ();
                    fn deserialize<D: serde::Deserializer<'de>>(
                        self,
                        _: D,
                    ) -> Result<(), D::Error> {
                        Err(serde::de::Error::custom("too many provider items"))
                    }
                }
                seq.next_element_seed(Excess)?;
                Ok(Bounded(items))
            }
        }
        d.deserialize_seq(Items::<T, N>(std::marker::PhantomData))
    }
}
fn endpoint(text: &str) -> Result<Url, Status> {
    if text.len() > 2048 {
        return Err(invalid());
    }
    let mut url = Url::parse(text).map_err(|_| invalid())?;
    let local = matches!(url.host_str(), Some("127.0.0.1" | "[::1]"));
    if (url.scheme() != "https" && !(url.scheme() == "http" && local))
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path().trim_end_matches('/'), "" | "/api2/json")
    {
        return Err(invalid());
    }
    url.set_path("/api2/json/");
    Ok(url)
}
fn path(base: &Url, parts: &[&str]) -> Result<Url, Status> {
    let mut url = base.clone();
    {
        let mut segments = url.path_segments_mut().map_err(|_| invalid())?;
        segments.pop_if_empty();
        for part in parts {
            segments.push(part);
        }
    }
    Ok(url)
}
async fn decode<T: DeserializeOwned>(mut response: reqwest::Response) -> Result<T, Status> {
    if !response.status().is_success() {
        return Err(match response.status().as_u16() {
            401 => error(
                Code::Unauthenticated,
                "PROVIDER_AUTH",
                "Proxmox requires valid credentials, including the account realm.",
            ),
            403 => error(
                Code::PermissionDenied,
                "PROVIDER_DENIED",
                "The supplied account cannot read this Proxmox resource.",
            ),
            _ => error(
                Code::Unavailable,
                "PROVIDER_RESPONSE",
                "The Proxmox endpoint could not complete this read.",
            ),
        });
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_BODY as u64)
    {
        return Err(format_error());
    }
    let mut bytes = Vec::with_capacity(MAX_BODY);
    while let Some(chunk) = response.chunk().await.map_err(|_| {
        error(
            Code::Unavailable,
            "PROVIDER_READ",
            "The provider response was interrupted.",
        )
    })? {
        if chunk.len() > MAX_BODY - bytes.len() {
            return Err(format_error());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice::<Envelope<T>>(&bytes)
        .map(|r| r.data)
        .map_err(|_| format_error())
}
struct Connection {
    client: Client,
    base: Url,
    authorization: Option<(reqwest::header::HeaderName, HeaderValue)>,
}
impl Connection {
    async fn get<T: DeserializeOwned>(&self, parts: &[&str]) -> Result<T, Status> {
        let mut request = self.client.get(path(&self.base, parts)?);
        if let Some((name, value)) = &self.authorization {
            request = request.header(name, value);
        }
        decode(request.send().await.map_err(|_|error(Code::Unavailable,"PROVIDER_CONNECT","Cannot connect securely to the selected Proxmox endpoint. Check its address and certificate."))?).await
    }
}
fn add_address(
    node: &mut api::ObservedHypervisor,
    interface: &Interface,
    value: &str,
) -> Result<(), Status> {
    if value.len() > 128 {
        return Err(format_error());
    }
    let (literal, prefix) = match value.split_once('/') {
        Some((ip, prefix)) => (ip, Some(prefix.parse::<u32>().map_err(|_| format_error())?)),
        None => (value, None),
    };
    let ip: IpAddr = literal.parse().map_err(|_| format_error())?;
    let local = match ip {
        IpAddr::V6(ip) => ip
            .to_ipv4_mapped()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V6(ip)),
        IpAddr::V4(_) => ip,
    };
    if local.is_loopback()
        || local.is_unspecified()
        || match local {
            IpAddr::V4(ip) => ip.is_link_local(),
            IpAddr::V6(ip) => ip.is_unicast_link_local(),
        }
    {
        return Ok(());
    }
    if prefix.is_some_and(|n| n > if ip.is_ipv4() { 32 } else { 128 })
        || interface.vlan_id.is_some_and(|n| !(1..=4094).contains(&n))
    {
        return Err(format_error());
    }
    let address = ip.to_string();
    if node
        .addresses
        .iter()
        .any(|a| a.address == address && a.interface_name.as_deref() == Some(&interface.iface))
    {
        return Ok(());
    }
    if node.addresses.len() == 128 {
        return Err(format_error());
    }
    node.addresses.push(api::InitialAddress {
        address,
        prefix_length: prefix,
        vlan_tag: interface.vlan_id,
        interface_name: Some(interface.iface.clone()),
    });
    Ok(())
}
async fn inner(request: api::ObserveProxmox) -> Result<api::ObservationPreview, Status> {
    if [
        &request.username,
        &request.password,
        &request.token_id,
        &request.token_secret,
    ]
    .iter()
    .any(|s| s.len() > 1024)
    {
        return Err(invalid());
    }
    let base = endpoint(&request.endpoint)?;
    let token = !request.token_id.is_empty() || !request.token_secret.is_empty();
    let login = !request.username.is_empty() || !request.password.is_empty();
    if (token && login)
        || (token && (request.token_id.is_empty() || request.token_secret.is_empty()))
        || (login && (request.username.is_empty() || request.password.is_empty()))
    {
        return Err(invalid());
    }
    let roots = rustls::RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    };
    let tls =
        rustls::ClientConfig::builder_with_provider(Arc::new(rustls_graviola::default_provider()))
            .with_safe_default_protocol_versions()
            .map_err(|_| format_error())?
            .with_root_certificates(roots)
            .with_no_client_auth();
    let client = Client::builder()
        .tls_backend_preconfigured(tls)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|_| format_error())?;
    let mut connection = Connection {
        client,
        base,
        authorization: None,
    };
    if token {
        let mut header: HeaderValue =
            format!("PVEAPIToken={}={}", request.token_id, request.token_secret)
                .parse()
                .map_err(|_| invalid())?;
        header.set_sensitive(true);
        connection.authorization = Some((AUTHORIZATION, header));
    } else if login {
        let response = connection
            .client
            .post(path(&connection.base, &["access", "ticket"])?)
            .form(&[
                ("username", request.username.as_str()),
                ("password", request.password.as_str()),
            ])
            .send()
            .await
            .map_err(|_| {
                error(
                    Code::Unavailable,
                    "PROVIDER_CONNECT",
                    "Cannot securely authenticate to the selected endpoint.",
                )
            })?;
        let ticket: Ticket = decode(response).await?;
        if ticket.ticket.starts_with("PVE:tfa!") || ticket.ticket.len() > 8192 {
            return Err(error(
                Code::Unauthenticated,
                "PROVIDER_MFA",
                "Interactive second-factor login requires an API token for this observer.",
            ));
        }
        let mut header: HeaderValue = format!("PVEAuthCookie={}", ticket.ticket)
            .parse()
            .map_err(|_| format_error())?;
        header.set_sensitive(true);
        connection.authorization = Some((COOKIE, header));
    }
    let version: Version = connection.get(&["version"]).await?;
    if version.version.len() > 64 {
        return Err(format_error());
    }
    let nodes: Bounded<Node, 16> = connection.get(&["nodes"]).await?;
    let mut result = api::ObservationPreview {
        version: version.version,
        nodes: Vec::with_capacity(nodes.0.len()),
    };
    for source in nodes.0 {
        if source.node.is_empty() || source.node.len() > 160 {
            return Err(format_error());
        }
        let mut node=api::ObservedHypervisor {name:source.node,addresses:Vec::with_capacity(128),cpu_model:None,memory_bytes:None,limitations:vec!["Hardware chassis identity is not reported by these API reads. Do not merge a physical server by node name or IP.".into()]};
        match connection
            .get::<NodeStatus>(&["nodes", &node.name, "status"])
            .await
        {
            Ok(status) => {
                node.cpu_model = status.cpuinfo.and_then(|c| c.model);
                node.memory_bytes = status.memory.and_then(|m| m.total);
                if node.cpu_model.as_ref().is_some_and(|s| s.len() > 512) {
                    return Err(format_error());
                }
            }
            Err(status) => node.limitations.push(status.message().to_owned()),
        }
        match connection
            .get::<Bounded<Interface, 128>>(&["nodes", &node.name, "network"])
            .await
        {
            Ok(network) => {
                for interface in network.0 {
                    if interface.iface.is_empty() || interface.iface.len() > 160 {
                        return Err(format_error());
                    }
                    for value in [
                        interface.cidr.as_ref().or(interface.address.as_ref()),
                        interface.cidr6.as_ref().or(interface.address6.as_ref()),
                    ]
                    .into_iter()
                    .flatten()
                    {
                        if !value.is_empty() {
                            add_address(&mut node, &interface, value)?;
                        }
                    }
                }
            }
            Err(status) => node.limitations.push(status.message().to_owned()),
        }
        result.nodes.push(node);
    }
    Ok(result)
}
pub(crate) async fn probe(request: api::ObserveProxmox) -> Result<api::ObservationPreview, Status> {
    tokio::time::timeout(Duration::from_secs(12), inner(request))
        .await
        .map_err(|_| {
            error(
                Code::DeadlineExceeded,
                "PROVIDER_TIMEOUT",
                "The bounded observation timed out. No inventory was changed.",
            )
        })?
}

#[cfg(test)]
mod tests;
