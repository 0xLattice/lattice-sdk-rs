use lattice_sdk::{ClientError, chain_nodes::*};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
async fn response(status: u16, body: &str) -> (String, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let body = body.to_string();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = vec![];
        let mut byte = [0];
        while !request.ends_with(b"\r\n\r\n") {
            socket.read_exact(&mut byte).await.unwrap();
            request.push(byte[0]);
        }
        let response = format!(
            "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(response.as_bytes()).await.unwrap();
        String::from_utf8(request).unwrap()
    });
    (url, task)
}
#[tokio::test]
async fn actual_http_serves_distinguishes_absent_mismatch_and_failure() {
    let http = HttpChainNodeDiscovery {
        timeout: Duration::from_secs(1),
        ..Default::default()
    };
    let path = vec!["Nexus".into(), "Alpha".into()];
    for (status, body, want) in [
        (200, r#"{"chain":["Nexus","Alpha"]}"#, Some(true)),
        (200, r#"{"chain":["Nexus","Other"]}"#, Some(false)),
        (404, r#"{"error":"absent"}"#, Some(false)),
        (503, r#"{"error":"unavailable"}"#, None),
    ] {
        let (url, task) = response(status, body).await;
        let actual = http.serves(&url, &path).await;
        match want {
            Some(value) => assert_eq!(actual.unwrap(), value),
            None => assert!(matches!(actual, Err(ClientError::Node { status: 503, .. }))),
        }
        let request = task.await.unwrap();
        assert!(request.starts_with("GET /api/chain/info?chainPath=Nexus%2FAlpha "));
        assert!(!request.to_lowercase().contains("authorization:"));
    }
}
#[tokio::test]
async fn actual_http_serves_timeout_is_failure_not_offline() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (_socket, _) = listener.accept().await.unwrap();
        std::future::pending::<()>().await;
    });
    let http = HttpChainNodeDiscovery {
        timeout: Duration::from_millis(20),
        ..Default::default()
    };
    assert!(matches!(
        http.serves(&url, &["Nexus".into()]).await,
        Err(ClientError::Timeout)
    ));
    server.abort();
}
