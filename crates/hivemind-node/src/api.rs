use axum::{
    body::Body,
    extract::{Path, Query, Request, State},
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
use hivemind_core::{ChunkId, ObjectEnvelope, ObjectId, ObjectKind, Payload};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::state::{
    AuditEvent, ConsumedInvite, NodeStateStoreError, SqliteNodeStateStore,
    CLIENT_TOKEN_SCOPE_MEMORY_IMPORT, CLIENT_TOKEN_SCOPE_MEMORY_READ,
    CLIENT_TOKEN_SCOPE_MEMORY_WRITE, DEFAULT_CLIENT_TOKEN_SCOPES, DEFAULT_CLIENT_TOKEN_TTL_MS,
};

#[derive(Clone, Debug)]
pub struct ApiConfig {
    pub admin_token: String,
    pub state_store: Arc<SqliteNodeStateStore>,
}

#[derive(Clone)]
pub struct AppState {
    pub identity: Arc<dyn IdentityPort>,
    pub clock: Arc<dyn ClockPort>,
    pub content_store: Arc<FsContentStore>,
    pub metadata_store: Arc<SqliteMetadataStore>,
    pub config: ApiConfig,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InviteRecord {
    pub node_url: String,
    pub expires_at_ms: u64,
    pub uses_remaining: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PeerRecord {
    pub node_url: String,
    pub node_id: String,
    pub trusted: bool,
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

#[derive(Debug, Deserialize)]
pub struct ImportObjectEnvelopeRequest {
    pub envelope_cbor_base64: String,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct ImportObjectEnvelopeResponse {
    pub object_id: String,
    pub chunk_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct PlanObjectEnvelopeImportRequest {
    pub envelope_cbor_base64: String,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct PlanObjectEnvelopeImportResponse {
    pub object_id: String,
    pub object_type: String,
    pub author_agent_id: String,
    pub created_at_ms: u64,
    pub mime_type: String,
    pub tags: Vec<String>,
    pub references: Vec<String>,
    pub payload_size: u64,
    pub chunk_count: u32,
    pub chunk_ids: Vec<String>,
    pub chunks: Vec<TransferChunk>,
    pub missing_chunk_ids: Vec<String>,
    pub already_stored: bool,
    pub importable: bool,
    pub verified: bool,
}

#[derive(Debug, Deserialize)]
pub struct PutChunkRequest {
    pub bytes_base64: String,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct PutChunkResponse {
    pub chunk_id: String,
    pub size: u64,
    pub verified: bool,
}

#[derive(Debug, Deserialize)]
pub struct CreateInviteRequest {
    pub node_url: String,
    #[serde(default)]
    pub ttl_seconds: Option<u64>,
    #[serde(default)]
    pub uses: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct CreateInviteResponse {
    pub invite_code: String,
    pub invite_url: String,
    pub node_url: String,
    pub expires_at_ms: u64,
    pub uses_remaining: u32,
}

#[derive(Debug, Deserialize)]
pub struct JoinInviteRequest {
    pub invite_code: String,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct JoinInviteResponse {
    pub node_url: String,
    pub api_token: String,
    pub api_token_expires_at_ms: u64,
    pub api_token_scope: String,
    pub peers: Vec<PeerSummary>,
}

#[derive(Debug, Deserialize)]
pub struct UpsertPeerRequest {
    pub node_url: String,
    pub node_id: String,
    #[serde(default)]
    pub trusted: bool,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct UpsertPeerResponse {
    pub peer: PeerSummary,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct PeerListResponse {
    pub peers: Vec<PeerSummary>,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct RevokeClientTokenResponse {
    pub revoked: bool,
}

#[derive(Debug, Deserialize)]
pub struct AuditLogQuery {
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct AuditLogResponse {
    pub events: Vec<AuditEvent>,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq, Clone)]
pub struct PeerSummary {
    pub node_url: String,
    pub node_id: String,
    pub trusted: bool,
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
pub struct GetChunkResponse {
    pub chunk_id: String,
    pub size: u64,
    pub bytes_base64: String,
    pub verified: bool,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct GetObjectEnvelopeResponse {
    pub object_id: String,
    pub object_type: String,
    pub author_agent_id: String,
    pub created_at_ms: u64,
    pub mime_type: String,
    pub tags: Vec<String>,
    pub references: Vec<String>,
    pub payload_size: u64,
    pub chunk_count: u32,
    pub envelope_cbor_base64: String,
    pub chunk_ids: Vec<String>,
    pub chunks: Vec<TransferChunk>,
    pub verified: bool,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct TransferChunk {
    pub index: u32,
    pub chunk_id: String,
    pub size: u32,
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

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct ErrorResponse {
    pub error: ErrorBody,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<ErrorDetails>,
}

#[derive(Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ErrorDetails {
    MissingChunks { chunk_ids: Vec<String> },
}

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("unauthorized")]
    Unauthorized,

    #[error("invalid object type")]
    InvalidObjectType,

    #[error("invalid object id")]
    InvalidObjectId,

    #[error("invalid chunk id")]
    InvalidChunkId,

    #[error("invalid chunk content")]
    InvalidChunkContent,

    #[error("invalid object envelope")]
    InvalidObjectEnvelope,

    #[error("invalid invite")]
    InvalidInvite,

    #[error("invalid client token")]
    InvalidClientToken,

    #[error("invite not found")]
    InviteNotFound,

    #[error("invite expired")]
    InviteExpired,

    #[error("object envelope references missing chunks")]
    MissingObjectChunks { chunk_ids: Vec<String> },

    #[error("stored content conflicts with expected content")]
    ContentConflict,

    #[error("object not found")]
    ObjectNotFound,

    #[error("chunk not found")]
    ChunkNotFound,

    #[error("invalid base64 payload")]
    InvalidBase64,

    #[error("application error: {0}")]
    App(String),

    #[error("metadata error: {0}")]
    Metadata(String),
}

impl ApiError {
    fn status(&self) -> StatusCode {
        match self {
            ApiError::Unauthorized => StatusCode::UNAUTHORIZED,
            ApiError::InvalidObjectType
            | ApiError::InvalidObjectId
            | ApiError::InvalidChunkId
            | ApiError::InvalidChunkContent
            | ApiError::InvalidObjectEnvelope
            | ApiError::InvalidInvite
            | ApiError::InvalidClientToken
            | ApiError::InvalidBase64 => StatusCode::BAD_REQUEST,
            ApiError::MissingObjectChunks { .. } | ApiError::ContentConflict => {
                StatusCode::CONFLICT
            }
            ApiError::ObjectNotFound | ApiError::ChunkNotFound | ApiError::InviteNotFound => {
                StatusCode::NOT_FOUND
            }
            ApiError::InviteExpired => StatusCode::GONE,
            ApiError::App(_) | ApiError::Metadata(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn code(&self) -> &'static str {
        match self {
            ApiError::Unauthorized => "unauthorized",
            ApiError::InvalidObjectType => "invalid_object_type",
            ApiError::InvalidObjectId => "invalid_object_id",
            ApiError::InvalidChunkId => "invalid_chunk_id",
            ApiError::InvalidChunkContent => "invalid_chunk_content",
            ApiError::InvalidObjectEnvelope => "invalid_object_envelope",
            ApiError::InvalidInvite => "invalid_invite",
            ApiError::InvalidClientToken => "invalid_client_token",
            ApiError::InviteNotFound => "invite_not_found",
            ApiError::InviteExpired => "invite_expired",
            ApiError::MissingObjectChunks { .. } => "missing_object_chunks",
            ApiError::ContentConflict => "content_conflict",
            ApiError::ObjectNotFound => "object_not_found",
            ApiError::ChunkNotFound => "chunk_not_found",
            ApiError::InvalidBase64 => "invalid_base64",
            ApiError::App(_) => "application_error",
            ApiError::Metadata(_) => "metadata_error",
        }
    }

    fn public_message(&self) -> String {
        match self {
            ApiError::App(_) | ApiError::Metadata(_) => "internal server error".to_owned(),
            other => other.to_string(),
        }
    }

    fn details(&self) -> Option<ErrorDetails> {
        match self {
            ApiError::MissingObjectChunks { chunk_ids } => Some(ErrorDetails::MissingChunks {
                chunk_ids: chunk_ids.clone(),
            }),
            _ => None,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.status();
        let body = ErrorResponse {
            error: ErrorBody {
                code: self.code().to_owned(),
                message: self.public_message(),
                details: self.details(),
            },
        };
        (status, Json(body)).into_response()
    }
}

pub fn app(state: AppState) -> Router {
    let protected_routes = Router::new()
        .route(
            "/v1/objects",
            post(publish_object).route_layer(middleware::from_fn_with_state(
                AuthzState::new(state.clone(), CLIENT_TOKEN_SCOPE_MEMORY_WRITE),
                require_any_auth,
            )),
        )
        .route(
            "/v1/objects/envelope",
            post(import_object_envelope).route_layer(middleware::from_fn_with_state(
                AuthzState::new(state.clone(), CLIENT_TOKEN_SCOPE_MEMORY_IMPORT),
                require_any_auth,
            )),
        )
        .route(
            "/v1/objects/envelope/plan",
            post(plan_object_envelope_import).route_layer(middleware::from_fn_with_state(
                AuthzState::new(state.clone(), CLIENT_TOKEN_SCOPE_MEMORY_IMPORT),
                require_any_auth,
            )),
        )
        .route(
            "/v1/objects/{object_id}",
            get(get_object).route_layer(middleware::from_fn_with_state(
                AuthzState::new(state.clone(), CLIENT_TOKEN_SCOPE_MEMORY_READ),
                require_any_auth,
            )),
        )
        .route(
            "/v1/objects/{object_id}/envelope",
            get(get_object_envelope).route_layer(middleware::from_fn_with_state(
                AuthzState::new(state.clone(), CLIENT_TOKEN_SCOPE_MEMORY_READ),
                require_any_auth,
            )),
        )
        .route(
            "/v1/objects/{object_id}/referrers",
            get(get_referrers).route_layer(middleware::from_fn_with_state(
                AuthzState::new(state.clone(), CLIENT_TOKEN_SCOPE_MEMORY_READ),
                require_any_auth,
            )),
        )
        .route(
            "/v1/chunks/{chunk_id}",
            get(get_chunk)
                .route_layer(middleware::from_fn_with_state(
                    AuthzState::new(state.clone(), CLIENT_TOKEN_SCOPE_MEMORY_READ),
                    require_any_auth,
                ))
                .put(put_chunk)
                .layer(middleware::from_fn_with_state(
                    AuthzState::new(state.clone(), CLIENT_TOKEN_SCOPE_MEMORY_IMPORT),
                    require_any_auth,
                )),
        )
        .route(
            "/v1/tags/{tag}",
            get(get_tag).route_layer(middleware::from_fn_with_state(
                AuthzState::new(state.clone(), CLIENT_TOKEN_SCOPE_MEMORY_READ),
                require_any_auth,
            )),
        );

    let admin_routes = Router::new()
        .route("/v1/audit", get(get_audit_log))
        .route("/v1/invites", post(create_invite))
        .route("/v1/peers", get(get_peers).post(upsert_peer))
        .route(
            "/v1/client-tokens/{token}/revoke",
            post(revoke_client_token),
        )
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_admin_auth,
        ));

    Router::new()
        .route("/health", get(health))
        .route("/v1/join", post(join_invite))
        .merge(protected_routes)
        .merge(admin_routes)
        .with_state(state)
}

async fn health() -> &'static str {
    "ok"
}

async fn require_admin_auth(
    State(state): State<AppState>,
    headers: HeaderMap,
    request: Request<Body>,
    next: Next,
) -> Result<Response, ApiError> {
    if !has_admin_auth(&state.config, &headers) {
        return Err(ApiError::Unauthorized);
    }

    Ok(next.run(request).await)
}

#[derive(Clone)]
struct AuthzState {
    app: AppState,
    required_scope: &'static str,
}

impl AuthzState {
    fn new(app: AppState, required_scope: &'static str) -> Self {
        Self {
            app,
            required_scope,
        }
    }
}

async fn require_any_auth(
    State(authz): State<AuthzState>,
    headers: HeaderMap,
    request: Request<Body>,
    next: Next,
) -> Result<Response, ApiError> {
    if !has_admin_auth(&authz.app.config, &headers) && !has_client_auth(&authz, &headers).await? {
        return Err(ApiError::Unauthorized);
    }

    Ok(next.run(request).await)
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
}

fn has_admin_auth(config: &ApiConfig, headers: &HeaderMap) -> bool {
    bearer_token(headers).is_some_and(|token| token == config.admin_token)
}

async fn has_client_auth(authz: &AuthzState, headers: &HeaderMap) -> Result<bool, ApiError> {
    let Some(token) = bearer_token(headers) else {
        return Ok(false);
    };
    let now_ms = authz.app.clock.now_ms().await.map_err(app_error)?;
    authz
        .app
        .config
        .state_store
        .has_client_token(token, now_ms, authz.required_scope)
        .map_err(state_error)
}

async fn get_peers(State(state): State<AppState>) -> Result<Json<PeerListResponse>, ApiError> {
    Ok(Json(PeerListResponse {
        peers: peer_summaries(&state, true)?,
    }))
}

async fn get_audit_log(
    State(state): State<AppState>,
    Query(query): Query<AuditLogQuery>,
) -> Result<Json<AuditLogResponse>, ApiError> {
    let events = state
        .config
        .state_store
        .audit_events(query.limit.unwrap_or(100))
        .map_err(state_error)?;
    Ok(Json(AuditLogResponse { events }))
}

async fn revoke_client_token(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> Result<Json<RevokeClientTokenResponse>, ApiError> {
    let token = validate_client_token(token)?;
    let now_ms = state.clock.now_ms().await.map_err(app_error)?;
    let revoked = state
        .config
        .state_store
        .revoke_client_token(&token, now_ms)
        .map_err(state_error)?;
    record_audit_event(
        &state,
        now_ms,
        "client_token_revoked",
        Some(&redacted_secret("token", &token)),
        &format!("revoked={revoked}"),
    )?;
    Ok(Json(RevokeClientTokenResponse { revoked }))
}

async fn upsert_peer(
    State(state): State<AppState>,
    Json(request): Json<UpsertPeerRequest>,
) -> Result<Json<UpsertPeerResponse>, ApiError> {
    let node_url = validate_node_url(request.node_url)?;
    let node_id = validate_node_id(request.node_id)?;
    let peer = PeerRecord {
        node_url: node_url.clone(),
        node_id,
        trusted: request.trusted,
    };
    state
        .config
        .state_store
        .upsert_peer(&peer)
        .map_err(state_error)?;
    let now_ms = state.clock.now_ms().await.map_err(app_error)?;
    record_audit_event(
        &state,
        now_ms,
        "peer_upserted",
        Some(&peer.node_id),
        &format!("node_url={} trusted={}", peer.node_url, peer.trusted),
    )?;

    Ok(Json(UpsertPeerResponse {
        peer: PeerSummary {
            node_url: peer.node_url,
            node_id: peer.node_id,
            trusted: peer.trusted,
        },
    }))
}

async fn create_invite(
    State(state): State<AppState>,
    Json(request): Json<CreateInviteRequest>,
) -> Result<Json<CreateInviteResponse>, ApiError> {
    let node_url = validate_node_url(request.node_url)?;
    let ttl_seconds = request
        .ttl_seconds
        .unwrap_or(24 * 60 * 60)
        .min(7 * 24 * 60 * 60);
    let uses_remaining = request.uses.unwrap_or(1).clamp(1, 100);
    let now_ms = state.clock.now_ms().await.map_err(app_error)?;
    let expires_at_ms = now_ms.saturating_add(ttl_seconds.saturating_mul(1000));
    let invite_code = generate_invite_code()?;
    let invite_url = format!(
        "hive://join?node={}&invite={}",
        percent_encode_query_value(&node_url),
        percent_encode_query_value(&invite_code)
    );

    state
        .config
        .state_store
        .insert_invite(
            &invite_code,
            &InviteRecord {
                node_url: node_url.clone(),
                expires_at_ms,
                uses_remaining,
            },
        )
        .map_err(state_error)?;
    record_audit_event(
        &state,
        now_ms,
        "invite_created",
        Some(&redacted_secret("invite", &invite_code)),
        &format!("node_url={node_url} expires_at_ms={expires_at_ms} uses={uses_remaining}"),
    )?;

    Ok(Json(CreateInviteResponse {
        invite_code,
        invite_url,
        node_url,
        expires_at_ms,
        uses_remaining,
    }))
}

async fn join_invite(
    State(state): State<AppState>,
    Json(request): Json<JoinInviteRequest>,
) -> Result<Json<JoinInviteResponse>, ApiError> {
    let invite_code = normalize_invite_code(&request.invite_code)?;
    let now_ms = state.clock.now_ms().await.map_err(app_error)?;
    let api_token = generate_token()?;
    let api_token_expires_at_ms = now_ms.saturating_add(DEFAULT_CLIENT_TOKEN_TTL_MS);
    let node_url = match state
        .config
        .state_store
        .exchange_invite_for_client_token(
            &invite_code,
            now_ms,
            &api_token,
            api_token_expires_at_ms,
            DEFAULT_CLIENT_TOKEN_SCOPES,
        )
        .map_err(state_error)?
    {
        ConsumedInvite::Active { node_url } => node_url,
        ConsumedInvite::Expired => return Err(ApiError::InviteExpired),
        ConsumedInvite::NotFound => return Err(ApiError::InviteNotFound),
    };

    record_audit_event(
        &state,
        now_ms,
        "join_exchanged",
        Some(&redacted_secret("invite", &invite_code)),
        &format!(
            "node_url={node_url} token={} token_expires_at_ms={api_token_expires_at_ms} scopes={DEFAULT_CLIENT_TOKEN_SCOPES}",
            redacted_secret("token", &api_token)
        ),
    )?;
    let peers = peer_summaries(&state, false)?;

    Ok(Json(JoinInviteResponse {
        node_url,
        api_token,
        api_token_expires_at_ms,
        api_token_scope: DEFAULT_CLIENT_TOKEN_SCOPES.to_owned(),
        peers,
    }))
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

async fn import_object_envelope(
    State(state): State<AppState>,
    Json(request): Json<ImportObjectEnvelopeRequest>,
) -> Result<Json<ImportObjectEnvelopeResponse>, ApiError> {
    let envelope = parse_verified_envelope_base64(&request.envelope_cbor_base64)?;
    ensure_chunks_available(&state.content_store, &envelope.body.payload).await?;

    state
        .content_store
        .put_object_envelope(&envelope)
        .await
        .map_err(fs_write_error)?;
    let chunks = stored_chunks_from_payload(&state.content_store, &envelope.body.payload);
    let received_at_ms = state.clock.now_ms().await.map_err(app_error)?;
    state
        .metadata_store
        .record_object(
            &envelope,
            state.content_store.object_path(envelope.object_id),
            &chunks,
            received_at_ms,
        )
        .map_err(|err| ApiError::Metadata(err.to_string()))?;

    Ok(Json(ImportObjectEnvelopeResponse {
        object_id: envelope.object_id.to_string(),
        chunk_ids: chunk_ids_from_payload(&envelope.body.payload),
    }))
}

async fn plan_object_envelope_import(
    State(state): State<AppState>,
    Json(request): Json<PlanObjectEnvelopeImportRequest>,
) -> Result<Json<PlanObjectEnvelopeImportResponse>, ApiError> {
    let envelope = parse_verified_envelope_base64(&request.envelope_cbor_base64)?;
    let missing_chunk_ids =
        missing_chunk_ids_for_payload(&state.content_store, &envelope.body.payload).await?;
    let already_stored = state
        .content_store
        .has_object(envelope.object_id)
        .await
        .map_err(|err| ApiError::App(err.to_string()))?;
    let (mime_type, payload_size, chunk_count) = payload_metadata(&envelope.body.payload);
    let importable = missing_chunk_ids.is_empty();

    Ok(Json(PlanObjectEnvelopeImportResponse {
        object_id: envelope.object_id.to_string(),
        object_type: object_kind_to_str(envelope.body.kind).to_owned(),
        author_agent_id: envelope.body.author.to_string(),
        created_at_ms: envelope.body.created_at_ms,
        mime_type,
        tags: envelope.body.tags.clone(),
        references: envelope
            .body
            .references
            .iter()
            .map(|object_id| object_id.to_string())
            .collect(),
        payload_size,
        chunk_count,
        chunk_ids: chunk_ids_from_payload(&envelope.body.payload),
        chunks: transfer_chunks_from_payload(&envelope.body.payload),
        missing_chunk_ids,
        already_stored,
        importable,
        verified: true,
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

async fn get_object_envelope(
    State(state): State<AppState>,
    Path(object_id): Path<String>,
) -> Result<Json<GetObjectEnvelopeResponse>, ApiError> {
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
    let mut envelope_cbor = Vec::new();
    minicbor::encode(&envelope, &mut envelope_cbor)
        .map_err(|_| ApiError::App("failed to encode object envelope".to_owned()))?;

    let (mime_type, payload_size, chunk_count) = payload_metadata(&envelope.body.payload);

    Ok(Json(GetObjectEnvelopeResponse {
        object_id: object_id.to_string(),
        object_type: object_kind_to_str(envelope.body.kind).to_owned(),
        author_agent_id: envelope.body.author.to_string(),
        created_at_ms: envelope.body.created_at_ms,
        mime_type,
        tags: envelope.body.tags.clone(),
        references: envelope
            .body
            .references
            .iter()
            .map(|object_id| object_id.to_string())
            .collect(),
        payload_size,
        chunk_count,
        envelope_cbor_base64: STANDARD.encode(envelope_cbor),
        chunk_ids: chunk_ids_from_payload(&envelope.body.payload),
        chunks: transfer_chunks_from_payload(&envelope.body.payload),
        verified: true,
    }))
}

async fn put_chunk(
    State(state): State<AppState>,
    Path(chunk_id): Path<String>,
    Json(request): Json<PutChunkRequest>,
) -> Result<Json<PutChunkResponse>, ApiError> {
    let chunk_id = chunk_id
        .parse::<ChunkId>()
        .map_err(|_| ApiError::InvalidChunkId)?;
    let bytes = STANDARD
        .decode(request.bytes_base64.as_bytes())
        .map_err(|_| ApiError::InvalidBase64)?;
    state
        .content_store
        .put_chunk_bytes(chunk_id, &bytes)
        .await
        .map_err(fs_write_error)?;

    Ok(Json(PutChunkResponse {
        chunk_id: chunk_id.to_string(),
        size: bytes.len() as u64,
        verified: true,
    }))
}

async fn get_chunk(
    State(state): State<AppState>,
    Path(chunk_id): Path<String>,
) -> Result<Json<GetChunkResponse>, ApiError> {
    let chunk_id = chunk_id
        .parse::<ChunkId>()
        .map_err(|_| ApiError::InvalidChunkId)?;
    let bytes = state
        .content_store
        .get_chunk(chunk_id)
        .await
        .map_err(|err| match err {
            hivemind_adapters::fs::FsStoreError::Io(io_err)
                if io_err.kind() == std::io::ErrorKind::NotFound =>
            {
                ApiError::ChunkNotFound
            }
            other => ApiError::App(other.to_string()),
        })?;
    Ok(Json(GetChunkResponse {
        chunk_id: chunk_id.to_string(),
        size: bytes.len() as u64,
        bytes_base64: STANDARD.encode(bytes),
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

async fn ensure_chunks_available(
    store: &FsContentStore,
    payload: &Payload,
) -> Result<(), ApiError> {
    let missing_chunk_ids = missing_chunk_ids_for_payload(store, payload).await?;
    if !missing_chunk_ids.is_empty() {
        return Err(ApiError::MissingObjectChunks {
            chunk_ids: missing_chunk_ids,
        });
    }
    Ok(())
}

async fn missing_chunk_ids_for_payload(
    store: &FsContentStore,
    payload: &Payload,
) -> Result<Vec<String>, ApiError> {
    let mut missing_chunk_ids = Vec::new();
    if let Payload::Chunked(chunked) = payload {
        for chunk in &chunked.chunks {
            let bytes = match store.get_chunk(chunk.chunk_id).await {
                Ok(bytes) => bytes,
                Err(hivemind_adapters::fs::FsStoreError::Io(io_err))
                    if io_err.kind() == std::io::ErrorKind::NotFound =>
                {
                    missing_chunk_ids.push(chunk.chunk_id.to_string());
                    continue;
                }
                Err(hivemind_adapters::fs::FsStoreError::ObjectVerification(_)) => {
                    return Err(ApiError::InvalidChunkContent);
                }
                Err(other) => return Err(ApiError::App(other.to_string())),
            };
            if bytes.len() != chunk.size as usize {
                return Err(ApiError::InvalidChunkContent);
            }
        }
    }
    Ok(missing_chunk_ids)
}

fn parse_verified_envelope_base64(envelope_cbor_base64: &str) -> Result<ObjectEnvelope, ApiError> {
    let envelope_bytes = STANDARD
        .decode(envelope_cbor_base64.as_bytes())
        .map_err(|_| ApiError::InvalidBase64)?;
    let envelope: ObjectEnvelope =
        minicbor::decode(&envelope_bytes).map_err(|_| ApiError::InvalidObjectEnvelope)?;
    envelope
        .verify()
        .map_err(|_| ApiError::InvalidObjectEnvelope)?;
    Ok(envelope)
}

fn fs_write_error(err: hivemind_adapters::fs::FsStoreError) -> ApiError {
    match err {
        hivemind_adapters::fs::FsStoreError::ObjectVerification(_) => ApiError::InvalidChunkContent,
        hivemind_adapters::fs::FsStoreError::ContentMismatch => ApiError::ContentConflict,
        other => ApiError::App(other.to_string()),
    }
}

fn payload_metadata(payload: &Payload) -> (String, u64, u32) {
    match payload {
        Payload::Inline(inline) => (inline.mime_type.clone(), inline.bytes.len() as u64, 0),
        Payload::Chunked(chunked) => (
            chunked.mime_type.clone(),
            chunked.total_size,
            chunked.chunks.len() as u32,
        ),
    }
}

fn chunk_ids_from_payload(payload: &Payload) -> Vec<String> {
    match payload {
        Payload::Inline(_) => Vec::new(),
        Payload::Chunked(chunked) => chunked
            .chunks
            .iter()
            .map(|chunk| chunk.chunk_id.to_string())
            .collect(),
    }
}

fn transfer_chunks_from_payload(payload: &Payload) -> Vec<TransferChunk> {
    match payload {
        Payload::Inline(_) => Vec::new(),
        Payload::Chunked(chunked) => chunked
            .chunks
            .iter()
            .map(|chunk| TransferChunk {
                index: chunk.index,
                chunk_id: chunk.chunk_id.to_string(),
                size: chunk.size,
            })
            .collect(),
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

fn validate_node_url(node_url: String) -> Result<String, ApiError> {
    let node_url = node_url.trim().trim_end_matches('/').to_owned();
    let uri = node_url
        .parse::<axum::http::Uri>()
        .map_err(|_| ApiError::InvalidInvite)?;
    let valid_scheme = matches!(uri.scheme_str(), Some("http" | "https"));
    if node_url.is_empty()
        || node_url.chars().any(char::is_whitespace)
        || !valid_scheme
        || uri.authority().is_none()
    {
        return Err(ApiError::InvalidInvite);
    }
    Ok(node_url)
}

fn validate_node_id(node_id: String) -> Result<String, ApiError> {
    let node_id = node_id.trim().to_owned();
    if node_id.len() != 64 || !node_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ApiError::InvalidInvite);
    }
    Ok(node_id)
}

fn validate_client_token(token: String) -> Result<String, ApiError> {
    let token = token.trim().to_owned();
    if token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ApiError::InvalidClientToken);
    }
    Ok(token)
}

fn peer_summaries(state: &AppState, include_trust: bool) -> Result<Vec<PeerSummary>, ApiError> {
    state
        .config
        .state_store
        .peer_summaries(include_trust)
        .map_err(state_error)
}

fn normalize_invite_code(invite_code: &str) -> Result<String, ApiError> {
    let invite_code = invite_code.trim().to_ascii_uppercase();
    if invite_code.is_empty()
        || invite_code.len() > 64
        || !invite_code
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(ApiError::InvalidInvite);
    }
    Ok(invite_code)
}

fn generate_invite_code() -> Result<String, ApiError> {
    let mut bytes = [0_u8; 12];
    getrandom::getrandom(&mut bytes).map_err(|err| ApiError::App(err.to_string()))?;
    let encoded = hex::encode_upper(bytes);
    Ok(format!(
        "{}-{}-{}",
        &encoded[0..8],
        &encoded[8..16],
        &encoded[16..24]
    ))
}

fn generate_token() -> Result<String, ApiError> {
    let mut bytes = [0_u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|err| ApiError::App(err.to_string()))?;
    Ok(hex::encode(bytes))
}

fn percent_encode_query_value(value: &str) -> String {
    let mut output = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                output.push(byte as char);
            }
            _ => output.push_str(&format!("%{byte:02X}")),
        }
    }
    output
}

fn app_error(err: hivemind_app::AppError) -> ApiError {
    ApiError::App(err.to_string())
}

fn state_error(err: NodeStateStoreError) -> ApiError {
    ApiError::App(err.to_string())
}

fn record_audit_event(
    state: &AppState,
    created_at_ms: u64,
    event_type: &str,
    subject: Option<&str>,
    detail: &str,
) -> Result<(), ApiError> {
    state
        .config
        .state_store
        .record_audit_event(created_at_ms, event_type, subject, detail)
        .map_err(state_error)
}

fn redacted_secret(prefix: &str, secret: &str) -> String {
    let suffix = secret
        .chars()
        .rev()
        .take(8)
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("{prefix}:...{suffix}")
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
        test_app_with_state_store(
            tempdir,
            Arc::new(SqliteNodeStateStore::in_memory().unwrap()),
        )
    }

    fn test_app_with_state_store(
        tempdir: &tempfile::TempDir,
        state_store: Arc<SqliteNodeStateStore>,
    ) -> TestApp {
        let content_store = Arc::new(FsContentStore::new(tempdir.path()));
        let metadata_store = Arc::new(SqliteMetadataStore::in_memory().unwrap());
        let state = AppState {
            identity: Arc::new(DevIdentity::from_seed([1_u8; 32])),
            clock: Arc::new(TestClock),
            content_store: Arc::clone(&content_store),
            metadata_store: Arc::clone(&metadata_store),
            config: ApiConfig {
                admin_token: "secret".to_owned(),
                state_store,
            },
        };
        TestApp {
            router: app(state),
            content_store,
            metadata_store,
        }
    }

    fn authorized_get_request(path: &str) -> Request<Body> {
        authorized_get_request_with_token(path, "secret")
    }

    fn authorized_get_request_with_token(path: &str, token: &str) -> Request<Body> {
        Request::builder()
            .method(Method::GET)
            .uri(path)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
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

    fn public_json_request(path: &str, body: serde_json::Value) -> Request<Body> {
        Request::builder()
            .method(Method::POST)
            .uri(path)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn authorized_put_json_request(path: &str, body: serde_json::Value) -> Request<Body> {
        Request::builder()
            .method(Method::PUT)
            .uri(path)
            .header(header::AUTHORIZATION, "Bearer secret")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    async fn error_response(response: Response) -> ErrorResponse {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn internal_errors_return_generic_json_message() {
        let response = ApiError::App("secret database path leaked".to_owned()).into_response();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = error_response(response).await;
        assert_eq!(body.error.code, "application_error");
        assert_eq!(body.error.message, "internal server error");
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
    async fn create_invite_requires_auth() {
        let tempdir = tempfile::tempdir().unwrap();
        let response = test_app(&tempdir)
            .router
            .oneshot(public_json_request(
                "/v1/invites",
                serde_json::json!({"node_url": "https://hive.example.internal"}),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn peers_require_auth_and_can_be_listed() {
        let tempdir = tempfile::tempdir().unwrap();
        let router = test_app(&tempdir).router;

        let response = router
            .clone()
            .oneshot(public_json_request(
                "/v1/peers",
                serde_json::json!({"node_url": "https://node-b.internal", "node_id": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let response = router
            .clone()
            .oneshot(authorized_json_request(
                "/v1/peers",
                serde_json::json!({"node_url": "https://node-b.internal", "node_id": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "trusted": true}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = router
            .oneshot(authorized_get_request("/v1/peers"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: PeerListResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            body.peers,
            vec![PeerSummary {
                node_url: "https://node-b.internal".to_owned(),
                node_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    .to_owned(),
                trusted: true,
            }]
        );
    }

    #[tokio::test]
    async fn create_invite_and_join_exchanges_for_token_once() {
        let tempdir = tempfile::tempdir().unwrap();
        let router = test_app(&tempdir).router;
        let response = router
            .clone()
            .oneshot(authorized_json_request(
                "/v1/peers",
                serde_json::json!({"node_url": "https://node-b.internal", "node_id": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "trusted": true}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = router
            .clone()
            .oneshot(authorized_json_request(
                "/v1/invites",
                serde_json::json!({
                    "node_url": "https://hive.example.internal/",
                    "ttl_seconds": 3600,
                    "uses": 1
                }),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let invite: CreateInviteResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(invite.node_url, "https://hive.example.internal");
        assert_eq!(invite.uses_remaining, 1);
        assert!(invite.invite_url.starts_with("hive://join?node="));

        let response = router
            .clone()
            .oneshot(public_json_request(
                "/v1/join",
                serde_json::json!({"invite_code": invite.invite_code.clone()}),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let joined: JoinInviteResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(joined.node_url, "https://hive.example.internal");
        assert_ne!(joined.api_token, "secret");
        assert_eq!(joined.api_token_scope, DEFAULT_CLIENT_TOKEN_SCOPES);
        assert_eq!(
            joined.api_token_expires_at_ms,
            1_700_000_000_000 + DEFAULT_CLIENT_TOKEN_TTL_MS
        );
        assert_eq!(
            joined.peers,
            vec![PeerSummary {
                node_url: "https://node-b.internal".to_owned(),
                node_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    .to_owned(),
                trusted: false,
            }]
        );

        let response = router
            .clone()
            .oneshot(authorized_get_request_with_token(
                "/v1/tags/unknown",
                &joined.api_token,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v1/invites")
                    .header(
                        header::AUTHORIZATION,
                        format!("Bearer {}", joined.api_token),
                    )
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({"node_url": "https://other.internal"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let response = router
            .oneshot(public_json_request(
                "/v1/join",
                serde_json::json!({"invite_code": invite.invite_code}),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn audit_log_records_admin_security_actions() {
        let tempdir = tempfile::tempdir().unwrap();
        let router = test_app(&tempdir).router;

        let response = router
            .clone()
            .oneshot(authorized_json_request(
                "/v1/peers",
                serde_json::json!({"node_url": "https://node-b.internal", "node_id": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "trusted": true}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = router
            .clone()
            .oneshot(authorized_json_request(
                "/v1/invites",
                serde_json::json!({"node_url": "https://hive.example.internal"}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let invite: CreateInviteResponse = serde_json::from_slice(&bytes).unwrap();

        let response = router
            .clone()
            .oneshot(public_json_request(
                "/v1/join",
                serde_json::json!({"invite_code": invite.invite_code}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let joined: JoinInviteResponse = serde_json::from_slice(&bytes).unwrap();

        let response = router
            .clone()
            .oneshot(authorized_json_request(
                &format!("/v1/client-tokens/{}/revoke", joined.api_token),
                serde_json::json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = router
            .oneshot(authorized_get_request("/v1/audit?limit=10"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: AuditLogResponse = serde_json::from_slice(&bytes).unwrap();
        let event_types = body
            .events
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            event_types,
            vec![
                "client_token_revoked",
                "join_exchanged",
                "invite_created",
                "peer_upserted",
            ]
        );
        assert!(body.events.iter().any(|event| event
            .subject
            .as_deref()
            .is_some_and(|subject| subject.starts_with("token:..."))));
        assert!(body.events.iter().any(|event| event
            .subject
            .as_deref()
            .is_some_and(|subject| subject.starts_with("invite:..."))));
    }

    #[tokio::test]
    async fn joined_client_token_persists_after_state_reload() {
        let tempdir = tempfile::tempdir().unwrap();
        let state_path = tempdir.path().join("state.sqlite3");
        let router = test_app_with_state_store(
            &tempdir,
            Arc::new(SqliteNodeStateStore::open(&state_path).unwrap()),
        )
        .router;

        let response = router
            .clone()
            .oneshot(authorized_json_request(
                "/v1/invites",
                serde_json::json!({"node_url": "https://hive.example.internal"}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let invite: CreateInviteResponse = serde_json::from_slice(&bytes).unwrap();

        let response = router
            .oneshot(public_json_request(
                "/v1/join",
                serde_json::json!({"invite_code": invite.invite_code}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let joined: JoinInviteResponse = serde_json::from_slice(&bytes).unwrap();

        let reloaded_router = test_app_with_state_store(
            &tempdir,
            Arc::new(SqliteNodeStateStore::open(&state_path).unwrap()),
        )
        .router;
        let response = reloaded_router
            .oneshot(authorized_get_request_with_token(
                "/v1/tags/unknown",
                &joined.api_token,
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn admin_can_revoke_joined_client_token() {
        let tempdir = tempfile::tempdir().unwrap();
        let router = test_app(&tempdir).router;

        let response = router
            .clone()
            .oneshot(authorized_json_request(
                "/v1/invites",
                serde_json::json!({"node_url": "https://hive.example.internal"}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let invite: CreateInviteResponse = serde_json::from_slice(&bytes).unwrap();

        let response = router
            .clone()
            .oneshot(public_json_request(
                "/v1/join",
                serde_json::json!({"invite_code": invite.invite_code}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let joined: JoinInviteResponse = serde_json::from_slice(&bytes).unwrap();

        let response = router
            .clone()
            .oneshot(authorized_get_request_with_token(
                "/v1/tags/unknown",
                &joined.api_token,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = router
            .clone()
            .oneshot(authorized_json_request(
                &format!("/v1/client-tokens/{}/revoke", joined.api_token),
                serde_json::json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let revoked: RevokeClientTokenResponse = serde_json::from_slice(&bytes).unwrap();
        assert!(revoked.revoked);

        let response = router
            .oneshot(authorized_get_request_with_token(
                "/v1/tags/unknown",
                &joined.api_token,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn read_only_client_token_can_read_but_not_write_or_import() {
        let tempdir = tempfile::tempdir().unwrap();
        let state_store = Arc::new(SqliteNodeStateStore::in_memory().unwrap());
        let token = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        state_store
            .insert_client_token(token, 1, 2_000_000_000_000, CLIENT_TOKEN_SCOPE_MEMORY_READ)
            .unwrap();
        let router = test_app_with_state_store(&tempdir, state_store).router;

        let response = router
            .clone()
            .oneshot(authorized_get_request_with_token("/v1/tags/unknown", token))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v1/objects")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "object_type": "fact",
                            "mime_type": "text/plain",
                            "payload_base64": STANDARD.encode(b"hello"),
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let response = router
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v1/objects/envelope/plan")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({"envelope_cbor_base64": "invalid"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn legacy_memory_scope_can_read_write_and_import() {
        let tempdir = tempfile::tempdir().unwrap();
        let state_store = Arc::new(SqliteNodeStateStore::in_memory().unwrap());
        let token = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        state_store
            .insert_client_token(token, 1, 2_000_000_000_000, "memory")
            .unwrap();
        let router = test_app_with_state_store(&tempdir, state_store).router;

        let response = router
            .clone()
            .oneshot(authorized_get_request_with_token("/v1/tags/unknown", token))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v1/objects")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "object_type": "fact",
                            "mime_type": "text/plain",
                            "payload_base64": STANDARD.encode(b"hello"),
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = router
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v1/objects/envelope/plan")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({"envelope_cbor_base64": "invalid"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn expired_client_token_is_unauthorized() {
        let tempdir = tempfile::tempdir().unwrap();
        let state_store = Arc::new(SqliteNodeStateStore::in_memory().unwrap());
        state_store
            .insert_client_token(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                1,
                2,
                CLIENT_TOKEN_SCOPE_MEMORY_READ,
            )
            .unwrap();
        let router = test_app_with_state_store(&tempdir, state_store).router;

        let response = router
            .oneshot(authorized_get_request_with_token(
                "/v1/tags/unknown",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn invite_use_persists_after_state_reload() {
        let tempdir = tempfile::tempdir().unwrap();
        let state_path = tempdir.path().join("state.sqlite3");
        let router = test_app_with_state_store(
            &tempdir,
            Arc::new(SqliteNodeStateStore::open(&state_path).unwrap()),
        )
        .router;

        let response = router
            .oneshot(authorized_json_request(
                "/v1/invites",
                serde_json::json!({"node_url": "https://hive.example.internal", "uses": 1}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let invite: CreateInviteResponse = serde_json::from_slice(&bytes).unwrap();

        let reloaded_router = test_app_with_state_store(
            &tempdir,
            Arc::new(SqliteNodeStateStore::open(&state_path).unwrap()),
        )
        .router;
        let response = reloaded_router
            .clone()
            .oneshot(public_json_request(
                "/v1/join",
                serde_json::json!({"invite_code": invite.invite_code.clone()}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let reloaded_router = test_app_with_state_store(
            &tempdir,
            Arc::new(SqliteNodeStateStore::open(&state_path).unwrap()),
        )
        .router;
        let response = reloaded_router
            .oneshot(public_json_request(
                "/v1/join",
                serde_json::json!({"invite_code": invite.invite_code}),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn peers_persist_after_state_reload() {
        let tempdir = tempfile::tempdir().unwrap();
        let state_path = tempdir.path().join("state.sqlite3");
        let router = test_app_with_state_store(
            &tempdir,
            Arc::new(SqliteNodeStateStore::open(&state_path).unwrap()),
        )
        .router;

        let response = router
            .oneshot(authorized_json_request(
                "/v1/peers",
                serde_json::json!({"node_url": "https://node-b.internal", "node_id": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "trusted": true}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let reloaded_router = test_app_with_state_store(
            &tempdir,
            Arc::new(SqliteNodeStateStore::open(&state_path).unwrap()),
        )
        .router;
        let response = reloaded_router
            .oneshot(authorized_get_request("/v1/peers"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: PeerListResponse = serde_json::from_slice(&bytes).unwrap();

        assert_eq!(
            body.peers,
            vec![PeerSummary {
                node_url: "https://node-b.internal".to_owned(),
                node_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    .to_owned(),
                trusted: true,
            }]
        );
    }

    #[tokio::test]
    async fn create_invite_rejects_invalid_node_url() {
        let tempdir = tempfile::tempdir().unwrap();
        let response = test_app(&tempdir)
            .router
            .oneshot(authorized_json_request(
                "/v1/invites",
                serde_json::json!({"node_url": "javascript:alert(1)"}),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = error_response(response).await;
        assert_eq!(body.error.code, "invalid_invite");
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
    async fn get_object_envelope_roundtrips_canonical_cbor() {
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
                "/v1/objects/{}/envelope",
                published.object_id
            )))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: GetObjectEnvelopeResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body.object_id, published.object_id);
        assert_eq!(body.object_type, "fact");
        assert!(!body.author_agent_id.is_empty());
        assert_eq!(body.mime_type, "text/plain");
        assert_eq!(body.tags, vec!["rust"]);
        assert!(body.references.is_empty());
        assert_eq!(body.payload_size, 5);
        assert_eq!(body.chunk_count, 0);
        assert!(body.chunk_ids.is_empty());
        assert!(body.chunks.is_empty());
        assert!(body.verified);
        let envelope_bytes = STANDARD.decode(body.envelope_cbor_base64).unwrap();
        let envelope: hivemind_core::ObjectEnvelope = minicbor::decode(&envelope_bytes).unwrap();
        envelope.verify().unwrap();
        assert_eq!(envelope.object_id.to_string(), published.object_id);
        assert_eq!(envelope.body.tags, vec!["rust"]);
    }

    #[tokio::test]
    async fn get_chunked_object_envelope_returns_chunk_ids() {
        let tempdir = tempfile::tempdir().unwrap();
        let test_app = test_app(&tempdir);
        let payload = vec![7_u8; hivemind_core::DEFAULT_CHUNK_SIZE + 1];
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
        assert!(published.chunk_ids.len() > 1);

        let response = test_app
            .router
            .oneshot(authorized_get_request(&format!(
                "/v1/objects/{}/envelope",
                published.object_id
            )))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: GetObjectEnvelopeResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body.object_id, published.object_id);
        assert_eq!(body.object_type, "fact");
        assert_eq!(body.mime_type, "application/octet-stream");
        assert_eq!(body.payload_size, payload.len() as u64);
        assert_eq!(body.chunk_count, published.chunk_ids.len() as u32);
        assert_eq!(body.chunk_ids, published.chunk_ids);
        assert_eq!(body.chunks.len(), published.chunk_ids.len());
        assert_eq!(body.chunks[0].index, 0);
        assert_eq!(body.chunks[0].chunk_id, published.chunk_ids[0]);
        assert_eq!(
            body.chunks[0].size,
            hivemind_core::DEFAULT_CHUNK_SIZE as u32
        );
        assert_eq!(body.chunks[1].index, 1);
        assert_eq!(body.chunks[1].chunk_id, published.chunk_ids[1]);
        assert_eq!(body.chunks[1].size, 1);
        assert!(body.verified);
    }

    #[tokio::test]
    async fn get_object_envelope_requires_auth() {
        let tempdir = tempfile::tempdir().unwrap();
        let response = test_app(&tempdir)
            .router
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v1/objects/not-an-id/envelope")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn get_object_envelope_invalid_object_id_returns_bad_request() {
        let tempdir = tempfile::tempdir().unwrap();
        let response = test_app(&tempdir)
            .router
            .oneshot(authorized_get_request("/v1/objects/not-an-id/envelope"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn get_missing_object_envelope_returns_not_found() {
        let tempdir = tempfile::tempdir().unwrap();
        let missing_id = "00".repeat(32);
        let response = test_app(&tempdir)
            .router
            .oneshot(authorized_get_request(&format!(
                "/v1/objects/{missing_id}/envelope"
            )))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn plan_inline_envelope_import_reports_importable_without_storing() {
        let source_tempdir = tempfile::tempdir().unwrap();
        let target_tempdir = tempfile::tempdir().unwrap();
        let source_app = test_app(&source_tempdir);
        let target_app = test_app(&target_tempdir);

        let response = source_app
            .router
            .clone()
            .oneshot(authorized_json_request(
                "/v1/objects",
                serde_json::json!({
                    "object_type": "fact",
                    "mime_type": "text/plain",
                    "payload_base64": STANDARD.encode(b"hello"),
                    "tags": ["planned"]
                }),
            ))
            .await
            .unwrap();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let published: PublishObjectResponse = serde_json::from_slice(&bytes).unwrap();
        let response = source_app
            .router
            .oneshot(authorized_get_request(&format!(
                "/v1/objects/{}/envelope",
                published.object_id
            )))
            .await
            .unwrap();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let envelope: GetObjectEnvelopeResponse = serde_json::from_slice(&bytes).unwrap();

        let response = target_app
            .router
            .clone()
            .oneshot(authorized_json_request(
                "/v1/objects/envelope/plan",
                serde_json::json!({
                    "envelope_cbor_base64": envelope.envelope_cbor_base64
                }),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let plan: PlanObjectEnvelopeImportResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(plan.object_id, published.object_id);
        assert_eq!(plan.object_type, "fact");
        assert_eq!(plan.mime_type, "text/plain");
        assert_eq!(plan.tags, vec!["planned"]);
        assert_eq!(plan.payload_size, 5);
        assert_eq!(plan.chunk_count, 0);
        assert!(plan.chunk_ids.is_empty());
        assert!(plan.chunks.is_empty());
        assert!(plan.missing_chunk_ids.is_empty());
        assert!(!plan.already_stored);
        assert!(plan.importable);
        assert!(plan.verified);

        let response = target_app
            .router
            .oneshot(authorized_get_request(&format!(
                "/v1/objects/{}",
                published.object_id
            )))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn plan_chunked_envelope_import_reports_missing_chunks() {
        let source_tempdir = tempfile::tempdir().unwrap();
        let target_tempdir = tempfile::tempdir().unwrap();
        let source_app = test_app(&source_tempdir);
        let target_app = test_app(&target_tempdir);
        let payload = vec![7_u8; hivemind_core::DEFAULT_CHUNK_SIZE + 1];

        let response = source_app
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
        assert!(published.chunk_ids.len() > 1);
        let response = source_app
            .router
            .oneshot(authorized_get_request(&format!(
                "/v1/objects/{}/envelope",
                published.object_id
            )))
            .await
            .unwrap();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let envelope: GetObjectEnvelopeResponse = serde_json::from_slice(&bytes).unwrap();

        let response = target_app
            .router
            .oneshot(authorized_json_request(
                "/v1/objects/envelope/plan",
                serde_json::json!({
                    "envelope_cbor_base64": envelope.envelope_cbor_base64
                }),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let plan: PlanObjectEnvelopeImportResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(plan.object_id, published.object_id);
        assert_eq!(plan.payload_size, payload.len() as u64);
        assert_eq!(plan.chunk_count, published.chunk_ids.len() as u32);
        assert_eq!(plan.chunk_ids, published.chunk_ids);
        assert_eq!(plan.chunks, envelope.chunks);
        assert_eq!(plan.missing_chunk_ids, published.chunk_ids);
        assert!(!plan.already_stored);
        assert!(!plan.importable);
        assert!(plan.verified);
    }

    #[tokio::test]
    async fn import_inline_envelope_stores_object_and_metadata() {
        let source_tempdir = tempfile::tempdir().unwrap();
        let target_tempdir = tempfile::tempdir().unwrap();
        let source_app = test_app(&source_tempdir);
        let target_app = test_app(&target_tempdir);

        let response = source_app
            .router
            .clone()
            .oneshot(authorized_json_request(
                "/v1/objects",
                serde_json::json!({
                    "object_type": "fact",
                    "mime_type": "text/plain",
                    "payload_base64": STANDARD.encode(b"hello"),
                    "tags": ["imported"]
                }),
            ))
            .await
            .unwrap();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let published: PublishObjectResponse = serde_json::from_slice(&bytes).unwrap();
        let response = source_app
            .router
            .oneshot(authorized_get_request(&format!(
                "/v1/objects/{}/envelope",
                published.object_id
            )))
            .await
            .unwrap();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let envelope: GetObjectEnvelopeResponse = serde_json::from_slice(&bytes).unwrap();

        let response = target_app
            .router
            .clone()
            .oneshot(authorized_json_request(
                "/v1/objects/envelope",
                serde_json::json!({
                    "envelope_cbor_base64": envelope.envelope_cbor_base64
                }),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let imported: ImportObjectEnvelopeResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(imported.object_id, published.object_id);
        assert!(imported.chunk_ids.is_empty());
        let response = target_app
            .router
            .oneshot(authorized_get_request("/v1/tags/imported"))
            .await
            .unwrap();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: TagLookupResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body.objects.len(), 1);
        assert_eq!(body.objects[0].object_id, published.object_id);
    }

    #[tokio::test]
    async fn import_envelope_requires_auth() {
        let tempdir = tempfile::tempdir().unwrap();
        let response = test_app(&tempdir)
            .router
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/v1/objects/envelope")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn import_invalid_envelope_returns_bad_request() {
        let tempdir = tempfile::tempdir().unwrap();
        let response = test_app(&tempdir)
            .router
            .oneshot(authorized_json_request(
                "/v1/objects/envelope",
                serde_json::json!({
                    "envelope_cbor_base64": STANDARD.encode(b"not-cbor")
                }),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn import_chunked_envelope_requires_chunks_first() {
        let source_tempdir = tempfile::tempdir().unwrap();
        let target_tempdir = tempfile::tempdir().unwrap();
        let source_app = test_app(&source_tempdir);
        let target_app = test_app(&target_tempdir);
        let payload = vec![7_u8; hivemind_core::DEFAULT_CHUNK_SIZE + 1];
        let response = source_app
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
        assert!(published.chunk_ids.len() > 1);
        let response = source_app
            .router
            .oneshot(authorized_get_request(&format!(
                "/v1/objects/{}/envelope",
                published.object_id
            )))
            .await
            .unwrap();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let envelope: GetObjectEnvelopeResponse = serde_json::from_slice(&bytes).unwrap();

        let response = target_app
            .router
            .oneshot(authorized_json_request(
                "/v1/objects/envelope",
                serde_json::json!({
                    "envelope_cbor_base64": envelope.envelope_cbor_base64
                }),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = error_response(response).await;
        assert_eq!(body.error.code, "missing_object_chunks");
        assert_eq!(
            body.error.message,
            "object envelope references missing chunks"
        );
        assert_eq!(
            body.error.details,
            Some(ErrorDetails::MissingChunks {
                chunk_ids: published.chunk_ids,
            })
        );
    }

    #[tokio::test]
    async fn put_chunk_then_import_chunked_envelope_roundtrips_payload() {
        let source_tempdir = tempfile::tempdir().unwrap();
        let target_tempdir = tempfile::tempdir().unwrap();
        let source_app = test_app(&source_tempdir);
        let target_app = test_app(&target_tempdir);
        let payload = vec![7_u8; hivemind_core::DEFAULT_CHUNK_SIZE + 1];
        let response = source_app
            .router
            .clone()
            .oneshot(authorized_json_request(
                "/v1/objects",
                serde_json::json!({
                    "object_type": "fact",
                    "mime_type": "application/octet-stream",
                    "payload_base64": STANDARD.encode(&payload),
                    "tags": ["transferred"]
                }),
            ))
            .await
            .unwrap();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let published: PublishObjectResponse = serde_json::from_slice(&bytes).unwrap();
        assert!(published.chunk_ids.len() > 1);
        let response = source_app
            .router
            .clone()
            .oneshot(authorized_get_request(&format!(
                "/v1/objects/{}/envelope",
                published.object_id
            )))
            .await
            .unwrap();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let envelope: GetObjectEnvelopeResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(envelope.chunks.len(), published.chunk_ids.len());

        for chunk_id in &published.chunk_ids {
            let response = source_app
                .router
                .clone()
                .oneshot(authorized_get_request(&format!("/v1/chunks/{chunk_id}")))
                .await
                .unwrap();
            let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
            let chunk: GetChunkResponse = serde_json::from_slice(&bytes).unwrap();

            let response = target_app
                .router
                .clone()
                .oneshot(authorized_put_json_request(
                    &format!("/v1/chunks/{chunk_id}"),
                    serde_json::json!({
                        "bytes_base64": chunk.bytes_base64
                    }),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }

        let response = target_app
            .router
            .clone()
            .oneshot(authorized_json_request(
                "/v1/objects/envelope",
                serde_json::json!({
                    "envelope_cbor_base64": envelope.envelope_cbor_base64
                }),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let imported: ImportObjectEnvelopeResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(imported.object_id, published.object_id);
        assert_eq!(imported.chunk_ids, published.chunk_ids);

        let response = target_app
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
        assert_eq!(STANDARD.decode(body.payload_base64).unwrap(), payload);
        assert!(body.verified);
    }

    #[tokio::test]
    async fn put_chunk_rejects_mismatched_bytes() {
        let tempdir = tempfile::tempdir().unwrap();
        let chunk_id = hivemind_core::ChunkId::from_chunk_bytes(b"expected");
        let response = test_app(&tempdir)
            .router
            .oneshot(authorized_put_json_request(
                &format!("/v1/chunks/{chunk_id}"),
                serde_json::json!({
                    "bytes_base64": STANDARD.encode(b"actual")
                }),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn get_chunk_roundtrips_bytes() {
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
        let chunk_id = published.chunk_ids[0].clone();

        let response = test_app
            .router
            .oneshot(authorized_get_request(&format!("/v1/chunks/{chunk_id}")))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: GetChunkResponse = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body.chunk_id, chunk_id);
        assert_eq!(body.size, payload.len() as u64);
        assert_eq!(STANDARD.decode(body.bytes_base64).unwrap(), payload);
        assert!(body.verified);
    }

    #[tokio::test]
    async fn get_chunk_requires_auth() {
        let tempdir = tempfile::tempdir().unwrap();
        let response = test_app(&tempdir)
            .router
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/v1/chunks/not-an-id")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn get_chunk_invalid_chunk_id_returns_bad_request() {
        let tempdir = tempfile::tempdir().unwrap();
        let response = test_app(&tempdir)
            .router
            .oneshot(authorized_get_request("/v1/chunks/not-an-id"))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn get_missing_chunk_returns_not_found() {
        let tempdir = tempfile::tempdir().unwrap();
        let missing_id = "00".repeat(32);
        let response = test_app(&tempdir)
            .router
            .oneshot(authorized_get_request(&format!("/v1/chunks/{missing_id}")))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
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
        let body = error_response(response).await;
        assert_eq!(body.error.code, "invalid_object_id");
        assert_eq!(body.error.message, "invalid object id");
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
        let body = error_response(response).await;
        assert_eq!(body.error.code, "unauthorized");
        assert_eq!(body.error.message, "unauthorized");
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
