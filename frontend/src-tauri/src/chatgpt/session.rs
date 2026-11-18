use std::{path::PathBuf, sync::Arc};

use once_cell::sync::OnceCell;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use super::{
    inference,
    oauth::{self, Grant, Registration},
    storage::{SavedSession, Storage},
};

const NOT_CONNECTED: &str = "Connect ChatGPT before using your plan.";
const SESSION_CHANGED: &str = "ChatGPT connection changed or was disconnected.";
const PLAN_DISABLED: &str =
    "ChatGPT plan sharing was not enabled. Disconnect and sign in again to grant access.";
const AUTO_DISABLED: &str =
    "Allow automatic summaries with your ChatGPT plan before using it in the background.";
const CANCELED: &str = "ChatGPT sign-in was canceled.";
const DIRECT_SCOPE: &str = "chatgpt.tokens.use.direct";

static COORDINATOR: OnceCell<Arc<Coordinator>> = OnceCell::new();

struct Coordinator {
    path: PathBuf,
    client: reqwest::Client,
    state: Mutex<State>,
    refresh: Mutex<()>,
}

struct State {
    loaded: Option<(Storage, SavedSession)>,
    generation: u64,
    manual_cancel: CancellationToken,
    automatic_cancel: CancellationToken,
    pending: Option<(uuid::Uuid, CancellationToken, CancellationToken)>,
    disconnecting: bool,
    revocation_confirmed: Option<bool>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            loaded: None,
            generation: 0,
            manual_cancel: CancellationToken::new(),
            automatic_cancel: CancellationToken::new(),
            pending: None,
            disconnecting: false,
            revocation_confirmed: None,
        }
    }
}

impl State {
    fn saved(&self) -> &SavedSession {
        &self.loaded.as_ref().expect("session loaded").1
    }

    fn invalidate_all(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.manual_cancel.cancel();
        self.automatic_cancel.cancel();
        self.manual_cancel = CancellationToken::new();
        self.automatic_cancel = CancellationToken::new();
    }

    fn invalidate_automatic(&mut self) {
        self.automatic_cancel.cancel();
        self.automatic_cancel = CancellationToken::new();
    }
}

impl Coordinator {
    async fn state(&self) -> Result<tokio::sync::MutexGuard<'_, State>, String> {
        let mut state = self.state.lock().await;
        if state.loaded.is_none() {
            let path = self.path.clone();
            state.loaded = Some(
                tokio::task::spawn_blocking(move || Storage::open(path))
                    .await
                    .map_err(|_| "Secure ChatGPT session storage is unavailable.".to_string())??,
            );
        }
        Ok(state)
    }

    async fn save(state: &mut State, saved: SavedSession) -> Result<(), String> {
        let storage = state.loaded.as_ref().expect("session loaded").0.clone();
        let copy = saved.clone();
        tokio::task::spawn_blocking(move || storage.write(&copy))
            .await
            .map_err(|_| "Secure ChatGPT session storage is unavailable.".to_string())??;
        state.loaded.as_mut().expect("session loaded").1 = saved;
        Ok(())
    }
}

fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Could not initialize ChatGPT connection.".to_string())
}

pub fn initialize(path: PathBuf) -> Result<(), String> {
    let client = http_client()?;
    COORDINATOR
        .set(Arc::new(Coordinator {
            path,
            client,
            state: Mutex::new(State::default()),
            refresh: Mutex::new(()),
        }))
        .map_err(|_| "ChatGPT connection is already initialized.".to_string())
}

fn coordinator() -> Result<Arc<Coordinator>, String> {
    COORDINATOR
        .get()
        .cloned()
        .ok_or_else(|| "ChatGPT connection is unavailable.".to_string())
}

fn plan_enabled(grant: &Grant) -> bool {
    grant.scopes.iter().any(|scope| scope == DIRECT_SCOPE) && !grant.access_token.is_empty()
}

