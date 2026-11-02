//! Disposable real-node integration driver; see tools/node-integration.py.
use lattice_sdk::{
    ClientOptions, NodeClient,
    chain_nodes::{ChainNodeDiscovery, ChainNodes, ChainNodesOptions, DiscoveryFuture},
    lattice_client::LatticeClient,
    node_cookie_authorization,
    proofs::{self, StateDictionaryProof, StateKind},
    submission::HttpTransactionSubmitter,
    transactions::{SigningAccount, build_transfer},
    volumes::{HttpVolumeOptions, HttpVolumeTransport, VolumeClient, VolumeLimits},
};
use std::{collections::BTreeMap, sync::Arc};
use tokio_util::sync::CancellationToken;
struct LocalDiscovery {
    url: String,
    options: ClientOptions,
}
impl ChainNodeDiscovery for LocalDiscovery {
    fn serves<'a>(&'a self, url: &'a str, path: &'a [String]) -> DiscoveryFuture<'a, bool> {
        Box::pin(async move {
            assert_eq!(url, self.url);
            let n = NodeClient::new(url, path.to_vec(), self.options.clone())?;
            Ok(n.chain_info(&CancellationToken::new()).await?.chain == path)
        })
    }
    fn declared<'a>(
        &'a self,
        _: &'a str,
        _: &'a [String],
        _: &'a [String],
    ) -> DiscoveryFuture<'a, Vec<String>> {
        Box::pin(async { panic!("fixture must never discover another credential destination") })
    }
}
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let sender = SigningAccount::from_private_key(&[71; 32])?;
    let receiver = SigningAccount::from_private_key(&[72; 32])?;
    if args[1] == "identity" {
        println!(
            "{}",
            serde_json::json!({"sender":sender.address(),"receiver":receiver.address()})
        );
        return Ok(());
    }
    let base = &args[2];
    let cookie = std::fs::read_to_string(&args[3])?;
    let auth = node_cookie_authorization(cookie.trim())?;
    let opts = ClientOptions {
        authorization: Some(auth.clone()),
        ..Default::default()
    };
    let cancel = CancellationToken::new();
    let path = vec!["Nexus".into()];
    let node = NodeClient::new(base, path.clone(), opts.clone())?;
    let discovery = ChainNodes::new(
        vec![base.clone()],
        Arc::new(LocalDiscovery {
            url: base.clone(),
            options: opts.clone(),
        }),
        ChainNodesOptions::default(),
    )?;
    assert!(discovery.list(&path).await?.iter().any(|n| n.url == *base));
    let info = node.chain_info(&cancel).await?;
    assert_eq!(info.chain, path);
    if args[1] == "submit" {
        let fee = std::env::var("LATTICE_TEST_FEE")
            .unwrap_or_else(|_| "0".into())
            .parse()?;
        let body = build_transfer(sender.address(), receiver.address(), 3, fee, 0, &path)?;
        let signed = sender.sign_body(&body)?;
        let signatures = BTreeMap::from([(signed.public_key, signed.signature)]);
        let result = HttpTransactionSubmitter::new(&format!("{base}/transactions"), Some(opts))?
            .submit_signed(&signatures, &body, &cancel)
            .await?;
        println!(
            "{}",
            serde_json::json!({"transactionCID":result.transaction_cid})
        );
        return Ok(());
    }
    let expected = &args[4];
    let transaction = &args[5];
    let client = LatticeClient::new(
        node,
        VolumeClient::new(
            HttpVolumeTransport::new(
                &format!("{base}/volumes"),
                HttpVolumeOptions {
                    authorization: Some(auth),
                    ..Default::default()
                },
            )?,
            VolumeLimits::default(),
        ),
    );
    let block = client.block(expected, &cancel).await?;
    assert_eq!(client.node.latest_block(&cancel).await?.hash, *expected);
    eprintln!("poststate {}", block.canonical.post_state_cid);
    let state = client.post_state(expected, &cancel).await?;
    assert_eq!(state.volume.root_cid(), block.canonical.post_state_cid);
    eprintln!("transactions {}", block.canonical.transactions_cid);
    let header = proofs::ChainHeader {
        hash: expected.clone(),
        height: block.canonical.height,
        previous_hash: None,
        state_root: state.volume.root_cid().into(),
        target: String::new(),
        timestamp: block.canonical.timestamp,
        cumulative_work: String::new(),
    };
    let accounts = client
        .volumes
        .get(&state.canonical.account_state_cid, &cancel)
        .await?;
    let mut witness: BTreeMap<String, Vec<u8>> = state
        .volume
        .entries()
        .iter()
        .map(|e| (e.cid.clone(), e.bytes.clone()))
        .collect();
    witness.extend(
        accounts
            .entries()
            .iter()
            .map(|e| (e.cid.clone(), e.bytes.clone())),
    );
    let mut account_proof = proofs::LightClientProof {
        block_hash: expected.clone(),
        block_height: block.canonical.height,
        header,
        state_root: state.volume.root_cid().into(),
        address: receiver.address().into(),
        balance: 3,
        nonce: 0,
        account_root: state.canonical.account_state_cid.clone(),
        witness: witness
            .iter()
            .map(|(cid, bytes)| proofs::WitnessNode::new(cid, bytes))
            .collect(),
        timestamp: block.canonical.timestamp,
    };
    // Follow only missing content requested by the sealed verifier, bounded to
    // this tiny fixture; each fetched boundary is CID-checked by VolumeClient.
    for _ in 0..16 {
        match proofs::account_at_state(
            &account_proof,
            &block.canonical.post_state_cid,
            receiver.address(),
        ) {
            Ok(_) => break,
            Err(proofs::ProofError::Invalid(proofs::VerifyError::Content(
                lattice_cashew::Error::MissingContent(cid),
            ))) => {
                let volume = client.volumes.get(&cid, &cancel).await?;
                witness.extend(
                    volume
                        .entries()
                        .iter()
                        .map(|e| (e.cid.clone(), e.bytes.clone())),
                );
                account_proof.witness = witness
                    .iter()
                    .map(|(cid, bytes)| proofs::WitnessNode::new(cid, bytes))
                    .collect();
            }
            Err(e) => return Err(e.into()),
        }
    }
    assert_eq!(
        proofs::account_at_state(
            &account_proof,
            &block.canonical.post_state_cid,
            receiver.address()
        )?
        .balance(),
        3
    );
    assert!(proofs::account_at_state(&account_proof, "wrong-state", receiver.address()).is_err());
    let mut forged = account_proof.clone();
    forged.balance = 4;
    assert!(
        proofs::account_at_state(&forged, &block.canonical.post_state_cid, receiver.address())
            .is_err()
    );
    let transactions = client.transactions(expected, &cancel).await?;
    assert_eq!(transactions.root_cid(), block.canonical.transactions_cid);
    let source: BTreeMap<String, Vec<u8>> = transactions
        .entries()
        .iter()
        .map(|e| (e.cid.clone(), e.bytes.clone()))
        .collect();
    let dictionary: lattice_cashew::MerkleDictionary<
        lattice_cashew::Volume<lattice_sdk::primitives::Transaction>,
    > = lattice_cashew::cbor::decode(transactions.root_bytes())?;
    let dictionary = dictionary.resolve_key("0", &source)?;
    assert_eq!(
        &dictionary
            .get("0")?
            .expect("included transaction")
            .0
            .raw_cid,
        transaction
    );
    eprintln!("transaction {transaction}");
    let tx = client.transaction(transaction, &cancel).await?;
    assert_eq!(tx.transaction.root_cid(), transaction);
    let account = client.node.account(receiver.address(), &cancel).await?;
    assert_eq!(account.balance.0.to_string(), "3");
    let proof: StateDictionaryProof = serde_json::from_slice(&std::fs::read(&args[6])?)?;
    let key = &args[7];
    let claim = proofs::claim_at_block(&proof, expected, StateKind::Deposits, key)?;
    assert!(claim.value().is_none());
    assert!(proofs::claim_at_block(&proof, "wrong-block", StateKind::Deposits, key).is_err());
    assert!(proofs::claim_at_block(&proof, expected, StateKind::Receipts, key).is_err());
    println!(
        "{}",
        serde_json::json!({"blockCID":expected,"transactionCID":transaction,"stateCID":state.volume.root_cid(),"recipientBalance":"3","verifiedAbsence":key,"discovery":true,"accountWitness":true})
    );
    Ok(())
}
