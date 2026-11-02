use lattice_sdk::volumes::*;
use serde_json::{Value, json};
fn limits(v: &Value) -> VolumeLimits {
    VolumeLimits {
        maximum_entries: v["maximumEntries"].as_u64().unwrap() as usize,
        maximum_bytes: v["maximumBytes"].as_u64().unwrap() as usize,
    }
}
fn bytes(v: &Value) -> Vec<u8> {
    serde_json::from_value(v.clone()).unwrap()
}
fn result(r: Result<Value, VolumeError>) -> Value {
    match r {
        Ok(v) => v,
        Err(e) => {
            json!({"ok":false,"code":match e{VolumeError::Verification{code,..}=>code.as_str(),VolumeError::Archive(_)=>"archive",VolumeError::Cbor(_)=>"cbor",_=>panic!("unexpected error {e}")}})
        }
    }
}
#[test]
fn actual_typescript_volume_archive_and_proof_corpus() {
    let rows: Vec<Value> = serde_json::from_str(include_str!("volumes-typescript.json")).unwrap();
    assert_eq!(rows.len(), 180);
    for row in rows {
        let name = row["name"].as_str().unwrap();
        if [
            "sha256_any_codec",
            "identity_80",
            "identity_5100",
            "identity_5120",
            "encode_identity_5100",
            "encode_sorted",
            "decode_roundtrip",
            "cbor_depth80",
        ]
        .contains(&name)
        {
            assert_eq!(row["result"]["ok"], true, "intended positive {name}");
        }
        if name == "encode_identity_5120" {
            assert_eq!(row["result"], json!({"ok":false,"code":"archive"}));
        }
        let actual = match row["op"].as_str().unwrap() {
            "verify" => result(
                verify_volume(
                    &serde_json::from_value(row["volume"].clone()).unwrap(),
                    row["expected"].as_str().unwrap(),
                    limits(&row["limits"]),
                )
                .map(|v| json!({"ok":true,"size":v.len()})),
            ),
            "encode" => result(
                encode_volume_archive(&serde_json::from_value(row["volume"].clone()).unwrap())
                    .map(|v| json!({"ok":true,"archive":v})),
            ),
            "decode" => result(
                decode_volume_archive(
                    row["root"].as_str().unwrap(),
                    &bytes(&row["archive"]),
                    limits(&row["limits"]),
                )
                .map(|v| json!({"ok":true,"size":v.entries.len()})),
            ),
            "cbor" => result(
                decode_dag_cbor(&bytes(&row["bytes"]), DagCborLimits::default())
                    .map(|_| json!({"ok":true})),
            ),
            "proof" => {
                let valid = verify_uint64_sparse_proof(&Uint64SparseProof {
                    root_cid: row["root"].as_str().unwrap().into(),
                    key: row["key"].as_str().unwrap().into(),
                    value: row["value"].as_str().map(|s| s.parse().unwrap()),
                    entries: serde_json::from_value(row["entries"].clone()).unwrap(),
                });
                assert_eq!(valid, row["valid"].as_bool().unwrap(), "{name}");
                continue;
            }
            _ => panic!("unknown case"),
        };
        assert_eq!(actual, row["result"], "{name}");
    }
}
#[test]
fn explicit_decoder_resource_limits_and_large_collection() {
    let deep = [vec![0x81; 80], vec![0]].concat();
    assert!(decode_dag_cbor(&deep, DagCborLimits::default()).is_ok());
    assert!(
        decode_dag_cbor(
            &deep,
            DagCborLimits {
                maximum_depth: 20,
                maximum_values: 1000
            }
        )
        .is_err()
    );
    let mut large = vec![0x9a, 0, 1, 0x11, 0x70];
    large.extend(vec![0; 70000]);
    assert!(decode_dag_cbor(&large, DagCborLimits::default()).is_ok());
}
#[tokio::test]
async fn memory_client_verifies_and_owns_bytes() {
    let raw = vec![0xa0];
    let cid = lattice_cashew::cid_for_bytes(0x71, &raw);
    let v = SerializedVolume {
        root_cid: cid.clone(),
        entries: vec![VolumeEntry {
            cid: cid.clone(),
            bytes: raw,
        }],
    };
    let transport = MemoryVolumeTransport::new([v]);
    let client = VolumeClient::new(transport, VolumeLimits::default());
    let cancel = tokio_util::sync::CancellationToken::new();
    let verified = client.get(&cid, &cancel).await.unwrap();
    assert_eq!(verified.root_bytes(), &[0xa0]);
    assert!(verified.decode_root().is_ok());
    assert!(matches!(
        client.get("missing", &cancel).await,
        Err(VolumeError::Verification {
            code: VolumeErrorCode::NotFound,
            ..
        })
    ));
}
