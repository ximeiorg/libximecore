use axum::{routing::get, Router};
use base64::Engine;
use std::sync::Arc;
use tower_http::{
    cors::{Any, CorsLayer},
    trace::TraceLayer,
};

use crate::{
    handlers::health_check,
    router::{clipboard_routes, pair_routes},
    AppState,
};

pub fn create_router(state: AppState) -> Router {
    let auth = Arc::new(state.auth.clone());
    Router::new()
        .nest("/pair", pair_routes(auth.clone()))
        .nest("/clipboard", clipboard_routes(auth))
        .route("/health", get(health_check))
        .layer(TraceLayer::new_for_http())
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .with_state(state)
}

pub async fn serve(port: u16, secret_b64: Option<String>) -> Result<(), anyhow::Error> {
    let providers = crate::platform::PlatformProviders::default();
    let state = if let Some(secret_b64) = secret_b64 {
        let secret_bytes = base64::engine::general_purpose::STANDARD
            .decode(secret_b64.as_bytes())
            .map_err(|e| anyhow::anyhow!("Invalid base64 secret: {e}"))?;
        AppState::with_auth_secret(providers, secret_bytes)
    } else {
        AppState::new(providers)
    };
    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;

    tracing::info!("HTTP server listening on {}", addr);

    axum::serve(listener, create_router(state)).await?;
    Ok(())
}
