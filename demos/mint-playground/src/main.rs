#![forbid(unsafe_code)]

mod db;

use axum::{
    extract::{DefaultBodyLimit, Query, State},
    http::{header, HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use std::{path::PathBuf, time::Duration};
use tokio::sync::{mpsc, oneshot};

type Reply<T> = oneshot::Sender<Result<T, db::Error>>;

enum Command {
    Snapshot(bool, Reply<db::Snapshot>),
    Submit(db::ContributionRequest, Reply<db::Submitted>),
    Rewards(Reply<Vec<db::RewardBalance>>),
    Tick(Reply<()>),
    Stop(oneshot::Sender<()>),
}

#[derive(Clone)]
struct Store(mpsc::Sender<Command>);

impl Store {
    fn open(path: PathBuf) -> Result<(Self, std::thread::JoinHandle<()>), String> {
        let (sender, mut receiver) = mpsc::channel(32);
        let (ready, started) = std::sync::mpsc::sync_channel(1);
        let thread = std::thread::Builder::new()
            .name("mint-sqlite".into())
            .spawn(move || {
                let mut database = match db::Database::open(&path) {
                    Ok(database) => database,
                    Err(error) => {
                        let _ = ready.send(Err(error.to_string()));
                        return;
                    }
                };
                let _ = ready.send(Ok(()));
                while let Some(command) = receiver.blocking_recv() {
                    match command {
                        Command::Snapshot(full_history, reply) => {
                            let _ = reply.send(database.snapshot(full_history));
                        }
                        Command::Submit(request, reply) => {
                            let _ = reply.send(database.submit(request));
                        }
                        Command::Rewards(reply) => {
                            let _ = reply.send(database.rewards());
                        }
                        Command::Tick(reply) => {
                            let _ = reply.send(database.tick());
                        }
                        Command::Stop(reply) => {
                            let _ = reply.send(());
                            break;
                        }
                    }
                }
            })
            .map_err(|error| error.to_string())?;
        started.recv().map_err(|error| error.to_string())??;
        Ok((Self(sender), thread))
    }

    async fn request<T>(&self, command: impl FnOnce(Reply<T>) -> Command) -> Result<T, ApiError> {
        let (reply, result) = oneshot::channel();
        self.0
            .try_send(command(reply))
            .map_err(|_| ApiError::unavailable())?;
        result
            .await
            .map_err(|_| ApiError::unavailable())?
            .map_err(Into::into)
    }

    async fn stop(&self) {
        let (reply, result) = oneshot::channel();
        if self.0.send(Command::Stop(reply)).await.is_ok() {
            let _ = result.await;
        }
    }
}

struct ApiError(StatusCode, String);

impl ApiError {
    fn unavailable() -> Self {
        Self(
            StatusCode::SERVICE_UNAVAILABLE,
            "Database busy or unavailable; retry shortly.".into(),
        )
    }
}

impl From<db::Error> for ApiError {
    fn from(error: db::Error) -> Self {
        match error {
            db::Error::Invalid(message) => Self(StatusCode::BAD_REQUEST, message),
            db::Error::Conflict(message) => Self(StatusCode::CONFLICT, message),
            error => {
                eprintln!("Database error: {error}");
                Self(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Database operation failed; no partial accounting was committed.".into(),
                )
            }
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({"error": self.1}))).into_response()
    }
}

#[derive(Default, serde::Deserialize)]
struct SnapshotQuery {
    #[serde(default)]
    full_history: bool,
}

async fn snapshot(
    State(store): State<Store>,
    Query(query): Query<SnapshotQuery>,
) -> Result<Json<db::Snapshot>, ApiError> {
    store
        .request(|reply| Command::Snapshot(query.full_history, reply))
        .await
        .map(Json)
}

async fn rewards(State(store): State<Store>) -> Result<Json<Vec<db::RewardBalance>>, ApiError> {
    store.request(Command::Rewards).await.map(Json)
}

async fn submit(
    State(store): State<Store>,
    headers: HeaderMap,
    request: Result<Json<db::ContributionRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<db::Submitted>, ApiError> {
    // JSON-only writes, no CORS, and an origin check keep other sites out of this local demo.
    if let Some(origin) = headers.get(header::ORIGIN) {
        let host = headers
            .get(header::HOST)
            .and_then(|value| value.to_str().ok());
        let origin = origin.to_str().ok().and_then(|origin| {
            origin
                .strip_prefix("http://")
                .or_else(|| origin.strip_prefix("https://"))
        });
        if host.is_none() || origin != host {
            return Err(ApiError(
                StatusCode::FORBIDDEN,
                "Cross-origin submissions are not allowed.".into(),
            ));
        }
    }
    let Json(request) = request.map_err(|error| {
        ApiError(
            error.status(),
            "Expected JSON containing address, budget_sats, duration, optional delay_blocks and request_id.".into(),
        )
    })?;
    store
        .request(|reply| Command::Submit(request, reply))
        .await
        .map(Json)
}

async fn wallet_asset(js: bool) -> Response {
    let filename = if js {
        "flamewallet_wasm.js"
    } else {
        "flamewallet_wasm_bg.wasm"
    };
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/wallet-wasm/pkg")
        .join(filename);
    match tokio::fs::read(path).await {
        Ok(bytes) => (
            [(
                header::CONTENT_TYPE,
                if js {
                    "text/javascript; charset=utf-8"
                } else {
                    "application/wasm"
                },
            )],
            bytes,
        )
            .into_response(),
        Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            "Browser wallet is not built. Start the app with ./run.sh; see README.md.",
        )
            .into_response(),
    }
}

fn router(store: Store) -> Router {
    Router::new()
        .route(
            "/",
            get(|| async { Html(include_str!("../web/index.html")) }),
        )
        .route(
            "/mint",
            get(|| async { Html(include_str!("../web/index.html")) }),
        )
        .route(
            "/app.js",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                    include_str!("../web/app.js"),
                )
            }),
        )
        .route(
            "/style.css",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
                    include_str!("../web/style.css"),
                )
            }),
        )
        .route("/wallet/flamewallet_wasm.js", get(|| wallet_asset(true)))
        .route(
            "/wallet/flamewallet_wasm_bg.wasm",
            get(|| wallet_asset(false)),
        )
        .route("/api/state", get(snapshot))
        .route("/api/rewards", get(rewards))
        .route("/api/contributions", post(submit))
        .layer(DefaultBodyLimit::max(2048))
        .layer(middleware::from_fn(security_headers))
        .with_state(store)
}

