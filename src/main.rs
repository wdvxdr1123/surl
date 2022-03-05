use std::{
    collections::HashMap,
    future::Future,
    net::SocketAddr,
    pin::Pin,
    str::{from_utf8, FromStr},
    sync::{atomic::AtomicU64, Arc},
    task::{Context, Poll},
};

use byteorder::{ByteOrder, LittleEndian};
use hyper::{
    body::to_bytes, http::status::StatusCode, http::Method, service::Service, Body, Request,
    Response,
};
use rusty_leveldb::{CompressionType, Options, DB};
use tokio::sync::Mutex;

const ALPHA_SET: &str = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ";

const ALPHA_SET_LEN: u64 = ALPHA_SET.len() as u64;

pub(crate) fn id_to_string(mut x: u64) -> String {
    let mut id = vec![b'/'];
    let chars = ALPHA_SET.as_bytes();
    while x >= ALPHA_SET_LEN {
        id.push(chars[(x % ALPHA_SET_LEN) as usize]);
        x = x / ALPHA_SET_LEN;
    }
    id.push(chars[x as usize]);
    from_utf8(&id).unwrap().into()
}
struct MainSvc {
    context: Arc<AppContext>,
}

struct MakeSvc {
    context: Arc<AppContext>,
}

impl<T> Service<T> for MakeSvc {
    type Response = MainSvc;
    type Error = hyper::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _: T) -> Self::Future {
        let context = self.context.clone();
        let fut = async move { Ok(MainSvc { context }) };
        Box::pin(fut)
    }
}

struct AppContext {
    website: String,
    id: AtomicU64,
    database: Mutex<DB>,
}

impl Service<Request<Body>> for MainSvc {
    type Response = Response<Body>;
    type Error = hyper::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(self: &mut Self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(self: &mut Self, req: Request<Body>) -> Self::Future {
        let req = Box::pin(req);
        let context = self.context.clone();

        println!("{:?} {} {}", req.version(), req.method(), req.uri().path());

        let x = async move {
            match (req.method(), req.uri().path()) {
                (&Method::GET, path) => context.get(path).await,
                (&Method::POST, "/new") => {
                    let body = to_bytes(req).await?;
                    let form = url::form_urlencoded::parse(&body).into_owned().collect();
                    context.new(form).await
                }
                (&Method::HEAD, _) => Ok(Response::new(Body::empty())),
                (_, _) => context.not_found().await,
            }
        };
        Box::pin(x)
    }
}

impl AppContext {
    async fn get(self: Arc<Self>, path: &str) -> Result<Response<Body>, hyper::Error> {
        println!("{},", path);
        let mut database = self.database.lock().await;
        let resp = match database.get(&path.as_bytes()) {
            Some(val) => {
                let x = val.to_vec();
                let s = std::str::from_utf8(&x).unwrap();
                Response::builder()
                    .status(StatusCode::MOVED_PERMANENTLY)
                    .header("Location", s.to_string())
                    .body(Body::empty())
                    .unwrap()
            }
            _ => Response::new(Body::empty()),
        };
        Ok(resp)
    }

    async fn new(
        self: Arc<Self>,
        form: HashMap<String, String>,
    ) -> Result<Response<Body>, hyper::Error> {
        if let Some(url) = form.get("url") {
            let mut database = self.database.lock().await;
            let nid = self.id.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            let id = id_to_string(nid);
            database.put(id.as_bytes(), url.as_bytes()).unwrap();

            let mut buf = [0; 8];
            LittleEndian::write_u64(&mut buf, nid);
            database.put("__count__".as_bytes(), &buf).unwrap();

            return Ok(Response::builder()
                .header("Content-Type", "application/json")
                .body(format!("{{\"url\":\"{}{}\"}}", self.website.clone(), id).into())
                .unwrap());
        };
        Ok(Response::builder()
            .status(StatusCode::BAD_REQUEST)
            .body("url is required".into())
            .unwrap())
    }

    async fn not_found(self: Arc<Self>) -> Result<Response<Body>, hyper::Error> {
        Ok(Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::empty())
            .unwrap())
    }
}

async fn shutdown_signal() {
    // Wait for the CTRL+C signal
    tokio::signal::ctrl_c()
        .await
        .expect("failed to install CTRL+C signal handler");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use std::env::var;

    let website = var("SURL_WEBSITE").unwrap_or_default();
    let host = var("SURL_HOST").unwrap_or("127.0.0.1".into());
    let port = var("SURL_PORT").unwrap_or("7777".into());
    let addr = SocketAddr::from_str(format!("{}:{}", host, port).as_str())?;

    let mut opt = Options::default();
    opt.compression_type = CompressionType::CompressionSnappy;
    opt.write_buffer_size = 32 * 1024; // 32KiB
    let mut db = DB::open("surl_db", opt)?;

    let count = db.get("__count__".as_bytes()).unwrap_or_default();
    let count = if count.len() == 8 {
        LittleEndian::read_u64(&count)
    } else {
        0
    };

    let service = MakeSvc {
        context: Arc::new(AppContext {
            website,
            id: AtomicU64::new(count),
            database: Mutex::new(db),
        }),
    };
    let server = hyper::server::Server::bind(&addr).serve(service);

    let graceful = server.with_graceful_shutdown(shutdown_signal());

    if let Err(e) = graceful.await {
        eprintln!("server error: {}", e);
    }
    Ok(())
}
