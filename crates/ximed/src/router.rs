use crate::auth::AuthState;
use crate::handlers::{
    auth_middleware, clipboard_read, clipboard_write, pair_auth_middleware, pair_confirm,
    pair_list, pair_remove, pair_request, pair_status,
};
use crate::state::AppState;
use axum::middleware;
use axum::routing::{get, post};
use axum::Router;
use std::sync::Arc;

pub(crate) fn pair_routes(auth: Arc<AuthState>) -> Router<AppState> {
    // 无需认证的配对流程接口
    let unauth = Router::new()
        .route("/request", post(pair_request))
        .route("/status", get(pair_status))
        .route("/confirm", post(pair_confirm));

    // 需要 Bearer Token 认证的管理接口
    let auth_routes = Router::new()
        .route("/list", get(pair_list))
        .route("/remove/{device_id}", post(pair_remove))
        .layer(middleware::from_fn_with_state(auth, pair_auth_middleware));

    unauth.merge(auth_routes)
}

pub(crate) fn clipboard_routes(auth: Arc<AuthState>) -> Router<AppState> {
    Router::new()
        .route("/read", get(clipboard_read))
        .route("/write", post(clipboard_write))
        .layer(middleware::from_fn_with_state(auth, auth_middleware))
}
