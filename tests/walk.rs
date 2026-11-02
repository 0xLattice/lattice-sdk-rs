use lattice_sdk::{
    ChildDirectoriesPage, ClientError, endpoint_walk::LONGEST_NAME_PAGE_BYTES, endpoints::*,
};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio_util::sync::CancellationToken;
fn short(s: &str) -> String {
    if s.len() > 64 {
        "<long-name>".into()
    } else {
        s.into()
    }
}
#[derive(Clone)]
struct Reader {
    mode: String,
    requests: Arc<AtomicUsize>,
    directories: Arc<Mutex<Vec<Value>>>,
    caps: Arc<Mutex<Vec<(u64, usize)>>>,
}
impl Reader {
    fn new(mode: &str) -> Self {
        Self {
            mode: mode.into(),
            requests: Arc::new(AtomicUsize::new(0)),
            directories: Arc::new(Mutex::new(vec![])),
            caps: Arc::new(Mutex::new(vec![])),
        }
    }
}
impl EndpointReader for Reader {
    async fn read(
        &self,
        _base: &str,
        path: &[String],
        query: EndpointQuery,
        _cancel: &CancellationToken,
    ) -> Result<Value, ClientError> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        let answer = match query {
            EndpointQuery::Genesis => json!({"genesisHash":"network"}),
            EndpointQuery::Info => json!({"chain":path,"height":"1","tipCID":"tip"}),
            EndpointQuery::Declared => {
                let urls = if path.len() == 2 && path[1] == "A" {
                    vec![
                        "https://a1.example".to_owned(),
                        "https://a2.example".to_owned(),
                    ]
                } else {
                    let name = path.last().unwrap();
                    vec![format!(
                        "https://{}.example",
                        if name.len() > 64 {
                            "long".to_owned()
                        } else {
                            name.to_lowercase()
                        }
                    )]
                };
                json!({"chainPath":path,"endpoints":urls,"submitEndpoints":urls[..1]})
            }
        };
        if serde_json::to_vec(&answer).unwrap().len() > self.maximum_response_bytes() {
            return Err(ClientError::Node {
                status: 200,
                refusal: Some("response too large".into()),
            });
        }
        Ok(answer)
    }
    async fn directories(
        &self,
        base: &str,
        path: &[String],
        after: Option<&str>,
        limit: u64,
        cap: usize,
        _cancel: &CancellationToken,
    ) -> Result<ChildDirectoriesPage, ClientError> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        self.directories.lock().unwrap().push(json!({"source":base,"path":path.iter().map(|p|short(p)).collect::<Vec<_>>(),"after":after.map(short),"limit":limit}));
        self.caps.lock().unwrap().push((limit, cap));
        let page = |names: Vec<String>, next: Option<&str>| {
            Ok(ChildDirectoriesPage {
                directories: names,
                next: next.map(str::to_owned),
            })
        };
        if path.len() == 1 {
            match self.mode.as_str() {
                "halving_reset" => {
                    if limit > 100 {
                        return Err(ClientError::Node {
                            status: 200,
                            refusal: Some("response too large".into()),
                        });
                    }
                    return if after.is_none() {
                        page(vec!["A".into()], Some("A"))
                    } else {
                        page(vec!["B".into()], None)
                    };
                }
                "root_timeout" => {
                    return std::future::pending().await;
                }
                "root404" | "root503" => {
                    return Err(ClientError::Node {
                        status: if self.mode == "root404" { 404 } else { 503 },
                        refusal: Some("failure".into()),
                    });
                }
                "oversize" => {
                    if limit > 1 {
                        return Err(ClientError::Node {
                            status: 200,
                            refusal: Some("response too large".into()),
                        });
                    }
                    return page(vec!["\"".repeat(40000)], None);
                }
                "invalid_names" => {
                    return page(vec!["".into(), "A/B".into(), "Valid".into()], None);
                }
                _ => {
                    return if after.is_none() {
                        page(vec!["A".into(), "B".into()], Some("B"))
                    } else {
                        page(vec!["C".into()], None)
                    };
                }
            }
        }
        if ["bfs_failover", "timeout_failover"].contains(&self.mode.as_str())
            && path == ["Nexus", "A"]
        {
            if base == "https://a1.example" {
                return if after.is_none() {
                    page(vec!["AA".into()], Some("AA"))
                } else if self.mode == "timeout_failover" {
                    std::future::pending().await
                } else {
                    Err(ClientError::Node {
                        status: 503,
                        refusal: Some("down".into()),
                    })
                };
            }
            return page(vec!["AB".into()], None);
        }
        if ["bfs_failover", "timeout_failover"].contains(&self.mode.as_str())
            && path == ["Nexus", "B"]
        {
            return page(vec!["BA".into()], None);
        }
        page(vec![], None)
    }
}
#[tokio::test]
async fn actual_typescript_bfs_pagination_failover_and_lazy_stop() {
    let cases: Vec<Value> = serde_json::from_str(include_str!("walk-typescript.json")).unwrap();
    assert_eq!(cases.len(), 9);
    for case in cases {
        let mode = case["mode"].as_str().unwrap();
        let reader = Reader::new(mode);
        let observed = reader.clone();
        let resolver = EndpointResolver::new(
            "https://root.example",
            vec!["Nexus".into()],
            reader,
            ResolverOptions {
                maximum_requests: 1,
                timeout: Duration::from_millis(10),
                ..Default::default()
            },
        )
        .unwrap();
        let cancel = CancellationToken::new();
        let mut walk = resolver.walk(&cancel);
        assert_eq!(
            observed.requests.load(Ordering::SeqCst),
            0,
            "walk must be lazy"
        );
        let mut yielded = vec![];
        let mut error = Value::Null;
        loop {
            match walk.next().await {
                Ok(Some(e)) => {
                    yielded.push(json!({"url":e.url,"path":e.chain_path.iter().map(|p|short(p)).collect::<Vec<_>>(),"submit":e.declares_submit}));
                    if mode == "stop_early" {
                        break;
                    }
                }
                Ok(None) => break,
                Err(ClientError::Node { status, .. }) => {
                    error = json!(status);
                    break;
                }
                Err(ClientError::Timeout) => {
                    error = json!("TimeoutError");
                    break;
                }
                Err(e) => panic!("unexpected {mode}: {e}"),
            }
        }
        let actual = json!({"mode":mode,"yielded":yielded,"directories":*observed.directories.lock().unwrap(),"requests":observed.requests.load(Ordering::SeqCst),"error":error});
        assert_eq!(actual, case, "{mode}");
        if mode == "oversize" {
            assert!(
                observed
                    .caps
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|&(limit, cap)| limit == 1 && cap == LONGEST_NAME_PAGE_BYTES)
            );
        }
        drop(walk);
        let count = observed.requests.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(1)).await;
        assert_eq!(observed.requests.load(Ordering::SeqCst), count);
    }
}
struct Delayed {
    once: AtomicUsize,
}
impl EndpointReader for Delayed {
    async fn read(
        &self,
        _: &str,
        path: &[String],
        query: EndpointQuery,
        _: &CancellationToken,
    ) -> Result<Value, ClientError> {
        if matches!(query, EndpointQuery::Info) && self.once.fetch_add(1, Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        Ok(match query {
            EndpointQuery::Genesis => json!({"genesisHash":"n"}),
            EndpointQuery::Info => json!({"chain":path,"height":"1"}),
            EndpointQuery::Declared => json!({"chainPath":path,"endpoints":["https://a.example"]}),
        })
    }
    async fn directories(
        &self,
        _: &str,
        path: &[String],
        _: Option<&str>,
        _: u64,
        _: usize,
        _: &CancellationToken,
    ) -> Result<ChildDirectoriesPage, ClientError> {
        Ok(ChildDirectoriesPage {
            directories: if path.len() == 1 {
                vec!["A".into()]
            } else {
                vec![]
            },
            next: None,
        })
    }
}
#[tokio::test]
async fn dropped_next_future_does_not_lose_inflight_directory() {
    let resolver = EndpointResolver::new(
        "https://root.example",
        vec!["Nexus".into()],
        Delayed {
            once: AtomicUsize::new(0),
        },
        Default::default(),
    )
    .unwrap();
    let cancel = CancellationToken::new();
    let mut walk = resolver.walk(&cancel);
    assert!(
        tokio::time::timeout(Duration::from_millis(10), walk.next())
            .await
            .is_err()
    );
    let e = walk.next().await.unwrap().unwrap();
    assert_eq!(e.chain_path, ["Nexus", "A"]);
    assert!(walk.next().await.unwrap().is_none());
}
#[tokio::test]
async fn cancellation_closes_walk_and_starts_no_requests() {
    let reader = Reader::new("bfs_failover");
    let count = reader.requests.clone();
    let resolver = EndpointResolver::new(
        "https://root.example",
        vec!["Nexus".into()],
        reader,
        Default::default(),
    )
    .unwrap();
    let cancel = CancellationToken::new();
    let mut walk = resolver.walk(&cancel);
    cancel.cancel();
    assert!(matches!(walk.next().await, Err(ClientError::Cancelled)));
    assert!(walk.next().await.unwrap().is_none());
    assert_eq!(count.load(Ordering::SeqCst), 0);
}