async fn security_headers(request: axum::extract::Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_SECURITY_POLICY, "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; connect-src 'self' https://api.coinbase.com; img-src 'self' data:; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'".parse().unwrap());
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    headers.insert(header::REFERRER_POLICY, "no-referrer".parse().unwrap());
    headers.insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    response
}

async fn clock(store: Store) -> Result<(), ApiError> {
    loop {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let (reply, result) = oneshot::channel();
        // HTTP callers shed load; the internal clock waits instead of stopping on a full queue.
        store
            .0
            .send(Command::Tick(reply))
            .await
            .map_err(|_| ApiError::unavailable())?;
        result
            .await
            .map_err(|_| ApiError::unavailable())?
            .map_err(ApiError::from)?;
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data/mint.sqlite3"));
    if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let (store, thread) = Store::open(path).map_err(std::io::Error::other)?;
    let address = std::env::var("MINT_LISTEN").unwrap_or_else(|_| "127.0.0.1:3000".into());
    let listener = tokio::net::TcpListener::bind(&address).await?;
    println!("Mint playground: http://{address} (simulated funds, testnet addresses only)");
    let result = tokio::select! {
        result = axum::serve(listener, router(store.clone())) => result,
        result = clock(store.clone()) => Err(std::io::Error::other(result.err().map(|error| error.1).unwrap_or_default())),
        result = tokio::signal::ctrl_c() => result,
    };
    store.stop().await;
    thread
        .join()
        .map_err(|_| std::io::Error::other("SQLite thread panicked"))?;
    result?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    async fn json(response: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(response.into_body(), 1_000_000)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn contribution() -> serde_json::Value {
        let address = flamekd::SpendKey::from_seed(&[42; 64])
            .unwrap()
            .to_recv()
            .to_address()
            .to_bech32(flamekd::Network::Testnet);
        serde_json::json!({"address": address, "budget_sats": 400000, "duration": 200, "request_id": "test-submission-00001"})
    }

    fn post_request(value: &serde_json::Value) -> Request<Body> {
        Request::post("/api/contributions")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(value.to_string()))
            .unwrap()
    }

    #[tokio::test]
    async fn clock_waits_for_capacity_instead_of_stopping_on_a_full_queue() {
        let (sender, mut receiver) = mpsc::channel(1);
        let (reply, _result) = oneshot::channel();
        sender
            .try_send(Command::Snapshot(false, reply))
            .ok()
            .unwrap();
        let task = tokio::spawn(clock(Store(sender)));
        tokio::time::sleep(Duration::from_millis(1100)).await;
        assert!(!task.is_finished());
        receiver.recv().await.unwrap();
        let command = tokio::time::timeout(Duration::from_secs(1), receiver.recv())
            .await
            .unwrap()
            .unwrap();
        match command {
            Command::Tick(reply) => {
                reply.send(Ok(())).unwrap();
            }
            _ => panic!("Expected the delayed block tick"),
        }
        assert!(!task.is_finished());
        task.abort();
    }

    #[tokio::test]
    async fn routes_submission_retry_and_public_reward_export() {
        let (store, thread) = Store::open(PathBuf::from(":memory:")).unwrap();
        let app = router(store.clone());
        for path in ["/", "/mint", "/app.js", "/style.css", "/api/state"] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            assert!(response
                .headers()
                .contains_key(header::CONTENT_SECURITY_POLICY));
            assert!(response.headers()[header::CONTENT_SECURITY_POLICY]
                .to_str()
                .unwrap()
                .contains("connect-src 'self' https://api.coinbase.com;"));
        }
        let contribution = contribution();
        let first = json(
            app.clone()
                .oneshot(post_request(&contribution))
                .await
                .unwrap(),
        )
        .await;
        let retry = json(
            app.clone()
                .oneshot(post_request(&contribution))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(first, retry);
        assert_eq!(first["user_id"], 1);
        assert_eq!(first["start_block"], 1);
        let mut pending_states = Vec::new();
        for path in ["/api/state", "/api/state?full_history=true"] {
            pending_states.push(
                json(
                    app.clone()
                        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                        .await
                        .unwrap(),
                )
                .await,
            );
        }
        assert_eq!(
            pending_states[0]["contributions"].as_array().unwrap().len(),
            1
        );
        assert_eq!(pending_states[0]["contributions"][0]["delay_blocks"], 0);
        assert_eq!(
            pending_states[1]["contributions"].as_array().unwrap().len(),
            1
        );
        assert_eq!(pending_states[0]["totals"], pending_states[1]["totals"]);
        assert_eq!(pending_states[0]["totals"]["contribution_count"], 1);
        assert_eq!(pending_states[0]["totals"]["burned_sats"], "0");
        assert_eq!(pending_states[0]["totals"]["reward_sparks"], "0");
        store
            .request(Command::Tick)
            .await
            .unwrap_or_else(|error| panic!("{}", error.1));
        let state = json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri("/api/state")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(state["totals"]["reward_sparks"], "5000000000");
        assert_eq!(state["contributions"].as_array().unwrap().len(), 1);
        assert_eq!(state["contributions"][0]["user_id"], 1);
        assert_eq!(
            state["contributions"][0]["address"],
            contribution["address"]
        );
        let rewards = json(
            app.oneshot(
                Request::builder()
                    .uri("/api/rewards")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
        )
        .await;
        assert_eq!(rewards[0]["address"], contribution["address"]);
        assert_eq!(rewards[0]["reward_sparks"], "5000000000");
        store.stop().await;
        thread.join().unwrap();
    }

    #[tokio::test]
    async fn state_route_limits_dashboard_history_but_keeps_full_history_and_totals() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("mint.sqlite3");
        {
            let mut database = db::Database::open(&path).unwrap();
            database
                .submit(serde_json::from_value(contribution()).unwrap())
                .unwrap();
            database.tick().unwrap();
        }
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute(
                "INSERT INTO blocks(height, burned_sats, fees_sats, reward_sparks)
                 VALUES (10201, 0, 0, 0)",
                [],
            )
            .unwrap();
        let (store, thread) = Store::open(path).unwrap();
        let app = router(store.clone());
        let mut states = Vec::new();
        for path in ["/api/state", "/api/state?full_history=true"] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            states.push(json(response).await);
        }
        assert_eq!(states[0]["height"], 10_201);
        assert!(states[0]["contributions"].as_array().unwrap().is_empty());
        assert_eq!(states[1]["contributions"].as_array().unwrap().len(), 1);
        assert_eq!(states[0]["totals"], states[1]["totals"]);
        assert_eq!(states[0]["totals"]["contribution_count"], 1);
        assert_eq!(states[0]["totals"]["burned_sats"], "1000");
        assert_eq!(states[0]["totals"]["fees_sats"], "1000");
        assert_eq!(states[0]["totals"]["reward_sparks"], "5000000000");
        store.stop().await;
        thread.join().unwrap();
    }

    #[tokio::test]
    async fn schedules_nonstep_duration_and_validates_delay_through_json_api() {
        let (store, thread) = Store::open(PathBuf::from(":memory:")).unwrap();
        let app = router(store.clone());
        let mut value = contribution();
        value["duration"] = 145.into();
        value["delay_blocks"] = 4_464.into();
        let response = app.clone().oneshot(post_request(&value)).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let first = json(response).await;
        assert_eq!(first["start_block"], 4_465);
        store
            .request(Command::Tick)
            .await
            .unwrap_or_else(|e| panic!("{}", e.1));
        let retry = json(app.clone().oneshot(post_request(&value)).await.unwrap()).await;
        assert_eq!(first, retry);
        let state = json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri("/api/state")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(state["contributions"][0]["delay_blocks"], 4_464);
        assert_eq!(state["contributions"][0]["duration"], 145);
        assert_eq!(state["totals"]["burned_sats"], "0");
        value["delay_blocks"] = 0.into();
        assert_eq!(
            app.clone()
                .oneshot(post_request(&value))
                .await
                .unwrap()
                .status(),
            StatusCode::CONFLICT
        );
        value["request_id"] = "another-submission-00001".into();
        for delay in [serde_json::json!(-1), serde_json::json!(4_465)] {
            value["delay_blocks"] = delay;
            assert_eq!(
                app.clone()
                    .oneshot(post_request(&value))
                    .await
                    .unwrap()
                    .status(),
                StatusCode::BAD_REQUEST
            );
        }
        value["delay_blocks"] = serde_json::json!(1.5);
        assert_eq!(
            app.oneshot(post_request(&value)).await.unwrap().status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        store.stop().await;
        thread.join().unwrap();
    }

    #[tokio::test]
    async fn rejects_cross_origin_private_fields_and_invalid_input() {
        let (store, thread) = Store::open(PathBuf::from(":memory:")).unwrap();
        let app = router(store.clone());
        let mut request = post_request(&contribution());
        request
            .headers_mut()
            .insert(header::HOST, "127.0.0.1:3000".parse().unwrap());
        request
            .headers_mut()
            .insert(header::ORIGIN, "https://evil.example".parse().unwrap());
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );
        let mut value = contribution();
        value["mnemonic"] = "must never be accepted".into();
        assert_eq!(
            app.clone()
                .oneshot(post_request(&value))
                .await
                .unwrap()
                .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        let request = Request::post("/api/contributions")
            .body(Body::from("address=bad"))
            .unwrap();
        assert_eq!(
            app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        );
        value = contribution();
        value["budget_sats"] = 2000.into();
        assert_eq!(
            app.oneshot(post_request(&value)).await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );
        store.stop().await;
        thread.join().unwrap();
    }
}
