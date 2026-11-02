#[cfg(target_arch = "wasm32")]
mod browser {
    use lattice_sdk::{ClientError, ClientOptions, NodeClient, runtime};
    use std::{sync::Arc, time::Duration};
    use tokio_util::sync::CancellationToken;
    use wasm_bindgen::prelude::*;
    fn client(origin: &str, path: &str, timeout: u64, cap: usize) -> NodeClient {
        NodeClient::new(
            &format!("{origin}/{path}"),
            vec!["root".into()],
            ClientOptions {
                timeout: Duration::from_millis(timeout),
                maximum_response_bytes: cap,
                authorization: Some("Bearer browser-test".into()),
            },
        )
        .unwrap()
    }
    #[wasm_bindgen]
    pub async fn run(origin: String) -> Result<String, JsValue> {
        let mut checks = vec![];
        let cancel = CancellationToken::new();
        let info = client(&origin, "normal", 2000, 4096)
            .chain_info(&cancel)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        assert_eq!(info.chain, vec!["root"]);
        checks.push("HTTP read and explicit authorization");
        let result = client(&origin, "oversize", 2000, 32)
            .chain_info(&cancel)
            .await;
        assert!(
            matches!(result,Err(ClientError::Node{refusal:Some(ref message),..}) if message=="response too large")
        );
        checks.push("streaming response cap");
        let result = client(&origin, "delayed", 30, 4096)
            .chain_info(&cancel)
            .await;
        assert!(matches!(result, Err(ClientError::Timeout)));
        checks.push("deadline abort");
        let cancelled = CancellationToken::new();
        let signal = cancelled.clone();
        wasm_bindgen_futures::spawn_local(async move {
            runtime::sleep(Duration::from_millis(30)).await;
            signal.cancel();
        });
        assert!(matches!(
            client(&origin, "delayed", 2000, 4096)
                .chain_info(&cancelled)
                .await,
            Err(ClientError::Cancelled)
        ));
        checks.push("cancellation abort");
        assert!(
            client(&origin, "redirect", 2000, 4096)
                .chain_info(&cancel)
                .await
                .is_err()
        );
        checks.push("redirect refusal");
        assert!(matches!(
            client(&origin, "slow-body", 30, 4096)
                .chain_info(&cancel)
                .await,
            Err(ClientError::Timeout)
        ));
        let body_cancel = CancellationToken::new();
        let signal = body_cancel.clone();
        wasm_bindgen_futures::spawn_local(async move {
            runtime::sleep(Duration::from_millis(30)).await;
            signal.cancel();
        });
        assert!(matches!(
            client(&origin, "slow-body", 2000, 4096)
                .chain_info(&body_cancel)
                .await,
            Err(ClientError::Cancelled)
        ));
        checks.push("body-stream cancellation and deadline");
        use lattice_sdk::volumes::{
            HttpVolumeOptions, HttpVolumeTransport, VolumeClient, VolumeLimits,
        };
        let root = lattice_cashew::cid_for_bytes(0x71, &[0xa0]);
        let volumes = VolumeClient::new(
            HttpVolumeTransport::new(&format!("{origin}/volume"), HttpVolumeOptions::default())
                .unwrap(),
            VolumeLimits::default(),
        );
        let volume = volumes
            .get(&root, &cancel)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        assert_eq!(volume.root_cid(), root);
        checks.push("complete authenticated Volume HTTP transfer");
        let submission = lattice_sdk::submission::HttpTransactionSubmitter::new(
            &format!("{origin}/submit"),
            None,
        )
        .unwrap();
        let submitted = submission
            .submit(&serde_json::json!({"browser":"test"}), &cancel)
            .await
            .map_err(|e| JsValue::from_str(&e.to_string()))?;
        assert_eq!(submitted.transaction_cid, "test-cid");
        checks.push("explicit HTTP submission");

        use lattice_sdk::endpoints::{EndpointReader, HttpEndpointReader};
        let reader = HttpEndpointReader {
            check_dns: true,
            ..Default::default()
        };
        assert!(matches!(
            reader.lookup("example.com", &cancel).await,
            Err(ClientError::Configuration(_))
        ));
        checks.push("explicit DNS capability refusal");
        use lattice_sdk::chain_nodes::{ChainNodes, ChainNodesOptions, HttpChainNodeDiscovery};
        let nodes = ChainNodes::new(
            [format!("{origin}/normal")],
            Arc::new(HttpChainNodeDiscovery::default()),
            ChainNodesOptions::default(),
        )
        .unwrap();
        let path = vec!["root".into()];
        let (a, b) = futures_util::join!(nodes.list(&path), nodes.list(&path));
        assert_eq!(a.unwrap().len(), 1);
        assert_eq!(b.unwrap().len(), 1);
        checks.push("shared browser discovery task");
        // Ed25519 key generation exercises browser CSPRNG; the deadline uses
        // browser monotonic time, and CPU search yields to timer tasks.
        use lattice_sdk::sponsor::ShareSearch;
        use num_bigint::BigUint;
        let mut rng = rand_core::OsRng;
        let key = ed25519_dalek::SigningKey::generate(&mut rng);
        assert_ne!(key.to_bytes(), [0; 32]);
        let mut search = ShareSearch::new(
            "00",
            &((BigUint::from(1u8) << 256usize) - 1u8),
            0u8.into(),
            1u8.into(),
            runtime::Instant::now() + Duration::from_secs(1),
        )
        .unwrap();
        assert!(search.next(&cancel).await.unwrap().is_some());
        checks.push("browser entropy and sponsor share search");
        Ok(serde_json::to_string(&checks).unwrap())
    }
}
