use lattice_sdk::volumes::*;
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::oneshot,
};
use tokio_util::sync::CancellationToken;
async fn server(response: Vec<u8>) -> (String, oneshot::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (tx, rx) = oneshot::channel();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = vec![];
        let mut byte = [0u8; 1];
        while !request.ends_with(b"\r\n\r\n") {
            if socket.read_exact(&mut byte).await.is_err() {
                return;
            }
            request.push(byte[0]);
        }
        let _ = tx.send(String::from_utf8(request).unwrap());
        let _ = socket.write_all(&response).await;
    });
    (format!("http://{address}/volumes"), rx)
}
fn volume() -> (String, Vec<u8>) {
    let bytes = vec![0xa0];
    let cid = lattice_cashew::cid_for_bytes(0x71, &bytes);
    let archive = encode_volume_archive(&SerializedVolume {
        root_cid: cid.clone(),
        entries: vec![VolumeEntry {
            cid: cid.clone(),
            bytes,
        }],
    })
    .unwrap();
    (cid, archive)
}
fn response(status: &str, media: &str, body: &[u8]) -> Vec<u8> {
    [format!("HTTP/1.1 {status}\r\nContent-Type: {media}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len()).into_bytes(),body.to_vec()].concat()
}
#[test]
fn content_url_policy() {
    for url in [
        "http://remote.example/v",
        "ftp://localhost/v",
        "https://user:secret@example.com/v",
        "https://example.com/v?q=1",
        "https://example.com/v#anchor",
    ] {
        assert!(
            HttpVolumeTransport::new(url, HttpVolumeOptions::default()).is_err(),
            "{url}"
        );
    }
    for url in [
        "https://example.com/v",
        "http://localhost/v",
        "http://127.1/v",
        "http://[::1]/v",
    ] {
        assert!(
            HttpVolumeTransport::new(url, HttpVolumeOptions::default()).is_ok(),
            "{url}"
        );
    }
    assert!(
        HttpVolumeTransport::new(
            "https://example.com",
            HttpVolumeOptions {
                chain_path: Some(vec!["".into()]),
                ..Default::default()
            }
        )
        .is_err()
    );
}
#[tokio::test]
async fn archive_request_headers_path_and_verification() {
    let (root, archive) = volume();
    let (url, request) = server(response(
        "200 OK",
        "Application/Vnd.Lattice.Volume; charset=binary",
        &archive,
    ))
    .await;
    let transport = HttpVolumeTransport::new(
        &url,
        HttpVolumeOptions {
            authorization: Some("Bearer private".into()),
            chain_path: Some(vec!["Nexus".into(), "Child".into()]),
            ..Default::default()
        },
    )
    .unwrap();
    let volume = transport
        .get_volume(&root, &CancellationToken::new())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(volume.root_cid, root);
    let request = request.await.unwrap();
    assert!(request.contains(&format!(
        "GET /volumes/{root}?chainPath=Nexus%2FChild HTTP/1.1"
    )));
    let lower = request.to_ascii_lowercase();
    assert!(lower.contains("accept: application/vnd.lattice.volume"));
    assert!(lower.contains("authorization: bearer private"));
}
#[tokio::test]
async fn rejects_media_status_redirects_and_declared_oversize() {
    let (root, _) = volume();
    for (name,raw,expected) in [
  ("not_found",response("404 Missing","text/plain",b"not found"),404),
  ("media",response("200 OK","text/html",b"page"),200),
  ("failure",response("500 Error","text/plain",b"error"),500),
  ("redirect",b"HTTP/1.1 302 Found\r\nLocation: https://example.invalid/secret\r\nContent-Length: 0\r\n\r\n".to_vec(),302),
  ("oversize",format!("HTTP/1.1 200 OK\r\nContent-Type: {VOLUME_ARCHIVE_MEDIA_TYPE}\r\nContent-Length: {}\r\n\r\n",MAXIMUM_VOLUME_ARCHIVE_BYTES+1).into_bytes(),200)
 ]{let (url,_)=server(raw).await;let transport=HttpVolumeTransport::new(&url,HttpVolumeOptions::default()).unwrap();let result=transport.get_volume(&root,&CancellationToken::new()).await;if expected==404{assert!(result.unwrap().is_none());}else{assert!(matches!(result,Err(VolumeError::Http{status,..})if status==expected),"{name}: {result:?}");}}
}
async fn stalled() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buffer = [0u8; 4096];
        let _ = socket.read(&mut buffer).await;
        let _=socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: {VOLUME_ARCHIVE_MEDIA_TYPE}\r\nContent-Length: 100\r\n\r\n\0\x01").as_bytes()).await;
        tokio::time::sleep(Duration::from_secs(1)).await;
    });
    format!("http://{address}/")
}
#[tokio::test]
async fn cancellation_and_timeout_cover_response_body() {
    let (root, _) = volume();
    let url = stalled().await;
    let t = HttpVolumeTransport::new(
        &url,
        HttpVolumeOptions {
            timeout: Duration::from_millis(20),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(matches!(
        t.get_volume(&root, &CancellationToken::new()).await,
        Err(VolumeError::Timeout)
    ));
    let url = stalled().await;
    let t = HttpVolumeTransport::new(&url, Default::default()).unwrap();
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        trigger.cancel();
    });
    assert!(matches!(
        t.get_volume(&root, &cancel).await,
        Err(VolumeError::Cancelled)
    ));
    let t = HttpVolumeTransport::new("https://example.invalid", Default::default()).unwrap();
    assert!(matches!(
        t.get_volume(&root, &cancel).await,
        Err(VolumeError::Cancelled)
    ));
}
#[tokio::test]
async fn chunked_reads_stop_at_archive_byte_cap() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0u8; 4096];
        let _ = socket.read(&mut request).await;
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: {VOLUME_ARCHIVE_MEDIA_TYPE}\r\nTransfer-Encoding: chunked\r\n\r\n").as_bytes()).await.unwrap();
        let chunk = vec![0u8; 1024 * 1024];
        for _ in 0..65 {
            if socket.write_all(b"100000\r\n").await.is_err()
                || socket.write_all(&chunk).await.is_err()
                || socket.write_all(b"\r\n").await.is_err()
            {
                return;
            }
        }
        let _ = socket.write_all(b"0\r\n\r\n").await;
    });
    let t = HttpVolumeTransport::new(&format!("http://{address}"), Default::default()).unwrap();
    assert!(matches!(
        t.get_volume("unused", &CancellationToken::new()).await,
        Err(VolumeError::Http {
            message: "response is too large",
            ..
        })
    ));
    handle.await.unwrap();
}
