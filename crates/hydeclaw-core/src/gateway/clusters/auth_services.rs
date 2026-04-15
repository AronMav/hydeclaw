use std::collections::HashMap;
use std::sync::Arc;

use crate::gateway::state::AccessGuardMap;
use crate::secrets::SecretsManager;

#[derive(Clone)]
pub struct AuthServices {
    pub secrets:       Arc<SecretsManager>,
    pub access_guards: AccessGuardMap,
    pub oauth:         Arc<crate::oauth::OAuthManager>,
    pub ws_tickets:    Arc<tokio::sync::Mutex<HashMap<String, std::time::Instant>>>,
}

impl AuthServices {
    pub fn new(
        secrets: Arc<SecretsManager>,
        access_guards: AccessGuardMap,
        oauth: Arc<crate::oauth::OAuthManager>,
        ws_tickets: Arc<tokio::sync::Mutex<HashMap<String, std::time::Instant>>>,
    ) -> Self {
        Self { secrets, access_guards, oauth, ws_tickets }
    }

    #[cfg(test)]
    pub fn test_new() -> Self {
        Self {
            secrets: Arc::new(SecretsManager::new_noop()),
            access_guards: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            oauth: Arc::new(crate::oauth::OAuthManager::new_noop()),
            ws_tickets: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn auth_services_ws_tickets_empty_on_new() {
        let auth = AuthServices::test_new();
        let tickets = auth.ws_tickets.lock().await;
        assert!(tickets.is_empty());
    }
}
