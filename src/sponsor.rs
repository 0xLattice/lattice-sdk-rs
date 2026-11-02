//! Work-for-fee sponsor proposal checks. A proposal is not an admission proof;
//! its signature is retained for normal transaction signature validation.
use crate::transactions::{TransactionError, transaction_payload};
use lattice_primitives::{AccountAction, TransactionBody, content_id};
use num_bigint::BigUint;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
#[derive(Debug, thiserror::Error)]
pub enum SponsorProposalError {
    #[error("invalid sponsor response: {0}")]
    Invalid(&'static str),
    #[error(transparent)]
    Transaction(#[from] TransactionError),
    #[error(transparent)]
    Content(#[from] lattice_cashew::Error),
}
pub(crate) fn unsigned(value: Option<&Value>) -> Result<BigUint, SponsorProposalError> {
    let text = value
        .and_then(Value::as_str)
        .ok_or(SponsorProposalError::Invalid("expected decimal string"))?;
    if text.is_empty()
        || !text.bytes().all(|c| c.is_ascii_digit())
        || text.len() > 1 && text.starts_with('0')
    {
        return Err(SponsorProposalError::Invalid("noncanonical decimal string"));
    }
    text.parse()
        .map_err(|_| SponsorProposalError::Invalid("invalid decimal"))
}
#[derive(Debug)]
pub struct SponsoredProposal {
    pub body: TransactionBody,
    pub signatures: BTreeMap<String, String>,
    pub fee: BigUint,
    pub pay: BigUint,
}
/// Reconstruct the exact requested body plus sponsor fee/pay actions. Rejects
/// any changed field, extra action, signer, or signature. This deliberately does
/// not call the supplied signature cryptographically valid (same as the SDK).
pub fn validate_sponsor_answer(
    draft: &TransactionBody,
    pay: &BigUint,
    answer: &Value,
) -> Result<SponsoredProposal, SponsorProposalError> {
    use SponsorProposalError::Invalid;
    let fee = unsigned(answer.get("fee"))?;
    if unsigned(answer.get("pay"))? != *pay {
        return Err(Invalid("sponsor changed pay"));
    }
    let transaction = answer
        .get("transaction")
        .ok_or(Invalid("missing transaction"))?;
    let sponsor = transaction
        .get("body")
        .and_then(|b| b.get("signers"))
        .and_then(|s| s.get(1))
        .and_then(Value::as_str)
        .ok_or(Invalid("not a sponsored transaction"))?;
    let [signer] = draft.signers.as_slice() else {
        return Err(Invalid("draft must have one signer"));
    };
    if sponsor == signer {
        return Err(Invalid("sponsor must differ from client"));
    }
    let debit = i64::try_from(&fee + pay).map_err(|_| Invalid("fee plus pay exceeds Int64"))?;
    let mut body = draft.clone();
    if pay != &BigUint::default() {
        body.account_actions.push(AccountAction {
            owner: signer.clone(),
            delta: i64::try_from(pay.clone()).map_err(|_| Invalid("pay exceeds Int64"))?,
        });
    }
    body.account_actions.push(AccountAction {
        owner: sponsor.into(),
        delta: -debit,
    });
    body.signers = vec![signer.clone(), sponsor.into()];
    let expected = transaction_payload(&BTreeMap::new(), &body)?;
    if transaction.get("body") != Some(&expected["transaction"]["body"]) {
        return Err(Invalid("sponsored body is not draft plus fee and pay"));
    }
    let signatures = transaction
        .get("signatures")
        .and_then(Value::as_object)
        .ok_or(Invalid("missing sponsor signature"))?;
    if signatures.len() != 1 {
        return Err(Invalid("expected exactly one sponsor signature"));
    }
    let (key, value) = signatures.iter().next().expect("one entry");
    let signature = value
        .as_str()
        .ok_or(Invalid("signature must be a string"))?;
    #[derive(Serialize)]
    struct Key<'a> {
        key: &'a str,
    }
    if content_id(&Key { key })? != sponsor {
        return Err(Invalid("signature key does not name sponsor"));
    }
    Ok(SponsoredProposal {
        body,
        signatures: BTreeMap::from([(key.clone(), signature.into())]),
        fee,
        pay: pay.clone(),
    })
}

pub(crate) fn prefix_bytes(prefix_hex: &str) -> Result<Vec<u8>, SponsorProposalError> {
    let raw = prefix_hex.strip_prefix("0x").unwrap_or(prefix_hex);
    if !raw.len().is_multiple_of(2)
        || !raw
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(SponsorProposalError::Invalid("expected lowercase byte hex"));
    }
    hex::decode(raw).map_err(|_| SponsorProposalError::Invalid("invalid prefix hex"))
}

fn nonce_bytes(nonce: &num_bigint::BigInt) -> [u8; 8] {
    // JavaScript DataView.setBigUint64 uses the low 64 bits, even for negative
    // and arbitrarily large values. Preserve that public shareHash behavior.
    let modulus = num_bigint::BigInt::from(1u8) << 64;
    let reduced: num_bigint::BigInt = ((nonce % &modulus) + &modulus) % &modulus;
    let (_, bytes) = reduced.to_bytes_be();
    let mut result = [0; 8];
    result[8 - bytes.len()..].copy_from_slice(&bytes);
    result
}

/// SHA-256(prefix || nonce modulo 2^64 as eight big-endian bytes).
pub fn share_hash(
    prefix_hex: &str,
    nonce: &num_bigint::BigInt,
) -> Result<[u8; 32], SponsorProposalError> {
    use sha2::{Digest, Sha256};
    Ok(Sha256::new()
        .chain_update(prefix_bytes(prefix_hex)?)
        .chain_update(nonce_bytes(nonce))
        .finalize()
        .into())
}

#[derive(Debug, thiserror::Error)]
#[error("sponsor share search cancelled")]
pub struct ShareSearchCancelled;

/// Lazy CPU share search. Construct the monotonic deadline from the request's
/// start time, not receipt time; no work runs until `next` is polled. This does
/// not submit shares or establish a sponsor's credit acknowledgement.
pub struct ShareSearch {
    midstate: sha2::Sha256,
    target: [u8; 32],
    nonce: BigUint,
    end: BigUint,
    deadline: crate::runtime::Instant,
    step: usize,
}
impl ShareSearch {
    pub fn new(
        prefix_hex: &str,
        target: &BigUint,
        start_nonce: BigUint,
        count: BigUint,
        deadline: crate::runtime::Instant,
    ) -> Result<Self, SponsorProposalError> {
        use sha2::Digest;
        let bytes = target.to_bytes_be();
        if bytes.len() > 32 {
            return Err(SponsorProposalError::Invalid(
                "share target exceeds UInt256",
            ));
        }
        let mut target = [0; 32];
        target[32 - bytes.len()..].copy_from_slice(&bytes);
        Ok(Self {
            midstate: sha2::Sha256::new().chain_update(prefix_bytes(prefix_hex)?),
            target,
            end: &start_nonce + count,
            nonce: start_nonce,
            deadline,
            step: 0,
        })
    }

    pub async fn next(
        &mut self,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> Result<Option<BigUint>, ShareSearchCancelled> {
        use sha2::Digest;
        if cancel.is_cancelled() {
            return Err(ShareSearchCancelled);
        }
        // Check again after the consumer has held a returned share.
        if crate::runtime::Instant::now() >= self.deadline {
            return Ok(None);
        }
        while self.nonce < self.end {
            if self.step.is_multiple_of(0x4000) {
                crate::runtime::yield_now().await;
                if cancel.is_cancelled() {
                    return Err(ShareSearchCancelled);
                }
                if crate::runtime::Instant::now() >= self.deadline {
                    return Ok(None);
                }
            }
            let candidate = self.nonce.clone();
            let hash: [u8; 32] = self
                .midstate
                .clone()
                .chain_update(nonce_bytes(&candidate.clone().into()))
                .finalize()
                .into();
            self.nonce += 1u8;
            self.step = self.step.wrapping_add(1);
            if hash <= self.target {
                return Ok(Some(candidate));
            }
        }
        Ok(None)
    }
}
