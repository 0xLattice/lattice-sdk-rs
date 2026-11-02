use lattice_sdk::{
    ClientError, ClientOptions,
    endpoint_walk::LONGEST_NAME_PAGE_BYTES,
    endpoints::{EndpointReader, HttpEndpointReader},
};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::oneshot,
};
use tokio_util::sync::CancellationToken;
async fn server() -> (String, oneshot::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (tx, rx) = oneshot::channel();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = vec![];
        let mut byte = [0u8; 1];
        while !request.ends_with(b"\r\n\r\n") {
            socket.read_exact(&mut byte).await.unwrap();
            request.push(byte[0]);
        }
        let _ = tx.send(String::from_utf8(request).unwrap());
        let body = serde_json::to_vec(
            &serde_json::json!({"chainPath":["Nexus"],"directories":["\"".repeat(40000)]}),
        )
        .unwrap();
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = socket.write_all(header.as_bytes()).await;
        let _ = socket.write_all(&body).await;
    });
    (format!("http://{address}"), rx)
}
#[tokio::test]
async fn directory_http_override_handles_long_names_without_forwarding_credentials() {
    let reader = HttpEndpointReader {
        check_dns: false,
        options: ClientOptions {
            timeout: Duration::from_secs(1),
            maximum_response_bytes: 64 * 1024,
            authorization: Some("Bearer operator-secret".into()),
        },
    };
    let (url, request) = server().await;
    let page = reader
        .directories(
            &url,
            &["Nexus".into()],
            None,
            1,
            LONGEST_NAME_PAGE_BYTES,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(page.directories[0].len(), 40000);
    let request = request.await.unwrap();
    assert!(request.contains("/api/chain/children?chainPath=Nexus&limit=1"));
    assert!(!request.to_lowercase().contains("authorization"));
    assert!(!request.contains("operator-secret"));
    let (url, _) = server().await;
    let result = reader
        .directories(
            &url,
            &["Nexus".into()],
            None,
            200,
            reader.maximum_response_bytes(),
            &CancellationToken::new(),
        )
        .await;
    assert!(
        matches!(result,Err(ClientError::Node{refusal:Some(refusal),..})if refusal=="response too large")
    );
}
