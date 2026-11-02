use lattice_cashew::{Header, MerkleDictionary, Volume, cbor, volume::serialize_recursive};
use lattice_primitives::*;
use lattice_sdk::proofs::*;
use std::collections::BTreeMap;
fn state(nonce: Option<u64>) -> LatticeState {
    let mut state = LatticeState::empty().unwrap();
    let mut account = AccountState::default()
        .inserting("alice", 42)
        .unwrap()
        .inserting("bob", 99)
        .unwrap();
    if let Some(nonce) = nonce {
        account = account.inserting("_nonce_alice", nonce).unwrap();
    }
    state.account_state = Volume::new(account).unwrap();
    state.deposit_state = Volume::new(
        DepositState::default()
            .inserting("chain/alice", u64::MAX)
            .unwrap(),
    )
    .unwrap();
    state.receipt_state = Volume::new(
        ReceiptState::default()
            .inserting("tx/alice", "receipt-cid".into())
            .unwrap(),
    )
    .unwrap();
    state
}
fn witness(state: &LatticeState) -> (String, Vec<WitnessNode>) {
    let root = Volume::new(state.clone()).unwrap();
    let entries: BTreeMap<_, _> = serialize_recursive(&root, &())
        .unwrap()
        .into_iter()
        .flat_map(|v| v.entries)
        .collect();
    (
        root.raw_cid.clone(),
        entries
            .into_iter()
            .map(|(cid, data)| WitnessNode::new(cid, &data))
            .collect(),
    )
}
fn account(nonce: Option<u64>) -> LightClientProof {
    let state = state(nonce);
    let (state_root, witness) = witness(&state);
    LightClientProof {
        block_hash: "untrusted-header-hash".into(),
        block_height: 7,
        header: ChainHeader {
            hash: "untrusted-header-hash".into(),
            height: 7,
            previous_hash: None,
            state_root: state_root.clone(),
            target: "untrusted-target".into(),
            timestamp: 123,
            cumulative_work: "untrusted-work".into(),
        },
        state_root,
        address: "alice".into(),
        balance: 42,
        nonce: nonce.unwrap_or(0),
        account_root: state.account_state.raw_cid.clone(),
        witness,
        timestamp: 123,
    }
}
fn dictionary(kind: StateKind, claims: Vec<Claim>) -> StateDictionaryProof {
    let state = state(None);
    let (state_root, witness) = witness(&state);
    let empty = Volume::new(LatticeState::empty().unwrap()).unwrap();
    let block = Block {
        version: 1,
        parent: None,
        transactions: Header::new(MerkleDictionary::default()).unwrap(),
        target: UInt256([255; 32]),
        next_target: UInt256([255; 32]),
        spec: Volume::new(ChainSpec {
            max_number_of_transactions_per_block: 100,
            max_state_growth: 100_000,
            max_block_size: 1_000_000,
            premine: 0,
            target_block_time: 1000,
            initial_reward: 1024,
            halving_interval: 210_000,
            half_life: 120,
            wasm_policies: vec![],
        })
        .unwrap(),
        parent_state: empty.clone(),
        prev_state: empty,
        post_state: Volume::new(state.clone()).unwrap(),
        children: Header::new(BTreeMap::new()).unwrap(),
        height: 7,
        timestamp: 123,
        reward_recipient: None,
        nonce: 0,
    };
    let block_hash = content_id(&block).unwrap();
    StateDictionaryProof {
        block_hash: block_hash.clone(),
        block_height: 7,
        block: WitnessNode::new(block_hash, &cbor::encode(&block).unwrap()),
        state_root,
        dictionary: kind,
        dictionary_root: match kind {
            StateKind::Deposits => state.deposit_state.raw_cid.clone(),
            StateKind::Receipts => state.receipt_state.raw_cid.clone(),
        },
        claims,
        witness,
    }
}

#[test]
fn account_commitment_and_subject_cannot_be_substituted() {
    let proof = account(Some(u64::MAX));
    let value = account_at_state(&proof, &proof.state_root, "alice").unwrap();
    assert_eq!(value.balance(), 42);
    assert_eq!(value.stored_nonce(), u64::MAX);
    assert_eq!(value.address(), "alice");
    assert_eq!(value.state_root(), proof.state_root);
    assert!(matches!(
        account_at_state(&proof, "another-state", "alice"),
        Err(ProofError::RequestMismatch)
    ));
    assert!(matches!(
        account_at_state(&proof, &proof.state_root, "bob"),
        Err(ProofError::RequestMismatch)
    ));
    let mut tampered = proof.clone();
    tampered.balance += 1;
    assert!(matches!(
        account_at_state(&tampered, &proof.state_root, "alice"),
        Err(ProofError::Invalid(_))
    ));
    // Header metadata alone is not a chain binding; the expected state root is.
    let mut renamed = proof.clone();
    renamed.block_hash = "arbitrary-header".into();
    renamed.header.hash = renamed.block_hash.clone();
    assert!(account_at_state(&renamed, &proof.state_root, "alice").is_ok());
}

#[test]
fn dictionary_binding_distinguishes_absent_value_from_missing_claim() {
    let proof = dictionary(
        StateKind::Deposits,
        vec![
            Claim {
                key: "chain/alice".into(),
                value: Some(u64::MAX.to_string()),
            },
            Claim {
                key: "missing".into(),
                value: None,
            },
        ],
    );
    let got = claim_at_block(
        &proof,
        &proof.block_hash,
        StateKind::Deposits,
        "chain/alice",
    )
    .unwrap();
    assert_eq!(got.value(), Some(u64::MAX.to_string().as_str()));
    assert_eq!(got.block_hash(), proof.block_hash);
    assert_eq!(got.state_root(), proof.state_root);
    assert_eq!(got.key(), "chain/alice");
    assert_eq!(got.dictionary(), StateKind::Deposits);
    assert_eq!(
        claim_at_block(&proof, &proof.block_hash, StateKind::Deposits, "missing")
            .unwrap()
            .value(),
        None
    );
    for (block, kind, key) in [
        ("another-block", StateKind::Deposits, "chain/alice"),
        (
            proof.block_hash.as_str(),
            StateKind::Receipts,
            "chain/alice",
        ),
        (proof.block_hash.as_str(), StateKind::Deposits, "unclaimed"),
    ] {
        assert!(matches!(
            claim_at_block(&proof, block, kind, key),
            Err(ProofError::RequestMismatch)
        ));
    }
    let mut duplicate = proof.clone();
    duplicate.claims.push(duplicate.claims[0].clone());
    assert!(
        claim_at_block(
            &duplicate,
            &proof.block_hash,
            StateKind::Deposits,
            "chain/alice"
        )
        .is_err()
    );
    let mut altered = proof.clone();
    altered.claims[0].value = Some("1".into());
    assert!(matches!(
        claim_at_block(
            &altered,
            &proof.block_hash,
            StateKind::Deposits,
            "chain/alice"
        ),
        Err(ProofError::Invalid(_))
    ));
    let mut removed = proof.clone();
    removed.witness.clear();
    assert!(claim_at_block(&removed, &proof.block_hash, StateKind::Deposits, "missing").is_err());
}
