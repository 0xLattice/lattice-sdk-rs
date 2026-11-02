use lattice_sdk::ChainInfo;
use serde_json::json;
#[test]
fn chain_info_finality_is_optional_decimal_and_preserves_u64_width() {
    let omitted: ChainInfo = serde_json::from_value(json!({"chain":["Nexus"]})).unwrap();
    assert!(omitted.finality_window.is_none());
    let present: ChainInfo = serde_json::from_value(
        json!({"chain":["Nexus","A"],"finalityWindow":"18446744073709551615"}),
    )
    .unwrap();
    assert_eq!(
        present.finality_window.unwrap().0.to_string(),
        "18446744073709551615"
    );
    assert!(
        serde_json::from_value::<ChainInfo>(json!({"chain":["Nexus"],"finalityWindow":null}))
            .is_err()
    );
}
