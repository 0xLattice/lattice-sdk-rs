use lattice_sdk::{primitives::crypto, transactions::*};
use std::collections::BTreeMap;
#[test]
fn actual_typescript_transaction_bytes_signatures_and_payloads_match() {
    let cases: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("transactions-typescript.json")).unwrap();
    assert_eq!(cases.len(), 27);
    for case in cases {
        let account =
            SigningAccount::from_private_key(&[case["seed"].as_u64().unwrap() as u8; 32]).unwrap();
        assert_eq!(account.address(), case["address"].as_str().unwrap());
        assert_eq!(account.public_key(), case["publicKey"].as_str().unwrap());
        let path: Vec<String> = serde_json::from_value(case["chainPath"].clone()).unwrap();
        let integer = |field: &str| case[field].as_str().unwrap().parse::<u64>().unwrap();
        let body = build_transfer(
            account.address(),
            case["recipient"].as_str().unwrap(),
            integer("amount"),
            integer("fee"),
            integer("nonce"),
            &path,
        )
        .unwrap();
        let encoded = encode_transaction_body(&body).unwrap();
        assert_eq!(encoded.cid, case["bodyCID"]);
        assert_eq!(hex::encode(&encoded.bytes), case["bodyHex"]);
        let signed = account.sign_body(&body).unwrap();
        assert_eq!(signed.signature, case["signature"]);
        assert!(crypto::verify(
            &body.signing_preimage().unwrap(),
            &signed.signature,
            &signed.public_key
        ));
        let signatures = BTreeMap::from([(signed.public_key, signed.signature)]);
        let envelope = encode_signed_transaction(&signatures, &encoded.cid).unwrap();
        assert_eq!(envelope.cid, case["transactionCID"]);
        assert_eq!(hex::encode(&envelope.bytes), case["transactionHex"]);
        assert_eq!(
            signed_transaction_size(&body).unwrap() as u64,
            case["size"].as_u64().unwrap()
        );
        assert_eq!(
            state_growth_ceiling(&body),
            i128::from(case["growth"].as_i64().unwrap())
        );
        assert_eq!(
            transaction_payload(&signatures, &body).unwrap(),
            case["payload"]
        );
    }
}
#[test]
fn signing_rejects_ambiguous_keys_and_unencodable_inputs() {
    let account = SigningAccount::from_private_key(&[7; 32]).unwrap();
    let mut body =
        build_transfer(account.address(), "recipient", 1, 0, 0, &["Nexus".into()]).unwrap();
    let signed = account.sign_body(&body).unwrap();
    let canonical = BTreeMap::from([(signed.public_key.clone(), signed.signature.clone())]);
    let alias = BTreeMap::from([(
        format!("0x{}", signed.public_key.to_uppercase()),
        format!("0x{}", signed.signature.to_uppercase()),
    )]);
    assert_eq!(
        encode_signed_transaction(&canonical, &signed.body_cid)
            .unwrap()
            .cid,
        encode_signed_transaction(&alias, &signed.body_cid)
            .unwrap()
            .cid
    );
    let mut duplicate = canonical;
    duplicate.extend(alias);
    assert!(encode_signed_transaction(&duplicate, &signed.body_cid).is_err());
    body.chain_path.clear();
    assert!(account.sign_body(&body).is_err());
    assert!(build_transfer("same", "same", 1, 0, 0, &[]).is_err());
    assert!(build_transfer("a", "b", i64::MAX as u64, 1, 0, &[]).is_err());
}

#[test]
fn cross_chain_nonce_limit_and_wire_decimal_fields_are_enforced() {
    use lattice_sdk::primitives::{DepositAction, ReceiptAction, WithdrawalAction};
    let mut body = build_transfer("alice", "bob", 1, 0, 0, &["Nexus".into()]).unwrap();
    body.deposit_actions.push(DepositAction {
        nonce: u64::MAX.into(),
        demander: "alice".into(),
        amount_demanded: u64::MAX,
        amount_deposited: 9,
    });
    body.receipt_actions.push(ReceiptAction {
        withdrawer: "bob".into(),
        nonce: 1,
        demander: "alice".into(),
        amount_demanded: 2,
        directory: "Child".into(),
    });
    body.withdrawal_actions.push(WithdrawalAction {
        withdrawer: "bob".into(),
        nonce: 1,
        demander: "alice".into(),
        amount_demanded: 2,
        amount_withdrawn: 3,
    });
    let payload = transaction_payload(&BTreeMap::new(), &body).unwrap();
    assert_eq!(
        payload["transaction"]["body"]["depositActions"][0]["nonce"],
        u64::MAX.to_string()
    );
    assert_eq!(
        payload["transaction"]["body"]["receiptActions"][0]["amountDemanded"],
        "2"
    );
    assert_eq!(
        payload["transaction"]["body"]["withdrawalActions"][0]["amountWithdrawn"],
        "3"
    );
    assert!(encode_transaction_body(&body).is_ok());
    body.deposit_actions[0].nonce = u128::from(u64::MAX) + 1;
    assert!(encode_transaction_body(&body).is_err());
    assert!(transaction_payload(&BTreeMap::new(), &body).is_err());
}
