//! Minimal `futures::future` join replacements without `FuturesOrdered`/`FuturesUnordered` bloat.

use std::{
    future::{poll_fn, Future},
    pin::Pin,
    task::Poll,
};

#[cfg(feature = "thread-safe-futures")]
pub(crate) trait MaybeSend: Send {}
#[cfg(feature = "thread-safe-futures")]
impl<T: Send + ?Sized> MaybeSend for T {}

#[cfg(not(feature = "thread-safe-futures"))]
pub(crate) trait MaybeSend {}
#[cfg(not(feature = "thread-safe-futures"))]
impl<T: ?Sized> MaybeSend for T {}

#[cfg(feature = "thread-safe-futures")]
type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
#[cfg(not(feature = "thread-safe-futures"))]
type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// Polls two fallible futures concurrently and resolves when both succeed or either fails.
pub(crate) async fn try_join2<A, B, E>(
    fut_a: impl Future<Output = Result<A, E>>,
    fut_b: impl Future<Output = Result<B, E>>,
) -> Result<(A, B), E> {
    let mut fut_a = std::pin::pin!(fut_a);
    let mut fut_b = std::pin::pin!(fut_b);
    let mut out_a: Option<A> = None;
    let mut out_b: Option<B> = None;

    poll_fn(|cx| {
        if out_a.is_none() {
            if let Poll::Ready(res) = fut_a.as_mut().poll(cx) {
                match res {
                    Ok(val) => out_a = Some(val),
                    Err(err) => return Poll::Ready(Err(err)),
                }
            }
        }
        if out_b.is_none() {
            if let Poll::Ready(res) = fut_b.as_mut().poll(cx) {
                match res {
                    Ok(val) => out_b = Some(val),
                    Err(err) => return Poll::Ready(Err(err)),
                }
            }
        }
        if out_a.is_some() && out_b.is_some() {
            if let (Some(a), Some(b)) = (out_a.take(), out_b.take()) {
                return Poll::Ready(Ok((a, b)));
            }
        }
        Poll::Pending
    })
    .await
}

async fn poll_all_dyn<T, R, E>(
    mut pending: Vec<Option<BoxFut<'_, T>>>,
    mut step: impl FnMut(&mut Vec<Option<R>>, usize, T) -> Result<(), E>,
) -> Result<Vec<R>, E> {
    if pending.is_empty() {
        return Ok(Vec::new());
    }
    let mut results: Vec<Option<R>> = (0..pending.len()).map(|_| None).collect();
    let mut remaining = pending.len();

    poll_fn(|cx| {
        for (idx, fut_opt) in pending.iter_mut().enumerate() {
            if let Some(fut) = fut_opt.as_mut() {
                if let Poll::Ready(val) = fut.as_mut().poll(cx) {
                    *fut_opt = None;
                    if let Err(err) = step(&mut results, idx, val) {
                        return Poll::Ready(Err(err));
                    }
                    remaining -= 1;
                }
            }
        }
        if remaining == 0 {
            Poll::Ready(Ok(std::mem::take(&mut results)
                .into_iter()
                .flatten()
                .collect()))
        } else {
            Poll::Pending
        }
    })
    .await
}

/// Polls all fallible futures in `futs` concurrently, preserving input order without inlining
/// large future state machines into every caller's poll loop.
pub(crate) async fn try_join_all_concurrent<'a, T, E, F>(futs: Vec<F>) -> Result<Vec<T>, E>
where
    F: Future<Output = Result<T, E>> + MaybeSend + 'a,
{
    let pending: Vec<Option<BoxFut<'a, Result<T, E>>>> = futs
        .into_iter()
        .map(|f| Some(Box::pin(f) as BoxFut<'a, Result<T, E>>))
        .collect();
    poll_all_dyn(pending, |results, idx, res| {
        if let Some(slot) = results.get_mut(idx) {
            *slot = Some(res?);
        }
        Ok(())
    })
    .await
}

/// Polls all infallible futures concurrently, preserving input order without `FuturesOrdered`.
pub(crate) async fn join_all_concurrent<'a, T, F>(futs: impl IntoIterator<Item = F>) -> Vec<T>
where
    F: Future<Output = T> + MaybeSend + 'a,
{
    enum Never {}
    let pending: Vec<Option<BoxFut<'a, T>>> = futs
        .into_iter()
        .map(|f| Some(Box::pin(f) as BoxFut<'a, T>))
        .collect();
    match poll_all_dyn::<T, T, Never>(pending, |results, idx, val| {
        if let Some(slot) = results.get_mut(idx) {
            *slot = Some(val);
        }
        Ok(())
    })
    .await
    {
        Ok(values) => values,
        Err(never) => match never {},
    }
}
