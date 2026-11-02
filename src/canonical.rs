//! Typed projections of CID-verified canonical content. Decoding alone does not
//! verify a CID, signature, chain membership, state transition or child proof.
use crate::volumes::{DagCborLimits, VolumeError, decode_dag_cbor};
use lattice_cashew::cbor::CborValue as Value;
use num_bigint::BigUint;
use sha2::{Digest, Sha256};

#[derive(Debug, thiserror::Error)]
pub enum CanonicalError {
    #[error(transparent)]
    Cbor(#[from] VolumeError),
    #[error("invalid canonical content: {0}")]
    Invalid(&'static str),
}
type Result<T> = std::result::Result<T, CanonicalError>;
fn invalid(message: &'static str) -> CanonicalError {
    CanonicalError::Invalid(message)
}
fn map(value: &Value) -> Result<&[(Value, Value)]> {
    if let Value::Map(value) = value {
        Ok(value)
    } else {
        Err(invalid("expected map"))
    }
}
fn field<'a>(record: &'a [(Value, Value)], key: &str) -> Option<&'a Value> {
    record
        .iter()
        .find_map(|(k, v)| matches!(k,Value::Text(s) if s==key).then_some(v))
}
fn required<'a>(record: &'a [(Value, Value)], key: &str) -> Result<&'a Value> {
    field(record, key).ok_or_else(|| invalid("missing field"))
}
fn text(value: &Value) -> Result<String> {
    if let Value::Text(value) = value {
        Ok(value.clone())
    } else {
        Err(invalid("expected text"))
    }
}
fn integer(value: &Value) -> Result<i128> {
    if let Value::Integer(value) = value {
        Ok((*value).into())
    } else {
        Err(invalid("expected integer"))
    }
}
fn reference(value: &Value) -> Result<String> {
    text(required(map(value)?, "rawCID")?)
}
fn link(record: &[(Value, Value)], key: &str) -> Result<String> {
    reference(required(record, key)?)
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalBlock {
    pub version: u16,
    pub parent_cid: Option<String>,
    pub transactions_cid: String,
    pub target: String,
    pub next_target: String,
    pub spec_cid: String,
    pub parent_state_cid: String,
    pub previous_state_cid: String,
    pub post_state_cid: String,
    pub children_cid: String,
    pub height: u64,
    pub timestamp: i64,
    pub reward_recipient: Option<String>,
    pub nonce: u64,
}
pub fn decode_canonical_block(bytes: &[u8]) -> Result<CanonicalBlock> {
    let value = decode_dag_cbor(bytes, DagCborLimits::default())?;
    let b = map(&value)?;
    Ok(CanonicalBlock {
        version: u16::try_from(integer(required(b, "version")?)?)
            .map_err(|_| invalid("version out of range"))?,
        parent_cid: field(b, "parent").map(reference).transpose()?,
        transactions_cid: link(b, "transactions")?,
        target: text(required(b, "target")?)?,
        next_target: text(required(b, "nextTarget")?)?,
        spec_cid: link(b, "spec")?,
        parent_state_cid: link(b, "parentState")?,
        previous_state_cid: link(b, "prevState")?,
        post_state_cid: link(b, "postState")?,
        children_cid: link(b, "children")?,
        height: u64::try_from(integer(required(b, "height")?)?)
            .map_err(|_| invalid("height out of range"))?,
        timestamp: i64::try_from(integer(required(b, "timestamp")?)?)
            .map_err(|_| invalid("timestamp out of range"))?,
        reward_recipient: field(b, "rewardRecipient").map(text).transpose()?,
        nonce: u64::try_from(integer(required(b, "nonce")?)?)
            .map_err(|_| invalid("nonce out of range"))?,
    })
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalLatticeState {
    pub account_state_cid: String,
    pub general_state_cid: String,
    pub deposit_state_cid: String,
    pub receipt_state_cid: String,
}
pub fn decode_canonical_lattice_state(bytes: &[u8]) -> Result<CanonicalLatticeState> {
    let value = decode_dag_cbor(bytes, DagCborLimits::default())?;
    let s = map(&value)?;
    Ok(CanonicalLatticeState {
        account_state_cid: link(s, "accountState")?,
        general_state_cid: link(s, "generalState")?,
        deposit_state_cid: link(s, "depositState")?,
        receipt_state_cid: link(s, "receiptState")?,
    })
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalSignature {
    pub public_key: String,
    pub signature: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalSignedTransaction {
    pub body_cid: String,
    pub signatures: Vec<CanonicalSignature>,
}
pub fn decode_canonical_signed_transaction(bytes: &[u8]) -> Result<CanonicalSignedTransaction> {
    let value = decode_dag_cbor(bytes, DagCborLimits::default())?;
    let tx = map(&value)?;
    let Value::Array(entries) = required(tx, "signatures")? else {
        return Err(invalid("expected signature array"));
    };
    let mut signatures: Vec<CanonicalSignature> = Vec::new();
    for entry in entries {
        let entry = map(entry)?;
        let key = text(required(entry, "key")?)?;
        if signatures.last().is_some_and(|prev| {
            prev.public_key
                .encode_utf16()
                .cmp(key.encode_utf16())
                .is_ge()
        }) {
            return Err(invalid("duplicate or unordered signature key"));
        }
        signatures.push(CanonicalSignature {
            public_key: key,
            signature: text(required(entry, "value")?)?,
        });
    }
    Ok(CanonicalSignedTransaction {
        body_cid: link(tx, "body")?,
        signatures,
    })
}
pub fn assert_block_continuity(
    parent_cid: &str,
    parent: &CanonicalBlock,
    child: &CanonicalBlock,
) -> Result<()> {
    if child.parent_cid.as_deref() != Some(parent_cid) {
        return Err(invalid("child does not commit parent"));
    }
    if parent.height.checked_add(1) != Some(child.height) {
        return Err(invalid("noncontiguous height"));
    }
    if child.previous_state_cid != parent.post_state_cid {
        return Err(invalid("post-state discontinuity"));
    }
    Ok(())
}
fn target_integer(target: &str) -> Result<BigUint> {
    let raw = target
        .strip_prefix("0x")
        .ok_or_else(|| invalid("target must start with 0x"))?;
    if raw.is_empty()
        || raw.len() > 64
        || !raw
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid("target must be lowercase UInt256 hex"));
    }
    BigUint::parse_bytes(raw.as_bytes(), 16).ok_or_else(|| invalid("invalid target"))
}
pub fn canonical_block_pow_preimage(block: &CanonicalBlock) -> Result<Vec<u8>> {
    let fields = [
        block.version.to_string(),
        block.parent_cid.clone().unwrap_or_default(),
        block.transactions_cid.clone(),
        target_integer(&block.target)?.to_str_radix(16),
        target_integer(&block.next_target)?.to_str_radix(16),
        block.spec_cid.clone(),
        block.parent_state_cid.clone(),
        block.previous_state_cid.clone(),
        block.post_state_cid.clone(),
        block.children_cid.clone(),
        block.height.to_string(),
        block.timestamp.to_string(),
    ];
    let mut bytes = Vec::new();
    for field in fields {
        bytes.extend_from_slice(field.as_bytes());
        bytes.push(0);
    }
    if let Some(recipient) = &block.reward_recipient {
        bytes.push(1);
        bytes.extend_from_slice(recipient.as_bytes());
    }
    bytes.push(0);
    bytes.extend_from_slice(&block.nonce.to_be_bytes());
    Ok(bytes)
}
pub fn canonical_block_pow_hash(block: &CanonicalBlock) -> Result<[u8; 32]> {
    Ok(Sha256::digest(canonical_block_pow_preimage(block)?).into())
}
/// Own PoW only, for Nexus blocks. Descendants need authenticated securing proof
/// context; this predicate does not provide that context or full block admission.
pub fn verify_nexus_block_pow(block: &CanonicalBlock) -> Result<bool> {
    let target = target_integer(&block.target)?;
    Ok(target != BigUint::default()
        && BigUint::from_bytes_be(&canonical_block_pow_hash(block)?) <= target)
}
pub fn work_for_target(target: &str) -> Result<BigUint> {
    let target = target_integer(target)?;
    if target == BigUint::default() {
        return Ok(target);
    }
    Ok((BigUint::from(1u8) << 256) / (target + 1u8))
}
