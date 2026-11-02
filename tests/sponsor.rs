use lattice_sdk::{primitives::TransactionBody, sponsor::*};
#[test]
fn actual_typescript_sponsor_proposal_checks_match() {
    let corpus: serde_json::Value =
        serde_json::from_str(include_str!("sponsor-typescript.json")).unwrap();
    let mut draft = corpus["draft"].clone();
    draft["nonce"] = draft["nonce"]
        .as_str()
        .unwrap()
        .parse::<u64>()
        .unwrap()
        .into();
    for action in draft["accountActions"].as_array_mut().unwrap() {
        action["delta"] = action["delta"]
            .as_str()
            .unwrap()
            .parse::<i64>()
            .unwrap()
            .into();
    }
    let draft: TransactionBody = serde_json::from_value(draft).unwrap();
    let cases = corpus["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 18);
    let mut accepted = 0;
    for case in cases {
        let result = validate_sponsor_answer(&draft, &2u64.into(), &case["answer"]);
        assert_eq!(
            result.is_ok(),
            case["accepted"].as_bool().unwrap(),
            "{}",
            case["name"]
        );
        if let Ok(proposal) = result {
            accepted += 1;
            assert_eq!(proposal.fee, 5u64.into());
            assert_eq!(proposal.pay, 2u64.into());
            assert_eq!(proposal.body.signers.len(), 2);
        }
    }
    assert_eq!(accepted, 2); // Valid signature and opaque signature awaiting validation.
}

#[tokio::test]
async fn actual_typescript_share_hashes_and_search_ranges_match() {
    use std::time::{Duration, Instant};
    use tokio_util::sync::CancellationToken;
    let corpus: serde_json::Value =
        serde_json::from_str(include_str!("sponsor-shares-typescript.json")).unwrap();
    let hashes = corpus["hashes"].as_array().unwrap();
    assert_eq!(hashes.len(), 77);
    for row in hashes {
        let result = share_hash(
            row["prefix"].as_str().unwrap(),
            &row["nonce"].as_str().unwrap().parse().unwrap(),
        );
        assert_eq!(
            result.ok().map(hex::encode),
            row["hash"].as_str().map(str::to_owned),
            "{row}"
        );
    }
    let searches = corpus["searches"].as_array().unwrap();
    assert_eq!(searches.len(), 6);
    for row in searches {
        let mut search = ShareSearch::new(
            row["prefix"].as_str().unwrap(),
            &row["target"].as_str().unwrap().parse().unwrap(),
            row["start"].as_str().unwrap().parse().unwrap(),
            row["count"].as_str().unwrap().parse().unwrap(),
            Instant::now() + Duration::from_secs(10),
        )
        .unwrap();
        let mut found = Vec::new();
        while let Some(nonce) = search.next(&CancellationToken::new()).await.unwrap() {
            found.push(nonce.to_string());
        }
        assert_eq!(serde_json::json!(found), row["found"], "{row}");
    }
}

#[tokio::test]
async fn share_search_yields_to_cancellation_and_checks_expiry_after_consumer() {
    use num_bigint::BigUint;
    use std::time::{Duration, Instant};
    use tokio_util::sync::CancellationToken;
    let max = (BigUint::from(1u8) << 256) - 1u8;
    let mut expired = ShareSearch::new("", &max, 0u8.into(), 10u8.into(), Instant::now()).unwrap();
    assert!(
        expired
            .next(&CancellationToken::new())
            .await
            .unwrap()
            .is_none()
    );
    let cancel = CancellationToken::new();
    let mut search = ShareSearch::new(
        "",
        &0u8.into(),
        0u8.into(),
        BigUint::from(1u8) << 100,
        Instant::now() + Duration::from_secs(10),
    )
    .unwrap();
    let other = cancel.clone();
    let cancellation = tokio::spawn(async move {
        tokio::task::yield_now().await;
        other.cancel();
    });
    assert!(search.next(&cancel).await.is_err());
    cancellation.await.unwrap();
    let mut search = ShareSearch::new(
        "",
        &max,
        0u8.into(),
        10u8.into(),
        Instant::now() + Duration::from_millis(20),
    )
    .unwrap();
    assert_eq!(
        search.next(&CancellationToken::new()).await.unwrap(),
        Some(0u8.into())
    );
    tokio::time::sleep(Duration::from_millis(25)).await;
    assert!(
        search
            .next(&CancellationToken::new())
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        ShareSearch::new(
            "",
            &(BigUint::from(1u8) << 256),
            0u8.into(),
            0u8.into(),
            Instant::now()
        )
        .is_err()
    );
}
