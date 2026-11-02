//! Transaction composition; construction/signing does not prove node admission.
use ed25519_dalek::{Signer, SigningKey};
use lattice_cashew::cbor;
use lattice_primitives::{AccountAction, TransactionBody, content_id, crypto, signing_preimage};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, thiserror::Error)]
pub enum TransactionError {
    #[error("invalid transaction input: {0}")]
    Invalid(&'static str),
    #[error(transparent)]
    Content(#[from] lattice_cashew::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
#[derive(Debug)]
pub struct EncodedContent {
    pub bytes: Vec<u8>,
    pub cid: String,
}
/// Secret material is not exposed by Debug or Serialize.
pub struct SigningAccount {
    key: SigningKey,
    public_key: String,
    address: String,
}
impl SigningAccount {
    pub fn public_key(&self) -> &str {
        &self.public_key
    }
    pub fn address(&self) -> &str {
        &self.address
    }
    pub fn from_private_key(seed: &[u8; 32]) -> Result<Self, TransactionError> {
        let key = SigningKey::from_bytes(seed);
        let public_key = format!("ed01{}", hex::encode(key.verifying_key().as_bytes()));
        let address = crypto::create_address(&public_key)?;
        Ok(Self {
            key,
            public_key,
            address,
        })
    }
    pub fn generate() -> Result<Self, TransactionError> {
        Self::from_private_key(&SigningKey::generate(&mut rand_core::OsRng).to_bytes())
    }
    pub fn sign_body(&self, body: &TransactionBody) -> Result<SignedBody, TransactionError> {
        let encoded = encode_transaction_body(body)?;
        let preimage = signing_preimage(&encoded.cid, &body.chain_path, body.nonce);
        let signature = hex::encode(
            self.key
                .sign(&crypto::signature_payload(&preimage))
                .to_bytes(),
        );
        Ok(SignedBody {
            body_cid: encoded.cid,
            public_key: self.public_key.clone(),
            signature,
        })
    }
}
#[derive(Debug)]
pub struct SignedBody {
    pub body_cid: String,
    pub public_key: String,
    pub signature: String,
}
fn assert_body(body: &TransactionBody) -> Result<(), TransactionError> {
    if body.chain_path.is_empty() || body.chain_path.iter().any(String::is_empty) {
        return Err(TransactionError::Invalid(
            "chain path must contain nonempty components",
        ));
    }
    if body
        .deposit_actions
        .iter()
        .any(|a| a.nonce > u64::MAX.into())
        || body
            .receipt_actions
            .iter()
            .any(|a| a.nonce > u64::MAX.into())
        || body
            .withdrawal_actions
            .iter()
            .any(|a| a.nonce > u64::MAX.into())
    {
        return Err(TransactionError::Invalid(
            "cross-chain nonce is not encodable as UInt64",
        ));
    }
    Ok(())
}
pub fn encode_transaction_body(body: &TransactionBody) -> Result<EncodedContent, TransactionError> {
    assert_body(body)?;
    Ok(EncodedContent {
        bytes: cbor::encode(body)?,
        cid: content_id(body)?,
    })
}
pub fn encode_signed_transaction(
    signatures: &BTreeMap<String, String>,
    body_cid: &str,
) -> Result<EncodedContent, TransactionError> {
    let mut normalized = BTreeMap::new();
    for (key, value) in signatures {
        let key = key.strip_prefix("0x").unwrap_or(key);
        let value = value.strip_prefix("0x").unwrap_or(value);
        let key =
            hex::decode(key).map_err(|_| TransactionError::Invalid("signature key is not hex"))?;
        let value =
            hex::decode(value).map_err(|_| TransactionError::Invalid("signature is not hex"))?;
        if key.len() != 34 || key[..2] != [0xed, 1] || value.len() != 64 {
            return Err(TransactionError::Invalid(
                "signature key or signature length",
            ));
        }
        if normalized
            .insert(hex::encode(key), hex::encode(value))
            .is_some()
        {
            return Err(TransactionError::Invalid("duplicate signature key"));
        }
    }
    #[derive(Serialize)]
    struct Entry {
        key: String,
        value: String,
    }
    #[derive(Serialize)]
    struct Body<'a> {
        #[serde(rename = "rawCID")]
        raw_cid: &'a str,
    }
    #[derive(Serialize)]
    struct Envelope<'a> {
        body: Body<'a>,
        signatures: Vec<Entry>,
    }
    let envelope = Envelope {
        body: Body { raw_cid: body_cid },
        signatures: normalized
            .into_iter()
            .map(|(key, value)| Entry { key, value })
            .collect(),
    };
    Ok(EncodedContent {
        bytes: cbor::encode(&envelope)?,
        cid: content_id(&envelope)?,
    })
}
pub fn signed_transaction_size(body: &TransactionBody) -> Result<usize, TransactionError> {
    let encoded = encode_transaction_body(body)?;
    let signatures = body
        .signers
        .iter()
        .enumerate()
        .map(|(index, _)| (format!("ed01{index:064x}"), "0".repeat(128)))
        .collect();
    Ok(encoded.bytes.len()
        + encode_signed_transaction(&signatures, &encoded.cid)?
            .bytes
            .len())
}
pub fn transaction_payload(
    signatures: &BTreeMap<String, String>,
    body: &TransactionBody,
) -> Result<serde_json::Value, TransactionError> {
    assert_body(body)?;
    let mut wire = serde_json::to_value(body)?;
    wire["nonce"] = body.nonce.to_string().into();
    for (family, fields) in [
        ("accountActions", &["delta"][..]),
        (
            "depositActions",
            &["nonce", "amountDemanded", "amountDeposited"][..],
        ),
        ("receiptActions", &["nonce", "amountDemanded"][..]),
        (
            "withdrawalActions",
            &["nonce", "amountDemanded", "amountWithdrawn"][..],
        ),
    ] {
        for action in wire[family].as_array_mut().expect("serialized action list") {
            for field in fields {
                action[*field] = action[*field].to_string().into();
            }
        }
    }
    Ok(serde_json::json!({"transaction":{"signatures":signatures,"body":wire}}))
}
pub fn build_transfer(
    from: &str,
    to: &str,
    amount: u64,
    fee: u64,
    nonce: u64,
    chain_path: &[String],
) -> Result<TransactionBody, TransactionError> {
    let debit = amount
        .checked_add(fee)
        .and_then(|n| i64::try_from(n).ok())
        .ok_or(TransactionError::Invalid("amount plus fee too large"))?;
    if amount == 0 || from == to {
        return Err(TransactionError::Invalid(
            "positive amount and distinct parties required",
        ));
    }
    Ok(TransactionBody {
        account_actions: vec![
            AccountAction {
                owner: from.into(),
                delta: -debit,
            },
            AccountAction {
                owner: to.into(),
                delta: amount as i64,
            },
        ],
        actions: vec![],
        deposit_actions: vec![],
        receipt_actions: vec![],
        withdrawal_actions: vec![],
        signers: vec![from.into()],
        nonce,
        chain_path: chain_path.to_vec(),
    })
}
/// State-growth upper bound used to price a transaction before reading state.
pub fn state_growth_ceiling(body: &TransactionBody) -> i128 {
    use num_bigint::BigInt;
    let len = |s: &str| s.len() as i128;
    let mut growth = 0;
    for a in &body.actions {
        growth += match (&a.old_value, &a.new_value) {
            (None, new) => len(&a.key) + new.as_deref().map(&len).unwrap_or(0),
            (Some(old), None) => -len(&a.key) - len(old),
            (Some(old), Some(new)) => len(new) - len(old),
        };
    }
    for a in &body.deposit_actions {
        growth += 32 + len(&a.demander);
    }
    for a in &body.receipt_actions {
        growth += 24 + len(&a.withdrawer) + len(&a.demander) + len(&a.directory);
    }
    for a in &body.withdrawal_actions {
        growth += 32 + len(&a.withdrawer) + len(&a.demander);
    }
    let mut net: BTreeMap<&str, BigInt> = BTreeMap::new();
    for a in &body.account_actions {
        *net.entry(&a.owner).or_default() += a.delta;
    }
    for a in &body.receipt_actions {
        *net.entry(&a.withdrawer).or_default() -= a.amount_demanded;
        *net.entry(&a.demander).or_default() += a.amount_demanded;
    }
    for (owner, delta) in net {
        if delta > BigInt::default() {
            growth += len(owner) + 8;
        }
    }
    if let Some(signer) = body.signers.first() {
        growth += len(signer) + 15;
    }
    growth
}
