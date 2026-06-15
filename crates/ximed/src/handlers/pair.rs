use crate::auth::{AuthState, DeviceAuth};
use crate::error::ApiError;
use crate::state::AppState;
use crate::types::*;
use axum::extract::{Json, Path, Query, State};
use axum::http::header;
use axum::middleware::Next;
use axum::Extension;
use std::sync::Arc;

/// 从请求中提取并验证 Bearer Token，注入 DeviceAuth
pub async fn pair_auth_middleware(
    State(auth): State<Arc<AuthState>>,
    request: axum::extract::Request,
    next: Next,
) -> Result<axum::response::Response, ApiError> {
    let device_auth = extract_device_auth(&auth, request.headers()).await?;
    let mut req = request;
    req.extensions_mut().insert(device_auth);
    Ok(next.run(req).await)
}

async fn extract_device_auth(
    auth: &AuthState,
    headers: &axum::http::HeaderMap,
) -> Result<DeviceAuth, ApiError> {
    let auth_header = headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .ok_or(ApiError::InvalidToken)?;

    if !auth_header.starts_with("Bearer ") {
        return Err(ApiError::InvalidToken);
    }

    let token = &auth_header[7..];
    let device_auth = auth.verify_token(token)?;

    if !device_auth.is_valid() {
        return Err(ApiError::TokenExpired);
    }

    Ok(device_auth)
}

pub async fn pair_request(
    State(state): State<AppState>,
    Json(req): Json<PairRequest>,
) -> Result<Json<PairRequestResponse>, ApiError> {
    if req.device_name.is_empty() {
        return Err(ApiError::InvalidRequest("device_name is required".into()));
    }

    // 服务端生成设备 ID
    let device_id = uuid::Uuid::new_v4().to_string();

    let mut store = state
        .pair_store
        .lock()
        .map_err(|e| ApiError::InternalError(format!("Mutex poisoned: {}", e)))?;

    let session = store.create_session(device_id.clone(), req.device_name);
    let code = session.code.clone();
    let expires_in = session.expires_in_seconds();

    Ok(Json(PairRequestResponse {
        device_id,
        code,
        expires_in,
    }))
}

pub async fn pair_status(
    State(state): State<AppState>,
    Query(query): Query<PairStatusQuery>,
) -> Result<Json<PairStatusResponse>, ApiError> {
    let mut store = state
        .pair_store
        .lock()
        .map_err(|e| ApiError::InternalError(format!("Mutex poisoned: {}", e)))?;

    let session = store
        .get_session_mut(&query.code)
        .ok_or(ApiError::PairCodeNotFound)?;

    if session.is_expired() {
        store.pending_sessions.remove(&query.code);
        return Err(ApiError::PairCodeExpired);
    }

    let status = session.status;
    let expires_in_seconds = session.expires_in_seconds();
    let requester_id = session.requester_id.clone();

    let token = if status == PairStatus::Confirmed {
        // 从 session 中获取已存储的请求方 token
        session.requester_token.clone()
    } else {
        None
    };

    // 如果已确认，移除 session（token 已取走）
    if status == PairStatus::Confirmed {
        store.pending_sessions.remove(&query.code);
    }

    Ok(Json(PairStatusResponse {
        status,
        device_id: Some(requester_id),
        token,
        expires_in: if status == PairStatus::Confirmed {
            Some(7 * 24 * 3600)
        } else {
            Some(expires_in_seconds)
        },
    }))
}

pub async fn pair_confirm(
    State(state): State<AppState>,
    Json(req): Json<PairConfirmRequest>,
) -> Result<Json<PairConfirmResponse>, ApiError> {
    if req.device_name.is_empty() {
        return Err(ApiError::InvalidRequest(
            "confirmer device_name is required".into(),
        ));
    }

    let mut store = state
        .pair_store
        .lock()
        .map_err(|e| ApiError::InternalError(format!("Mutex poisoned: {}", e)))?;

    let session = store
        .get_session(&req.code)
        .ok_or(ApiError::PairCodeNotFound)?
        .clone();

    if session.is_expired() {
        return Err(ApiError::PairCodeExpired);
    }

    if req.approve {
        // 服务端为确认方生成设备 ID
        let confirmer_id = uuid::Uuid::new_v4().to_string();

        // 为请求方生成 token
        let requester_token = state.auth.generate_token(&session.requester_id)?;
        // 为确认方生成 token
        let confirmer_token = state.auth.generate_token(&confirmer_id)?;

        store.confirm_session(
            &req.code,
            requester_token.token().to_string(),
            &confirmer_id,
            &req.device_name,
            confirmer_token.token().to_string(),
        )?;

        Ok(Json(PairConfirmResponse {
            success: true,
            device_id: confirmer_id,
            token: confirmer_token.token().to_string(),
        }))
    } else {
        store.reject_session(&req.code)?;
        Ok(Json(PairConfirmResponse {
            success: true,
            device_id: String::new(),
            token: String::new(),
        }))
    }
}

/// 获取与当前认证设备配对的所有设备
pub async fn pair_list(
    State(state): State<AppState>,
    Extension(device_auth): Extension<DeviceAuth>,
) -> Result<Json<DeviceListResponse>, ApiError> {
    let store = state
        .pair_store
        .lock()
        .map_err(|e| ApiError::InternalError(format!("Mutex poisoned: {}", e)))?;

    let paired = store.get_paired_devices(device_auth.device_id());
    let devices: Vec<DeviceInfo> = paired
        .into_iter()
        .map(|d| DeviceInfo {
            device_id: d.device_id.clone(),
            device_name: d.device_name.clone(),
            paired_at: d.paired_at,
            last_seen: d.last_seen,
        })
        .collect();

    Ok(Json(DeviceListResponse { devices }))
}

/// 移除指定设备及其所有配对关系（需要 Bearer Token 认证）
pub async fn pair_remove(
    State(state): State<AppState>,
    Path(device_id): Path<String>,
    Extension(device_auth): Extension<DeviceAuth>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut store = state
        .pair_store
        .lock()
        .map_err(|e| ApiError::InternalError(format!("Mutex poisoned: {}", e)))?;
    store.remove_device(&device_id)?;
    // 更新操作方的 last_seen
    store.update_last_seen(device_auth.device_id());
    Ok(Json(serde_json::json!({ "removed": true })))
}
