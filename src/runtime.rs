//! Native Tokio and browser event-loop primitives. Browser futures are local;
//! native futures retain their Send guarantees.
#[cfg(not(target_arch = "wasm32"))]
pub use std::time::{Instant, SystemTime, UNIX_EPOCH};
use std::{future::Future, time::Duration};
#[cfg(target_arch = "wasm32")]
pub use web_time::{Instant, SystemTime, UNIX_EPOCH};
#[cfg(not(target_arch = "wasm32"))]
pub type BoxFuture<'a, T> = futures_util::future::BoxFuture<'a, T>;
#[cfg(target_arch = "wasm32")]
pub type BoxFuture<'a, T> = futures_util::future::LocalBoxFuture<'a, T>;

#[cfg(not(target_arch = "wasm32"))]
pub async fn timeout<F: Future>(duration: Duration, future: F) -> Result<F::Output, ()> {
    tokio::time::timeout(duration, future).await.map_err(|_| ())
}
#[cfg(target_arch = "wasm32")]
pub async fn timeout<F: Future>(duration: Duration, future: F) -> Result<F::Output, ()> {
    use futures_util::{FutureExt, pin_mut};
    let future = future.fuse();
    let timer = sleep(duration).fuse();
    pin_mut!(future, timer);
    futures_util::select_biased! { value = future => Ok(value), _ = timer => Err(()) }
}
#[cfg(target_arch = "wasm32")]
pub async fn sleep(duration: Duration) {
    let mut remaining = duration.as_millis().saturating_add(u128::from(
        !duration.subsec_nanos().is_multiple_of(1_000_000),
    ));
    loop {
        // Browser setTimeout accepts signed 32-bit milliseconds.
        let step = remaining.min(i32::MAX as u128) as u32;
        gloo_timers::future::TimeoutFuture::new(step).await;
        remaining -= u128::from(step);
        if remaining == 0 {
            break;
        }
    }
}
#[cfg(not(target_arch = "wasm32"))]
pub async fn yield_now() {
    tokio::task::yield_now().await;
}
#[cfg(target_arch = "wasm32")]
pub async fn yield_now() {
    sleep(Duration::ZERO).await;
}

#[cfg(not(target_arch = "wasm32"))]
pub fn spawn<F: Future + Send + 'static>(future: F) -> BoxFuture<'static, F::Output>
where
    F::Output: Send,
{
    let task = tokio::spawn(future);
    Box::pin(async move { task.await.expect("background task panicked") })
}
#[cfg(target_arch = "wasm32")]
pub fn spawn<F: Future + 'static>(future: F) -> BoxFuture<'static, F::Output> {
    let (sender, receiver) = futures_channel::oneshot::channel();
    wasm_bindgen_futures::spawn_local(async move {
        let _ = sender.send(future.await);
    });
    Box::pin(async move { receiver.await.expect("background task terminated") })
}
