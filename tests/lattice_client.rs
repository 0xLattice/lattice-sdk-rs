use lattice_cashew::cbor;
use lattice_sdk::{NodeClient, canonical::*, lattice_client::*, volumes::*};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tokio_util::sync::CancellationToken;
fn volume(bytes: Vec<u8>) -> SerializedVolume {
    let mut cid = vec![1, 0x71, 0x12, 32];
    cid.extend_from_slice(&Sha256::digest(&bytes));
    let cid = format!(
        "b{}",
        data_encoding::BASE32_NOPAD
            .encode(&cid)
            .to_ascii_lowercase()
    );
    SerializedVolume {
        root_cid: cid.clone(),
        entries: vec![VolumeEntry { cid, bytes }],
    }
}
async fn server(answers: Vec<Value>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
        for answer in answers {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let mut buf = [0; 4096];
                let n = socket.read(&mut buf).await.unwrap();
                assert!(n > 0);
                request.extend_from_slice(&buf[..n]);
            }
            requests.push(String::from_utf8(request).unwrap());
            socket
                .write_all(
                    format!("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{answer}").as_bytes(),
                )
                .await
                .unwrap();
        }
        requests
    });
    (format!("http://{addr}"), task)
}
fn block(
    height: u64,
    target: &str,
) -> (SerializedVolume, SerializedVolume, SerializedVolume, Value) {
    let state=volume(cbor::encode(&json!({"accountState":{"rawCID":"a"},"generalState":{"rawCID":"g"},"depositState":{"rawCID":"d"},"receiptState":{"rawCID":"r"}})).unwrap());
    let transactions = volume(cbor::encode(&json!({})).unwrap());
    let mut block=volume(cbor::encode(&json!({"version":1,"transactions":{"rawCID":transactions.root_cid},"target":target,"nextTarget":"0x01","spec":{"rawCID":"s"},"parentState":{"rawCID":"p"},"prevState":{"rawCID":"v"},"postState":{"rawCID":state.root_cid},"children":{"rawCID":"c"},"height":height,"timestamp":-1,"nonce":7})).unwrap());
    block.entries.extend(transactions.entries.clone());
    let view = json!({"height":height.to_string(),"hash":block.root_cid,"timestamp":"-1","transactionCount":0,"childBlockCount":0,"nonce":"7","version":1,"target":target,"nextTarget":"0x01","transactionsCID":transactions.root_cid,"postStateCID":state.root_cid,"chain":["Nexus"]});
    (block, state, transactions, view)
}
fn client(
    url: &str,
    path: Vec<String>,
    volumes: Vec<SerializedVolume>,
) -> LatticeClient<MemoryVolumeTransport> {
    LatticeClient::new(
        NodeClient::new(url, path, Default::default()).unwrap(),
        VolumeClient::new(MemoryVolumeTransport::new(volumes), Default::default()),
    )
}
#[tokio::test]
async fn verified_blocks_bind_every_projected_consensus_field() {
    let (block, state, tx, view) = block(0, "0x0");
    let cancel = CancellationToken::new();
    for (key, bad) in [
        ("height", json!("1")),
        ("timestamp", json!("0")),
        ("nonce", json!("8")),
        ("version", json!(2)),
        ("previousBlock", json!("parent")),
        ("target", json!("0x00")),
        ("nextTarget", json!("0x1")),
        ("transactionsCID", json!("wrong")),
        ("postStateCID", json!("wrong")),
        ("rewardRecipient", json!("recipient")),
    ] {
        let mut changed = view.clone();
        changed[key] = bad;
        let (url, task) = server(vec![changed]).await;
        assert!(
            matches!(
                client(&url, vec!["Nexus".into()], vec![block.clone()])
                    .block("0", &cancel)
                    .await,
                Err(LatticeClientError::Mismatch(_))
            ),
            "{key}"
        );
        task.await.unwrap();
    }
    let (url, task) = server(vec![view.clone(), view.clone(), view.clone(), view.clone()]).await;
    let client = client(
        &url,
        vec!["Nexus".into()],
        vec![block.clone(), state.clone()],
    );
    assert_eq!(
        client
            .latest_block(&cancel)
            .await
            .unwrap()
            .volume
            .root_cid(),
        block.root_cid
    );
    assert_eq!(
        client
            .post_state("0", &cancel)
            .await
            .unwrap()
            .canonical
            .account_state_cid,
        "a"
    );
    assert_eq!(
        client.transactions("0", &cancel).await.unwrap().root_cid(),
        tx.root_cid
    );
    let requests = task.await.unwrap();
    assert!(requests[0].starts_with("GET /api/block/latest?"));
    // A valid hash label cannot authenticate altered bytes.
    let mut corrupt = block.clone();
    corrupt.entries[0].bytes.push(0);
    let (url, task) = server(vec![view]).await;
    assert!(matches!(
        self::client(&url, vec!["Nexus".into()], vec![corrupt])
            .block("0", &cancel)
            .await,
        Err(LatticeClientError::Volume(_))
    ));
    task.await.unwrap();
}
#[tokio::test]
async fn root_pow_is_enforced_but_child_content_does_not_claim_securing_proof() {
    let (block, _, _, view) = block(1, "0x0");
    for path in [vec!["Nexus".into()], vec!["Nexus".into(), "Child".into()]] {
        let (url, task) = server(vec![view.clone()]).await;
        let result = client(&url, path.clone(), vec![block.clone()])
            .block("1", &CancellationToken::new())
            .await;
        if path.len() == 1 {
            assert!(matches!(result, Err(LatticeClientError::ProofOfWork)));
        } else {
            assert!(result.is_ok());
        }
        task.await.unwrap();
    }
}
#[tokio::test]
async fn transaction_requires_requested_cid_and_contained_verified_body() {
    let body = volume(cbor::encode(&json!({"example":"body"})).unwrap());
    let mut tx =
        volume(cbor::encode(&json!({"body":{"rawCID":body.root_cid},"signatures":[]})).unwrap());
    let projection = json!({"txCID":tx.root_cid,"nonce":"0","signers":[],"chainPath":["Nexus"],"accountActions":[],"depositActions":[],"receiptActions":[],"withdrawalActions":[]});
    for mode in 0..3 {
        let mut volume = tx.clone();
        if mode != 0 {
            volume.entries.extend(body.entries.clone());
        }
        let mut projection = projection.clone();
        if mode == 2 {
            projection["txCID"] = "wrong".into();
        }
        let (url, task) = server(vec![projection]).await;
        let result = client(&url, vec!["Nexus".into()], vec![volume])
            .transaction(&tx.root_cid, &CancellationToken::new())
            .await;
        match mode {
            0 => assert!(matches!(result, Err(LatticeClientError::MissingBody))),
            1 => assert_eq!(result.unwrap().body, body.entries[0].bytes),
            _ => assert!(matches!(
                result,
                Err(LatticeClientError::Mismatch("transaction CID"))
            )),
        };
        task.await.unwrap();
    }
    // The envelope decoder deliberately does not treat empty signatures as admission.
    assert!(
        decode_canonical_signed_transaction(&tx.entries.remove(0).bytes)
            .unwrap()
            .signatures
            .is_empty()
    );
}
