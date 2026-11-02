//! Explicit sponsor HTTP operations. Answers are sponsor claims, not proofs of
//! credit, storage durability, transaction validity or inclusion.
use crate::runtime::Instant;
use crate::sponsor::{
    ShareSearch, SponsorProposalError, SponsoredProposal, validate_sponsor_answer,
};
use crate::transactions::transaction_payload;
use lattice_primitives::TransactionBody;
use num_bigint::{BigInt, BigUint};
use serde::Serialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, time::Duration};
use tokio_util::sync::CancellationToken;
use url::{Host, Url};

#[derive(Clone, Debug)]
pub struct SponsorClientOptions {
    pub timeout: Duration,
    pub maximum_response_bytes: usize,
}
impl Default for SponsorClientOptions {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            maximum_response_bytes: 1024 * 1024,
        }
    }
}
#[derive(Debug, thiserror::Error)]
pub enum SponsorError {
    #[error("invalid sponsor configuration: {0}")]
    Configuration(&'static str),
    #[error("sponsor request cancelled")]
    Cancelled,
    #[error("sponsor request deadline exceeded")]
    Timeout,
    #[error("HTTP {status}: {refusal:?}")]
    Refused {
        status: u16,
        refusal: Option<String>,
    },
    #[error(transparent)]
    Invalid(#[from] SponsorProposalError),
    #[error(transparent)]
    Transport(#[from] crate::transport::Error),
}
#[derive(Default)]
pub struct WorkRequest<'a> {
    pub client: Option<&'a str>,
    pub chain: Option<&'a str>,
    pub pay: Option<&'a BigUint>,
    pub store: Option<&'a str>,
}
#[derive(Debug)]
pub struct SponsorWork {
    pub client: String,
    pub assignment: String,
    pub prefix_hex: String,
    pub share_target: BigUint,
    pub start_nonce: BigUint,
    pub count: BigUint,
    pub fee: BigUint,
    pub pay: BigUint,
    pub shares_for_fee: BigUint,
    pub deadline: Instant,
}
impl SponsorWork {
    pub fn search(&self) -> Result<ShareSearch, SponsorProposalError> {
        ShareSearch::new(
            &self.prefix_hex,
            &self.share_target,
            self.start_nonce.clone(),
            self.count.clone(),
            self.deadline,
        )
    }
}
#[derive(Debug)]
pub struct StoreStatus {
    pub root: String,
    pub bytes: BigUint,
    pub needed: BTreeMap<String, BigUint>,
    pub credit: BigUint,
    pub stored: bool,
}
#[derive(Debug, Serialize)]
pub struct StoreVolume {
    pub cid: String,
    pub entries: BTreeMap<String, String>,
}
fn invalid(message: &'static str) -> SponsorProposalError {
    SponsorProposalError::Invalid(message)
}
fn text(value: &Value, field: &str) -> Result<String, SponsorProposalError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| invalid("expected string"))
}
fn number(value: &Value, field: &str) -> Result<BigUint, SponsorProposalError> {
    crate::sponsor::unsigned(value.get(field))
}
fn parse_work(value: &Value, sent: Instant) -> Result<SponsorWork, SponsorProposalError> {
    let prefix_hex = text(value, "prefixHex")?;
    crate::sponsor::prefix_bytes(&prefix_hex)?;
    let target = text(value, "shareTarget")?;
    if target.is_empty()
        || target.len() > 64
        || !target
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid("shareTarget must be lowercase UInt256 hex"));
    }
    let share_target =
        BigUint::parse_bytes(target.as_bytes(), 16).ok_or_else(|| invalid("invalid target"))?;
    // Reject unrepresentable deadlines rather than extending a lease to infinity.
    let milliseconds = u64::try_from(number(value, "expiresInMilliseconds")?)
        .map_err(|_| invalid("expiry exceeds clock range"))?;
    let deadline = sent
        .checked_add(Duration::from_millis(milliseconds))
        .ok_or_else(|| invalid("expiry exceeds clock range"))?;
    Ok(SponsorWork {
        client: text(value, "client")?,
        assignment: text(value, "assignment")?,
        prefix_hex,
        share_target,
        start_nonce: number(value, "startNonce")?,
        count: number(value, "count")?,
        fee: number(value, "fee")?,
        pay: number(value, "pay")?,
        shares_for_fee: number(value, "sharesForFee")?,
        deadline,
    })
}
fn parse_status(value: &Value) -> Result<StoreStatus, SponsorProposalError> {
    // Object.entries in the reference also accepts arrays, exposing index keys.
    let needed = match value.get("needed") {
        Some(Value::Object(map)) => map
            .iter()
            .map(|(k, v)| Ok((k.clone(), crate::sponsor::unsigned(Some(v))?)))
            .collect::<Result<_, SponsorProposalError>>()?,
        Some(Value::Array(array)) => array
            .iter()
            .enumerate()
            .map(|(i, v)| Ok((i.to_string(), crate::sponsor::unsigned(Some(v))?)))
            .collect::<Result<_, SponsorProposalError>>()?,
        _ => return Err(invalid("needed must be an object")),
    };
    Ok(StoreStatus {
        root: text(value, "root")?,
        bytes: number(value, "bytes")?,
        needed,
        credit: number(value, "credit")?,
        stored: value
            .get("stored")
            .and_then(Value::as_bool)
            .ok_or_else(|| invalid("stored must be boolean"))?,
    })
}
pub struct SponsorClient {
    endpoint: String,
    http: crate::transport::Client,
    options: SponsorClientOptions,
}
impl SponsorClient {
    pub fn new(endpoint: &str, options: SponsorClientOptions) -> Result<Self, SponsorError> {
        let parsed =
            Url::parse(endpoint).map_err(|_| SponsorError::Configuration("invalid URL"))?;
        let loopback = match parsed.host() {
            Some(Host::Domain(h)) => h == "localhost",
            Some(Host::Ipv4(ip)) => ip.is_loopback(),
            Some(Host::Ipv6(ip)) => ip.is_loopback(),
            None => false,
        };
        if !parsed.username().is_empty()
            || parsed.password().is_some()
            || parsed.fragment().is_some_and(|s| !s.is_empty())
            || !(parsed.scheme() == "https" || parsed.scheme() == "http" && loopback)
        {
            return Err(SponsorError::Configuration(
                "HTTPS or loopback HTTP without credentials/fragment required",
            ));
        }
        if options.timeout.is_zero() || options.maximum_response_bytes == 0 {
            return Err(SponsorError::Configuration("positive limits required"));
        }
        let endpoint = parsed
            .as_str()
            .strip_suffix('/')
            .unwrap_or(parsed.as_str())
            .to_owned();
        Ok(Self {
            endpoint,
            http: crate::transport::client()?,
            options,
        })
    }
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }
    async fn call(
        &self,
        path: &str,
        body: Option<Value>,
        cancel: &CancellationToken,
    ) -> Result<Value, SponsorError> {
        let work = async {
            // Match reference string appending, including endpoints with query routing.
            let url = format!("{}/{path}", self.endpoint);
            let mut request = if body.is_some() {
                self.http.post(url)
            } else {
                self.http.get(url)
            }
            .header("Accept", "application/json");
            if let Some(body) = body {
                request = request
                    .header("Content-Type", "application/json")
                    .body(body.to_string());
            }
            let mut response =
                crate::transport::response_limit(request, self.options.maximum_response_bytes)
                    .send()
                    .await?;
            let status = response.status();
            let error = |text: Option<&str>| SponsorError::Refused {
                status: status.as_u16(),
                refusal: text.map(str::to_owned),
            };
            if response
                .content_length()
                .is_some_and(|n| n > self.options.maximum_response_bytes as u64)
            {
                return Err(error(Some("response too large")));
            }
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await? {
                if chunk.len()
                    > self
                        .options
                        .maximum_response_bytes
                        .saturating_sub(bytes.len())
                {
                    return Err(error(Some("response too large")));
                }
                bytes.extend_from_slice(&chunk);
            }
            let value: Value =
                serde_json::from_slice(bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes))
                    .map_err(|_| error(status.is_success().then_some("invalid JSON response")))?;
            if !status.is_success() {
                return Err(error(value.get("error").and_then(|e| {
                    e.as_str()
                        .or_else(|| e.get("message").and_then(Value::as_str))
                })));
            }
            if !value.is_object() {
                return Err(error(Some("invalid sponsor response")));
            }
            Ok(value)
        };
        tokio::select! {biased;
            _=cancel.cancelled()=>Err(SponsorError::Cancelled),
            result=crate::runtime::timeout(self.options.timeout,work)=>result.map_err(|_|SponsorError::Timeout)?,
        }
    }
    pub async fn request_work(
        &self,
        request: WorkRequest<'_>,
        cancel: &CancellationToken,
    ) -> Result<SponsorWork, SponsorError> {
        let mut payload = serde_json::Map::new();
        for (name, value) in [
            ("client", request.client),
            ("chain", request.chain),
            ("store", request.store),
        ] {
            if let Some(value) = value {
                payload.insert(name.into(), value.into());
            }
        }
        if let Some(pay) = request.pay {
            payload.insert("pay".into(), pay.to_string().into());
        }
        let sent = Instant::now();
        Ok(parse_work(
            &self
                .call("work", Some(Value::Object(payload)), cancel)
                .await?,
            sent,
        )?)
    }
    pub async fn submit_share(
        &self,
        assignment: &str,
        nonce: &BigInt,
        cancel: &CancellationToken,
    ) -> Result<BigUint, SponsorError> {
        Ok(number(
            &self
                .call(
                    "shares",
                    Some(json!({"assignment":assignment,"nonce":nonce.to_string()})),
                    cancel,
                )
                .await?,
            "credit",
        )?)
    }
    pub async fn sponsor_transaction(
        &self,
        client: &str,
        draft: &TransactionBody,
        pay: &BigUint,
        cancel: &CancellationToken,
    ) -> Result<SponsoredProposal, SponsorError> {
        let transaction =
            transaction_payload(&BTreeMap::new(), draft).map_err(SponsorProposalError::from)?;
        let mut body = json!({"client":client,"transaction":transaction["transaction"]});
        if pay != &BigUint::default() {
            body["pay"] = pay.to_string().into();
        }
        Ok(validate_sponsor_answer(
            draft,
            pay,
            &self.call("sponsor", Some(body), cancel).await?,
        )?)
    }
    pub async fn store(
        &self,
        root: &str,
        volumes: &[StoreVolume],
        cancel: &CancellationToken,
    ) -> Result<StoreStatus, SponsorError> {
        Ok(parse_status(
            &self
                .call(
                    "store",
                    Some(json!({"root":root,"volumes":volumes})),
                    cancel,
                )
                .await?,
        )?)
    }
    pub async fn store_status(
        &self,
        root: &str,
        cancel: &CancellationToken,
    ) -> Result<StoreStatus, SponsorError> {
        // encodeURIComponent leaves !'()* unescaped, unlike form-urlencoding.
        let encoded: String = root
            .bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
                    (b as char).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect();
        Ok(parse_status(
            &self.call(&format!("store/{encoded}"), None, cancel).await?,
        )?)
    }
}
