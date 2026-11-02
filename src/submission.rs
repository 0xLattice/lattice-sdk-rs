//! Explicit submission capability, separate from node reads. A relay response
//! acknowledges receipt; it does not establish inclusion or consensus validity.
use crate::{
    ClientOptions,
    transactions::{self, TransactionError},
    wire::{SafeInteger, present},
};
use lattice_primitives::TransactionBody;
use serde::Deserialize;
use std::{collections::BTreeMap, time::Duration};
use tokio_util::sync::CancellationToken;
use url::{Host, Url};
pub const SUBMISSION_REFUSALS: &[&str] = &[
    "belowMinRelayFee",
    "feeTooLow",
    "conflictingNonce",
    "nonceAhead",
    "unavailable",
    "insufficientFunds",
    "invalidState",
    "tooLarge",
    "full",
    "unresolved",
    "contextChanged",
    "unknownChain",
    "shuttingDown",
    "requestTooLarge",
];
#[derive(Debug, thiserror::Error)]
pub enum SubmissionError {
    #[error("invalid submission configuration: {0}")]
    Configuration(&'static str),
    #[error("submission cancelled")]
    Cancelled,
    #[error("submission deadline exceeded")]
    Timeout,
    #[error("HTTP {status}: {refusal:?}")]
    Refused {
        status: u16,
        refusal: Option<String>,
    },
    #[error("relay reported a transaction CID different from the signed content")]
    WrongTransaction,
    #[error(transparent)]
    Transport(#[from] crate::transport::Error),
    #[error(transparent)]
    Transaction(#[from] TransactionError),
}
impl SubmissionError {
    pub fn reason(&self) -> Option<&'static str> {
        if let Self::Refused {
            refusal: Some(text),
            ..
        } = self
        {
            SUBMISSION_REFUSALS
                .iter()
                .copied()
                .find(|name| *name == text)
        } else {
            None
        }
    }
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubmitResult {
    #[serde(rename = "transactionCID")]
    pub transaction_cid: String,
    #[serde(default, deserialize_with = "present")]
    pub mempool_count: Option<SafeInteger>,
    #[serde(default, deserialize_with = "present")]
    pub mempool_bytes: Option<SafeInteger>,
}
pub struct HttpTransactionSubmitter {
    endpoint: Url,
    http: crate::transport::Client,
    options: ClientOptions,
}
impl HttpTransactionSubmitter {
    pub fn new(endpoint: &str, options: Option<ClientOptions>) -> Result<Self, SubmissionError> {
        let endpoint =
            Url::parse(endpoint).map_err(|_| SubmissionError::Configuration("invalid URL"))?;
        let loopback = match endpoint.host() {
            Some(Host::Domain(h)) => h == "localhost",
            Some(Host::Ipv4(ip)) => ip.is_loopback(),
            Some(Host::Ipv6(ip)) => ip.is_loopback(),
            None => false,
        };
        if !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.fragment().is_some_and(|s| !s.is_empty())
            || !(endpoint.scheme() == "https" || endpoint.scheme() == "http" && loopback)
        {
            return Err(SubmissionError::Configuration(
                "HTTPS or loopback HTTP without credentials/fragment required",
            ));
        }
        let options = options.unwrap_or(ClientOptions {
            timeout: Duration::from_secs(8),
            maximum_response_bytes: 1024 * 1024,
            authorization: None,
        });
        if options.timeout.is_zero() || options.maximum_response_bytes == 0 {
            return Err(SubmissionError::Configuration("positive limits required"));
        }
        let http = crate::transport::client()?;
        Ok(Self {
            endpoint,
            http,
            options,
        })
    }
    pub fn endpoint(&self) -> &Url {
        &self.endpoint
    }
    pub async fn submit_signed(
        &self,
        signatures: &BTreeMap<String, String>,
        body: &TransactionBody,
        cancel: &CancellationToken,
    ) -> Result<SubmitResult, SubmissionError> {
        let encoded = transactions::encode_transaction_body(body)?;
        let expected = transactions::encode_signed_transaction(signatures, &encoded.cid)?.cid;
        let payload = transactions::transaction_payload(signatures, body)?;
        let result = self.submit(&payload, cancel).await?;
        if result.transaction_cid != expected {
            return Err(SubmissionError::WrongTransaction);
        }
        Ok(result)
    }
    pub async fn submit(
        &self,
        payload: &serde_json::Value,
        cancel: &CancellationToken,
    ) -> Result<SubmitResult, SubmissionError> {
        let work = async {
            let mut request = self
                .http
                .post(self.endpoint.clone())
                .header("Accept", "application/json")
                .header("Content-Type", "application/json")
                .body(payload.to_string());
            if let Some(auth) = &self.options.authorization {
                request = request.header("Authorization", auth);
            }
            let mut response =
                crate::transport::response_limit(request, self.options.maximum_response_bytes)
                    .send()
                    .await?;
            let status = response.status();
            let error = |message: Option<&str>| SubmissionError::Refused {
                status: status.as_u16(),
                refusal: message.map(str::to_owned),
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
            let value: serde_json::Value =
                serde_json::from_slice(bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes))
                    .map_err(|_| error(status.is_success().then_some("invalid JSON response")))?;
            if !status.is_success() {
                let refusal = value.get("error").and_then(|e| {
                    e.as_str()
                        .or_else(|| e.get("message").and_then(|v| v.as_str()))
                });
                return Err(error(refusal));
            }
            if !value.is_object() {
                return Err(error(Some("invalid submission response")));
            }
            if !value.get("transactionCID").is_some_and(|v| v.is_string()) {
                return Err(error(Some("submission response omitted transactionCID")));
            }
            let result: SubmitResult = serde_json::from_value(value)
                .map_err(|_| error(Some("invalid submission counts")))?;
            if result.mempool_count.is_some_and(|n| n.0 < 0)
                || result.mempool_bytes.is_some_and(|n| n.0 < 0)
            {
                return Err(error(Some("invalid submission counts")));
            }
            Ok(result)
        };
        tokio::select! {biased;
            _=cancel.cancelled()=>Err(SubmissionError::Cancelled),
            result=crate::runtime::timeout(self.options.timeout,work)=>result.map_err(|_|SubmissionError::Timeout)?,
        }
    }
}
