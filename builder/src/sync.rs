use std::{collections::BTreeMap, mem, ops::DerefMut, pin::Pin, sync::Arc};

use tokio::{
    spawn,
    sync::{RwLock, broadcast::Sender},
};

pub struct FutureOnce<T> {
    state: Arc<RwLock<State<T>>>,
}

pub struct WaitFutureOnce<T> {
    inner: Pin<Box<dyn Future<Output = Arc<T>> + Send + Sync>>,
}

impl<T> Future for WaitFutureOnce<T> {
    type Output = Arc<T>;

    fn poll(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        self.inner.as_mut().poll(cx)
    }
}

impl<T> Clone for FutureOnce<T> {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
        }
    }
}

enum State<T> {
    Uninit(Box<dyn Future<Output = T> + Send + Sync>),
    Running(Sender<()>),
    Done(Arc<T>),
}

impl<T> FutureOnce<T>
where
    T: Send + Sync + 'static,
{
    #[allow(unused)]
    pub fn new<F>(f: F) -> Self
    where
        F: Future<Output = T> + Send + Sync + 'static,
    {
        Self {
            state: RwLock::new(State::Uninit(Box::new(f))).into(),
        }
    }

    pub fn join(self) -> WaitFutureOnce<T> {
        WaitFutureOnce {
            inner: Box::pin(self.join_inner()),
        }
    }

    async fn join_inner(self) -> Arc<T> {
        loop {
            if let Some(result) = self.read_state_loop().await {
                return result;
            }

            let sender = Sender::new(1);

            let mut state_write = self.state.write().await;
            let future = match &*state_write {
                State::Uninit(_) => {
                    mem::replace(state_write.deref_mut(), State::Running(sender.clone()))
                }
                State::Running(_sender) => {
                    drop(state_write);
                    return self.read_state_loop().await.unwrap();
                }
                State::Done(result) => return result.clone(),
            };
            let State::Uninit(future) = future else {
                log::error!("huh?");
                unreachable!()
            };

            let state = self.state.clone();
            spawn(async move { Self::runner(state, future, sender).await });
        }
    }

    async fn read_state_loop(&self) -> Option<Arc<T>> {
        loop {
            let state_read = self.state.read().await;
            match &*state_read {
                State::Uninit(_) => {
                    break;
                }
                State::Running(send) => {
                    let mut subscribe = send.subscribe();
                    drop(state_read);
                    subscribe.recv().await.unwrap();
                }
                State::Done(result) => return Some(result.clone()),
            }
        }
        None
    }

    async fn runner(
        state: Arc<RwLock<State<T>>>,
        f: Box<dyn Future<Output = T> + Send + Sync>,
        done: Sender<()>,
    ) {
        let result = Box::into_pin(f).await;
        *state.write().await = State::Done(result.into());
        done.send(()).expect("Done message is only ever send once therefor a broadcast channel with capacity 1 should never fail to send");
    }
}

pub struct FutureCache<I, T> {
    cache: RwLock<BTreeMap<I, FutureOnce<T>>>,
}

impl<I, T> FutureCache<I, T>
where
    I: Ord,
    T: Send + Sync + 'static,
{
    #[allow(unused)]
    pub const fn const_new() -> Self {
        Self {
            cache: RwLock::const_new(BTreeMap::new()),
        }
    }

    #[allow(unused)]
    pub fn new() -> Self {
        Self {
            cache: RwLock::new(BTreeMap::new()),
        }
    }

    pub async fn spawn_or_get<F>(&self, id: I, f: F) -> FutureOnce<T>
    where
        F: Future<Output = T> + Send + Sync + 'static,
    {
        if let Some(out) = self.cache.read().await.get(&id) {
            return out.clone();
        }

        let mut cache = self.cache.write().await;

        if let Some(out) = cache.get(&id) {
            return out.clone();
        }

        let once = FutureOnce::new(f);
        cache.insert(id, once.clone());

        once
    }
}
