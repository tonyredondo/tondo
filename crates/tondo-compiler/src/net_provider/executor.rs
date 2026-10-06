//! Readiness context for the owning Tondo executor, not a second task scheduler.

use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    task::Poll,
};
use tondo_stdlib::net::{NetError, NetOptions, TlsError};

pub(crate) const MAX_PENDING_OPERATIONS: usize = 256;
type ProviderFuture<T, E> = Pin<Box<dyn Future<Output = Result<T, E>> + Send>>;

pub(crate) trait OperationError {
    fn from_net(error: NetError) -> Self;
}

impl OperationError for NetError {
    fn from_net(error: NetError) -> Self {
        error
    }
}

impl OperationError for TlsError {
    fn from_net(error: NetError) -> Self {
        match error {
            NetError::Timeout => Self::Timeout,
            NetError::Cancelled => Self::Cancelled,
            NetError::ResourceLimit => Self::ResourceLimit,
            error => Self::Transport(error),
        }
    }
}

struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// A current-thread I/O reactor entered only when the owner services a host
/// operation. It neither spawns an OS worker nor runs Tondo tasks.
pub(crate) struct NetworkExecutor {
    runtime: tokio::runtime::Runtime,
    domain: u64,
    slots: Arc<AtomicUsize>,
    limit: usize,
}

pub(crate) struct PendingOperation<T, E> {
    future: Option<ProviderFuture<T, E>>,
    options: NetOptions,
    owner: Arc<AtomicUsize>,
    slot: Option<Slot>,
    cancelled: bool,
    finished: bool,
}

impl NetworkExecutor {
    pub(crate) fn new(domain: u64, limit: usize) -> Result<Self, NetError> {
        if domain == 0 {
            return Err(NetError::InvalidDeadline);
        }
        if limit == 0 {
            return Err(NetError::InvalidLimit);
        }
        if limit > MAX_PENDING_OPERATIONS {
            return Err(NetError::ResourceLimit);
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_io()
            .enable_time()
            .build()
            .map_err(|_| NetError::Host)?;
        Ok(Self {
            runtime,
            domain,
            slots: Arc::new(AtomicUsize::new(0)),
            limit,
        })
    }

    /// Numeric bind/listen execute synchronously inside this readiness context.
    pub(crate) fn enter<T>(&self, function: impl FnOnce() -> T) -> T {
        let _context = self.runtime.enter();
        function()
    }

    /// Admit metadata and arguments before the future can perform any I/O.
    /// The supplied `now` comes from the declared monotonic provider; this
    /// helper never substitutes another clock or a global deadline.
    pub(crate) fn start<T, E: OperationError>(
        &self,
        options: NetOptions,
        now: i128,
        future: impl Future<Output = Result<T, E>> + Send + 'static,
    ) -> Result<PendingOperation<T, E>, E> {
        options
            .before_commit(self.domain, now, false)
            .map_err(E::from_net)?;
        self.slots
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < self.limit).then_some(count + 1)
            })
            .map_err(|_| E::from_net(NetError::ResourceLimit))?;
        Ok(PendingOperation {
            future: Some(Box::pin(future)),
            options,
            owner: self.slots.clone(),
            slot: Some(Slot(self.slots.clone())),
            cancelled: false,
            finished: false,
        })
    }

    pub(crate) fn pending_count(&self) -> usize {
        self.slots.load(Ordering::Acquire)
    }

    /// Service readiness without waiting for a host descriptor. Yield once so
    /// the reactor observes readiness, then return the operation's Poll to the
    /// Tondo owner. Tondo decides when and which operation to service again.
    pub(crate) fn poll<T, E: OperationError>(
        &self,
        operation: &mut PendingOperation<T, E>,
        now: i128,
    ) -> Poll<Result<T, E>> {
        if operation.finished {
            return Poll::Ready(Err(E::from_net(NetError::Closed)));
        }
        if !Arc::ptr_eq(&self.slots, &operation.owner) {
            operation.retire();
            return Poll::Ready(Err(E::from_net(NetError::Host)));
        }
        if let Err(error) = operation
            .options
            .before_commit(self.domain, now, operation.cancelled)
        {
            operation.retire();
            return Poll::Ready(Err(E::from_net(error)));
        }
        let result = self.runtime.block_on(async {
            tokio::task::yield_now().await;
            std::future::poll_fn(|context| {
                Poll::Ready(
                    operation
                        .future
                        .as_mut()
                        .map_or(Poll::Ready(Err(E::from_net(NetError::Closed))), |future| {
                            future.as_mut().poll(context)
                        }),
                )
            })
            .await
        });
        if result.is_ready() {
            operation.retire();
        }
        result
    }
}

impl<T, E> PendingOperation<T, E> {
    fn retire(&mut self) {
        // Resource-owning futures retire before their metadata reservation.
        self.future.take();
        self.slot.take();
        self.finished = true;
    }

