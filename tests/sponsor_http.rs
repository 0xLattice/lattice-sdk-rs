use lattice_sdk::{primitives::TransactionBody, sponsor_http::*, transactions::*};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tokio_util::sync::CancellationToken;
async fn server(
    status: u16,
    body: String,
    stall: bool,
) -> (String, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        loop {
            let mut buffer = [0; 4096];
            let count = socket.read(&mut buffer).await.unwrap();
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..count]);
            if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                let header = String::from_utf8_lossy(&bytes[..end]);
                let length = header
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .and_then(|v| v.parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                if bytes.len() >= end + 4 + length {
                    break;
                }
            }
        }
        let response = if stall {
            "HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\n{".into()
        } else {
            format!(
                "HTTP/1.1 {status} Test\r\nConnection: close\r\nLocation: http://127.0.0.1:1/\r\n\r\n{body}"
            )
        };
        socket.write_all(response.as_bytes()).await.unwrap();
        if stall {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        String::from_utf8(bytes).unwrap()
    });
    (format!("http://{addr}/prefix/"), task)
}
fn payload(request: &str) -> Value {
    serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap()
}
#[tokio::test]
async fn actual_typescript_work_and_store_parsers_match_over_http() {
    let fixture: Value =
        serde_json::from_str(include_str!("sponsor-http-typescript.json")).unwrap();
    let cancel = CancellationToken::new();
    assert_eq!(fixture["work"].as_array().unwrap().len(), 17);
    assert_eq!(fixture["status"].as_array().unwrap().len(), 10);
    for row in fixture["work"].as_array().unwrap() {
        let (url, task) = server(200, row["answer"].to_string(), false).await;
        let client = SponsorClient::new(&url, Default::default()).unwrap();
        let before = Instant::now();
        let result = client
            .request_work(
                WorkRequest {
                    client: Some("client"),
                    chain: Some("Nexus/Child"),
                    pay: Some(&2u8.into()),
                    store: Some("root"),
                },
                &cancel,
            )
            .await;
        assert_eq!(result.is_ok(), row["accepted"].as_bool().unwrap(), "{row}");
        if let Ok(work) = result {
            assert_eq!(work.start_nonce.to_string(), "18446744073709551616");
            assert_eq!(
                work.share_target.to_string(),
                if row["name"] == "zero-target" {
                    "0"
                } else {
                    "15"
                }
            );
            let elapsed = Duration::from_millis(
                row["answer"]["expiresInMilliseconds"]
                    .as_str()
                    .unwrap()
                    .parse()
                    .unwrap(),
            );
            assert!(work.deadline >= before + elapsed && work.deadline <= Instant::now() + elapsed);
        }
        let request = task.await.unwrap();
        assert!(request.starts_with("POST /prefix/work HTTP/1.1"));
        assert_eq!(
            payload(&request),
            json!({"client":"client","chain":"Nexus/Child","pay":"2","store":"root"})
        );
        assert!(!request.contains("authorization:"));
    }
    for row in fixture["status"].as_array().unwrap() {
        let (url, task) = server(200, row["answer"].to_string(), false).await;
        let result = SponsorClient::new(&url, Default::default())
            .unwrap()
            .store_status("root /雪?", &cancel)
            .await;
        assert_eq!(result.is_ok(), row["accepted"].as_bool().unwrap(), "{row}");
        if let Ok(status) = result {
            assert_eq!(status.bytes.to_string(), "184467440737095516160");
        }
        let request = task.await.unwrap();
        assert!(request.starts_with("GET /prefix/store/root%20%2F%E9%9B%AA%3F HTTP/1.1"));
        assert!(!request.contains("content-type:"));
    }
}
#[tokio::test]
async fn shares_upload_and_sponsored_transaction_use_exact_bodies() {
    let cancel = CancellationToken::new();
    let (url, task) = server(
        200,
        json!({"credit":"123456789012345678901234567890"}).to_string(),
        false,
    )
    .await;
    let client = SponsorClient::new(&url, Default::default()).unwrap();
    assert_eq!(
        client
            .submit_share("job", &(-1).into(), &cancel)
            .await
            .unwrap()
            .to_string(),
        "123456789012345678901234567890"
    );
    let request = task.await.unwrap();
    assert!(request.starts_with("POST /prefix/shares HTTP/1.1"));
    assert_eq!(payload(&request), json!({"assignment":"job","nonce":"-1"}));
    let (url, task) = server(
        200,
        json!({"root":"root","bytes":"3","needed":{},"credit":"3","stored":true}).to_string(),
        false,
    )
    .await;
    let volume = StoreVolume {
        cid: "volume".into(),
        entries: BTreeMap::from([("entry".into(), "YWJj".into())]),
    };
    let status = SponsorClient::new(&url, Default::default())
        .unwrap()
        .store("root", &[volume], &cancel)
        .await
        .unwrap();
    assert!(status.stored);
    let request = task.await.unwrap();
    assert!(request.starts_with("POST /prefix/store HTTP/1.1"));
    assert_eq!(
        payload(&request),
        json!({"root":"root","volumes":[{"cid":"volume","entries":{"entry":"YWJj"}}]})
    );
    let fixture: Value = serde_json::from_str(include_str!("sponsor-typescript.json")).unwrap();
    let mut draft = fixture["draft"].clone();
    draft["nonce"] = 3.into();
    for action in draft["accountActions"].as_array_mut().unwrap() {
        action["delta"] = action["delta"]
            .as_str()
            .unwrap()
            .parse::<i64>()
            .unwrap()
            .into();
    }
    let draft: TransactionBody = serde_json::from_value(draft).unwrap();
    let (url, task) = server(200, fixture["cases"][0]["answer"].to_string(), false).await;
    let proposal = SponsorClient::new(&url, Default::default())
        .unwrap()
        .sponsor_transaction("token", &draft, &2u8.into(), &cancel)
        .await
        .unwrap();
    assert_eq!(proposal.fee, 5u8.into());
    let request = task.await.unwrap();
    assert!(request.starts_with("POST /prefix/sponsor HTTP/1.1"));
    assert_eq!(
        payload(&request),
        json!({"client":"token","transaction":transaction_payload(&BTreeMap::new(),&draft).unwrap()["transaction"],"pay":"2"})
    );
}
#[tokio::test]
async fn refusals_limits_deadlines_and_cancellation_cover_body() {
    let cancel = CancellationToken::new();
    for (status, body, reason) in [
        (
            403,
            "{\"error\":\"insufficientCredit\"}",
            Some("insufficientCredit"),
        ),
        (
            503,
            "{\"error\":{\"message\":\"expired\"}}",
            Some("expired"),
        ),
        (302, "", None),
        (200, "[]", Some("invalid sponsor response")),
        (200, "bad", Some("invalid JSON response")),
    ] {
        let (url, task) = server(status, body.into(), false).await;
        let result = SponsorClient::new(&url, Default::default())
            .unwrap()
            .submit_share("job", &0.into(), &cancel)
            .await;
        match result {
            Err(SponsorError::Refused {
                status: actual,
                refusal,
            }) => {
                assert_eq!(actual, status);
                assert_eq!(refusal.as_deref(), reason);
            }
            other => panic!("{other:?}"),
        }
        task.await.unwrap();
    }
    let (url, task) = server(200, "x".repeat(100), false).await;
    let client = SponsorClient::new(
        &url,
        SponsorClientOptions {
            maximum_response_bytes: 10,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        matches!(client.submit_share("job",&0.into(),&cancel).await,Err(SponsorError::Refused{refusal:Some(r),..})if r=="response too large")
    );
    task.await.unwrap();
    let (url, task) = server(200, String::new(), true).await;
    let client = SponsorClient::new(
        &url,
        SponsorClientOptions {
            timeout: Duration::from_millis(10),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(matches!(
        client.submit_share("job", &0.into(), &cancel).await,
        Err(SponsorError::Timeout)
    ));
    task.abort();
    let (url, task) = server(200, String::new(), true).await;
    let client = SponsorClient::new(
        &url,
        SponsorClientOptions {
            maximum_response_bytes: 10,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        matches!(client.submit_share("job",&0.into(),&cancel).await,Err(SponsorError::Refused{refusal:Some(r),..})if r=="response too large")
    );
    task.abort();
    cancel.cancel();
    assert!(matches!(
        client.submit_share("job", &0.into(), &cancel).await,
        Err(SponsorError::Cancelled)
    ));
    for url in [
        "http://example.org",
        "https://user:pass@example.org",
        "https://example.org/#fragment",
    ] {
        assert!(SponsorClient::new(url, Default::default()).is_err());
    }
}

#[tokio::test]
async fn expiry_starts_before_response_and_inflight_cancellation_stops_body() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buffer = [0; 4096];
        let mut request = Vec::new();
        while !request.windows(4).any(|part| part == b"\r\n\r\n") {
            let count = socket.read(&mut buffer).await.unwrap();
            assert!(count > 0);
            request.extend_from_slice(&buffer[..count]);
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
        let answer = json!({"client":"c","assignment":"a","prefixHex":"","shareTarget":"f","startNonce":"0","count":"1","fee":"0","pay":"0","sharesForFee":"0","expiresInMilliseconds":"1"});
        socket
            .write_all(format!("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n{answer}").as_bytes())
            .await
            .unwrap();
    });
    let client = SponsorClient::new(&format!("http://{address}"), Default::default()).unwrap();
    let work = client
        .request_work(Default::default(), &CancellationToken::new())
        .await
        .unwrap();
    assert!(work.deadline < Instant::now());
    assert!(
        work.search()
            .unwrap()
            .next(&CancellationToken::new())
            .await
            .unwrap()
            .is_none()
    );
    task.await.unwrap();
    let (url, task) = server(200, String::new(), true).await;
    let client = SponsorClient::new(&url, Default::default()).unwrap();
    let cancel = CancellationToken::new();
    let other = cancel.clone();
    let canceller = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        other.cancel();
    });
    assert!(matches!(
        client.submit_share("job", &0.into(), &cancel).await,
        Err(SponsorError::Cancelled)
    ));
    task.abort();
    canceller.await.unwrap();
    let (url, task) = server(200, "\u{feff}{\"credit\":\"1\"}".into(), false).await;
    assert_eq!(
        SponsorClient::new(&url, Default::default())
            .unwrap()
            .submit_share("job", &0.into(), &CancellationToken::new())
            .await
            .unwrap(),
        1u8.into()
    );
    task.await.unwrap();
}