fn same_registration(first: &Registration, second: &Registration) -> bool {
    first.sub == second.sub && first.client_id == second.client_id
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatGptStatus {
    connected: bool,
    email: Option<String>,
    account_key: Option<String>,
    welcome_key: Option<String>,
    registered_accounts: Vec<RegisteredAccount>,
    auto_summary: bool,
    plan_enabled: bool,
    connecting: bool,
    disconnect_pending: bool,
    revocation_confirmed: Option<bool>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisteredAccount {
    account_key: String,
    email: Option<String>,
}

fn account_key(host_id: &str, registration: &Registration) -> String {
    let mut hash = Sha256::new();
    hash.update(host_id.as_bytes());
    hash.update([0]);
    hash.update(registration.client_id.as_bytes());
    hash.update([0]);
    hash.update(registration.sub.as_bytes());
    format!("{:x}", hash.finalize())
}

fn welcome_key(host_id: &str, registration: &Registration) -> String {
    let mut hash = Sha256::new();
    hash.update(host_id.as_bytes());
    hash.update([0]);
    hash.update(registration.sub.as_bytes());
    format!("{:x}", hash.finalize())
}

fn known_registrations(saved: &SavedSession) -> Vec<Registration> {
    let mut registrations = saved.registrations.clone();
    if let Some(current) = &saved.registration {
        if !registrations
            .iter()
            .any(|entry| entry.client_id == current.client_id)
        {
            registrations.push(current.clone());
        }
    }
    registrations
}

fn status(state: &State) -> ChatGptStatus {
    let saved = state.saved();
    let grant = saved.grant.as_ref();
    ChatGptStatus {
        connected: grant.is_some() && !saved.disconnect_pending && !state.disconnecting,
        email: grant.and_then(|grant| grant.registration.email.clone()),
        account_key: saved
            .registration
            .as_ref()
            .map(|registration| account_key(&saved.host_id, registration)),
        welcome_key: grant.map(|grant| welcome_key(&saved.host_id, &grant.registration)),
        registered_accounts: known_registrations(saved)
            .iter()
            .map(|registration| RegisteredAccount {
                account_key: account_key(&saved.host_id, registration),
                email: registration.email.clone(),
            })
            .collect(),
        auto_summary: !saved.disconnect_pending
            && !state.disconnecting
            && grant.is_some_and(plan_enabled)
            && saved.auto_summary,
        plan_enabled: !saved.disconnect_pending
            && !state.disconnecting
            && grant.is_some_and(plan_enabled),
        connecting: state.pending.is_some(),
        disconnect_pending: saved.disconnect_pending || state.disconnecting,
        revocation_confirmed: state.revocation_confirmed,
    }
}

#[derive(Clone)]
pub struct SessionPermit {
    generation: u64,
    automatic: bool,
    cancel: CancellationToken,
}

impl SessionPermit {
    pub fn cancellation_token(&self) -> &CancellationToken {
        &self.cancel
    }
}

fn check_permit(state: &State, permit: &SessionPermit) -> Result<(), String> {
    if state.disconnecting || permit.cancel.is_cancelled() || state.generation != permit.generation
    {
        return Err(SESSION_CHANGED.to_string());
    }
    let saved = state.saved();
    if saved.disconnect_pending {
        return Err("Finish disconnecting ChatGPT before using this account.".to_string());
    }
    let grant = saved
        .grant
        .as_ref()
        .ok_or_else(|| NOT_CONNECTED.to_string())?;
    if !plan_enabled(grant) {
        return Err(PLAN_DISABLED.to_string());
    }
    if permit.automatic && !saved.auto_summary {
        return Err(AUTO_DISABLED.to_string());
    }
    Ok(())
}

pub async fn acquire(automatic: bool) -> Result<SessionPermit, String> {
    let coordinator = coordinator()?;
    let state = coordinator.state().await?;
    let permit = SessionPermit {
        generation: state.generation,
        automatic,
        cancel: if automatic {
            state.automatic_cancel.clone()
        } else {
            state.manual_cancel.clone()
        },
    };
    check_permit(&state, &permit)?;
    Ok(permit)
}

pub async fn bearer(permit: &SessionPermit) -> Result<String, String> {
    let permit = permit.clone();
    tokio::spawn(async move { bearer_owned(&permit).await })
        .await
        .map_err(|_| "ChatGPT session refresh failed.".to_string())?
}

async fn bearer_owned(permit: &SessionPermit) -> Result<String, String> {
    let coordinator = coordinator()?;
    let _refresh = tokio::select! {
        _ = permit.cancel.cancelled() => return Err(SESSION_CHANGED.to_string()),
        guard = coordinator.refresh.lock() => guard,
    };
    let (grant, pending) = {
        let state = coordinator.state().await?;
        check_permit(&state, permit)?;
        (
            state.saved().grant.clone().expect("checked grant"),
            state.saved().pending_refresh.clone(),
        )
    };
    let now = chrono::Utc::now().timestamp();
    if pending.is_none() && grant.expires_at > now + 60 {
        return Ok(grant.access_token);
    }
    if pending.is_none() && grant.earliest_refresh_at.is_some_and(|time| time > now) {
        if grant.expires_at > now {
            return Ok(grant.access_token);
        }
        return Err("ChatGPT session refresh is not available yet. Try again shortly.".to_string());
    }
    if pending.is_none() && grant.refresh_token.is_empty() {
        return Err("ChatGPT session cannot be refreshed. Sign in again.".to_string());
    }
    let result = if let Some(pending) = pending {
        tokio::select! {
            _ = permit.cancel.cancelled() => return Err(SESSION_CHANGED.to_string()),
            result = oauth::resume_refresh(&coordinator.client, &grant, &pending) => result,
        }
    } else {
        let checkpoint_coordinator = coordinator.clone();
        let old_refresh_token = grant.refresh_token.clone();
        let checkpoint = move |pending| async move {
            checkpoint_refresh(&checkpoint_coordinator, &old_refresh_token, pending).await
        };
        oauth::refresh(&coordinator.client, &grant, checkpoint).await
    };
    let refreshed = match result {
        Ok(grant) => grant,
        Err(error) => {
            if error == "ChatGPT session expired or was revoked; sign in again."
                || error == "ChatGPT registration is invalid; sign in again."
            {
                let mut state = coordinator.state().await?;
                check_permit(&state, permit)?;
                state.invalidate_all();
                let mut saved = state.saved().clone();
                saved.grant = None;
                saved.pending_refresh = None;
                saved.auto_summary = false;
                if let Err(storage_error) = Coordinator::save(&mut state, saved.clone()).await {
                    state.loaded.as_mut().expect("session loaded").1 = saved;
                    return Err(storage_error);
                }
            }
            return Err(error);
        }
    };
    let mut state = coordinator.state().await?;
    check_permit(&state, permit)?;
    let mut saved = state.saved().clone();
    saved.registration = Some(refreshed.registration.clone());
    if let Some(existing) = saved
        .registrations
        .iter_mut()
        .find(|entry| entry.client_id == refreshed.registration.client_id)
    {
        *existing = refreshed.registration.clone();
    }
    saved.grant = Some(refreshed.clone());
    saved.pending_refresh = None;
    let enabled = plan_enabled(&refreshed);
    if !enabled {
        state.invalidate_all();
        saved.auto_summary = false;
    }
    if let Err(storage_error) = Coordinator::save(&mut state, saved.clone()).await {
        if !enabled {
            state.loaded.as_mut().expect("session loaded").1 = saved;
        }
        return Err(storage_error);
    }
    if !enabled {
        return Err(PLAN_DISABLED.to_string());
    }
    Ok(refreshed.access_token)
}

async fn checkpoint_refresh(
    coordinator: &Coordinator,
    old_refresh_token: &str,
    pending: oauth::PendingRefresh,
) -> Result<(), String> {
    let mut state = coordinator.state().await?;
    if state
        .saved()
        .grant
        .as_ref()
        .map(|grant| grant.refresh_token.as_str())
        != Some(old_refresh_token)
    {
        return Err(SESSION_CHANGED.to_string());
    }
    let mut saved = state.saved().clone();
    saved.pending_refresh = Some(pending);
    if let Err(error) = Coordinator::save(&mut state, saved.clone()).await {
        state.loaded.as_mut().expect("session loaded").1 = saved;
        return Err(error);
    }
    Ok(())
}

#[tauri::command]
pub async fn chatgpt_status() -> Result<ChatGptStatus, String> {
    let coordinator = coordinator()?;
    let state = coordinator.state().await?;
    Ok(status(&state))
}

#[tauri::command]
pub async fn chatgpt_connect(
    use_different_account: Option<bool>,
    account_key: Option<String>,
    reconsent: Option<bool>,
) -> Result<ChatGptStatus, String> {
    let coordinator = coordinator()?;
    let (id, cancel, host_id, previous) = {
        let mut state = coordinator.state().await?;
        if state.saved().disconnect_pending {
            return Err("Finish disconnecting ChatGPT before signing in.".to_string());
        }
        if state.pending.is_some() || state.disconnecting {
            return Err("ChatGPT connection is already changing.".to_string());
        }
        let id = uuid::Uuid::new_v4();
        let cancel = CancellationToken::new();
        let saved = state.saved();
        if use_different_account.unwrap_or(false) && account_key.is_some() {
            return Err("Choose a saved ChatGPT account or a different account.".to_string());
        }
        let previous = if let Some(selected) = account_key {
            Some(
                known_registrations(saved)
                    .into_iter()
                    .find(|registration| {
                        self::account_key(&saved.host_id, registration) == selected
                    })
                    .ok_or_else(|| {
                        "The selected ChatGPT account is no longer available.".to_string()
                    })?,
            )
        } else if use_different_account.unwrap_or(false) {
            saved
                .pending_client_id
                .as_ref()
                .map(|client_id| Registration {
                    client_id: client_id.clone(),
                    sub: String::new(),
                    email: None,
                })
        } else {
            saved.registration.clone().or_else(|| {
                saved
                    .pending_client_id
                    .as_ref()
                    .map(|client_id| Registration {
                        client_id: client_id.clone(),
                        sub: String::new(),
                        email: None,
                    })
            })
        };
        let host_id = saved.host_id.clone();
        let initial = saved.clone();
        Coordinator::save(&mut state, initial).await?;
        state.pending = Some((id, cancel.clone(), CancellationToken::new()));
        (id, cancel, host_id, previous)
    };
    let checkpoint_coordinator = coordinator.clone();
    let pending_registration = previous
        .as_ref()
        .map_or(true, |registration| registration.sub.is_empty());
    let checkpoint = move |client_id| async move {
        let mut state = checkpoint_coordinator.state().await?;
        if state.pending.as_ref().map(|pending| pending.0) != Some(id) {
            return Err(CANCELED.to_string());
        }
        if pending_registration {
            let mut saved = state.saved().clone();
            saved.pending_client_id = Some(client_id);
            Coordinator::save(&mut state, saved).await?;
        }
        Ok(())
    };
    let result = oauth::authorize(
        &coordinator.client,
        &host_id,
        previous.as_ref(),
        &cancel,
        reconsent.unwrap_or(false),
        checkpoint,
    )
    .await;
    let _refresh = coordinator.refresh.lock().await;
    let mut state = coordinator.state().await?;
    if cancel.is_cancelled() || state.pending.as_ref().map(|pending| pending.0) != Some(id) {
        let finished = if state.pending.as_ref().map(|pending| pending.0) == Some(id) {
            state.pending.take().map(|pending| pending.2)
        } else {
            None
        };
        drop(state);
        if let Ok(grant) = result {
            let _ = oauth::revoke(&coordinator.client, &grant).await;
        }
        if let Some(finished) = finished {
            finished.cancel();
        }
        return Err(CANCELED.to_string());
    }
    let finished = state.pending.take().expect("pending sign-in").2;
    let grant = match result {
        Ok(grant) => grant,
        Err(error) => {
            finished.cancel();
            return Err(error);
        }
    };
    let mut saved = state.saved().clone();
    let old = saved.grant.clone();
    let old_pending = saved.pending_refresh.clone();
    if !saved
        .registration
        .as_ref()
        .is_some_and(|registration| same_registration(registration, &grant.registration))
    {
        saved.auto_summary = false;
    }
    if !plan_enabled(&grant) {
        saved.auto_summary = false;
    }
    saved.registrations = known_registrations(&saved);
    saved.registration = Some(grant.registration.clone());
    if let Some(existing) = saved
        .registrations
        .iter_mut()
        .find(|entry| entry.client_id == grant.registration.client_id)
    {
        *existing = grant.registration.clone();
    } else {
        saved.registrations.push(grant.registration.clone());
    }
    if saved.pending_client_id.as_deref() == Some(&grant.registration.client_id) {
        saved.pending_client_id = None;
    }
    saved.pending_refresh = None;
    saved.grant = Some(grant.clone());
    state.invalidate_all();
    state.revocation_confirmed = None;
    if let Err(error) = Coordinator::save(&mut state, saved).await {
        drop(state);
        let _ = oauth::revoke(&coordinator.client, &grant).await;
        finished.cancel();
        return Err(error);
    }
    let generation = state.generation;
    drop(state);
    let old_revocation = if let Some(old) =
        old.filter(|old| !old.refresh_token.is_empty() && old.refresh_token != grant.refresh_token)
    {
        if let Some(pending) = old_pending {
            Some(
                oauth::revoke_pending(&coordinator.client, &old, &pending)
                    .await
                    .is_ok(),
            )
        } else {
            Some(oauth::revoke(&coordinator.client, &old).await.is_ok())
        }
    } else {
        None
    };
    let mut state = coordinator.state().await?;
    if state.generation == generation {
        state.revocation_confirmed = old_revocation;
    }
    let result = status(&state);
    finished.cancel();
    Ok(result)
}

#[tauri::command]
pub async fn chatgpt_cancel_connect() -> Result<(), String> {
    let coordinator = coordinator()?;
    let state = coordinator.state().await?;
    let finished = if let Some((_, cancel, finished)) = &state.pending {
        cancel.cancel();
        Some(finished.clone())
    } else {
        None
    };
    drop(state);
    if let Some(finished) = finished {
        tokio::time::timeout(std::time::Duration::from_secs(100), finished.cancelled())
            .await
            .map_err(|_| {
                "ChatGPT sign-in cancellation is still finishing. Try again shortly.".to_string()
            })?;
    }
    Ok(())
}

#[tauri::command]
pub async fn chatgpt_disconnect() -> Result<ChatGptStatus, String> {
    let coordinator = coordinator()?;
    let client = coordinator.client.clone();
    tokio::spawn(async move {
        disconnect_with(coordinator, move |grant, pending| async move {
            match (grant, pending) {
                (Some(grant), Some(pending)) => Some(
                    oauth::revoke_pending(&client, &grant, &pending)
                        .await
                        .is_ok(),
                ),
                (Some(grant), None) if !grant.refresh_token.is_empty() => {
                    Some(oauth::revoke(&client, &grant).await.is_ok())
                }
                _ => None,
            }
        })
        .await
    })
    .await
    .map_err(|_| "ChatGPT disconnection failed.".to_string())?
}

async fn disconnect_with<F, Fut>(
    coordinator: Arc<Coordinator>,
    revoke: F,
) -> Result<ChatGptStatus, String>
where
    F: FnOnce(Option<Grant>, Option<oauth::PendingRefresh>) -> Fut,
    Fut: std::future::Future<Output = Option<bool>>,
{
    let mut state = coordinator.state().await?;
    if state.disconnecting {
        return Err("ChatGPT disconnection is already in progress.".to_string());
    }
    state.disconnecting = true;
    if let Some((_, cancel, _)) = &state.pending {
        cancel.cancel();
    }
    state.invalidate_all();
    drop(state);
    let _refresh = coordinator.refresh.lock().await;
    let mut state = coordinator.state().await?;
    let mut saved = state.saved().clone();
    let grant = saved.grant.clone();
    let pending = saved.pending_refresh.clone();
    if grant.is_none() && pending.is_none() && !saved.disconnect_pending {
        state.disconnecting = false;
        state.revocation_confirmed = None;
        return Ok(status(&state));
    }
    saved.disconnect_pending = true;
    saved.auto_summary = false;
    state.revocation_confirmed = None;
    if Coordinator::save(&mut state, saved.clone()).await.is_err() {
        state.loaded.as_mut().expect("session loaded").1 = saved;
    }
    drop(state);
    let revocation = revoke(grant, pending).await;
    let mut state = coordinator.state().await?;
    let mut cleared = state.saved().clone();
    cleared.grant = None;
    cleared.pending_refresh = None;
    cleared.auto_summary = false;
    cleared.disconnect_pending = false;
    let result = Coordinator::save(&mut state, cleared.clone()).await;
    if result.is_err() {
        state.loaded.as_mut().expect("session loaded").1 = cleared;
    }
    state.disconnecting = false;
    state.revocation_confirmed = revocation;
    result?;
    Ok(status(&state))
}

#[tauri::command]
pub async fn chatgpt_set_auto_summary(enabled: bool) -> Result<ChatGptStatus, String> {
    let coordinator = coordinator()?;
    let mut state = coordinator.state().await?;
    if enabled
        && (state.disconnecting
            || state.saved().disconnect_pending
            || !state.saved().grant.as_ref().is_some_and(plan_enabled))
    {
        return Err(NOT_CONNECTED.to_string());
    }
    if !enabled {
        state.invalidate_automatic();
    }
    let mut saved = state.saved().clone();
    saved.auto_summary = enabled;
    if let Err(error) = Coordinator::save(&mut state, saved.clone()).await {
        if !enabled {
            state.loaded.as_mut().expect("session loaded").1 = saved;
        }
        return Err(error);
    }
    Ok(status(&state))
}

#[tauri::command]
pub async fn chatgpt_models() -> Result<Vec<inference::PlanModel>, String> {
    let coordinator = coordinator()?;
    let permit = acquire(false).await?;
    let token = bearer(&permit).await?;
    inference::models(&coordinator.client, &token, permit.cancellation_token()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant() -> Grant {
        Grant {
            registration: Registration {
                client_id: "client".into(),
                sub: "user".into(),
                email: None,
            },
            access_token: "synthetic-access".into(),
            refresh_token: "synthetic-refresh".into(),
            id_token: "synthetic-id".into(),
            expires_at: 100,
            earliest_refresh_at: None,
            scopes: vec![DIRECT_SCOPE.into()],
        }
    }

    #[test]
    fn permits_bind_consent_and_connection_and_do_not_rebind() {
        let dir = tempfile::tempdir().unwrap();
        let mut saved = SavedSession::default();
        saved.grant = Some(grant());
        saved.auto_summary = true;
        let mut state = State::default();
        state.loaded = Some((Storage::for_test(dir.path().join("session")), saved));
        let manual = SessionPermit {
            generation: 0,
            automatic: false,
            cancel: state.manual_cancel.clone(),
        };
        let automatic = SessionPermit {
            generation: 0,
            automatic: true,
            cancel: state.automatic_cancel.clone(),
        };
        assert!(check_permit(&state, &manual).is_ok());
        assert!(check_permit(&state, &automatic).is_ok());
        state.invalidate_automatic();
        assert!(check_permit(&state, &automatic).is_err());
        assert!(check_permit(&state, &manual).is_ok());
        state.invalidate_all();
        assert!(check_permit(&state, &manual).is_err());
        let replacement = SessionPermit {
            generation: state.generation,
            automatic: false,
            cancel: state.manual_cancel.clone(),
        };
        state.loaded.as_mut().unwrap().1.grant = None;
        assert!(check_permit(&state, &replacement).is_err());
    }

    #[test]
    fn plan_scope_is_required_even_for_a_connected_identity() {
        let mut grant = grant();
        grant.scopes.clear();
        assert!(!plan_enabled(&grant));
        grant.scopes.push(DIRECT_SCOPE.into());
        grant.access_token.clear();
        assert!(!plan_enabled(&grant));
    }

    #[test]
    fn saved_registration_keys_allow_switching_back() {
        let mut saved = SavedSession::default();
        let first = grant().registration;
        let second = Registration {
            client_id: "other-client".into(),
            sub: "other-user".into(),
            email: None,
        };
        saved.registration = Some(second.clone());
        saved.registrations.push(first.clone());
        let registrations = known_registrations(&saved);
        assert_eq!(registrations.len(), 2);
        let first_key = account_key(&saved.host_id, &first);
        assert_ne!(first_key, account_key(&saved.host_id, &second));
        assert!(registrations
            .iter()
            .any(|entry| account_key(&saved.host_id, entry) == first_key));
        let same_user_other_registration = Registration {
            client_id: "third-client".into(),
            sub: first.sub.clone(),
            email: None,
        };
        assert_eq!(
            welcome_key(&saved.host_id, &first),
            welcome_key(&saved.host_id, &same_user_other_registration)
        );
    }

    #[test]
    fn interrupted_disconnect_blocks_use_after_reload() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::for_test(dir.path().join("session"));
        let mut saved = SavedSession::default();
        saved.grant = Some(grant());
        saved.auto_summary = false;
        saved.disconnect_pending = true;
        storage.write(&saved).unwrap();
        let reloaded = storage.read().unwrap();
        let mut state = State::default();
        state.loaded = Some((storage, reloaded));
        let permit = SessionPermit {
            generation: 0,
            automatic: false,
            cancel: state.manual_cancel.clone(),
        };
        assert!(!status(&state).connected);
        assert!(status(&state).disconnect_pending);
        assert!(check_permit(&state, &permit).is_err());
    }

    #[test]
    fn automatic_consent_is_bound_to_user_and_registration() {
        let first = grant().registration;
        let other_workspace = Registration {
            client_id: "another-client".into(),
            sub: first.sub.clone(),
            email: None,
        };
        assert!(same_registration(&first, &first));
        assert!(!same_registration(&first, &other_workspace));
    }

    #[tokio::test]
    async fn disconnect_cancels_immediately_and_revokes_rotated_checkpoint() {
        use tokio::time::{timeout, Duration};

        let dir = tempfile::tempdir().unwrap();
        let mut saved = SavedSession::default();
        saved.grant = Some(grant());
        saved.auto_summary = true;
        let mut state = State::default();
        state.loaded = Some((Storage::for_test(dir.path().join("session")), saved));
        let cancel = state.manual_cancel.clone();
        let coordinator = Arc::new(Coordinator {
            path: dir.path().to_owned(),
            client: http_client().unwrap(),
            state: Mutex::new(state),
            refresh: Mutex::new(()),
        });
        let refresh_guard = coordinator.refresh.lock().await;
        let (sent, received) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(disconnect_with(
            coordinator.clone(),
            move |_grant, pending| async move {
                let _ = sent.send(pending.map(|pending| pending.refresh_token));
                Some(true)
            },
        ));
        timeout(Duration::from_secs(3), cancel.cancelled())
            .await
            .unwrap();
        checkpoint_refresh(
            &coordinator,
            "synthetic-refresh",
            oauth::PendingRefresh {
                access_token: "rotated-access".into(),
                refresh_token: "rotated-refresh".into(),
                id_token: None,
                expires_at: 200,
                earliest_refresh_at: None,
                scopes: vec![DIRECT_SCOPE.into()],
                received_at: 100,
            },
        )
        .await
        .unwrap();
        drop(refresh_guard);
        assert_eq!(received.await.unwrap().as_deref(), Some("rotated-refresh"));
        let result = task.await.unwrap().unwrap();
        assert!(!result.connected);
        assert!(!result.disconnect_pending);
        assert_eq!(result.revocation_confirmed, Some(true));
        let state = coordinator.state().await.unwrap();
        assert!(state.saved().grant.is_none());
        assert!(state.saved().pending_refresh.is_none());
        assert!(!state.saved().auto_summary);
    }

    #[tokio::test]
    async fn production_client_does_not_follow_redirects_with_credentials() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
            time::{timeout, Duration},
        };

        let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let source = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_addr = target.local_addr().unwrap();
        let source_addr = source.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = source.accept().await.unwrap();
            let mut request = [0; 4096];
            let _ = socket.read(&mut request).await.unwrap();
            let response = format!("HTTP/1.1 307 Temporary Redirect\r\nLocation: http://{target_addr}/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let response = http_client()
            .unwrap()
            .post(format!("http://{source_addr}/"))
            .bearer_auth("synthetic-token")
            .body("synthetic transcript")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::TEMPORARY_REDIRECT);
        assert!(timeout(Duration::from_millis(300), target.accept())
            .await
            .is_err());
        server.await.unwrap();
    }
}
