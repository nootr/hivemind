use axum::{
    body::Body,
    extract::{Path, Request, State},
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
use hivemind_core::{ObjectId, ObjectKind, Payload};
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
    #[serde(default)]
    pub references: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct PublishObjectResponse {
    pub object_id: String,
    pub chunk_ids: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct GetObjectResponse {
    pub object_id: String,
    pub object_type: String,
    pub author_agent_id: String,
    pub created_at_ms: u64,
    pub mime_type: String,
    pub tags: Vec<String>,
    pub references: Vec<String>,
    pub payload_base64: String,
    pub verified: bool,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct TagLookupResponse {
    pub tag: String,
    pub objects: Vec<ObjectSummary>,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct ReferrersResponse {
    pub object_id: String,
    pub objects: Vec<ObjectSummary>,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct ObjectSummary {
    pub object_id: String,
    pub object_type: String,
    pub author_agent_id: String,
    pub created_at_ms: u64,
    pub mime_type: String,
    pub payload_size: u64,
    pub chunk_count: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("unauthorized")]
    Unauthorized,

    #[error("invalid object type")]
    InvalidObjectType,

    #[error("invalid object id")]
    InvalidObjectId,

    #[error("object not found")]
    ObjectNotFound,

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
            ApiError::InvalidObjectType | ApiError::InvalidObjectId | ApiError::InvalidBase64 => {
                StatusCode::BAD_REQUEST
            }
            ApiError::ObjectNotFound => StatusCode::NOT_FOUND,
            ApiError::App(_) | ApiError::Metadata(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, self.to_string()).into_response()
    }
}

pub fn app(state: AppState) -> Router {
    let protected_routes = Router::new()
        .route("/v1/objects", post(publish_object))
        .route("/v1/objects/{object_id}", get(get_object))
        .route("/v1/objects/{object_id}/referrers", get(get_referrers))
        .route("/v1/tags/{tag}", get(get_tag))
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
    let references = parse_object_ids(request.references)?;

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
            references,
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

async fn get_object(
    State(state): State<AppState>,
    Path(object_id): Path<String>,
) -> Result<Json<GetObjectResponse>, ApiError> {
    let object_id = object_id
        .parse::<ObjectId>()
        .map_err(|_| ApiError::InvalidObjectId)?;
    let envelope = state
        .content_store
        .get_object(object_id)
        .await
        .map_err(|err| match err {
            hivemind_adapters::fs::FsStoreError::Io(io_err)
                if io_err.kind() == std::io::ErrorKind::NotFound =>
            {
                ApiError::ObjectNotFound
            }
            other => ApiError::App(other.to_string()),
        })?;
    envelope
        .verify()
        .map_err(|err| ApiError::App(err.to_string()))?;
    let (mime_type, payload_bytes) =
        assemble_payload(&state.content_store, &envelope.body.payload).await?;

    Ok(Json(GetObjectResponse {
        object_id: envelope.object_id.to_string(),
        object_type: object_kind_to_str(envelope.body.kind).to_owned(),
        author_agent_id: envelope.body.author.to_string(),
        created_at_ms: envelope.body.created_at_ms,
        mime_type,
        tags: envelope.body.tags,
        references: envelope
            .body
            .references
            .into_iter()
            .map(|object_id| object_id.to_string())
            .collect(),
        payload_base64: STANDARD.encode(payload_bytes),
        verified: true,
    }))
}

async fn get_tag(
    State(state): State<AppState>,
    Path(tag): Path<String>,
) -> Result<Json<TagLookupResponse>, ApiError> {
    let object_ids = state
        .metadata_store
        .objects_for_tag(&tag)
        .map_err(|err| ApiError::Metadata(err.to_string()))?;
    let objects = object_summaries_for_ids(&state.metadata_store, object_ids)?;

    Ok(Json(TagLookupResponse { tag, objects }))
}

async fn get_referrers(
    State(state): State<AppState>,
    Path(object_id): Path<String>,
) -> Result<Json<ReferrersResponse>, ApiError> {
    let object_id = object_id
        .parse::<ObjectId>()
        .map_err(|_| ApiError::InvalidObjectId)?;
    let object_ids = state
        .metadata_store
        .objects_referencing(object_id)
        .map_err(|err| ApiError::Metadata(err.to_string()))?;
    let objects = object_summaries_for_ids(&state.metadata_store, object_ids)?;

    Ok(Json(ReferrersResponse {
        object_id: object_id.to_string(),
        objects,
    }))
}

fn object_summaries_for_ids(
    metadata_store: &SqliteMetadataStore,
    object_ids: Vec<ObjectId>,
) -> Result<Vec<ObjectSummary>, ApiError> {
    let mut objects = Vec::with_capacity(object_ids.len());

    for object_id in object_ids {
        let metadata = metadata_store
            .get_object_metadata(object_id)
            .map_err(|err| ApiError::Metadata(err.to_string()))?
            .ok_or_else(|| {
                ApiError::Metadata("object index points to missing object metadata".to_owned())
            })?;
        objects.push(ObjectSummary {
            object_id: metadata.object_id.to_string(),
            object_type: object_kind_to_str(metadata.object_kind).to_owned(),
            author_agent_id: metadata.author_agent_id.to_string(),
            created_at_ms: metadata.created_at_ms,
            mime_type: metadata.mime_type,
            payload_size: metadata.payload_size,
            chunk_count: metadata.chunk_count,
        });
    }

    Ok(objects)
}

async fn assemble_payload(
    store: &FsContentStore,
    payload: &Payload,
) -> Result<(String, Vec<u8>), ApiError> {
    match payload {
        Payload::Inline(inline) => Ok((inline.mime_type.clone(), inline.bytes.clone())),
        Payload::Chunked(chunked) => {
            let mut bytes = Vec::with_capacity(chunked.total_size as usize);
            for chunk in &chunked.chunks {
                bytes.extend(
                    store
                        .get_chunk(chunk.chunk_id)
                        .await
                        .map_err(|err| ApiError::App(err.to_string()))?,
                );
            }
            if bytes.len() as u64 != chunked.total_size {
                return Err(ApiError::App("assembled payload size mismatch".to_owned()));
            }
            Ok((chunked.mime_type.clone(), bytes))
        }
    }
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

fn object_kind_to_str(kind: ObjectKind) -> &'static str {
    match kind {
        ObjectKind::Skill => "skill",
        ObjectKind::Fact => "fact",
        ObjectKind::Procedure => "procedure",
        ObjectKind::Insight => "insight",
        ObjectKind::Rating => "rating",
        ObjectKind::Report => "report",
        ObjectKind::Tombstone => "tombstone",
        ObjectKind::Alias => "alias",
    }
}

fn parse_object_ids(values: Vec<String>) -> Result<Vec<ObjectId>, ApiError> {
    values
        .into_iter()
        .map(|value| {
            value
                .parse::<ObjectId>()
                .map_err(|_| ApiError::InvalidObjectId)
        })
        .collect()
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

    fn authorized_get_request(path: &str) -> Request<Body> {
        Request::builder()
            .method(Method::GET)
            .uri(path)
            .header(header::AUTHORIZATION, "Bearer secret")
            .body(Body::empty())
            .unwrap()
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
    async fn tag_lookup_requires_auth() {
        let tempdir = tempfile::tempdir().unwrap();
        let response = test_app(&tempdir)
            .router
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v1/tags/rust")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn unknown_tag_returns_empty_list() {
        let tempdir = tempfile::tempdir().unwrap();
        let response = test_app(&tempdir)
            .router
            .oneshot(authorized_get_request("/v1/tags/unknown"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: TagLookupResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body.tag, "unknown");
        assert!(body.objects.is_empty());
    }

    #[tokio::test]
    async fn tag_lookup_returns_published_object_summary() {
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
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let published: PublishObjectResponse = serde_json::from_slice(&bytes).unwrap();

        let response = test_app
            .router
            .oneshot(authorized_get_request("/v1/tags/rust"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: TagLookupResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body.tag, "rust");
        assert_eq!(body.objects.len(), 1);
        assert_eq!(body.objects[0].object_id, published.object_id);
        assert_eq!(body.objects[0].object_type, "fact");
        assert_eq!(body.objects[0].mime_type, "text/plain");
        assert_eq!(body.objects[0].payload_size, 5);
        assert_eq!(body.objects[0].chunk_count, 0);
    }

    #[tokio::test]
    async fn tag_lookup_is_exact_match() {
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
                    "tags": ["rust-libp2p"]
                }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = test_app
            .router
            .oneshot(authorized_get_request("/v1/tags/rust"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: TagLookupResponse = serde_json::from_slice(&bytes).unwrap();
        assert!(body.objects.is_empty());
    }

    #[tokio::test]
    async fn referrers_requires_auth() {
        let tempdir = tempfile::tempdir().unwrap();
        let object_id = "00".repeat(32);
        let response = test_app(&tempdir)
            .router
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri(format!("/v1/objects/{object_id}/referrers"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn referrers_invalid_object_id_returns_bad_request() {
        let tempdir = tempfile::tempdir().unwrap();
        let response = test_app(&tempdir)
            .router
            .oneshot(authorized_get_request("/v1/objects/not-an-id/referrers"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn unknown_referrers_returns_empty_list() {
        let tempdir = tempfile::tempdir().unwrap();
        let object_id = "00".repeat(32);
        let response = test_app(&tempdir)
            .router
            .oneshot(authorized_get_request(&format!(
                "/v1/objects/{object_id}/referrers"
            )))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: ReferrersResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body.object_id, object_id);
        assert!(body.objects.is_empty());
    }

    #[tokio::test]
    async fn referrers_returns_objects_referencing_target() {
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
                    "payload_base64": STANDARD.encode(b"target"),
                    "tags": []
                }),
            ))
            .await
            .unwrap();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let target: PublishObjectResponse = serde_json::from_slice(&bytes).unwrap();

        let response = test_app
            .router
            .clone()
            .oneshot(authorized_json_request(
                "/v1/objects",
                serde_json::json!({
                    "object_type": "insight",
                    "mime_type": "text/plain",
                    "payload_base64": STANDARD.encode(b"source"),
                    "tags": ["linked"],
                    "references": [target.object_id]
                }),
            ))
            .await
            .unwrap();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let source: PublishObjectResponse = serde_json::from_slice(&bytes).unwrap();

        let response = test_app
            .router
            .oneshot(authorized_get_request(&format!(
                "/v1/objects/{}/referrers",
                target.object_id
            )))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: ReferrersResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body.object_id, target.object_id);
        assert_eq!(body.objects.len(), 1);
        assert_eq!(body.objects[0].object_id, source.object_id);
        assert_eq!(body.objects[0].object_type, "insight");
        assert_eq!(body.objects[0].mime_type, "text/plain");
        assert_eq!(body.objects[0].payload_size, 6);
    }

    #[tokio::test]
    async fn get_inline_object_roundtrips_payload() {
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
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let published: PublishObjectResponse = serde_json::from_slice(&bytes).unwrap();

        let response = test_app
            .router
            .oneshot(authorized_get_request(&format!(
                "/v1/objects/{}",
                published.object_id
            )))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: GetObjectResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body.object_id, published.object_id);
        assert_eq!(body.object_type, "fact");
        assert!(!body.author_agent_id.is_empty());
        assert_eq!(body.created_at_ms, 1_700_000_000_000);
        assert_eq!(body.mime_type, "text/plain");
        assert_eq!(body.tags, vec!["rust"]);
        assert!(body.references.is_empty());
        assert_eq!(STANDARD.decode(body.payload_base64).unwrap(), b"hello");
        assert!(body.verified);
    }

    #[tokio::test]
    async fn publish_object_accepts_references() {
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
                    "payload_base64": STANDARD.encode(b"parent"),
                    "tags": []
                }),
            ))
            .await
            .unwrap();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let parent: PublishObjectResponse = serde_json::from_slice(&bytes).unwrap();

        let response = test_app
            .router
            .clone()
            .oneshot(authorized_json_request(
                "/v1/objects",
                serde_json::json!({
                    "object_type": "insight",
                    "mime_type": "text/plain",
                    "payload_base64": STANDARD.encode(b"child"),
                    "tags": ["linked"],
                    "references": [parent.object_id]
                }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let child: PublishObjectResponse = serde_json::from_slice(&bytes).unwrap();

        let response = test_app
            .router
            .oneshot(authorized_get_request(&format!(
                "/v1/objects/{}",
                child.object_id
            )))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: GetObjectResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body.object_id, child.object_id);
        assert_eq!(body.object_type, "insight");
        assert_eq!(body.references, vec![parent.object_id]);
        assert!(body.verified);
    }

    #[tokio::test]
    async fn get_chunked_object_roundtrips_payload() {
        let tempdir = tempfile::tempdir().unwrap();
        let test_app = test_app(&tempdir);
        let payload = vec![7_u8; hivemind_core::INLINE_OBJECT_THRESHOLD + 1];
        let response = test_app
            .router
            .clone()
            .oneshot(authorized_json_request(
                "/v1/objects",
                serde_json::json!({
                    "object_type": "fact",
                    "mime_type": "application/octet-stream",
                    "payload_base64": STANDARD.encode(&payload),
                    "tags": []
                }),
            ))
            .await
            .unwrap();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let published: PublishObjectResponse = serde_json::from_slice(&bytes).unwrap();
        assert!(!published.chunk_ids.is_empty());

        let response = test_app
            .router
            .oneshot(authorized_get_request(&format!(
                "/v1/objects/{}",
                published.object_id
            )))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: GetObjectResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body.mime_type, "application/octet-stream");
        assert_eq!(STANDARD.decode(body.payload_base64).unwrap(), payload);
        assert!(body.verified);
    }

    #[tokio::test]
    async fn get_requires_auth() {
        let tempdir = tempfile::tempdir().unwrap();
        let response = test_app(&tempdir)
            .router
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v1/objects/not-an-id")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn get_invalid_object_id_returns_bad_request() {
        let tempdir = tempfile::tempdir().unwrap();
        let response = test_app(&tempdir)
            .router
            .oneshot(authorized_get_request("/v1/objects/not-an-id"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn get_missing_object_returns_not_found() {
        let tempdir = tempfile::tempdir().unwrap();
        let missing_id = "00".repeat(32);
        let response = test_app(&tempdir)
            .router
            .oneshot(authorized_get_request(&format!("/v1/objects/{missing_id}")))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
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
    async fn invalid_reference_returns_bad_request() {
        let tempdir = tempfile::tempdir().unwrap();
        let response = test_app(&tempdir)
            .router
            .oneshot(authorized_json_request(
                "/v1/objects",
                serde_json::json!({
                    "object_type": "fact",
                    "mime_type": "text/plain",
                    "payload_base64": STANDARD.encode(b"hello"),
                    "tags": [],
                    "references": ["not-an-object-id"]
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
