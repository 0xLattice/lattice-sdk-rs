use lattice_sdk::canonical::*;
use serde_json::{Value, json};
fn block_json(b: &CanonicalBlock) -> Value {
    let mut result = json!({"version":b.version,"transactionsCID":b.transactions_cid,"target":b.target,"nextTarget":b.next_target,"specCID":b.spec_cid,"parentStateCID":b.parent_state_cid,"previousStateCID":b.previous_state_cid,"postStateCID":b.post_state_cid,"childrenCID":b.children_cid,"height":b.height.to_string(),"timestamp":b.timestamp.to_string(),"nonce":b.nonce.to_string()});
    if let Some(parent) = &b.parent_cid {
        result["parentCID"] = parent.clone().into();
    }
    if let Some(recipient) = &b.reward_recipient {
        result["rewardRecipient"] = recipient.clone().into();
    }
    result
}
#[test]
fn actual_typescript_canonical_decoders_pow_and_work_match() {
    let corpus: Value = serde_json::from_str(include_str!("canonical-typescript.json")).unwrap();
    let cases = corpus["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 50);
    let mut accepted = 0;
    for case in cases {
        let bytes = hex::decode(case["bytes"].as_str().unwrap()).unwrap();
        let result=match case["kind"].as_str().unwrap(){
            "block"=>decode_canonical_block(&bytes).map(|b|{
                let pow=canonical_block_pow_preimage(&b).map(|preimage|json!({"preimage":hex::encode(preimage),"hash":hex::encode(canonical_block_pow_hash(&b).unwrap()),"valid":verify_nexus_block_pow(&b).unwrap()})).unwrap_or(Value::Null);
                assert_eq!(pow,case["pow"],"{}",case["name"]);block_json(&b)
            }),
            "state"=>decode_canonical_lattice_state(&bytes).map(|s|json!({"accountStateCID":s.account_state_cid,"generalStateCID":s.general_state_cid,"depositStateCID":s.deposit_state_cid,"receiptStateCID":s.receipt_state_cid})),
            "transaction"=>decode_canonical_signed_transaction(&bytes).map(|t|json!({"bodyCID":t.body_cid,"signatures":t.signatures.iter().map(|s|json!({"publicKey":s.public_key,"signature":s.signature})).collect::<Vec<_>>()})),
            _=>panic!("unknown fixture")
        };
        if result.is_ok() {
            accepted += 1;
        }
        assert_eq!(
            result.unwrap_or(Value::Null),
            case["result"],
            "{}",
            case["name"]
        );
    }
    assert!(accepted > 10);
    for row in corpus["work"].as_array().unwrap() {
        assert_eq!(
            work_for_target(row["target"].as_str().unwrap())
                .ok()
                .map(|n| n.to_string()),
            row["result"].as_str().map(str::to_owned),
            "{row}"
        );
    }
}
#[test]
fn continuity_checks_exact_link_height_and_state_without_overflow() {
    let corpus: Value = serde_json::from_str(include_str!("canonical-typescript.json")).unwrap();
    let mut parent = decode_canonical_block(
        &hex::decode(corpus["cases"][0]["bytes"].as_str().unwrap()).unwrap(),
    )
    .unwrap();
    let mut child = parent.clone();
    child.parent_cid = Some("parent".into());
    child.height = 1;
    child.previous_state_cid = parent.post_state_cid.clone();
    assert!(assert_block_continuity("parent", &parent, &child).is_ok());
    assert!(assert_block_continuity("other", &parent, &child).is_err());
    child.height = 2;
    assert!(assert_block_continuity("parent", &parent, &child).is_err());
    child.height = 1;
    child.previous_state_cid = "wrong".into();
    assert!(assert_block_continuity("parent", &parent, &child).is_err());
    parent.height = u64::MAX;
    child.height = 0;
    child.previous_state_cid = parent.post_state_cid.clone();
    assert!(assert_block_continuity("parent", &parent, &child).is_err());
}