    /// Preserve an already committed reply. A pending cancellation drops its
    /// provider futures and sockets immediately, before poll publishes it.
    pub(crate) fn cancel(&mut self) {
        if !self.finished {
            self.future.take();
            self.slot.take();
            self.cancelled = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tondo_stdlib::net::{Deadline, NetLimits};

    fn options(deadline: Option<Deadline>) -> NetOptions {
        NetOptions::create(deadline, NetLimits::default(), 7).unwrap()
    }

    struct Never(Arc<AtomicUsize>);
    impl Future for Never {
        type Output = Result<(), NetError>;
        fn poll(self: Pin<&mut Self>, _: &mut std::task::Context<'_>) -> Poll<Self::Output> {
            Poll::Pending
        }
    }
    impl Drop for Never {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn network_executor_admits_bounded_slots_before_effects_and_retires_on_drop() {
        let executor = NetworkExecutor::new(7, 1).unwrap();
        let dropped = Arc::new(AtomicUsize::new(0));
        let first = executor
            .start(options(None), 0, Never(dropped.clone()))
            .unwrap();
        assert!(matches!(
            executor.start(options(None), 0, Never(dropped.clone())),
            Err(NetError::ResourceLimit)
        ));
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
        assert_eq!(executor.pending_count(), 1);
        drop(first);
        assert_eq!(executor.pending_count(), 0);
        assert_eq!(dropped.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn network_executor_cancel_precedes_timeout_and_cleanup_precedes_reply() {
        let executor = NetworkExecutor::new(7, 2).unwrap();
        let dropped = Arc::new(AtomicUsize::new(0));
        let deadline = Some(Deadline::create(7, 10).unwrap());
        let mut operation = executor
            .start(options(deadline), 9, Never(dropped.clone()))
            .unwrap();
        assert!(executor.poll(&mut operation, 9).is_pending());
        operation.cancel();
        operation.cancel();
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
        assert_eq!(executor.pending_count(), 0);
        assert_eq!(
            executor.poll(&mut operation, 10),
            Poll::Ready(Err(NetError::Cancelled))
        );
        let mut timeout = executor
            .start(options(deadline), 9, Never(dropped.clone()))
            .unwrap();
        assert_eq!(
            executor.poll(&mut timeout, 10),
            Poll::Ready(Err(NetError::Timeout))
        );
        assert_eq!(dropped.load(Ordering::Relaxed), 2);
        assert_eq!(executor.pending_count(), 0);
        assert!(matches!(
            executor.start(options(deadline), 10, Never(dropped.clone())),
            Err(NetError::Timeout)
        ));
    }

    #[test]
    fn network_executor_committed_progress_cannot_be_overwritten_by_late_cancel() {
        let executor = NetworkExecutor::new(7, 1).unwrap();
        let mut operation = executor
            .start(options(None), 0, async { Ok::<_, NetError>(3) })
            .unwrap();
        assert_eq!(executor.poll(&mut operation, 0), Poll::Ready(Ok(3)));
        operation.cancel();
        assert_eq!(
            executor.poll(&mut operation, 0),
            Poll::Ready(Err(NetError::Closed))
        );
        assert_eq!(executor.pending_count(), 0);
        assert_eq!(TlsError::from_net(NetError::Timeout), TlsError::Timeout);
        assert_eq!(TlsError::from_net(NetError::Cancelled), TlsError::Cancelled);
    }

    #[test]
    fn network_executor_rejects_cross_owner_operations_and_invalid_construction() {
        assert!(matches!(
            NetworkExecutor::new(0, 1),
            Err(NetError::InvalidDeadline)
        ));
        assert!(matches!(
            NetworkExecutor::new(7, 0),
            Err(NetError::InvalidLimit)
        ));
        assert!(matches!(
            NetworkExecutor::new(7, MAX_PENDING_OPERATIONS + 1),
            Err(NetError::ResourceLimit)
        ));
        let owner = NetworkExecutor::new(7, 1).unwrap();
        let other = NetworkExecutor::new(7, 1).unwrap();
        let dropped = Arc::new(AtomicUsize::new(0));
        let mut operation = owner
            .start(options(None), 0, Never(dropped.clone()))
            .unwrap();
        assert_eq!(
            other.poll(&mut operation, 0),
            Poll::Ready(Err(NetError::Host))
        );
        assert_eq!(owner.pending_count(), 0);
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn network_executor_drives_real_tcp_readiness_without_a_worker() {
        use super::super::transport::{Listener, TcpStream};
        let executor = NetworkExecutor::new(7, 2).unwrap();
        let address = tondo_stdlib::net::SocketAddress::create(
            tondo_stdlib::net::IpAddress::parse("127.0.0.1").unwrap(),
            0,
        )
        .unwrap();
        let listener = executor.enter(|| Arc::new(Listener::listen(address, 1).unwrap()));
        let destination = listener.local_address().unwrap();
        let mut connect = executor
            .start(options(None), 0, TcpStream::connect(destination))
            .unwrap();
        let mut accept = executor
            .start(options(None), 0, async move {
                listener.prepare_accept().await?.commit()
            })
            .unwrap();
        let bound = std::time::Instant::now() + Duration::from_secs(2);
        let mut connected = None;
        let mut accepted = None;
        while connected.is_none() || accepted.is_none() {
            assert!(std::time::Instant::now() < bound);
            if connected.is_none()
                && let Poll::Ready(result) = executor.poll(&mut connect, 0)
            {
                connected = Some(result.unwrap());
            }
            if accepted.is_none()
                && let Poll::Ready(result) = executor.poll(&mut accept, 0)
            {
                accepted = Some(result.unwrap());
            }
            std::thread::yield_now();
        }
        assert_eq!(connected.unwrap().peer_address().unwrap(), destination);
        drop(accepted);
        assert_eq!(executor.pending_count(), 0);
    }
}
