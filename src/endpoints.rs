//! Operator declarations and host consistency checks, never consensus proofs.
use crate::{ChainInfo, ClientError, ClientOptions, NodeClient, normalize_node_url};
use serde::Deserialize;
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    time::Duration,
};
use tokio_util::sync::CancellationToken;
use url::Host;

pub const OPERATOR_DECLARED: &str = "operator-declared, not independently verified";
fn public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(a == 0
        || a == 10
        || a == 127
        || a >= 224
        || (a == 100 && (64..=127).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 168)
        || (a == 192 && b == 0 && (c == 0 || c == 2))
        || (a == 192 && b == 88 && c == 99)
        || (a == 198 && (b == 18 || b == 19))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113))
}
fn public_v6(ip: Ipv6Addr) -> bool {
    let g = ip.segments();
    let embedded = |a: u16, b: u16| Ipv4Addr::new((a >> 8) as u8, a as u8, (b >> 8) as u8, b as u8);
    if g[..5] == [0; 5] && g[5] == 0xffff {
        return public_v4(embedded(g[6], g[7]));
    }
    if g[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        return public_v4(embedded(g[6], g[7]));
    }
    if g[0] == 0x2002 {
        return public_v4(embedded(g[1], g[2]));
    }
    (g[0] & 0xe000) == 0x2000 && !(g[0] == 0x2001 && (g[1] < 0x200 || g[1] == 0xdb8))
}
pub fn is_public_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => public_v4(v),
        IpAddr::V6(v) => public_v6(v),
    }
}
/// Literal-address/name policy. Like the reference's default resolver, this does
/// not pin DNS answers or prevent DNS rebinding; supply a transport with that
/// control when names originate from untrusted operators.
pub fn declared_endpoint_url(input: &str) -> Option<String> {
    let url = normalize_node_url(input).ok()?;
    if url.scheme() != "https" {
        return None;
    }
    match url.host()? {
        Host::Ipv4(ip) if !public_v4(ip) => return None,
        Host::Ipv6(ip) if !public_v6(ip) => return None,
        Host::Domain(host) => {
            let name = host.trim_end_matches('.');
            if !name.contains('.')
                || [".localhost", ".local", ".internal", ".home.arpa"]
                    .iter()
                    .any(|s| name.ends_with(s))
            {
                return None;
            }
        }
        _ => {}
    }
    Some(
        url.as_str()
            .strip_suffix('/')
            .unwrap_or(url.as_str())
            .to_owned(),
    )
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeclaredEndpoints {
    pub chain_path: Vec<String>,
    pub committed_block: Option<String>,
    pub endpoints: Vec<String>,
    #[serde(default, deserialize_with = "crate::wire::present")]
    pub submit_endpoints: Option<Vec<String>>,
}
#[derive(Debug, Clone)]
pub struct ResolvedEndpoint {
    pub url: String,
    pub chain_path: Vec<String>,
    pub declares_submit: bool,
    pub trust: &'static str,
}
#[derive(Clone, Copy)]
pub enum EndpointQuery {
    Genesis,
    Info,
    Declared,
}
#[allow(async_fn_in_trait)]
pub trait EndpointReader {
    /// None disables optional DNS checking, matching the reference default.
    /// Some(empty) and any non-public address refuse the declared host.
    async fn lookup(
        &self,
        _hostname: &str,
        _cancel: &CancellationToken,
    ) -> Result<Option<Vec<IpAddr>>, ClientError> {
        Ok(None)
    }

    fn maximum_response_bytes(&self) -> usize {
        64 * 1024
    }
    /// Implementations apply the same ordered/advancing-page checks as NodeClient.
    async fn directories(
        &self,
        _base: &str,
        _path: &[String],
        _after: Option<&str>,
        _limit: u64,
        _maximum_response_bytes: usize,
        _cancel: &CancellationToken,
    ) -> Result<crate::ChildDirectoriesPage, ClientError> {
        Err(ClientError::InvalidResponse(
            "directory reads not implemented",
        ))
    }

    async fn read(
        &self,
        base: &str,
        path: &[String],
        query: EndpointQuery,
        cancel: &CancellationToken,
    ) -> Result<serde_json::Value, ClientError>;
}
pub struct HttpEndpointReader {
    pub options: ClientOptions,
    /// Check all DNS answers before requests. This does not pin the HTTP
    /// connector's later lookup, so it does not eliminate DNS rebinding.
    pub check_dns: bool,
}
impl Default for HttpEndpointReader {
    fn default() -> Self {
        Self {
            check_dns: false,
            options: ClientOptions {
                timeout: Duration::from_secs(4),
                maximum_response_bytes: 64 * 1024,
                authorization: None,
            },
        }
    }
}
impl EndpointReader for HttpEndpointReader {
    async fn lookup(
        &self,
        hostname: &str,
        cancel: &CancellationToken,
    ) -> Result<Option<Vec<IpAddr>>, ClientError> {
        if !self.check_dns {
            return Ok(None);
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (hostname, cancel);
            Err(ClientError::Configuration(
                "browser DNS inspection is unavailable",
            ))
        }
        #[cfg(not(target_arch = "wasm32"))]
        tokio::select! { biased;
            _ = cancel.cancelled() => Err(ClientError::Cancelled),
            result = tokio::net::lookup_host((hostname, 443)) => {
                Ok(Some(result.map(|addresses| addresses.map(|a| a.ip()).collect()).unwrap_or_default()))
            }
        }
    }

    fn maximum_response_bytes(&self) -> usize {
        self.options.maximum_response_bytes
    }
    async fn directories(
        &self,
        base: &str,
        path: &[String],
        after: Option<&str>,
        limit: u64,
        maximum_response_bytes: usize,
        cancel: &CancellationToken,
    ) -> Result<crate::ChildDirectoriesPage, ClientError> {
        let mut options = self.options.clone();
        options.authorization = None;
        options.maximum_response_bytes = maximum_response_bytes;
        NodeClient::new(base, path.to_vec(), options)?
            .child_directories(after, Some(limit), cancel)
            .await
    }

    async fn read(
        &self,
        base: &str,
        path: &[String],
        query: EndpointQuery,
        cancel: &CancellationToken,
    ) -> Result<serde_json::Value, ClientError> {
        // Operator credentials must not travel to third-party declared hosts.
        let mut options = self.options.clone();
        options.authorization = None;
        let client = NodeClient::new(base, path.to_vec(), options)?;
        let route = match query {
            EndpointQuery::Genesis => "genesis",
            EndpointQuery::Info => "info",
            EndpointQuery::Declared => "endpoints",
        };
        client.get(&["api", "chain", route], cancel).await
    }
}
#[derive(Clone, Debug)]
pub struct ResolverOptions {
    pub maximum_sources: usize,
    pub maximum_candidates: usize,
    pub maximum_requests: usize,
    pub timeout: Duration,
}
impl Default for ResolverOptions {
    fn default() -> Self {
        Self {
            maximum_sources: 4,
            maximum_candidates: 8,
            maximum_requests: 64,
            timeout: Duration::from_secs(4),
        }
    }
}
pub(crate) enum RequestBudget {
    Limited(usize),
    Unlimited,
}
impl RequestBudget {
    fn spend(&mut self) -> Result<(), ClientError> {
        if let Self::Limited(remaining) = self {
            if *remaining == 0 {
                return Err(ClientError::Configuration(
                    "endpoint discovery request budget exhausted",
                ));
            }
            *remaining -= 1;
        }
        Ok(())
    }
}
pub struct EndpointResolver<R = HttpEndpointReader> {
    pub(crate) root_url: String,
    pub(crate) root_path: Vec<String>,
    pub(crate) reader: R,
    pub(crate) options: ResolverOptions,
}
impl<R: EndpointReader> EndpointResolver<R> {
    pub fn new(
        root_url: &str,
        root_path: Vec<String>,
        reader: R,
        options: ResolverOptions,
    ) -> Result<Self, ClientError> {
        if root_path.is_empty() || options.timeout.is_zero() {
            return Err(ClientError::Configuration(
                "root path and positive deadline required",
            ));
        }
        let url = normalize_node_url(root_url)?;
        Ok(Self {
            root_url: url
                .as_str()
                .strip_suffix('/')
                .unwrap_or(url.as_str())
                .into(),
            root_path,
            reader,
            options,
        })
    }
    pub(crate) async fn request(
        &self,
        source: &str,
        path: &[String],
        query: EndpointQuery,
        budget: &mut RequestBudget,
        cancel: &CancellationToken,
    ) -> Result<serde_json::Value, ClientError> {
        budget.spend()?;
        tokio::select! { biased;
            _=cancel.cancelled()=>Err(ClientError::Cancelled),
            result=crate::runtime::timeout(self.options.timeout,self.reader.read(source,path,query,cancel))=>result.map_err(|_|ClientError::Timeout)?,
        }
    }
    async fn declaration(
        &self,
        source: &str,
        path: &[String],
        budget: &mut RequestBudget,
        cancel: &CancellationToken,
    ) -> Result<DeclaredEndpoints, ClientError> {
        let value = self
            .request(source, path, EndpointQuery::Declared, budget, cancel)
            .await?;
        let answer: DeclaredEndpoints = serde_json::from_value(value)?;
        if answer.chain_path != path {
            return Err(ClientError::InvalidResponse(
                "endpoints name a different chain",
            ));
        }
        Ok(answer)
    }
    pub async fn declared(
        &self,
        source: &str,
        path: &[String],
        cancel: &CancellationToken,
    ) -> Result<DeclaredEndpoints, ClientError> {
        normalize_node_url(source)?;
        self.declaration(source, path, &mut RequestBudget::Limited(1), cancel)
            .await
    }
    async fn resolves_public(&self, url: &str, cancel: &CancellationToken) -> bool {
        let Ok(parsed) = url::Url::parse(url) else {
            return false;
        };
        let Some(Host::Domain(hostname)) = parsed.host() else {
            return true;
        };
        let result = tokio::select! { biased;
            _ = cancel.cancelled() => return false,
            result = crate::runtime::timeout(self.options.timeout, self.reader.lookup(hostname, cancel)) => result,
        };
        match result {
            Ok(Ok(None)) => true,
            Ok(Ok(Some(addresses))) => {
                !addresses.is_empty() && addresses.into_iter().all(is_public_address)
            }
            _ => false,
        }
    }
    pub(crate) fn root_failure(&self, source: &str, error: ClientError) -> Result<(), ClientError> {
        if source == self.root_url
            && (matches!(error, ClientError::Timeout)
                || matches!(&error,ClientError::Node{status,..}if *status>=400&&*status!=404))
        {
            Err(error)
        } else {
            Ok(())
        }
    }
    pub(crate) async fn level(
        &self,
        sources: &[String],
        level: &[String],
        network: &str,
        budget: &mut RequestBudget,
        cancel: &CancellationToken,
    ) -> Result<Vec<ResolvedEndpoint>, ClientError> {
        let mut candidates: Vec<(String, bool)> = Vec::new();
        for source in sources.iter().take(self.options.maximum_sources) {
            let answer = match self.declaration(source, level, budget, cancel).await {
                Ok(v) => v,
                Err(error) => {
                    if level.len() == self.root_path.len() + 1 {
                        self.root_failure(source, error)?;
                    }
                    continue;
                }
            };
            let submit: Vec<_> = answer
                .submit_endpoints
                .unwrap_or_default()
                .iter()
                .filter_map(|u| declared_endpoint_url(u))
                .collect();
            for declared in answer.endpoints {
                let Some(url) = declared_endpoint_url(&declared) else {
                    continue;
                };
                let declares = submit.contains(&url);
                if let Some(existing) = candidates.iter_mut().find(|(u, _)| u == &url) {
                    existing.1 |= declares;
                } else if candidates.len() < self.options.maximum_candidates {
                    candidates.push((url, declares));
                }
            }
        }
        let mut resolved = Vec::new();
        for (url, declares_submit) in candidates {
            if !self.resolves_public(&url, cancel).await {
                continue;
            }
            let info = self
                .request(&url, level, EndpointQuery::Info, budget, cancel)
                .await;
            let genesis = self
                .request(&url, level, EndpointQuery::Genesis, budget, cancel)
                .await;
            if let (Ok(info), Ok(genesis)) = (info, genesis)
                && let Ok(info) = serde_json::from_value::<ChainInfo>(info)
                && info.chain == level
                && info.height.is_some()
                && genesis.get("genesisHash").and_then(|v| v.as_str()) == Some(network)
            {
                resolved.push(ResolvedEndpoint {
                    url,
                    chain_path: level.to_vec(),
                    declares_submit,
                    trust: OPERATOR_DECLARED,
                });
            }
        }
        if cancel.is_cancelled() {
            return Err(ClientError::Cancelled);
        }
        Ok(resolved)
    }
    pub async fn resolve(
        &self,
        path: &[String],
        cancel: &CancellationToken,
    ) -> Result<Vec<ResolvedEndpoint>, ClientError> {
        if path.len() <= self.root_path.len()
            || !path.starts_with(&self.root_path)
            || path.iter().any(|p| p.is_empty() || p.contains('/'))
        {
            return Err(ClientError::Configuration(
                "path must name a descendant with nonempty components",
            ));
        }
        let mut budget = RequestBudget::Limited(self.options.maximum_requests);
        let genesis = self
            .request(
                &self.root_url,
                &self.root_path,
                EndpointQuery::Genesis,
                &mut budget,
                cancel,
            )
            .await?;
        let network = genesis
            .get("genesisHash")
            .and_then(|v| v.as_str())
            .ok_or(ClientError::InvalidResponse("invalid genesis response"))?;
        let mut sources = vec![self.root_url.clone()];
        let mut resolved = Vec::new();
        for depth in self.root_path.len() + 1..=path.len() {
            let level = &path[..depth];
            resolved = self
                .level(&sources, level, network, &mut budget, cancel)
                .await?;
            if resolved.is_empty() {
                return Ok(resolved);
            }
            sources = resolved.iter().map(|r| r.url.clone()).collect();
        }
        Ok(resolved)
    }
}
