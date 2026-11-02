use lattice_sdk::{ClientError, endpoints::*};
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;
#[derive(Clone)]
struct Reader {
    calls: Arc<Mutex<Vec<String>>>,
    root_status: Option<u16>,
}
impl EndpointReader for Reader {
    async fn read(
        &self,
        base: &str,
        path: &[String],
        query: EndpointQuery,
        _: &CancellationToken,
    ) -> Result<Value, ClientError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("{base}/{}", path.join("/")));
        Ok(match query {
            EndpointQuery::Genesis => {
                json!({"genesisHash":if base.contains("wrong") {"wrong"} else {"network"}})
            }
            EndpointQuery::Info => json!({"chain":path,"height":"1","tipCID":"arbitrary"}),
            EndpointQuery::Declared => {
                if let Some(status) = self.root_status {
                    return Err(ClientError::Node {
                        status,
                        refusal: None,
                    });
                }
                json!({"chainPath":path,"committedBlock":"irrelevant-carrier","endpoints":["https://127.0.0.1","https://wrong.example","https://first.example/","https://second.example","https://first.example"],"submitEndpoints":["https://first.example"]})
            }
        })
    }
}
fn reader() -> Reader {
    Reader {
        calls: Arc::new(Mutex::new(Vec::new())),
        root_status: None,
    }
}
#[tokio::test]
async fn resolution_keeps_declared_order_checks_network_and_marks_trust() {
    let reader = reader();
    let calls = reader.calls.clone();
    let resolver = EndpointResolver::new(
        "http://127.0.0.1:8000",
        vec!["Nexus".into()],
        reader,
        Default::default(),
    )
    .unwrap();
    let path = vec!["Nexus".into(), "Alpha".into(), "Beta".into()];
    let found = resolver
        .resolve(&path, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        found.iter().map(|e| e.url.as_str()).collect::<Vec<_>>(),
        ["https://first.example", "https://second.example"]
    );
    assert!(found[0].declares_submit);
    assert!(!found[1].declares_submit);
    assert_eq!(found[0].chain_path, path);
    assert_eq!(found[0].trust, OPERATOR_DECLARED);
    assert!(
        !calls
            .lock()
            .unwrap()
            .iter()
            .any(|s| s.starts_with("https://127.0.0.1"))
    );
}
#[tokio::test]
async fn request_budget_is_global_and_root_failures_are_distinct() {
    let r = reader();
    let calls = r.calls.clone();
    let resolver = EndpointResolver::new(
        "https://root.example",
        vec!["Nexus".into()],
        r,
        ResolverOptions {
            maximum_requests: 3,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        resolver
            .resolve(&["Nexus".into(), "Alpha".into()], &CancellationToken::new())
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(calls.lock().unwrap().len(), 3);
    for status in [404, 503] {
        let r = Reader {
            root_status: Some(status),
            ..reader()
        };
        let resolver = EndpointResolver::new(
            "https://root.example",
            vec!["Nexus".into()],
            r,
            Default::default(),
        )
        .unwrap();
        let result = resolver
            .resolve(&["Nexus".into(), "Alpha".into()], &CancellationToken::new())
            .await;
        assert_eq!(result.is_err(), status == 503);
    }
}
struct Stalled;
impl EndpointReader for Stalled {
    async fn read(
        &self,
        _: &str,
        _: &[String],
        _: EndpointQuery,
        _: &CancellationToken,
    ) -> Result<Value, ClientError> {
        std::future::pending().await
    }
}
#[tokio::test]
async fn injected_transport_cannot_ignore_deadline_or_cancellation() {
    let resolver = EndpointResolver::new(
        "https://root.example",
        vec!["Nexus".into()],
        Stalled,
        ResolverOptions {
            timeout: Duration::from_millis(5),
            ..Default::default()
        },
    )
    .unwrap();
    let path = ["Nexus".into(), "Alpha".into()];
    assert!(matches!(
        resolver.resolve(&path, &CancellationToken::new()).await,
        Err(ClientError::Timeout)
    ));
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        resolver.resolve(&path, &cancel).await,
        Err(ClientError::Cancelled)
    ));
}
#[test]
fn declared_urls_exclude_embedded_private_ips_and_special_ranges() {
    for url in [
        "https://[::ffff:127.0.0.1]",
        "https://[64:ff9b::a00:1]",
        "https://[2002:7f00:1::]",
        "https://[2001:db8::1]",
        "https://service.home.arpa",
        "https://internal",
        "http://8.8.8.8",
    ] {
        assert!(declared_endpoint_url(url).is_none(), "{url}");
    }
    for url in [
        "https://8.8.8.8",
        "https://[2001:4860:4860::8888]",
        "https://node.example/rpc",
    ] {
        assert!(declared_endpoint_url(url).is_some(), "{url}");
    }
}

#[test]
fn actual_typescript_declared_url_policy_matches() {
    #[derive(serde::Deserialize)]
    struct Case {
        input: String,
        result: Option<String>,
    }
    let cases: Vec<Case> = serde_json::from_str(include_str!("endpoints-typescript.json")).unwrap();
    assert_eq!(cases.len(), 310);
    for case in cases {
        assert_eq!(
            declared_endpoint_url(&case.input),
            case.result,
            "{}",
            case.input
        );
    }
}

struct DnsReader {
    inner: Reader,
    stall: bool,
}
impl EndpointReader for DnsReader {
    async fn lookup(
        &self,
        host: &str,
        _: &CancellationToken,
    ) -> Result<Option<Vec<std::net::IpAddr>>, ClientError> {
        if self.stall {
            return std::future::pending().await;
        }
        Ok(Some(match host {
            "first.example" => vec!["8.8.8.8".parse().unwrap()],
            "second.example" => vec!["8.8.8.8".parse().unwrap(), "10.0.0.1".parse().unwrap()],
            _ => vec![],
        }))
    }
    async fn read(
        &self,
        base: &str,
        path: &[String],
        query: EndpointQuery,
        cancel: &CancellationToken,
    ) -> Result<Value, ClientError> {
        self.inner.read(base, path, query, cancel).await
    }
}
#[tokio::test]
async fn dns_requires_every_answer_public_and_bounds_uncooperative_lookup() {
    for stall in [false, true] {
        let reader = DnsReader {
            inner: reader(),
            stall,
        };
        let calls = reader.inner.calls.clone();
        let resolver = EndpointResolver::new(
            "https://root.example",
            vec!["Nexus".into()],
            reader,
            ResolverOptions {
                timeout: Duration::from_millis(5),
                ..Default::default()
            },
        )
        .unwrap();
        let found = resolver
            .resolve(&["Nexus".into(), "Alpha".into()], &CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(found.len(), usize::from(!stall));
        assert!(
            !calls
                .lock()
                .unwrap()
                .iter()
                .any(|url| url.starts_with("https://second.example"))
        );
        if !stall {
            assert_eq!(found[0].url, "https://first.example");
        }
    }
}
