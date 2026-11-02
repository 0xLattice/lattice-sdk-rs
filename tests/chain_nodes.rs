use futures_util::FutureExt;
use lattice_sdk::{ClientError, chain_nodes::*};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};
#[derive(Clone)]
struct Fixture {
    case: Value,
    calls: Arc<Mutex<Vec<Value>>>,
}
fn err(status: u16) -> ClientError {
    ClientError::Node {
        status,
        refusal: Some("fixture".into()),
    }
}
impl ChainNodeDiscovery for Fixture {
    fn serves<'a>(&'a self, url: &'a str, path: &'a [String]) -> DiscoveryFuture<'a, bool> {
        async move {
            self.calls
                .lock()
                .unwrap()
                .push(json!({"kind":"serves","url":url,"path":path}));
            let v = &self.case["serves"][format!("{url}/{}", path.join("/"))];
            if let Some(status) = v.as_u64() {
                Err(err(status as u16))
            } else {
                Ok(v == true)
            }
        }
        .boxed()
    }
    fn declared<'a>(
        &'a self,
        url: &'a str,
        ancestor: &'a [String],
        path: &'a [String],
    ) -> DiscoveryFuture<'a, Vec<String>> {
        async move {
            self.calls
                .lock()
                .unwrap()
                .push(json!({"kind":"declared","url":url,"ancestor":ancestor,"path":path}));
            let v = &self.case["declared"][url];
            if let Some(status) = v.as_u64() {
                Err(err(status as u16))
            } else {
                Ok(v.as_array()
                    .map(|a| a.iter().map(|s| s.as_str().unwrap().into()).collect())
                    .unwrap_or_default())
            }
        }
        .boxed()
    }
}
fn error_value(e: ChainNodesError) -> Value {
    match e {
        ChainNodesError::Offline(_) => json!("ChainOfflineError"),
        ChainNodesError::Client(e) => match &*e {
            ClientError::Node { status, .. } => json!(status),
            other => panic!("unexpected {other}"),
        },
    }
}
#[tokio::test]
async fn actual_typescript_scenarios() {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("chain-nodes-typescript.json")).unwrap();
    assert_eq!(cases.len(), 13);
    for case in cases {
        if case["name"] == "own_and_found_dedup" {
            assert_eq!(
                case["expected"]["results"][0]["list"]
                    .as_array()
                    .unwrap()
                    .len(),
                3
            );
            assert_eq!(
                case["expected"]["results"][0]["chosen"]["url"],
                "https://a.example"
            );
        }
        if case["name"] == "all_offline" {
            assert_eq!(case["expected"]["results"][0]["error"], "ChainOfflineError");
        }
        let calls = Arc::new(Mutex::new(vec![]));
        let discovery = Fixture {
            case: case.clone(),
            calls: calls.clone(),
        };
        let now = Arc::new(AtomicU64::new(0));
        let clock = now.clone();
        let random = case["random"].as_f64().unwrap_or(0.0);
        let nodes = ChainNodes::new(
            [
                "https://a.example/",
                "https://b.example",
                "https://a.example",
            ]
            .map(String::from),
            Arc::new(discovery),
            ChainNodesOptions {
                ttl: Duration::from_millis(50),
                now: Arc::new(move || clock.load(Ordering::SeqCst)),
                random: Arc::new(move || random),
            },
        )
        .unwrap();
        let path: Vec<String> =
            serde_json::from_value(case.get("path").cloned().unwrap_or(json!(["Nexus", "A"])))
                .unwrap();
        let mut results = vec![];
        let read_calls = Arc::new(Mutex::new(vec![]));
        for _ in 0..case["reads"].as_u64().unwrap_or(1) {
            let result = async {
                let list = nodes.list(&path).await?;
                let chosen = nodes
                    .read(&path, |n| {
                        read_calls.lock().unwrap().push(n.url.clone());
                        let status = case["readErrors"][&n.url].as_u64();
                        async move {
                            if let Some(s) = status {
                                Err(ChainNodesError::from(err(s as u16)))
                            } else {
                                Ok(n)
                            }
                        }
                    })
                    .await?;
                Ok::<_, ChainNodesError>(json!({"list":&*list,"chosen":chosen}))
            }
            .await;
            results.push(match result {
                Ok(v) => v,
                Err(e) => json!({"error":error_value(e)}),
            });
            now.fetch_add(case["advance"].as_u64().unwrap_or(0), Ordering::SeqCst);
        }
        let actual = json!({"calls":*calls.lock().unwrap(),"readCalls":*read_calls.lock().unwrap(),"results":results});
        assert_eq!(actual, case["expected"], "{}", case["name"]);
    }
}
struct Slow {
    started: AtomicUsize,
    release: tokio::sync::Semaphore,
    now: Arc<AtomicU64>,
    fail: bool,
}
impl ChainNodeDiscovery for Slow {
    fn serves<'a>(&'a self, _url: &'a str, _path: &'a [String]) -> DiscoveryFuture<'a, bool> {
        async move {
            self.started.fetch_add(1, Ordering::SeqCst);
            let p = self.release.acquire().await.unwrap();
            p.forget();
            self.now.store(100, Ordering::SeqCst);
            if self.fail { Err(err(503)) } else { Ok(true) }
        }
        .boxed()
    }
    fn declared<'a>(
        &'a self,
        _url: &'a str,
        _ancestor: &'a [String],
        _target: &'a [String],
    ) -> DiscoveryFuture<'a, Vec<String>> {
        async { Ok(vec![]) }.boxed()
    }
}
#[tokio::test]
async fn concurrent_callers_share_discovery_and_completion_ttl() {
    let now = Arc::new(AtomicU64::new(0));
    let slow = Arc::new(Slow {
        started: AtomicUsize::new(0),
        release: tokio::sync::Semaphore::new(0),
        now: now.clone(),
        fail: false,
    });
    let clock = now.clone();
    let nodes = ChainNodes::new(
        ["https://a.example".into(), "https://b.example".into()],
        slow.clone(),
        ChainNodesOptions {
            ttl: Duration::from_millis(50),
            now: Arc::new(move || clock.load(Ordering::SeqCst)),
            random: Arc::new(|| 0.0),
        },
    )
    .unwrap();
    let path = vec!["Nexus".into()];
    let first = nodes.clone();
    let p = path.clone();
    let a = tokio::spawn(async move { first.list(&p).await.unwrap() });
    let second = nodes.clone();
    let p = path.clone();
    let b = tokio::spawn(async move { second.list(&p).await.unwrap() });
    while slow.started.load(Ordering::SeqCst) < 2 {
        tokio::task::yield_now().await;
    }
    assert_eq!(slow.started.load(Ordering::SeqCst), 2);
    slow.release.add_permits(2);
    let (a, b) = (a.await.unwrap(), b.await.unwrap());
    assert!(Arc::ptr_eq(&a, &b));
    now.store(149, Ordering::SeqCst);
    assert!(Arc::ptr_eq(&a, &nodes.list(&path).await.unwrap()));
    assert_eq!(slow.started.load(Ordering::SeqCst), 2);
    now.store(150, Ordering::SeqCst);
    slow.release.add_permits(2);
    let refreshed = nodes.list(&path).await.unwrap();
    assert!(!Arc::ptr_eq(&a, &refreshed));
    assert_eq!(slow.started.load(Ordering::SeqCst), 4);
}
#[tokio::test]
async fn shared_errors_preserve_identity_then_retry() {
    let now = Arc::new(AtomicU64::new(0));
    let slow = Arc::new(Slow {
        started: AtomicUsize::new(0),
        release: tokio::sync::Semaphore::new(0),
        now,
        fail: true,
    });
    let nodes = ChainNodes::new(
        ["https://a.example".into()],
        slow.clone(),
        ChainNodesOptions::default(),
    )
    .unwrap();
    let path = vec!["Nexus".into()];
    let a = nodes.clone();
    let p = path.clone();
    let a = tokio::spawn(async move { a.list(&p).await });
    let b = nodes.clone();
    let p = path.clone();
    let b = tokio::spawn(async move { b.list(&p).await });
    tokio::task::yield_now().await;
    assert_eq!(slow.started.load(Ordering::SeqCst), 1);
    slow.release.add_permits(1);
    let (ChainNodesError::Client(a), ChainNodesError::Client(b)) =
        (a.await.unwrap().unwrap_err(), b.await.unwrap().unwrap_err())
    else {
        panic!()
    };
    assert!(Arc::ptr_eq(&a, &b));
    slow.release.add_permits(1);
    assert!(nodes.list(&path).await.is_err());
    assert_eq!(slow.started.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn abandoned_waiter_does_not_cancel_shared_discovery() {
    let now = Arc::new(AtomicU64::new(0));
    let slow = Arc::new(Slow {
        started: AtomicUsize::new(0),
        release: tokio::sync::Semaphore::new(0),
        now: now.clone(),
        fail: false,
    });
    let clock = now.clone();
    let nodes = ChainNodes::new(
        ["https://a.example".into()],
        slow.clone(),
        ChainNodesOptions {
            now: Arc::new(move || clock.load(Ordering::SeqCst)),
            ..Default::default()
        },
    )
    .unwrap();
    let owner = nodes.clone();
    let task = tokio::spawn(async move { owner.list(&["Nexus".into()]).await });
    while slow.started.load(Ordering::SeqCst) == 0 {
        tokio::task::yield_now().await;
    }
    task.abort();
    slow.release.add_permits(1);
    while now.load(Ordering::SeqCst) != 100 {
        tokio::task::yield_now().await;
    }
    let list = nodes.list(&["Nexus".into()]).await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(slow.started.load(Ordering::SeqCst), 1);
}
