use axum::extract::FromRef;

use crate::auth::{compute_hash, AuthState};
use crate::pair_store::PairStore;
use crate::platform::PlatformProviders;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct AppState {
    pub auth: AuthState,
    pub pair_store: Arc<std::sync::Mutex<PairStore>>,
    pub clipboard: Arc<std::sync::Mutex<ClipboardState>>,
    pub providers: PlatformProviders,
}

impl FromRef<AppState> for AuthState {
    fn from_ref(input: &AppState) -> Self {
        input.auth.clone()
    }
}

impl FromRef<AppState> for Arc<std::sync::Mutex<PairStore>> {
    fn from_ref(input: &AppState) -> Self {
        input.pair_store.clone()
    }
}

#[derive(Debug, Clone)]
pub struct ClipboardState {
    pub content: String,
    pub hash: String,
}

impl Default for ClipboardState {
    fn default() -> Self {
        Self {
            content: String::new(),
            hash: compute_hash(""),
        }
    }
}

impl AppState {
    pub fn new(providers: PlatformProviders) -> Self {
        let pair_store = PairStore::load_from(providers.config_dir.config_dir());
        Self {
            auth: AuthState::new(),
            pair_store: Arc::new(std::sync::Mutex::new(pair_store)),
            clipboard: Arc::new(std::sync::Mutex::new(ClipboardState::default())),
            providers,
        }
    }

    pub fn with_auth_secret(providers: PlatformProviders, secret: Vec<u8>) -> Self {
        use crate::auth::AuthConfig;
        let pair_store = PairStore::load_from(providers.config_dir.config_dir());
        Self {
            auth: AuthState::with_config(AuthConfig::from_secret(secret)),
            pair_store: Arc::new(std::sync::Mutex::new(pair_store)),
            clipboard: Arc::new(std::sync::Mutex::new(ClipboardState::default())),
            providers,
        }
    }
}
