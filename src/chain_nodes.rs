//! Cached chain discovery and round-robin reads. Directory declarations are
//! operator claims, not consensus evidence. Discovery errors are shared intact.
use crate::endpoints::{EndpointResolver, HttpEndpointReader, ResolverOptions};
use crate::runtime::{BoxFuture, SystemTime, UNIX_EPOCH};
use crate::{ClientError, ClientOptions, NodeClient, normalize_node_url};
use futures_util::{
    FutureExt,
    future::{Shared, join_all},
};
use rand_core::{OsRng, RngCore};
#[cfg(target_arch = "wasm32")]
use std::rc::Rc as StateOwner;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc as StateOwner;
use std::{
    collections::BTreeMap,
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ChainNode {
    pub url: String,
    pub found: bool,
}
#[derive(Debug, Clone, thiserror::Error)]
pub enum ChainNodesError {
    #[error("No node serves {path}", path = .0.join("/"))]
    Offline(Vec<String>),
    #[error(transparent)]
    Client(Arc<ClientError>),
}
impl From<ClientError> for ChainNodesError {
    fn from(e: ClientError) -> Self {
        Self::Client(Arc::new(e))
    }
}
pub type DiscoveryFuture<'a, T> = BoxFuture<'a, Result<T, ClientError>>;
/// Transport seam: 404 is `Ok(false)` for serves; other failures stay errors.
/// `declared` resolves from the specified already-serving ancestor.
pub trait ChainNodeDiscovery: Send + Sync + 'static {
    fn serves<'a>(&'a self, url: &'a str, path: &'a [String]) -> DiscoveryFuture<'a, bool>;
    fn declared<'a>(
        &'a self,
        url: &'a str,
        ancestor: &'a [String],
        target: &'a [String],
    ) -> DiscoveryFuture<'a, Vec<String>>;
}
#[derive(Clone)]
pub struct HttpChainNodeDiscovery {
    pub timeout: Duration,
    pub resolver: ResolverOptions,
}
impl Default for HttpChainNodeDiscovery {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(15),
            resolver: ResolverOptions::default(),
        }
    }
}
impl ChainNodeDiscovery for HttpChainNodeDiscovery {
    fn serves<'a>(&'a self, url: &'a str, path: &'a [String]) -> DiscoveryFuture<'a, bool> {
        Box::pin(async move {
            let client = NodeClient::new(
                url,
                path.to_vec(),
                ClientOptions {
                    timeout: self.timeout,
                    ..Default::default()
                },
            )?;
            match client.chain_info(&CancellationToken::new()).await {
                Ok(info) => Ok(info.chain.join("/") == path.join("/")),
                Err(ClientError::Node { status: 404, .. }) => Ok(false),
                Err(e) => Err(e),
            }
        })
    }
    fn declared<'a>(
        &'a self,
        url: &'a str,
        ancestor: &'a [String],
        target: &'a [String],
    ) -> DiscoveryFuture<'a, Vec<String>> {
        Box::pin(async move {
            Ok(EndpointResolver::new(
                url,
                ancestor.to_vec(),
                HttpEndpointReader::default(),
                self.resolver.clone(),
            )?
            .resolve(target, &CancellationToken::new())
            .await?
            .into_iter()
            .map(|e| e.url)
            .collect())
        })
    }
}
#[derive(Clone)]
pub struct ChainNodesOptions {
    pub ttl: Duration,
    /// Milliseconds, matching the reference Date.now clock. Injectable for tests.
    pub now: Arc<dyn Fn() -> u64 + Send + Sync>,
    /// Must return a finite number in [0,1), like Math.random.
    pub random: Arc<dyn Fn() -> f64 + Send + Sync>,
}
impl Default for ChainNodesOptions {
    fn default() -> Self {
        Self {
            ttl: Duration::from_secs(30),
            now: Arc::new(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis()
                    .min(u64::MAX as u128) as u64
            }),
            random: Arc::new(|| f64::from(OsRng.next_u32()) / 4294967296.0),
        }
    }
}
type ListResult = Result<Arc<Vec<ChainNode>>, ChainNodesError>;
type SharedList = Shared<BoxFuture<'static, ListResult>>;
struct Entry {
    id: u64,
    until: Option<u64>,
    list: SharedList,
}
#[derive(Default)]
struct State {
    lists: BTreeMap<Vec<String>, Entry>,
    next_id: u64,
}
type Turns = Arc<Mutex<BTreeMap<Vec<String>, (u64, u64)>>>;
#[derive(Clone)]
pub struct ChainNodes {
    starts: Arc<Vec<String>>,
    discovery: Arc<dyn ChainNodeDiscovery>,
    options: ChainNodesOptions,
    state: StateOwner<Mutex<State>>,
    turns: Turns,
}
impl ChainNodes {
    pub fn new(
        starts: impl IntoIterator<Item = String>,
        discovery: Arc<dyn ChainNodeDiscovery>,
        options: ChainNodesOptions,
    ) -> Result<Self, ClientError> {
        let mut urls = vec![];
        for s in starts {
            let url = normalize_node_url(&s)?.to_string();
            let url = url.strip_suffix('/').unwrap_or(&url).to_string();
            if !urls.contains(&url) {
                urls.push(url);
            }
        }
        if urls.is_empty() {
            return Err(ClientError::Configuration(
                "ChainNodes needs at least one start node",
            ));
        }
        Ok(Self {
            starts: Arc::new(urls),
            discovery,
            options,
            state: StateOwner::new(Mutex::new(State::default())),
            turns: Arc::new(Mutex::new(BTreeMap::new())),
        })
    }
    pub fn start_urls(&self) -> &[String] {
        &self.starts
    }
    fn turn(&self, path: &[String], walk: bool) -> u64 {
        let mut turns = self.turns.lock().unwrap();
        let turns = turns.entry(path.to_vec()).or_insert_with(|| {
            let first = ((self.options.random)() * 4294967296.0).floor() as u64;
            (first, first)
        });
        let next = if walk { &mut turns.1 } else { &mut turns.0 };
        let turn = *next;
        *next = next.wrapping_add(1);
        turn
    }
    /// Concurrent callers share one discovery. Dropping a waiter does not evict
    /// the in-flight list, which continues on the platform executor. Errors are never cached.
    pub async fn list(&self, path: &[String]) -> ListResult {
        let future = {
            let mut state = self.state.lock().unwrap();
            if let Some(entry) = state
                .lists
                .get(path)
                .filter(|e| e.until.is_none_or(|until| (self.options.now)() < until))
            {
                entry.list.clone()
            } else {
                let id = state.next_id;
                state.next_id = state.next_id.wrapping_add(1);
                // Keep no strong reference back from cached future to State.
                let weak = StateOwner::downgrade(&self.state);
                let starts = self.starts.clone();
                let discovery = self.discovery.clone();
                let turns = self.turns.clone();
                let options = self.options.clone();
                let path = path.to_vec();
                let key = path.clone();
                let task = crate::runtime::spawn(async move {
                    let service = ChainNodes {
                        starts,
                        discovery,
                        options: options.clone(),
                        state: StateOwner::new(Mutex::new(State::default())),
                        turns,
                    };
                    let result = service.discover(&path).await.map(Arc::new);
                    if let Some(state) = weak.upgrade() {
                        let mut state = state.lock().unwrap();
                        if state.lists.get(&path).is_some_and(|e| e.id == id) {
                            match &result {
                                Ok(_) => {
                                    state.lists.get_mut(&path).unwrap().until =
                                        Some((options.now)().saturating_add(
                                            options.ttl.as_millis().min(u64::MAX as u128) as u64,
                                        ))
                                }
                                Err(_) => {
                                    state.lists.remove(&path);
                                }
                            }
                        }
                    }
                    result
                });
                let future = task.shared();
                state.lists.insert(
                    key,
                    Entry {
                        id,
                        until: None,
                        list: future.clone(),
                    },
                );
                future
            }
        };
        future.await
    }
    /// Start at the next per-chain turn, then try each node once. Return the
    /// callback's last failure unchanged. No discovery credentials are forwarded.
    pub async fn read<T, F, Fut>(&self, path: &[String], mut read: F) -> Result<T, ChainNodesError>
    where
        F: FnMut(ChainNode) -> Fut,
        Fut: Future<Output = Result<T, ChainNodesError>>,
    {
        let list = self.list(path).await?;
        if list.is_empty() {
            return Err(ChainNodesError::Offline(path.to_vec()));
        }
        let turn = self.turn(path, false);
        let mut failure = None;
        for step in 0..list.len() {
            match read(list[((turn % list.len() as u64) as usize + step) % list.len()].clone())
                .await
            {
                Ok(v) => return Ok(v),
                Err(e) => failure = Some(e),
            }
        }
        Err(failure.expect("nonempty list tried"))
    }
    async fn discover(&self, path: &[String]) -> Result<Vec<ChainNode>, ChainNodesError> {
        let checks = join_all(self.starts.iter().map(|s| self.discovery.serves(s, path))).await;
        let mut first_failure = None;
        let mut own = vec![];
        let mut answered = vec![];
        for (url, check) in self.starts.iter().zip(checks) {
            match check {
                Ok(serves) => {
                    answered.push(url.clone());
                    if serves {
                        own.push(ChainNode {
                            url: url.clone(),
                            found: false,
                        });
                    }
                }
                Err(e) => {
                    if first_failure.is_none() {
                        first_failure = Some(ChainNodesError::from(e));
                    }
                }
            }
        }
        if answered.is_empty() {
            return Err(first_failure.expect("nonempty start list"));
        }
        match self.declared(path, &answered).await {
            Ok(found) => {
                let own_urls: Vec<_> = own.iter().map(|n| n.url.clone()).collect();
                own.extend(found.into_iter().filter(|n| !own_urls.contains(&n.url)));
            }
            Err(e) if own.is_empty() => return Err(e),
            Err(_) => {}
        }
        if own.is_empty()
            && let Some(e) = first_failure
        {
            return Err(e);
        }
        Ok(own)
    }
    async fn declared(
        &self,
        path: &[String],
        starts: &[String],
    ) -> Result<Vec<ChainNode>, ChainNodesError> {
        let turn = self.turn(path, true);
        let mut failure = None;
        for step in 0..starts.len() {
            let start = &starts[((turn % starts.len() as u64) as usize + step) % starts.len()];
            let found = async {
                let mut at = &path[..path.len().saturating_sub(1)];
                while !at.is_empty() && !self.discovery.serves(start, at).await? {
                    at = &at[..at.len() - 1];
                }
                if at.is_empty() {
                    return Ok(None);
                }
                self.discovery.declared(start, at, path).await.map(Some)
            }
            .await;
            match found {
                Ok(Some(hosts)) if !hosts.is_empty() || failure.is_none() => {
                    return Ok(hosts
                        .into_iter()
                        .map(|url| ChainNode { url, found: true })
                        .collect());
                }
                Ok(_) => {}
                Err(e) => {
                    if failure.is_none() {
                        failure = Some(ChainNodesError::from(e));
                    }
                }
            }
        }
        match failure {
            Some(e) => Err(e),
            None => Ok(vec![]),
        }
    }
}
