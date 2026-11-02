//! Bind self-contained proofs to the state or block the caller actually asked
//! about. The caller must establish that commitment's chain and canonicality;
//! accepting a proof's own commitment as the expected one does not do that.
pub use lattice_light_client::{
    ChainHeader, Claim, LightClientProof, StateDictionaryProof, StateKind, VerifyError, WitnessNode,
};

#[derive(Debug, thiserror::Error)]
pub enum ProofError {
    #[error("proof does not match the requested commitment, address, dictionary, or key")]
    RequestMismatch,
    #[error(transparent)]
    Invalid(#[from] VerifyError),
}
/// An authenticated account under the caller-selected state root. This is not
/// a statement that any header or chain is canonical.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvenAccount {
    state_root: String,
    address: String,
    balance: u64,
    stored_nonce: u64,
}
impl ProvenAccount {
    pub fn state_root(&self) -> &str {
        &self.state_root
    }
    pub fn address(&self) -> &str {
        &self.address
    }
    pub fn balance(&self) -> u64 {
        self.balance
    }
    /// Last-used nonce; does not silently wrap to construct a next nonce.
    pub fn stored_nonce(&self) -> u64 {
        self.stored_nonce
    }
}
/// Authenticated presence or absence under the caller-selected block's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvenClaim {
    block_hash: String,
    state_root: String,
    dictionary: StateKind,
    key: String,
    value: Option<String>,
}
impl ProvenClaim {
    pub fn block_hash(&self) -> &str {
        &self.block_hash
    }
    pub fn state_root(&self) -> &str {
        &self.state_root
    }
    pub fn dictionary(&self) -> StateKind {
        self.dictionary
    }
    pub fn key(&self) -> &str {
        &self.key
    }
    pub fn value(&self) -> Option<&str> {
        self.value.as_deref()
    }
}

pub fn account_at_state(
    proof: &LightClientProof,
    expected_state_root: &str,
    expected_address: &str,
) -> Result<ProvenAccount, ProofError> {
    if expected_state_root.is_empty()
        || proof.state_root != expected_state_root
        || proof.address != expected_address
    {
        return Err(ProofError::RequestMismatch);
    }
    lattice_light_client::verify_account_checked(proof)?;
    Ok(ProvenAccount {
        state_root: proof.state_root.clone(),
        address: proof.address.clone(),
        balance: proof.balance,
        stored_nonce: proof.nonce,
    })
}

/// A batch proof may contain other claims; the requested key must occur exactly
/// once. An absent claim is not equivalent to an authenticated absent value.
/// No range-completeness guarantee follows from a batch of individual claims.
pub fn claim_at_block(
    proof: &StateDictionaryProof,
    expected_block_hash: &str,
    expected_dictionary: StateKind,
    expected_key: &str,
) -> Result<ProvenClaim, ProofError> {
    if expected_block_hash.is_empty()
        || proof.block_hash != expected_block_hash
        || proof.dictionary != expected_dictionary
    {
        return Err(ProofError::RequestMismatch);
    }
    let mut matching = proof
        .claims
        .iter()
        .filter(|claim| claim.key == expected_key);
    let claim = matching.next().ok_or(ProofError::RequestMismatch)?;
    if matching.next().is_some() {
        return Err(ProofError::RequestMismatch);
    }
    lattice_light_client::verify_dictionary_checked(proof)?;
    Ok(ProvenClaim {
        block_hash: proof.block_hash.clone(),
        state_root: proof.state_root.clone(),
        dictionary: proof.dictionary,
        key: claim.key.clone(),
        value: claim.value.clone(),
    })
}
