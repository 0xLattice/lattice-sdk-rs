use lattice_sdk::{ClientOptions, submission::*, transactions::*};
use std::{collections::BTreeMap, time::Duration};
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
    (format!("http://{addr}/transactions?chainPath=Nexus"), task)
}
#[tokio::test]
async fn signed_submission_checks_receipt_cid_and_preserves_wire_body() {
    let account = SigningAccount::from_private_key(&[7; 32]).unwrap();
    let body = build_transfer(account.address(), "recipient", 7, 1, 0, &["Nexus".into()]).unwrap();
    let signed = account.sign_body(&body).unwrap();
    let signatures = BTreeMap::from([(signed.public_key, signed.signature)]);
    let expected = encode_signed_transaction(&signatures, &signed.body_cid)
        .unwrap()
        .cid;
    for matched in [true, false] {
        let (url,task)=server(200,serde_json::json!({"transactionCID":if matched{expected.as_str()}else{"wrong"},"mempoolCount":1}).to_string(),false).await;
        let submitter = HttpTransactionSubmitter::new(
            &url,
            Some(ClientOptions {
                authorization: Some("Bearer test-cookie".into()),
                ..Default::default()
            }),
        )
        .unwrap();
        let result = submitter
            .submit_signed(&signatures, &body, &CancellationToken::new())
            .await;
        if matched {
            assert_eq!(result.unwrap().transaction_cid, expected);
        } else {
            assert!(matches!(result, Err(SubmissionError::WrongTransaction)));
        }
        let request = task.await.unwrap();
        assert!(request.starts_with("POST /transactions?chainPath=Nexus HTTP/1.1"));
        assert!(request.contains("authorization: Bearer test-cookie"));
        let sent: serde_json::Value =
            serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
        assert_eq!(sent, transaction_payload(&signatures, &body).unwrap());
    }
}
#[tokio::test]
async fn refusals_preserve_status_and_typed_policy_reason() {
    for (status, body, reason) in [
        (429, r#"{"error":"full"}"#, Some("full")),
        (429, "", None),
        (
            503,
            r#"{"error":{"message":"shuttingDown"}}"#,
            Some("shuttingDown"),
        ),
        (302, "", None),
    ] {
        let (url, task) = server(status, body.into(), false).await;
        let error = HttpTransactionSubmitter::new(&url, None)
            .unwrap()
            .submit(&serde_json::json!({}), &CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(error.reason(), reason);
        assert!(matches!(error,SubmissionError::Refused{status:s,..}if s==status));
        task.await.unwrap();
    }
}
#[tokio::test]
async fn malformed_counts_size_and_stalled_body_are_bounded() {
    for body in [
        r#"{"transactionCID":"x","mempoolCount":-1}"#,
        r#"{"transactionCID":"x","mempoolBytes":null}"#,
        r#"{"transactionCID":"x","mempoolBytes":9007199254740992}"#,
        r#"{"error":"no cid"}"#,
    ] {
        let (url, task) = server(200, body.into(), false).await;
        assert!(
            HttpTransactionSubmitter::new(&url, None)
                .unwrap()
                .submit(&serde_json::json!({}), &CancellationToken::new())
                .await
                .is_err()
        );
        task.await.unwrap();
    }
    let (url, task) = server(200, "x".repeat(20), false).await;
    let client = HttpTransactionSubmitter::new(
        &url,
        Some(ClientOptions {
            maximum_response_bytes: 8,
            ..Default::default()
        }),
    )
    .unwrap();
    assert!(
        matches!(client.submit(&serde_json::json!({}),&CancellationToken::new()).await,Err(SubmissionError::Refused{refusal:Some(s),..})if s=="response too large")
    );
    task.await.unwrap();
    let (url, task) = server(200, String::new(), true).await;
    let client = HttpTransactionSubmitter::new(
        &url,
        Some(ClientOptions {
            timeout: Duration::from_millis(20),
            ..Default::default()
        }),
    )
    .unwrap();
    assert!(matches!(
        client
            .submit(&serde_json::json!({}), &CancellationToken::new())
            .await,
        Err(SubmissionError::Timeout)
    ));
    task.abort();
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        client.submit(&serde_json::json!({}), &cancel).await,
        Err(SubmissionError::Cancelled)
    ));
}
