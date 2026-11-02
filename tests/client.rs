use lattice_sdk::wire::{Decimal, SafeInteger};
use lattice_sdk::*;
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tokio_util::sync::CancellationToken;

async fn server(
    response: impl AsRef<[u8]>,
    stall: bool,
) -> (String, tokio::task::JoinHandle<String>) {
    let response = response.as_ref().to_vec();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        loop {
            let mut chunk = [0; 1024];
            let size = stream.read(&mut chunk).await.unwrap();
            if size == 0 {
                break;
            }
            request.extend_from_slice(&chunk[..size]);
            if request.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        stream.write_all(&response).await.unwrap();
        if stall {
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
        String::from_utf8(request).unwrap()
    });
    (format!("http://{address}/rpc"), task)
}
#[test]
fn canonical_wire_values_and_url_policy() {
    for value in [r#""00""#, r#""-0""#, r#""+1""#, "1", "null", r#"" 1""#] {
        assert!(serde_json::from_str::<Decimal>(value).is_err(), "{value}");
    }
    let value: Decimal = serde_json::from_str(r#""18446744073709551616""#).unwrap();
    assert_eq!(value.0.to_string(), "18446744073709551616");
    for value in ["9007199254740992", "1.5", r#""1""#] {
        assert!(serde_json::from_str::<SafeInteger>(value).is_err());
    }
    assert_eq!(serde_json::from_str::<SafeInteger>("1.0").unwrap().0, 1);
    assert!(serde_json::from_str::<ChainInfo>(r#"{"chain":[],"height":null}"#).is_err());
    for url in [
        "http://example.org",
        "https://u:p@example.org",
        "https://example.org?",
        "https://example.org#",
        "file:///tmp/x",
    ] {
        assert!(normalize_node_url(url).is_err(), "{url}");
    }
    assert!(normalize_node_url("http://[::1]:8080").is_ok());
    assert_eq!(node_cookie_authorization(" abc ").unwrap(), "Bearer abc");
    assert_eq!(
        node_cookie_authorization("__cookie__:abc").unwrap(),
        "Basic X19jb29raWVfXzphYmM="
    );
    assert!(node_cookie_authorization("a\nb").is_err());
}
#[tokio::test]
async fn real_read_preserves_prefix_encodes_segments_and_exact_numbers() {
    let (url, task) = server(b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{\"owner\":\"x\",\"balance\":\"18446744073709551616\",\"nonce\":\"-1\"}", false).await;
    let client = NodeClient::new(
        &url,
        vec!["Nexus".into(), "Alpha".into()],
        ClientOptions {
            authorization: Some("Bearer test".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let account = client
        .account("x/y?#", &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(account.balance.0.to_string(), "18446744073709551616");
    let request = task.await.unwrap();
    assert!(
        request
            .starts_with("GET /rpc/api/state/account/x%2Fy%3F%23?chainPath=Nexus%2FAlpha HTTP/1.1"),
        "{request}"
    );
    assert!(request.contains("authorization: Bearer test"));
}
#[tokio::test]
async fn redirects_and_bounded_streams_are_refused() {
    for response in [
        &b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/\r\nContent-Length: 0\r\n\r\n"[..],
        &b"HTTP/1.1 200 OK\r\nContent-Length: 500\r\n\r\n"[..],
        &b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n123456789"[..],
    ] {
        let (url, task) = server(response, false).await;
        let client = NodeClient::new(
            &url,
            vec!["Nexus".into()],
            ClientOptions {
                maximum_response_bytes: 8,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(matches!(
            client.mempool(&CancellationToken::new()).await,
            Err(ClientError::Node { .. })
        ));
        task.await.unwrap();
    }
}
#[tokio::test]
async fn timeout_covers_body_and_cancellation_precedes_dial() {
    let (url, task) = server(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{", true).await;
    let client = NodeClient::new(
        &url,
        vec!["Nexus".into()],
        ClientOptions {
            timeout: Duration::from_millis(50),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(matches!(
        client.mempool(&CancellationToken::new()).await,
        Err(ClientError::Timeout)
    ));
    task.abort();
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        client.mempool(&cancel).await,
        Err(ClientError::Cancelled)
    ));
}

async fn json_client(body: serde_json::Value) -> (NodeClient, tokio::task::JoinHandle<String>) {
    let response = format!("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{body}");
    let (url, task) = server(response, false).await;
    (
        NodeClient::new(&url, vec!["Nexus".into()], Default::default()).unwrap(),
        task,
    )
}
#[tokio::test]
async fn block_pagination_checks_bounds_order_and_continuation() {
    use serde_json::json;
    let row = |height| json!({"height":height,"hash":"cid","timestamp":"1","transactionCount":0});
    let before = Decimal(10.into());
    for (body, valid) in [
        (json!({"blocks":[row("9"),row("8")],"nextBefore":"8"}), true),
        (json!({"blocks":[row("10")]}), false),
        (json!({"blocks":[row("9"),row("9")]}), false),
        (
            json!({"blocks":[row("9"),row("8")],"nextBefore":"9"}),
            false,
        ),
        (json!({"blocks":[row("9"),row("8"),row("7")]}), false),
        (json!({"blocks":[],"nextBefore":"0"}), false),
    ] {
        let (client, task) = json_client(body).await;
        assert_eq!(
            client
                .blocks(Some(&before), Some(2), &CancellationToken::new())
                .await
                .is_ok(),
            valid
        );
        let request = task.await.unwrap();
        assert!(request.contains("chainPath=Nexus&before=10&limit=2"));
    }
}
#[tokio::test]
async fn child_pages_filter_legacy_responses_and_refuse_nonprogress() {
    use serde_json::json;
    for (body, expected) in [
        (
            json!({"chainPath":["Nexus"],"directories":["A","B","C"],"next":"C"}),
            Some(vec!["C"]),
        ),
        (json!({"chainPath":["Wrong"],"directories":[]}), None),
        (json!({"chainPath":["Nexus"],"directories":["C","B"]}), None),
        (
            json!({"chainPath":["Nexus"],"directories":["B"],"next":"B"}),
            None,
        ),
        (
            json!({"chainPath":["Nexus"],"directories":["C"],"next":"D"}),
            None,
        ),
    ] {
        let (client, task) = json_client(body).await;
        let result = client
            .child_directories(Some("B"), Some(2), &CancellationToken::new())
            .await;
        assert_eq!(
            result.ok().map(|p| p.directories),
            expected.map(|v| v.into_iter().map(str::to_owned).collect())
        );
        task.await.unwrap();
    }
}
#[test]
fn fee_projection_uses_exact_rates_and_legacy_weight_fallback() {
    let block: ProjectedBlock = serde_json::from_str(
        r#"{"transactionCount":2,"bytes":600,"lowestFee":"10","lowestFeeBytes":300,"full":true}"#,
    )
    .unwrap();
    assert_eq!(block.lowest_fee_weight.0, 300);
    assert_eq!(
        fee_to_outbid(&block, SafeInteger(450))
            .unwrap()
            .0
            .to_string(),
        "16"
    );
    assert!(fee_to_outbid(&block, SafeInteger(0)).is_err());
    for bad in [
        r#"{"transactionCount":2,"bytes":600,"lowestFee":"10","lowestFeeBytes":0,"full":true}"#,
        r#"{"transactionCount":2,"bytes":600,"lowestFee":"10","lowestFeeBytes":300,"lowestFeeWeight":null,"full":true}"#,
    ] {
        assert!(serde_json::from_str::<ProjectedBlock>(bad).is_err());
    }
    let weight = FeeWeight {
        block_room: SafeInteger(1000),
        growth_room: SafeInteger(100),
        max_transactions: SafeInteger(10),
        transaction_allowance: SafeInteger(20),
    };
    assert_eq!(
        transaction_weight(SafeInteger(1), SafeInteger(11), Some(&weight)),
        110
    );
    assert_eq!(
        transaction_weight(SafeInteger(1), SafeInteger(0), Some(&weight)),
        100
    );
    assert_eq!(
        transaction_weight(SafeInteger(1), SafeInteger(101), Some(&weight)),
        1000
    );
}
