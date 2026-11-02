//! Lazy breadth-first traversal of operator-declared endpoints. A walk has no
//! global request budget. Each request still has a deadline and cancellation.
use crate::{
    ChildDirectoriesPage, ClientError,
    endpoints::{EndpointQuery, EndpointReader, EndpointResolver, RequestBudget, ResolvedEndpoint},
};
use std::collections::VecDeque;
use tokio_util::sync::CancellationToken;
pub const DIRECTORY_PAGE: u64 = 200;
pub const LONGEST_NAME_PAGE_BYTES: usize = 2 * 65535 + 4096;
struct Level {
    path: Vec<String>,
    sources: Vec<String>,
    source_index: usize,
    after: Option<String>,
    limit: u64,
    page: VecDeque<String>,
    page_read: bool,
    more: bool,
}
impl Level {
    fn new(path: Vec<String>, sources: Vec<String>) -> Self {
        Self {
            path,
            sources,
            source_index: 0,
            after: None,
            limit: DIRECTORY_PAGE,
            page: VecDeque::new(),
            page_read: false,
            more: false,
        }
    }
}
/// Call `next().await` to request only the work needed for the next result.
/// Dropping this value starts no background requests. Dropping a pending next
/// future can retry its in-flight level on the next call; no directory is lost.
pub struct EndpointWalk<'a, R> {
    resolver: &'a EndpointResolver<R>,
    cancel: &'a CancellationToken,
    network: Option<String>,
    frontier: VecDeque<Level>,
    next_frontier: VecDeque<Level>,
    current: Option<Level>,
    pending: VecDeque<ResolvedEndpoint>,
    resolving: Option<(Vec<String>, Vec<String>)>,
    finished: bool,
}
impl<R: EndpointReader> EndpointResolver<R> {
    pub fn walk<'a>(&'a self, cancel: &'a CancellationToken) -> EndpointWalk<'a, R> {
        EndpointWalk {
            resolver: self,
            cancel,
            network: None,
            frontier: VecDeque::new(),
            next_frontier: VecDeque::new(),
            current: None,
            pending: VecDeque::new(),
            resolving: None,
            finished: false,
        }
    }
}
impl<R: EndpointReader> EndpointWalk<'_, R> {
    pub async fn next(&mut self) -> Result<Option<ResolvedEndpoint>, ClientError> {
        if self.finished {
            return Ok(None);
        }
        let result = self.advance().await;
        if result.is_err() {
            self.finished = true;
        }
        result
    }
    async fn advance(&mut self) -> Result<Option<ResolvedEndpoint>, ClientError> {
        // TypeScript yield* drains an already resolved level before checking abort again.
        if let Some(endpoint) = self.pending.pop_front() {
            return Ok(Some(endpoint));
        }
        if self.cancel.is_cancelled() {
            return Err(ClientError::Cancelled);
        }
        if self.network.is_none() {
            let genesis = self
                .resolver
                .request(
                    &self.resolver.root_url,
                    &self.resolver.root_path,
                    EndpointQuery::Genesis,
                    &mut RequestBudget::Unlimited,
                    self.cancel,
                )
                .await?;
            self.network = Some(
                genesis
                    .get("genesisHash")
                    .and_then(|v| v.as_str())
                    .ok_or(ClientError::InvalidResponse("invalid genesis response"))?
                    .into(),
            );
            self.frontier.push_back(Level::new(
                self.resolver.root_path.clone(),
                vec![self.resolver.root_url.clone()],
            ));
        }
        loop {
            if let Some((path, sources)) = &self.resolving {
                let resolved = self
                    .resolver
                    .level(
                        sources,
                        path,
                        self.network.as_deref().unwrap(),
                        &mut RequestBudget::Unlimited,
                        self.cancel,
                    )
                    .await?;
                if !resolved.is_empty() {
                    self.next_frontier.push_back(Level::new(
                        path.clone(),
                        resolved.iter().map(|e| e.url.clone()).collect(),
                    ));
                }
                self.resolving = None;
                self.pending = resolved.into();
                if let Some(endpoint) = self.pending.pop_front() {
                    return Ok(Some(endpoint));
                }
            }
            if self.current.is_none() {
                if let Some(level) = self.frontier.pop_front() {
                    self.current = Some(level);
                } else if self.next_frontier.is_empty() {
                    self.finished = true;
                    return Ok(None);
                } else {
                    std::mem::swap(&mut self.frontier, &mut self.next_frontier);
                    continue;
                }
            }
            let level = self.current.as_mut().unwrap();
            if let Some(directory) = next_directory(self.resolver, level, self.cancel).await? {
                let mut path = level.path.clone();
                path.push(directory);
                self.resolving = Some((path, level.sources.clone()));
            } else {
                self.current = None;
            }
        }
    }
}
async fn next_directory<R: EndpointReader>(
    resolver: &EndpointResolver<R>,
    level: &mut Level,
    cancel: &CancellationToken,
) -> Result<Option<String>, ClientError> {
    loop {
        while let Some(directory) = level.page.pop_front() {
            level.after = Some(directory.clone());
            if !directory.is_empty() && !directory.contains('/') {
                return Ok(Some(directory));
            }
        }
        if level.page_read && !level.more {
            return Ok(None);
        }
        let Some(source) = level
            .sources
            .get(level.source_index)
            .filter(|_| level.source_index < resolver.options.maximum_sources)
        else {
            return Ok(None);
        };
        let cap = if level.limit == 1 {
            resolver
                .reader
                .maximum_response_bytes()
                .max(LONGEST_NAME_PAGE_BYTES)
        } else {
            resolver.reader.maximum_response_bytes()
        };
        let result: Result<ChildDirectoriesPage, ClientError> = tokio::select! {biased;
         _=cancel.cancelled()=>Err(ClientError::Cancelled),
         result=crate::runtime::timeout(resolver.options.timeout,resolver.reader.directories(source,&level.path,level.after.as_deref(),level.limit,cap,cancel))=>result.unwrap_or(Err(ClientError::Timeout)),
        };
        match result {
            Ok(page) => {
                level.limit = DIRECTORY_PAGE;
                level.page = page.directories.into();
                level.more = page.next.is_some();
                level.page_read = true;
            }
            Err(error) => {
                if cancel.is_cancelled() {
                    return Err(ClientError::Cancelled);
                }
                if level.limit > 1
                    && matches!(&error,ClientError::Node{refusal:Some(refusal),..}if refusal=="response too large")
                {
                    level.limit = (level.limit / 2).max(1);
                    continue;
                }
                if level.path == resolver.root_path {
                    resolver.root_failure(source, error)?;
                }
                level.source_index += 1;
                level.limit = DIRECTORY_PAGE;
                level.page_read = false;
                level.more = false;
            }
        }
    }
}
