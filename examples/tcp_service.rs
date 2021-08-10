use std::net::{TcpListener, TcpStream};
use cynthia::future::prelude::*;
use cynthia::runtime::{self, swap, Async};
use std::sync::Arc;

#[derive(Default, Clone)]
struct Server {
    id: i32,
}

impl Server {
    pub fn builder() -> Self {
        Server {
            id: 123,
        }
    }
}

impl Server {
    fn register<B>(&mut self, f: B) -> Router<B> 
    where
        B: Service + Clone,
    {
        Router::new(self.clone(), f)
    }

    async fn serve<B>(self, listener: Async<TcpListener>, route: Routes<B>) -> swap::Result<()>
    where
        B: Service + Send + Sync + 'static + Clone,
    {
        loop {
            let (stream, _peer_addr) = listener.accept().await?;
            let r = route.clone();
            runtime::spawn(r.echo(stream)).detach();
        }
    }
}

struct Router<B> {
    server: Server,
    routes: Routes<B>,
}

impl<B: Clone> Router<B> {
    fn new(server: Server, inner: B) -> Router<B> 
    where
        B: Clone,
    {
        Router { 
            server: server,
            routes: Routes::new(inner)
        }
    }

    async fn serve(self, listener: Async<TcpListener>) -> swap::Result<()> 
    where
        B: Service + Send + Sync + Clone + 'static,
    {
        self.server.serve(listener, self.routes).await
    }
}

#[derive(Default, Clone)]
struct Routes<B> {
    inner: B,
}

impl<B> Routes<B> {
    fn new(inner: B) -> Routes<B> {
        Routes { 
            inner: inner
        }
    }

    async fn echo(mut self, mut stream: Async<TcpStream>) -> swap::Result<()> 
    where
        B: Service + Send,
    {
        let mut buf = vec![0u8; 1024];
    
        loop {
            let n = stream.read(&mut buf).await?;
    
            if n == 0 {
                return Ok(());
            }

            let m = self.inner.call(buf[0]);
            buf[0] = m;
    
            stream.write_all(&buf[0..n]).await?;
        }
    }
}

impl<B> Service for Routes<B> 
where
    B: Service + Send + Sync + 'static,
{
    fn call(&mut self, n: u8) -> u8 {
        self.inner.call(n)
    }
}

trait Service {
    fn call(&mut self, n: u8) -> u8;
}

#[derive(Default, Clone)]
struct EchoServer<T: Echo> {
    inner: Arc<T>,
}

impl<T: Echo> EchoServer<T> {
    fn new(inner: T) -> Self {
        let inner = Arc::new(inner);
        Self::from_shared(inner)
    }

    fn from_shared(inner: Arc<T>) -> Self {
        Self {inner}
    }
}

impl<T: Echo> Service for EchoServer<T> {
    fn call(&mut self, n: u8) -> u8 {
        self.inner.echo(n)
    }
}

pub trait Echo: Send + Sync + 'static {
    fn echo(&self, _: u8) -> u8 {
        0
    }
}

#[derive(Default, Clone)]
struct MyEcho {}

impl Echo for MyEcho {
    fn echo(&self, n: u8) -> u8 {
        let m: u8 = n + 1;
        m
    }
}

#[cynthia::main]
async fn main() -> swap::Result<()> {
    let listener = Async::<TcpListener>::bind("127.0.0.1:7000").await?;

    let mut server = Server::builder();
    let route = server.register(EchoServer::new(MyEcho::default()));

    route.serve(listener).await?;

    Ok(())
}
