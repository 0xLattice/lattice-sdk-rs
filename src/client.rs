use crate::models::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::de::DeserializeOwned;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use url::{Host, Url};

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("invalid node URL or options: {0}")]
    Configuration(&'static str),
    #[error("invalid node response: {0}")]
    InvalidResponse(&'static str),
    #[error("request cancelled")]
    Cancelled,
    #[error("request deadline exceeded")]
    Timeout,
    #[error("HTTP {status}: {refusal:?}")]
    Node {
        status: u16,
        refusal: Option<String>,
    },
    #[error(transparent)]
    Transport(#[from] crate::transport::Error),
    #[error("invalid JSON response: {0}")]
    Json(#[from] serde_json::Error),
}
pub fn normalize_node_url(input: &str) -> Result<Url, ClientError> {
    let url = Url::parse(input).map_err(|_| ClientError::Configuration("invalid URL"))?;
    if !url.username().is_empty() || url.password().is_some() || input.contains(['?', '#']) {
        return Err(ClientError::Configuration(
            "credentials, query and fragment are forbidden",
        ));
    }
    let loopback = match url.host() {
        Some(Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        return Err(ClientError::Configuration(
            "HTTPS required except loopback HTTP",
        ));
    }
    Ok(url)
}
pub fn node_cookie_authorization(cookie: &str) -> Result<String, ClientError> {
    let value = cookie.trim();
    if value.is_empty() || value.chars().any(char::is_whitespace) {
        return Err(ClientError::Configuration(
            "cookie must be a token without whitespace",
        ));
    }
    if value.contains(':') {
        let bytes: Option<Vec<u8>> = value.chars().map(|c| u8::try_from(c as u32).ok()).collect();
        Ok(format!(
            "Basic {}",
            STANDARD.encode(bytes.ok_or(ClientError::Configuration("cookie is not Latin-1"))?)
        ))
    } else {
        Ok(format!("Bearer {value}"))
    }
}
#[derive(Clone, Debug)]
pub struct ClientOptions {
    pub timeout: Duration,
    pub maximum_response_bytes: usize,
    pub authorization: Option<String>,
}
impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(8),
            maximum_response_bytes: 4 * 1024 * 1024,
            authorization: None,
        }
    }
}
#[derive(Clone)]
pub struct NodeClient {
    base: Url,
    chain_path: Vec<String>,
    options: ClientOptions,
    http: crate::transport::Client,
}
impl NodeClient {
    pub fn new(
        base: &str,
        chain_path: Vec<String>,
        options: ClientOptions,
    ) -> Result<Self, ClientError> {
        if chain_path.is_empty() || options.timeout.is_zero() || options.maximum_response_bytes == 0
        {
            return Err(ClientError::Configuration(
                "nonempty path and positive limits required",
            ));
        }
        let http = crate::transport::client()?;
        Ok(Self {
            base: normalize_node_url(base)?,
            chain_path,
            options,
            http,
        })
    }
    pub fn base_url(&self) -> &Url {
        &self.base
    }
    pub fn chain_path(&self) -> &[String] {
        &self.chain_path
    }
    fn route(&self, path: &[&str]) -> Url {
        let mut url = self.base.clone();
        {
            let mut segments = url.path_segments_mut().expect("HTTP URL has path");
            segments.pop_if_empty();
            for part in path {
                segments.push(part);
            }
        }
        url.query_pairs_mut()
            .append_pair("chainPath", &self.chain_path.join("/"));
        url
    }
    pub(crate) async fn get<T: DeserializeOwned>(
        &self,
        path: &[&str],
        cancel: &CancellationToken,
    ) -> Result<T, ClientError> {
        self.get_parameters(path, &[], cancel).await
    }
    async fn get_parameters<T: DeserializeOwned>(
        &self,
        path: &[&str],
        parameters: &[(&str, String)],
        cancel: &CancellationToken,
    ) -> Result<T, ClientError> {
        let mut url = self.route(path);
        url.query_pairs_mut()
            .extend_pairs(parameters.iter().map(|(k, v)| (*k, v.as_str())));
        let work = async {
            let mut request = self
                .http
                .get(url)
                .header(reqwest::header::ACCEPT, "application/json");
            if let Some(auth) = &self.options.authorization {
                request = request.header(reqwest::header::AUTHORIZATION, auth);
            }
            let mut response =
                crate::transport::response_limit(request, self.options.maximum_response_bytes)
                    .send()
                    .await?;
            let status = response.status();
            let too_large = || ClientError::Node {
                status: status.as_u16(),
                refusal: Some("response too large".into()),
            };
            if response
                .content_length()
                .is_some_and(|n| n > self.options.maximum_response_bytes as u64)
            {
                return Err(too_large());
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await? {
                if chunk.len()
                    > self
                        .options
                        .maximum_response_bytes
                        .saturating_sub(bytes.len())
                {
                    return Err(too_large());
                }
                bytes.extend_from_slice(&chunk);
            }
            if !status.is_success() {
                let body = serde_json::from_slice::<serde_json::Value>(&bytes).ok();
                let error = body.as_ref().and_then(|v| v.get("error"));
                let refusal = error
                    .and_then(|e| {
                        e.as_str()
                            .or_else(|| e.get("message").and_then(|m| m.as_str()))
                    })
                    .map(str::to_owned);
                return Err(ClientError::Node {
                    status: status.as_u16(),
                    refusal,
                });
            }
            Ok(serde_json::from_slice(&bytes)?)
        };
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(ClientError::Cancelled),
            result = crate::runtime::timeout(self.options.timeout, work) => result.map_err(|_| ClientError::Timeout)?,
        }
    }
    pub async fn health(&self, cancel: &CancellationToken) -> Result<NodeStatus, ClientError> {
        self.get(&["health"], cancel).await
    }
    pub async fn chain_info(&self, cancel: &CancellationToken) -> Result<ChainInfo, ClientError> {
        self.get(&["api", "chain", "info"], cancel).await
    }
    pub async fn account(
        &self,
        owner: &str,
        cancel: &CancellationToken,
    ) -> Result<AccountState, ClientError> {
        self.get(&["api", "state", "account", owner], cancel).await
    }
    pub async fn block(
        &self,
        id: &str,
        cancel: &CancellationToken,
    ) -> Result<BlockView, ClientError> {
        self.get(&["api", "block", id], cancel).await
    }
    pub async fn latest_block(
        &self,
        cancel: &CancellationToken,
    ) -> Result<LatestBlockView, ClientError> {
        self.get(&["api", "block", "latest"], cancel).await
    }
    pub async fn mempool(&self, cancel: &CancellationToken) -> Result<Mempool, ClientError> {
        self.get(&["api", "mempool"], cancel).await
    }
    pub async fn nexus_genesis(&self, cancel: &CancellationToken) -> Result<String, ClientError> {
        #[derive(serde::Deserialize)]
        struct Genesis {
            #[serde(rename = "genesisHash")]
            hash: String,
        }
        Ok(self
            .get::<Genesis>(&["api", "chain", "genesis"], cancel)
            .await?
            .hash)
    }
}

impl NodeClient {
    pub async fn blocks(
        &self,
        before: Option<&crate::wire::Decimal>,
        limit: Option<u8>,
        cancel: &CancellationToken,
    ) -> Result<BlocksPage, ClientError> {
        if before.is_some_and(|v| v.0.sign() == num_bigint::Sign::Minus)
            || limit.is_some_and(|n| !(1..=25).contains(&n))
        {
            return Err(ClientError::Configuration(
                "nonnegative before and limit 1..25 required",
            ));
        }
        let mut parameters = Vec::new();
        if let Some(v) = before {
            parameters.push(("before", v.0.to_string()));
        }
        if let Some(v) = limit {
            parameters.push(("limit", v.to_string()));
        }
        let page: BlocksPage = self
            .get_parameters(&["api", "blocks"], &parameters, cancel)
            .await?;
        if page.blocks.len() > usize::from(limit.unwrap_or(25)) {
            return Err(ClientError::InvalidResponse("blocks page exceeds limit"));
        }
        let mut ceiling = before;
        for row in &page.blocks {
            if ceiling.is_some_and(|v| row.height >= *v) {
                return Err(ClientError::InvalidResponse(
                    "blocks page is not strictly descending below before",
                ));
            }
            ceiling = Some(&row.height);
        }
        if let Some(next) = &page.next_before
            && (next.0 < 1.into() || ceiling.is_some_and(|v| next > v))
        {
            return Err(ClientError::InvalidResponse(
                "nextBefore does not continue page",
            ));
        }
        Ok(page)
    }
    pub async fn child_directories(
        &self,
        after: Option<&str>,
        limit: Option<u64>,
        cancel: &CancellationToken,
    ) -> Result<ChildDirectoriesPage, ClientError> {
        if limit.is_some_and(|n| n == 0 || n > 9_007_199_254_740_991) {
            return Err(ClientError::Configuration("positive safe limit required"));
        }
        let mut parameters = Vec::new();
        if let Some(v) = after {
            parameters.push(("after", v.to_owned()));
        }
        if let Some(v) = limit {
            parameters.push(("limit", v.to_string()));
        }
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Page {
            chain_path: Vec<String>,
            directories: Vec<String>,
            #[serde(default, deserialize_with = "crate::wire::present")]
            next: Option<String>,
        }
        let page: Page = self
            .get_parameters(&["api", "chain", "children"], &parameters, cancel)
            .await?;
        if page.chain_path.join("/") != self.chain_path.join("/") {
            return Err(ClientError::InvalidResponse(
                "child directories name a different chain",
            ));
        }
        // JavaScript compares strings as UTF-16 code units, not UTF-8 bytes.
        if page
            .directories
            .windows(2)
            .any(|w| w[0].encode_utf16().cmp(w[1].encode_utf16()).is_ge())
        {
            return Err(ClientError::InvalidResponse(
                "child directories are not ordered",
            ));
        }
        if page
            .next
            .as_ref()
            .is_some_and(|n| Some(n) != page.directories.last())
        {
            return Err(ClientError::InvalidResponse(
                "next must be last listed directory",
            ));
        }
        let directories: Vec<_> = page
            .directories
            .into_iter()
            .filter(|n| after.is_none_or(|a| n.encode_utf16().cmp(a.encode_utf16()).is_gt()))
            .collect();
        if page.next.is_some() && directories.is_empty() {
            return Err(ClientError::InvalidResponse(
                "next does not advance past after",
            ));
        }
        Ok(ChildDirectoriesPage {
            directories,
            next: page.next,
        })
    }
    pub async fn children(
        &self,
        cid: &str,
        cancel: &CancellationToken,
    ) -> Result<Vec<BlockChild>, ClientError> {
        #[derive(serde::Deserialize)]
        struct Children {
            children: Vec<BlockChild>,
        }
        Ok(self
            .get::<Children>(&["api", "block", cid, "children"], cancel)
            .await?
            .children)
    }
    pub async fn fees(&self, cancel: &CancellationToken) -> Result<FeeEstimates, ClientError> {
        self.get(&["api", "fees"], cancel).await
    }
    pub async fn chain_spec(
        &self,
        cancel: &CancellationToken,
    ) -> Result<ChainSpecView, ClientError> {
        self.get(&["api", "chain", "spec"], cancel).await
    }
    pub async fn transaction(
        &self,
        cid: &str,
        cancel: &CancellationToken,
    ) -> Result<TransactionProjection, ClientError> {
        self.get(&["api", "transaction", cid], cancel).await
    }
    pub async fn transaction_content(
        &self,
        cid: &str,
        cancel: &CancellationToken,
    ) -> Result<TransactionContent, ClientError> {
        #[derive(serde::Deserialize)]
        struct Reply {
            cid: String,
            transaction: Transaction,
        }
        #[derive(serde::Deserialize)]
        struct Transaction {
            body: Body,
        }
        #[derive(serde::Deserialize)]
        struct Body {
            #[serde(rename = "rawCID")]
            raw_cid: String,
        }
        let reply: Reply = self.get(&["transactions", cid], cancel).await?;
        Ok(TransactionContent {
            cid: reply.cid,
            body_cid: reply.transaction.body.raw_cid,
        })
    }
}
