use axum::{
    body::Body,
    extract::{Request, State},
    http::{header, HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use hivemind_adapters::{
    fs::FsContentStore,
    sqlite::{SqliteMetadataStore, StoredChunk},
};
use hivemind_app::{AppResult, ClockPort, IdentityPort, PublishObject, PublishObjectInput};
use hivemind_core::{ObjectKind, Payload};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct ApiConfig {
    pub bearer_token: String,
}

#[derive(Clone)]
pub struct AppState {
    pub identity: Arc<dyn IdentityPort>,
    pub clock: Arc<dyn ClockPort>,
    pub content_store: Arc<FsContentStore>,
    pub metadata_store: Arc<SqliteMetadataStore>,
    pub config: ApiConfig,
}

#[derive(Debug, Deserialize)]
pub struct PublishObjectRequest {
    pub object_type: String,
    pub mime_type: String,
    pub payload_base64: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct PublishObjectResponse {
    pub object_id: String,
    pub chunk_ids: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("unauthorized")]
    Unauthorized,

    #[error("invalid object type")]
    InvalidObjectType,

    #[error("invalid base64 payload")]
    InvalidBase64,

    #[error("application error: {0}")]
    App(String),

    #[error("metadata error: {0}")]
    Metadata(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self {
            ApiError::Unauthorized => StatusCode::UNAUTHORIZED,
            ApiError::InvalidObjectType | ApiError::InvalidBase64 => StatusCode::BAD_REQUEST,
            ApiError::App(_) | ApiError::Metadata(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, self.to_string()).into_response()
    }
}

pub fn app(state: AppState) -> Router {
    let protected_routes = Router::new()
        .route("/v1/objects", post(publish_object))
        .route_layer(middleware::from_fn_with_state(
            state.config.clone(),
            require_bearer_auth,
        ));

    Router::new()
        .route("/health", get(health))
        .merge(protected_routes)
        .with_state(state)
}

async fn health() -> &'static str {
    "ok"
}

async fn require_bearer_auth(
    State(config): State<ApiConfig>,
    headers: HeaderMap,
    request: Request<Body>,
    next: Next,
) -> Result<Response, ApiError> {
    let expected = format!("Bearer {}", config.bearer_token);
    let authorized = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == expected);

    if !authorized {
        return Err(ApiError::Unauthorized);
    }

    Ok(next.run(request).await)
}

async fn publish_object(
    State(state): State<AppState>,
    Json(request): Json<PublishObjectRequest>,
) -> Result<Json<PublishObjectResponse>, ApiError> {
    let object_kind = parse_object_kind(&request.object_type)?;
    let payload = STANDARD
        .decode(request.payload_base64.as_bytes())
        .map_err(|_| ApiError::InvalidBase64)?;

    let publish = PublishObject::new(
        state.identity.as_ref(),
        state.clock.as_ref(),
        state.content_store.as_ref(),
        state.content_store.as_ref(),
    );
    let output = publish
        .execute(PublishObjectInput {
            kind: object_kind,
            mime_type: request.mime_type,
            payload,
            tags: request.tags,
            references: Vec::new(),
        })
        .await
        .map_err(app_error)?;

    let envelope = state
        .content_store
        .get_object(output.object_id)
        .await
        .map_err(|err| ApiError::App(err.to_string()))?;
    let chunks = stored_chunks_from_payload(&state.content_store, &envelope.body.payload);
    let received_at_ms = state.clock.now_ms().await.map_err(app_error)?;
    state
        .metadata_store
        .record_object(
            &envelope,
            state.content_store.object_path(output.object_id),
            &chunks,
            received_at_ms,
        )
        .map_err(|err| ApiError::Metadata(err.to_string()))?;

    Ok(Json(PublishObjectResponse {
        object_id: output.object_id.to_string(),
        chunk_ids: output
            .chunk_ids
            .into_iter()
            .map(|chunk_id| chunk_id.to_string())
            .collect(),
    }))
}

fn stored_chunks_from_payload(store: &FsContentStore, payload: &Payload) -> Vec<StoredChunk> {
    match payload {
        Payload::Inline(_) => Vec::new(),
        Payload::Chunked(chunked) => chunked
            .chunks
            .iter()
            .map(|chunk| StoredChunk {
                chunk_id: chunk.chunk_id,
                position: chunk.index,
                size: chunk.size,
                path: store
                    .chunk_path(chunk.chunk_id)
                    .to_string_lossy()
                    .into_owned(),
            })
            .collect(),
    }
}

fn parse_object_kind(value: &str) -> Result<ObjectKind, ApiError> {
    match value {
        "skill" => Ok(ObjectKind::Skill),
        "fact" => Ok(ObjectKind::Fact),
        "procedure" => Ok(ObjectKind::Procedure),
        "insight" => Ok(ObjectKind::Insight),
        "rating" => Ok(ObjectKind::Rating),
        "report" => Ok(ObjectKind::Report),
        "tombstone" => Ok(ObjectKind::Tombstone),
        "alias" => Ok(ObjectKind::Alias),
        _ => Err(ApiError::InvalidObjectType),
    }
}

fn app_error(err: hivemind_app::AppError) -> ApiError {
    ApiError::App(err.to_string())
}

#[derive(Clone)]
pub struct SystemClock;

#[async_trait::async_trait]
impl ClockPort for SystemClock {
    async fn now_ms(&self) -> AppResult<u64> {
        let duration = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|err| hivemind_app::AppError::Clock(err.to_string()))?;
        Ok(duration.as_millis() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DevIdentity;
    use axum::{
        body::to_bytes,
        http::{Method, Request},
    };
    use tower::ServiceExt;

    #[derive(Clone)]
    struct TestClock;

    #[async_trait::async_trait]
    impl ClockPort for TestClock {
        async fn now_ms(&self) -> AppResult<u64> {
            Ok(1_700_000_000_000)
        }
    }

    struct TestApp {
        router: Router,
        content_store: Arc<FsContentStore>,
        metadata_store: Arc<SqliteMetadataStore>,
    }

    fn test_app(tempdir: &tempfile::TempDir) -> TestApp {
        let content_store = Arc::new(FsContentStore::new(tempdir.path()));
        let metadata_store = Arc::new(SqliteMetadataStore::in_memory().unwrap());
        let state = AppState {
            identity: Arc::new(DevIdentity::from_seed([1_u8; 32])),
            clock: Arc::new(TestClock),
            content_store: Arc::clone(&content_store),
            metadata_store: Arc::clone(&metadata_store),
            config: ApiConfig {
                bearer_token: "secret".to_owned(),
            },
        };
        TestApp {
            router: app(state),
            content_store,
            metadata_store,
        }
    }

    fn authorized_json_request(path: &str, body: serde_json::Value) -> Request<Body> {
        Request::builder()
            .method(Method::POST)
            .uri(path)
            .header(header::AUTHORIZATION, "Bearer secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    #[tokio::test]
    async fn health_returns_ok_without_auth() {
        let tempdir = tempfile::tempdir().unwrap();
        let response = test_app(&tempdir)
            .router
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn publish_inline_object_returns_object_id() {
        let tempdir = tempfile::tempdir().unwrap();
        let test_app = test_app(&tempdir);
        let response = test_app
            .router
            .clone()
            .oneshot(authorized_json_request(
                "/v1/objects",
                serde_json::json!({
                    "object_type": "fact",
                    "mime_type": "text/plain",
                    "payload_base64": STANDARD.encode(b"hello"),
                    "tags": ["rust"]
                }),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: PublishObjectResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body.object_id.len(), 64);
        assert!(body.chunk_ids.is_empty());
        let object_id = body.object_id.parse().unwrap();
        assert!(test_app.content_store.has_object(object_id).await.unwrap());
        let metadata = test_app
            .metadata_store
            .get_object_metadata(object_id)
            .unwrap()
            .unwrap();
        assert_eq!(metadata.object_id, object_id);
        assert_eq!(metadata.mime_type, "text/plain");
        assert_eq!(metadata.payload_size, 5);
        assert_eq!(
            test_app.metadata_store.objects_for_tag("rust").unwrap(),
            vec![object_id]
        );
    }

    #[tokio::test]
    async fn publish_requires_auth() {
        let tempdir = tempfile::tempdir().unwrap();
        let response = test_app(&tempdir)
            .router
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v1/objects")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn invalid_base64_returns_bad_request() {
        let tempdir = tempfile::tempdir().unwrap();
        let response = test_app(&tempdir)
            .router
            .oneshot(authorized_json_request(
                "/v1/objects",
                serde_json::json!({
                    "object_type": "fact",
                    "mime_type": "text/plain",
                    "payload_base64": "not base64",
                    "tags": []
                }),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn invalid_object_type_returns_bad_request() {
        let tempdir = tempfile::tempdir().unwrap();
        let response = test_app(&tempdir)
            .router
            .oneshot(authorized_json_request(
                "/v1/objects",
                serde_json::json!({
                    "object_type": "nope",
                    "mime_type": "text/plain",
                    "payload_base64": STANDARD.encode(b"hello"),
                    "tags": []
                }),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
}
