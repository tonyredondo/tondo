//! Hickory DNS jobs owned and polled by one resolution, with immediate cleanup.

use hickory_resolver::{
    ConnectionProvider, PoolContext,
    config::{ConnectionConfig, ProtocolConfig},
    net::{
        runtime::{RuntimeProvider, Spawn, TokioRuntimeProvider},
        udp::UdpClientStream,
        xfer::DnsExchange,
    },
};
use std::{
    collections::VecDeque,
    future::Future,
    io,
    net::{IpAddr, SocketAddr},
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    task::Context,
    time::Duration,
};
use tondo_stdlib::net::NetError;

type DnsFuture = Pin<Box<dyn Future<Output = ()> + Send>>;
const MAX_DNS_JOBS: usize = 8;

/// No Tokio task is spawned: the query polls these bounded provider futures.
/// Thus cancellation drops sockets immediately, without an asynchronous abort
/// that could leave a descriptor alive after the cancelled reply is published.
#[derive(Clone)]
pub(super) struct DnsRuntime {
    jobs: Arc<Mutex<VecDeque<DnsFuture>>>,
    failed: Arc<AtomicBool>,
    active: Arc<AtomicUsize>,
}

pub(super) struct DnsOwner(DnsRuntime);

impl Drop for DnsOwner {
    fn drop(&mut self) {
        match self.0.jobs.lock() {
            Ok(mut jobs) => jobs.clear(),
            Err(poisoned) => poisoned.into_inner().clear(),
        }
        self.0.active.store(0, Ordering::Release);
    }
}

impl DnsRuntime {
    pub(super) fn new() -> Result<(Self, DnsOwner), NetError> {
        let mut jobs = VecDeque::new();
        jobs.try_reserve_exact(MAX_DNS_JOBS)
            .map_err(|_| NetError::ResourceLimit)?;
        let runtime = Self {
            jobs: Arc::new(Mutex::new(jobs)),
            failed: Arc::new(AtomicBool::new(false)),
            active: Arc::new(AtomicUsize::new(0)),
        };
        Ok((runtime.clone(), DnsOwner(runtime)))
    }

    pub(super) fn poll_jobs(&self, context: &mut Context<'_>) -> Result<(), NetError> {
        if self.failed.load(Ordering::Acquire) {
            return Err(NetError::ResourceLimit);
        }
        let count = self.jobs.lock().map_err(|_| NetError::Host)?.len();
        for _ in 0..count {
            let job = self.jobs.lock().map_err(|_| NetError::Host)?.pop_front();
            let Some(mut job) = job else {
                break;
            };
            // A provider future may enqueue work. Never poll while holding the
            // queue mutex, and never execute newly enqueued jobs without a bound.
            if job.as_mut().poll(context).is_pending() {
                self.jobs.lock().map_err(|_| NetError::Host)?.push_back(job);
            } else {
                self.active.fetch_sub(1, Ordering::AcqRel);
            }
        }
        if self.failed.load(Ordering::Acquire) {
            return Err(NetError::ResourceLimit);
        }
        Ok(())
    }
}

impl Spawn for DnsRuntime {
    fn spawn_bg(&mut self, future: impl Future<Output = ()> + Send + 'static) {
        let Ok(mut jobs) = self.jobs.lock() else {
            self.failed.store(true, Ordering::Release);
            return;
        };
        if self
            .active
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < MAX_DNS_JOBS).then_some(count + 1)
            })
            .is_err()
        {
            self.failed.store(true, Ordering::Release);
            return;
        }
        jobs.push_back(Box::pin(future));
    }
}

impl RuntimeProvider for DnsRuntime {
    type Handle = Self;
    type Timer = <TokioRuntimeProvider as RuntimeProvider>::Timer;
    type Udp = <TokioRuntimeProvider as RuntimeProvider>::Udp;
    type Tcp = <TokioRuntimeProvider as RuntimeProvider>::Tcp;
    fn create_handle(&self) -> Self::Handle {
        self.clone()
    }
    fn connect_tcp(
        &self,
        address: SocketAddr,
        bind: Option<SocketAddr>,
        wait: Option<Duration>,
    ) -> Pin<Box<dyn Future<Output = Result<Self::Tcp, io::Error>> + Send>> {
        TokioRuntimeProvider::new().connect_tcp(address, bind, wait)
    }
    fn bind_udp(
        &self,
        local: SocketAddr,
        server: SocketAddr,
    ) -> Pin<Box<dyn Future<Output = Result<Self::Udp, io::Error>> + Send>> {
        TokioRuntimeProvider::new().bind_udp(local, server)
    }
}

#[derive(Clone)]
pub(super) struct DnsConnector(pub(super) DnsRuntime);

impl ConnectionProvider for DnsConnector {
    type Conn = DnsExchange<DnsRuntime>;
    type FutureConn =
        Pin<Box<dyn Future<Output = Result<Self::Conn, hickory_resolver::net::NetError>> + Send>>;
    type RuntimeProvider = DnsRuntime;

    fn new_connection(
        &self,
        ip: IpAddr,
        config: &ConnectionConfig,
        context: &PoolContext,
    ) -> Result<Self::FutureConn, hickory_resolver::net::NetError> {
        if !matches!(config.protocol, ProtocolConfig::Udp) {
            return Err(
                io::Error::new(io::ErrorKind::Unsupported, "DNS transport is not UDP").into(),
            );
        }
        let address = SocketAddr::new(ip, config.port);
        let timeout = context.options.timeout;
        let runtime = self.0.clone();
        Ok(Box::pin(async move {
            Ok(UdpClientStream::builder(address, runtime)
                .with_timeout(Some(timeout))
                .with_os_port_selection(true)
                .with_max_retries(0)
                .exchange())
        }))
    }

    fn runtime_provider(&self) -> &Self::RuntimeProvider {
        &self.0
    }
}
