//! Event-driven website notification consent. Native profile changes are
//! serialized, and permission deferrals are owned until a current document's
//! answer has been applied and (for regular sites) saved by the host.
use crate::Msg;
use serde::Serialize;
use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    rc::Rc,
};
use tao::event_loop::EventLoopProxy;
use webview2_com::{
    take_pwstr, GetNonDefaultPermissionSettingsCompletedHandler,
    Microsoft::Web::WebView2::Win32::*, NavigationStartingEventHandler,
    NotificationReceivedEventHandler, PermissionRequestedEventHandler,
    SetPermissionStateCompletedHandler, SourceChangedEventHandler,
};
use windows::core::{Interface, HSTRING, PWSTR};
use wry::{WebView, WebViewExtWindows};

const MAX_PENDING: usize = 8;
const MAX_NATIVE_QUEUE: usize = 1024;
const MAX_SAVED: usize = 512;
fn may_defer(current: bool, user_initiated: bool, count: usize, duplicate: bool) -> bool {
    current && user_initiated && count < MAX_PENDING && !duplicate
}
type Scope = Option<usize>;
pub type Shared = Rc<RefCell<Manager>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Block,
}
impl Decision {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "allow" => Some(Self::Allow),
            "block" => Some(Self::Block),
            _ => None,
        }
    }
    pub fn text(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Block => "block",
        }
    }
    fn native(self) -> COREWEBVIEW2_PERMISSION_STATE {
        match self {
            Self::Allow => COREWEBVIEW2_PERMISSION_STATE_ALLOW,
            Self::Block => COREWEBVIEW2_PERMISSION_STATE_DENY,
        }
    }
}
pub fn secure_origin(value: &str) -> Option<String> {
    let parsed = url::Url::parse(value).ok()?;
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return None;
    }
    let local = match parsed.host()? {
        url::Host::Domain(host) => host == "localhost" || host.ends_with(".localhost"),
        url::Host::Ipv4(ip) => ip.is_loopback(),
        url::Host::Ipv6(ip) => ip.is_loopback(),
    };
    if parsed.scheme() != "https" && !(parsed.scheme() == "http" && local) {
        return None;
    }
    Some(parsed.origin().ascii_serialization())
}
fn native_origin(value: String) -> String {
    url::Url::parse(&value)
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some())
        .map_or(value, |url| url.origin().ascii_serialization())
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Owner {
    tab: usize,
    view: u64,
    document: u64,
    scope: Scope,
}
struct ViewState {
    nonce: u64,
    document: u64,
    scope: Scope,
    ready: bool,
}
struct Pending {
    owner: Owner,
    origin: String,
    args: ICoreWebView2PermissionRequestedEventArgs,
    deferral: Option<ICoreWebView2Deferral>,
    resolving: bool,
}
impl Pending {
    fn complete(mut self, allow: bool) -> Result<(), String> {
        unsafe {
            self.args
                .SetState(if allow {
                    COREWEBVIEW2_PERMISSION_STATE_ALLOW
                } else {
                    COREWEBVIEW2_PERMISSION_STATE_DENY
                })
                .map_err(|error| error.to_string())?;
            if let Some(deferral) = self.deferral.take() {
                deferral.Complete().map_err(|error| error.to_string())?;
            }
        }
        Ok(())
    }
}
impl Drop for Pending {
    fn drop(&mut self) {
        if let Some(deferral) = self.deferral.take() {
            unsafe {
                let _ = self.args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY);
                let _ = deferral.Complete();
            }
        }
    }
}
#[derive(Clone)]
struct Operation {
    origin: String,
    state: COREWEBVIEW2_PERMISSION_STATE,
    transaction: Option<usize>,
}
struct Profile {
    nonce: u64,
    native: ICoreWebView2Profile4,
    queue: VecDeque<Operation>,
    busy: bool,
    enumerating: bool,
    initialized: bool,
    supported: bool,
    error: Option<String>,
}
#[derive(Clone)]
struct Transaction {
    scope: Scope,
    origin: String,
    choice: Option<Decision>,
    owner: Option<Owner>,
    ready_to_save: bool,
    cancelled: bool,
}
pub struct Manager {
    regular: BTreeMap<String, Decision>,
    private: HashMap<(usize, String), Decision>,
    private_epochs: HashSet<usize>,
    session_blocks: HashSet<(Scope, String)>,
    views: HashMap<usize, ViewState>,
    profiles: HashMap<Scope, Profile>,
    pending: BTreeMap<usize, Pending>,
    transactions: HashMap<usize, Transaction>,
    next_view: u64,
    next_profile: u64,
    next_id: usize,
    ready_views: Vec<(usize, u64)>,
    shutting_down: bool,
    global_error: Option<String>,
}
impl Manager {
    pub fn new(saved: &BTreeMap<String, String>) -> Self {
        let regular = saved
            .iter()
            .filter_map(|(origin, value)| {
                let decision = Decision::parse(value)?;
                (secure_origin(origin).as_deref() == Some(origin.as_str()))
                    .then(|| (origin.clone(), decision))
            })
            .collect();
        Self {
            regular,
            private: HashMap::new(),
            private_epochs: HashSet::new(),
            session_blocks: HashSet::new(),
            views: HashMap::new(),
            profiles: HashMap::new(),
            pending: BTreeMap::new(),
            transactions: HashMap::new(),
            next_view: 0,
            next_profile: 0,
            next_id: 0,
            ready_views: Vec::new(),
            shutting_down: false,
            global_error: None,
        }
    }
    fn current(&self, owner: Owner) -> bool {
        !self.shutting_down
            && owner
                .scope
                .is_none_or(|epoch| self.private_epochs.contains(&epoch))
            && self.views.get(&owner.tab).is_some_and(|view| {
                view.nonce == owner.view
                    && view.document == owner.document
                    && view.scope == owner.scope
            })
    }
    fn transaction_current(&self, transaction: &Transaction) -> bool {
        !self.shutting_down
            && !transaction.cancelled
            && transaction
                .scope
                .is_none_or(|epoch| self.private_epochs.contains(&epoch))
            && transaction.owner.is_none_or(|owner| self.current(owner))
    }
    fn operation_current(&self, operation: &Operation) -> bool {
        operation.transaction.is_none_or(|id| {
            self.transactions
                .get(&id)
                .is_some_and(|transaction| self.transaction_current(transaction))
        })
    }
    fn operation_state(
        &self,
        scope: Scope,
        operation: &Operation,
    ) -> COREWEBVIEW2_PERMISSION_STATE {
        // SetPermissionState persists its choice. Private consent is held only
        // in this manager and answered through SavesInProfile(false) requests;
        // native profile writes in Stealth may only erase an existing choice.
        if scope.is_some() {
            COREWEBVIEW2_PERMISSION_STATE_DEFAULT
        } else if operation.transaction.is_none() {
            self.native_desired(scope, &operation.origin)
        } else {
            operation.state
        }
    }
    fn clear_private_state(&mut self, epoch: usize) -> Option<Profile> {
        self.private.retain(|(session, _), _| *session != epoch);
        self.private_epochs.remove(&epoch);
        self.session_blocks
            .retain(|(scope, _)| *scope != Some(epoch));
        self.views.retain(|_, view| view.scope != Some(epoch));
        self.transactions
            .retain(|_, transaction| transaction.scope != Some(epoch));
        self.profiles.remove(&Some(epoch))
    }
    fn rollback_state(&mut self, id: usize) -> Option<(Scope, Operation, Option<Pending>)> {
        let transaction = self.transactions.remove(&id)?;
        let operation = Operation {
            state: self.native_desired(transaction.scope, &transaction.origin),
            origin: transaction.origin,
            transaction: None,
        };
        Some((transaction.scope, operation, self.pending.remove(&id)))
    }
    fn decision(&self, scope: Scope, origin: &str) -> Option<Decision> {
        if self.session_blocks.contains(&(scope, origin.into())) {
            return Some(Decision::Block);
        }
        match scope {
            None => self.regular.get(origin).copied(),
            Some(epoch) => self.private.get(&(epoch, origin.into())).copied(),
        }
    }
    fn native_desired(&self, scope: Scope, origin: &str) -> COREWEBVIEW2_PERMISSION_STATE {
        if scope.is_some() {
            COREWEBVIEW2_PERMISSION_STATE_DEFAULT
        } else {
            self.decision(scope, origin)
                .map_or(COREWEBVIEW2_PERMISSION_STATE_DEFAULT, Decision::native)
        }
    }
    fn enqueue(&mut self, scope: Scope, operation: Operation) -> Result<(), String> {
        let profile = self
            .profiles
            .get_mut(&scope)
            .ok_or("Open a website tab before managing notification permissions")?;
        if !profile.supported || profile.queue.len() >= MAX_NATIVE_QUEUE {
            return Err("Notification permission queue is unavailable or full".into());
        }
        // Newest host policy is authoritative; don't accumulate duplicate resets.
        if operation.transaction.is_none() {
            profile
                .queue
                .retain(|old| old.origin != operation.origin || old.transaction.is_some());
        }
        profile.queue.push_back(operation);
        Ok(())
    }
}
#[derive(Serialize)]
pub struct Prompt {
    pub id: usize,
    pub origin: String,
    pub private: bool,
    pub saving: bool,
}
#[derive(Serialize)]
pub struct Rule {
    pub origin: String,
    pub decision: String,
    pub private: bool,
    pub session_only: bool,
}
#[derive(Serialize)]
pub struct UiState {
    pub prompt: Option<Prompt>,
    pub rules: Vec<Rule>,
    pub error: Option<String>,
    pub available: bool,
    pub busy: bool,
}
pub fn ui_state(
    manager: &Shared,
    tab: usize,
    private_epoch: Option<usize>,
    visible: bool,
) -> UiState {
    let m = manager.borrow();
    let prompt = visible
        .then(|| {
            m.pending
                .iter()
                .find(|(_, pending)| pending.owner.tab == tab && m.current(pending.owner))
                .map(|(id, pending)| Prompt {
                    id: *id,
                    origin: pending.origin.clone(),
                    private: pending.owner.scope.is_some(),
                    saving: pending.resolving,
                })
        })
        .flatten();
    let mut rules: Vec<_> = m
        .regular
        .iter()
        .map(|(origin, choice)| {
            let blocked = m.session_blocks.contains(&(None, origin.clone()));
            Rule {
                origin: origin.clone(),
                decision: if blocked { "block" } else { choice.text() }.into(),
                private: false,
                session_only: blocked,
            }
        })
        .collect();
    if let Some(epoch) = private_epoch {
        rules.extend(
            m.private
                .iter()
                .filter(|((session, _), _)| *session == epoch)
                .map(|((_, origin), choice)| Rule {
                    origin: origin.clone(),
                    decision: if m.session_blocks.contains(&(Some(epoch), origin.clone())) {
                        "block"
                    } else {
                        choice.text()
                    }
                    .into(),
                    private: true,
                    session_only: true,
                }),
        );
    }
    let scope = private_epoch;
    let profile = m.profiles.get(&scope);
    UiState {
        prompt,
        rules,
        error: profile
            .and_then(|p| p.error.clone())
            .or_else(|| m.global_error.clone()),
        available: profile.is_some_and(|p| p.initialized && p.supported)
            || m.profiles
                .get(&None)
                .is_some_and(|p| p.initialized && p.supported),
        busy: m
            .profiles
            .values()
            .any(|p| p.busy || p.enumerating || !p.queue.is_empty()),
    }
}
pub fn navigation_ready(manager: &Shared, tab: usize) -> bool {
    manager
        .borrow()
        .views
        .get(&tab)
        .is_some_and(|view| view.ready)
}
pub fn take_ready_views(manager: &Shared) -> Vec<(usize, u64)> {
    std::mem::take(&mut manager.borrow_mut().ready_views)
}
pub fn view_matches(manager: &Shared, tab: usize, nonce: u64) -> bool {
    manager
        .borrow()
        .views
        .get(&tab)
        .is_some_and(|view| view.nonce == nonce && view.ready)
}

fn changed(proxy: &EventLoopProxy<Msg>) {
    let _ = proxy.send_event(Msg::NotificationsChanged);
}
fn fail_profile(manager: &Shared, scope: Scope, error: String, proxy: &EventLoopProxy<Msg>) {
    let pending = {
        let mut m = manager.borrow_mut();
        if let Some(profile) = m.profiles.get_mut(&scope) {
            profile.supported = false;
            profile.enumerating = false;
            profile.busy = false;
            profile.queue.clear();
            profile.error = Some(error);
        }
        m.transactions
            .retain(|_, transaction| transaction.scope != scope);
        let ids: Vec<_> = m
            .pending
            .iter()
            .filter(|(_, pending)| pending.owner.scope == scope)
            .map(|(id, _)| *id)
            .collect();
        let pending: Vec<_> = ids
            .into_iter()
            .filter_map(|id| m.pending.remove(&id))
            .collect();
        let ready: Vec<_> = m
            .views
            .iter_mut()
            .filter(|(_, view)| view.scope == scope && !view.ready)
            .map(|(tab, view)| {
                view.ready = true;
                (*tab, view.nonce)
            })
            .collect();
        m.ready_views.extend(ready);
        pending
    };
    drop(pending);
    changed(proxy);
}
fn bootstrap(manager: &Shared, scope: Scope, proxy: &EventLoopProxy<Msg>) {
    let profile = manager
        .borrow()
        .profiles
        .get(&scope)
        .map(|profile| (profile.native.clone(), profile.nonce));
    let Some((profile, nonce)) = profile else {
        return;
    };
    let weak = Rc::downgrade(manager);
    let callback_proxy = proxy.clone();
    let result = unsafe {
        profile.GetNonDefaultPermissionSettings(
            &GetNonDefaultPermissionSettingsCompletedHandler::create(Box::new(
                move |hr, collection| {
                    let Some(manager) = weak.upgrade() else {
                        return Ok(());
                    };
                    if !manager
                        .borrow()
                        .profiles
                        .get(&scope)
                        .is_some_and(|profile| profile.nonce == nonce)
                    {
                        return Ok(());
                    }
                    let parsed = (|| -> windows::core::Result<Vec<String>> {
                        hr?;
                        let collection = collection.ok_or_else(|| {
                            windows::core::Error::new(
                                windows::Win32::Foundation::E_UNEXPECTED,
                                "WebView2 returned no permission settings collection",
                            )
                        })?;
                        let mut count = 0;
                        collection.Count(&mut count)?;
                        let mut origins = Vec::new();
                        for index in 0..count {
                            let setting = collection.GetValueAtIndex(index)?;
                            let mut kind = COREWEBVIEW2_PERMISSION_KIND_UNKNOWN_PERMISSION;
                            setting.PermissionKind(&mut kind)?;
                            if kind == COREWEBVIEW2_PERMISSION_KIND_NOTIFICATIONS {
                                let mut raw = PWSTR::null();
                                setting.PermissionOrigin(&mut raw)?;
                                origins.push(take_pwstr(raw));
                                if origins.len() > MAX_NATIVE_QUEUE {
                                    return Err(windows::core::Error::new(
                                        windows::Win32::Foundation::E_UNEXPECTED,
                                        "WebView2 returned too many notification permissions",
                                    ));
                                }
                            }
                        }
                        Ok(origins)
                    })();
                    // Collection getters are COM calls and can reenter the
                    // host. Recheck the anchor after reading native metadata.
                    if !manager
                        .borrow()
                        .profiles
                        .get(&scope)
                        .is_some_and(|profile| profile.nonce == nonce)
                    {
                        return Ok(());
                    }
                    match parsed {
                        Ok(existing) => {
                            let mut m = manager.borrow_mut();
                            let mut origins: HashSet<_> =
                                existing.into_iter().map(native_origin).collect();
                            match scope {
                                None => origins.extend(m.regular.keys().cloned()),
                                Some(epoch) => origins.extend(
                                    m.private
                                        .keys()
                                        .filter(|(session, _)| *session == epoch)
                                        .map(|(_, origin)| origin.clone()),
                                ),
                            };
                            let mut origins: Vec<_> = origins.into_iter().collect();
                            origins.sort();
                            if origins.len() > MAX_NATIVE_QUEUE {
                                drop(m);
                                fail_profile(
                                    &manager,
                                    scope,
                                    "Too many native notification permissions to reconcile safely"
                                        .into(),
                                    &callback_proxy,
                                );
                                return Ok(());
                            }
                            let operations: Vec<_> = origins
                                .into_iter()
                                .map(|origin| Operation {
                                    state: m.native_desired(scope, &origin),
                                    origin,
                                    transaction: None,
                                })
                                .collect();
                            if let Some(profile) = m.profiles.get_mut(&scope) {
                                profile.queue.extend(operations);
                                profile.enumerating = false;
                            }
                            drop(m);
                            changed(&callback_proxy);
                        }
                        Err(error) => fail_profile(
                            &manager,
                            scope,
                            format!("Website notifications are unavailable: {error}"),
                            &callback_proxy,
                        ),
                    }
                    Ok(())
                },
            )),
        )
    };
    if let Err(error) = result {
        if !manager
            .borrow()
            .profiles
            .get(&scope)
            .is_some_and(|profile| profile.nonce == nonce)
        {
            return;
        }
        fail_profile(
            manager,
            scope,
            format!("Could not initialize notification permissions: {error}"),
            proxy,
        )
    }
}
pub fn pump(manager: &Shared, proxy: &EventLoopProxy<Msg>) {
    let scopes: Vec<_> = manager.borrow().profiles.keys().copied().collect();
    for scope in scopes {
        let start = {
            let mut m = manager.borrow_mut();
            let Some(profile) = m.profiles.get(&scope) else {
                continue;
            };
            if !profile.supported || profile.enumerating || profile.busy {
                continue;
            }
            let operation = loop {
                let next = m.profiles.get_mut(&scope).unwrap().queue.pop_front();
                match next {
                    Some(mut operation) => {
                        let valid = m.operation_current(&operation);
                        if valid {
                            operation.state = m.operation_state(scope, &operation);
                            break Some(operation);
                        }
                    }
                    None => break None,
                }
            };
            if let Some(operation) = operation {
                let profile = m.profiles.get_mut(&scope).unwrap();
                profile.busy = true;
                Some((profile.native.clone(), profile.nonce, operation))
            } else {
                m.profiles.get_mut(&scope).unwrap().initialized = true;
                let ready: Vec<_> = m
                    .views
                    .iter_mut()
                    .filter(|(_, view)| view.scope == scope && !view.ready)
                    .map(|(tab, view)| {
                        view.ready = true;
                        (*tab, view.nonce)
                    })
                    .collect();
                m.ready_views.extend(ready);
                None
            }
        };
        let Some((profile, nonce, operation)) = start else {
            continue;
        };
        let weak = Rc::downgrade(manager);
        let callback_proxy = proxy.clone();
        let callback_operation = operation.clone();
        let result = unsafe {
            profile.SetPermissionState(
                COREWEBVIEW2_PERMISSION_KIND_NOTIFICATIONS,
                &HSTRING::from(&operation.origin),
                operation.state,
                &SetPermissionStateCompletedHandler::create(Box::new(move |hr| {
                    if let Some(manager) = weak.upgrade() {
                        finish_operation(
                            &manager,
                            scope,
                            nonce,
                            callback_operation.clone(),
                            hr.map_err(|error| error.to_string()),
                            &callback_proxy,
                        );
                    }
                    Ok(())
                })),
            )
        };
        if let Err(error) = result {
            finish_operation(
                manager,
                scope,
                nonce,
                operation,
                Err(error.to_string()),
                proxy,
            )
        }
    }
}
fn finish_operation(
    manager: &Shared,
    scope: Scope,
    nonce: u64,
    operation: Operation,
    result: Result<(), String>,
    proxy: &EventLoopProxy<Msg>,
) {
    if !manager
        .borrow()
        .profiles
        .get(&scope)
        .is_some_and(|profile| profile.nonce == nonce)
    {
        return;
    }
    if let Some(profile) = manager.borrow_mut().profiles.get_mut(&scope) {
        profile.busy = false;
    }
    if let Some(id) = operation.transaction {
        let valid = {
            let m = manager.borrow();
            !m.shutting_down
                && m.profiles.contains_key(&scope)
                && m.transactions
                    .get(&id)
                    .is_some_and(|transaction| m.transaction_current(transaction))
        };
        if result.is_ok() && valid {
            if let Some(profile) = manager.borrow_mut().profiles.get_mut(&scope) {
                profile.error = None;
            }
            if let Some(transaction) = manager.borrow_mut().transactions.get_mut(&id) {
                transaction.ready_to_save = true;
            }
            let _ = proxy.send_event(Msg::NotificationCommit(id));
        } else {
            abort_transaction(
                manager,
                id,
                result.err().unwrap_or_else(|| {
                    "The page changed before notification permission was granted".into()
                }),
                proxy,
            );
        }
    } else if let Err(error) = result {
        fail_profile(
            manager,
            scope,
            format!("Could not apply notification permission: {error}"),
            proxy,
        );
    }
    changed(proxy);
}
fn abort_transaction(manager: &Shared, id: usize, error: String, proxy: &EventLoopProxy<Msg>) {
    let pending = {
        let mut m = manager.borrow_mut();
        let Some((scope, operation, pending)) = m.rollback_state(id) else {
            return;
        };
        let _ = m.enqueue(scope, operation);
        if let Some(profile) = m.profiles.get_mut(&scope) {
            profile.error = Some(error);
        }
        pending
    };
    drop(pending);
    changed(proxy);
}
#[derive(Clone)]
pub struct Commit {
    pub origin: String,
    pub choice: Option<Decision>,
    pub private: bool,
}
pub fn commit_intent(manager: &Shared, id: usize) -> Option<Commit> {
    let m = manager.borrow();
    let transaction = m.transactions.get(&id)?;
    if m.shutting_down
        || !m.profiles.contains_key(&transaction.scope)
        || !transaction.ready_to_save
        || !m.transaction_current(transaction)
        || transaction.owner.is_some() && !m.pending.contains_key(&id)
    {
        return None;
    }
    Some(Commit {
        origin: transaction.origin.clone(),
        choice: transaction.choice,
        private: transaction.scope.is_some(),
    })
}
pub fn finalize(
    manager: &Shared,
    id: usize,
    saved: Result<(), String>,
    proxy: &EventLoopProxy<Msg>,
) {
    if let Err(error) = saved {
        abort_transaction(
            manager,
            id,
            format!("Notification choice was not saved: {error}"),
            proxy,
        );
        return;
    }
    let completed = {
        let mut m = manager.borrow_mut();
        let Some(transaction) = m.transactions.get(&id) else {
            return;
        };
        if !transaction.ready_to_save
            || m.shutting_down
            || !m.profiles.contains_key(&transaction.scope)
            || !m.transaction_current(transaction)
            || transaction.owner.is_some() && !m.pending.contains_key(&id)
        {
            drop(m);
            abort_transaction(
                manager,
                id,
                "The page changed before permission could be saved".into(),
                proxy,
            );
            return;
        }
        let transaction = m.transactions.remove(&id).unwrap();
        match transaction.scope {
            None => match transaction.choice {
                Some(choice) => {
                    m.regular.insert(transaction.origin.clone(), choice);
                }
                None => {
                    m.regular.remove(&transaction.origin);
                }
            },
            Some(epoch) => match transaction.choice {
                Some(choice) => {
                    m.private
                        .insert((epoch, transaction.origin.clone()), choice);
                }
                None => {
                    m.private.remove(&(epoch, transaction.origin.clone()));
                }
            },
        }
        m.session_blocks
            .remove(&(transaction.scope, transaction.origin));
        m.pending
            .remove(&id)
            .map(|pending| (pending, transaction.choice == Some(Decision::Allow)))
    };
    if let Some((pending, allow)) = completed {
        let scope = pending.owner.scope;
        if let Err(error) = pending.complete(allow) {
            if let Some(profile) = manager.borrow_mut().profiles.get_mut(&scope) {
                profile.error = Some(format!(
                    "Choice saved, but the page request could not complete: {error}"
                ));
            }
        }
    }
    changed(proxy);
}
pub fn answer(
    manager: &Shared,
    id: usize,
    choice: Option<Decision>,
    proxy: &EventLoopProxy<Msg>,
) -> Result<(), String> {
    if choice.is_none() {
        let owner_origin = manager
            .borrow()
            .pending
            .get(&id)
            .map(|pending| (pending.owner, pending.origin.clone()));
        if let Some((owner, origin)) = owner_origin {
            cancel_matching(
                manager,
                |pending| pending.owner == owner && pending.origin == origin,
                proxy,
            )
        }
        return Ok(());
    }
    let mut m = manager.borrow_mut();
    let pending = m
        .pending
        .get(&id)
        .ok_or("This notification request has expired")?;
    if !m.current(pending.owner) || pending.resolving {
        return Err("This notification request has expired".into());
    }
    if pending.owner.scope.is_none()
        && !m.regular.contains_key(&pending.origin)
        && m.regular.len() >= MAX_SAVED
    {
        return Err("Remove an unused notification permission before adding another site".into());
    }
    if let Some(epoch) = pending.owner.scope {
        if !m.private.contains_key(&(epoch, pending.origin.clone()))
            && m.private
                .keys()
                .filter(|(session, _)| *session == epoch)
                .count()
                >= MAX_SAVED
        {
            return Err(
                "Remove an unused Stealth notification permission before adding another site"
                    .into(),
            );
        }
    }
    let transaction = Transaction {
        scope: pending.owner.scope,
        origin: pending.origin.clone(),
        choice,
        owner: Some(pending.owner),
        ready_to_save: false,
        cancelled: false,
    };
    m.enqueue(
        transaction.scope,
        Operation {
            origin: transaction.origin.clone(),
            state: choice.unwrap().native(),
            transaction: Some(id),
        },
    )?;
    m.pending.get_mut(&id).unwrap().resolving = true;
    m.transactions.insert(id, transaction);
    drop(m);
    changed(proxy);
    Ok(())
}
pub fn reset(
    manager: &Shared,
    origin: &str,
    private_epoch: Option<usize>,
    proxy: &EventLoopProxy<Msg>,
) -> Result<(), String> {
    let canonical = secure_origin(origin).ok_or("Use an HTTPS or localhost website origin")?;
    if canonical != origin {
        return Err("Invalid website origin".into());
    }
    cancel_origin(manager, private_epoch, origin, proxy);
    let mut m = manager.borrow_mut();
    m.next_id += 1;
    let id = m.next_id;
    m.session_blocks.insert((private_epoch, origin.into()));
    m.enqueue(
        private_epoch,
        Operation {
            origin: origin.into(),
            state: COREWEBVIEW2_PERMISSION_STATE_DEFAULT,
            transaction: Some(id),
        },
    )?;
    m.transactions.insert(
        id,
        Transaction {
            scope: private_epoch,
            origin: origin.into(),
            choice: None,
            owner: None,
            ready_to_save: false,
            cancelled: false,
        },
    );
    drop(m);
    changed(proxy);
    Ok(())
}
fn cancel_matching(
    manager: &Shared,
    predicate: impl Fn(&Pending) -> bool,
    proxy: &EventLoopProxy<Msg>,
) {
    let (pending, ids) = {
        let mut m = manager.borrow_mut();
        let ids: Vec<_> = m
            .pending
            .iter()
            .filter(|(_, pending)| predicate(pending))
            .map(|(id, _)| *id)
            .collect();
        let pending: Vec<_> = ids.iter().filter_map(|id| m.pending.remove(id)).collect();
        for id in &ids {
            if let Some(transaction) = m.transactions.get_mut(id) {
                transaction.cancelled = true;
            }
        }
        (pending, ids)
    };
    let any = !pending.is_empty();
    drop(pending);
    for id in ids {
        let exists = manager.borrow().transactions.contains_key(&id);
        if exists {
            abort_transaction(
                manager,
                id,
                "The notification request was cancelled because its page changed".into(),
                proxy,
            )
        }
    }
    if any {
        changed(proxy)
    }
}
fn cancel_origin(manager: &Shared, scope: Scope, origin: &str, proxy: &EventLoopProxy<Msg>) {
    cancel_matching(
        manager,
        |pending| pending.owner.scope == scope && pending.origin == origin,
        proxy,
    )
}
pub fn cancel_pending(manager: &Shared, proxy: &EventLoopProxy<Msg>) {
    cancel_matching(manager, |_| true, proxy)
}
pub fn cancel_tab_pending(manager: &Shared, tab: usize, proxy: &EventLoopProxy<Msg>) {
    cancel_matching(manager, |pending| pending.owner.tab == tab, proxy)
}
pub fn prompt_belongs_to(manager: &Shared, id: usize, tab: usize) -> bool {
    let m = manager.borrow();
    m.pending
        .get(&id)
        .is_some_and(|pending| pending.owner.tab == tab && m.current(pending.owner))
}
pub fn detach(manager: &Shared, tab: usize, proxy: &EventLoopProxy<Msg>) {
    manager.borrow_mut().views.remove(&tab);
    cancel_matching(manager, |pending| pending.owner.tab == tab, proxy)
}
pub fn end_private(manager: &Shared, epoch: usize, proxy: &EventLoopProxy<Msg>) {
    let profile = manager.borrow_mut().clear_private_state(epoch);
    drop(profile);
    cancel_matching(manager, |pending| pending.owner.scope == Some(epoch), proxy);
}
pub fn shutdown(manager: &Shared, proxy: &EventLoopProxy<Msg>) {
    manager.borrow_mut().shutting_down = true;
    cancel_pending(manager, proxy);
    let profiles = {
        let mut m = manager.borrow_mut();
        m.transactions.clear();
        m.private.clear();
        m.private_epochs.clear();
        m.session_blocks.clear();
        std::mem::take(&mut m.profiles)
    };
    drop(profiles);
}

/// Bind profile operations to a hidden keeper that outlives all website views.
/// A Profile4 obtained from a website controller becomes unusable when that
/// controller closes, even if another controller keeps its profile resident.
pub fn initialize_profile(
    keeper: &WebView,
    manager: &Shared,
    scope: Scope,
    proxy: &EventLoopProxy<Msg>,
) -> Result<(), String> {
    if manager.borrow().profiles.contains_key(&scope) {
        return Ok(());
    }
    let native = (|| unsafe {
        let core = keeper.webview();
        core.cast::<ICoreWebView2_24>()?;
        core.cast::<ICoreWebView2_13>()?
            .Profile()?
            .cast::<ICoreWebView2Profile4>()
    })()
    .map_err(|error: windows::core::Error| {
        format!("Update Microsoft WebView2 to enable website notifications: {error}")
    });
    let native = match native {
        Ok(native) => native,
        Err(error) => {
            manager.borrow_mut().global_error = Some(error.clone());
            changed(proxy);
            return Err(error);
        }
    };
    {
        let mut m = manager.borrow_mut();
        m.next_profile += 1;
        let nonce = m.next_profile;
        if let Some(epoch) = scope {
            m.private_epochs.insert(epoch);
        }
        m.profiles.insert(
            scope,
            Profile {
                nonce,
                native,
                queue: VecDeque::new(),
                busy: false,
                enumerating: true,
                initialized: false,
                supported: true,
                error: None,
            },
        );
    }
    bootstrap(manager, scope, proxy);
    Ok(())
}
pub fn attach(
    view: &WebView,
    manager: Shared,
    tab: usize,
    private_epoch: Option<usize>,
    proxy: EventLoopProxy<Msg>,
) -> Result<(), String> {
    detach(&manager, tab, &proxy);
    let core = view.webview();
    let nonce = {
        let mut m = manager.borrow_mut();
        m.next_view += 1;
        let nonce = m.next_view;
        if let Some(epoch) = private_epoch {
            m.private_epochs.insert(epoch);
        }
        let ready = m
            .profiles
            .get(&private_epoch)
            .is_none_or(|profile| profile.initialized || !profile.supported);
        m.views.insert(
            tab,
            ViewState {
                nonce,
                document: 0,
                scope: private_epoch,
                // An unavailable interface must not prevent ordinary browsing.
                // Never fall back to a short-lived website's Profile4 handle.
                ready,
            },
        );
        nonce
    };
    let registration = (|| -> Result<(), String> {
        let weak = Rc::downgrade(&manager);
        let navigation_proxy = proxy.clone();
        let mut token = 0;
        unsafe {
            core.add_NavigationStarting(
                &NavigationStartingEventHandler::create(Box::new(move |_, _| {
                    if let Some(manager) = weak.upgrade() {
                        if let Some(view) = manager
                            .borrow_mut()
                            .views
                            .get_mut(&tab)
                            .filter(|view| view.nonce == nonce)
                        {
                            view.document += 1;
                        }
                        cancel_matching(
                            &manager,
                            |pending| pending.owner.tab == tab && pending.owner.view == nonce,
                            &navigation_proxy,
                        );
                    }
                    Ok(())
                })),
                &mut token,
            )
            .map_err(|error| error.to_string())?;
        }
        let weak = Rc::downgrade(&manager);
        let source_proxy = proxy.clone();
        unsafe {
            core.add_SourceChanged(
                &SourceChangedEventHandler::create(Box::new(move |_, _| {
                    if let Some(manager) = weak.upgrade() {
                        if let Some(view) = manager
                            .borrow_mut()
                            .views
                            .get_mut(&tab)
                            .filter(|view| view.nonce == nonce)
                        {
                            view.document += 1;
                        }
                        cancel_matching(
                            &manager,
                            |pending| pending.owner.tab == tab && pending.owner.view == nonce,
                            &source_proxy,
                        );
                    }
                    Ok(())
                })),
                &mut token,
            )
            .map_err(|error| error.to_string())?;
        }
        let weak = Rc::downgrade(&manager);
        let permission_proxy = proxy.clone();
        unsafe {
            core.add_PermissionRequested(
                &PermissionRequestedEventHandler::create(Box::new(move |sender, args| {
                    let (Some(sender), Some(args)) = (sender, args) else {
                        return Ok(());
                    };
                    let initial_owner = weak.upgrade().and_then(|manager| {
                        let m = manager.borrow();
                        m.views
                            .get(&tab)
                            .filter(|view| view.nonce == nonce)
                            .map(|view| Owner {
                                tab,
                                view: nonce,
                                document: view.document,
                                scope: view.scope,
                            })
                    });
                    let mut kind = COREWEBVIEW2_PERMISSION_KIND_UNKNOWN_PERMISSION;
                    if let Err(error) = args.PermissionKind(&mut kind) {
                        let _ = args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY);
                        return Err(error);
                    }
                    if kind != COREWEBVIEW2_PERMISSION_KIND_NOTIFICATIONS {
                        return Ok(());
                    }
                    args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY)?;
                    let Some(manager) = weak.upgrade() else {
                        return Ok(());
                    };
                    let args3 = match args.cast::<ICoreWebView2PermissionRequestedEventArgs3>() {
                        Ok(args3) => args3,
                        Err(_) => {
                            args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY)?;
                            return Ok(());
                        }
                    };
                    args3.SetSavesInProfile(false)?;
                    let mut raw = PWSTR::null();
                    args.Uri(&mut raw)?;
                    let requested = secure_origin(&take_pwstr(raw));
                    let mut raw = PWSTR::null();
                    sender.Source(&mut raw)?;
                    let top = secure_origin(&take_pwstr(raw));
                    let mut initiated = windows::core::BOOL::default();
                    args.IsUserInitiated(&mut initiated)?;
                    let m = manager.borrow();
                    let owner = initial_owner.filter(|owner| m.current(*owner));
                    let origin = requested.filter(|origin| Some(origin) == top.as_ref());
                    let supported = m
                        .profiles
                        .get(&private_epoch)
                        .is_some_and(|profile| profile.initialized && profile.supported);
                    let Some((owner, origin)) =
                        owner.zip(origin).filter(|_| supported && !m.shutting_down)
                    else {
                        drop(m);
                        args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY)?;
                        return Ok(());
                    };
                    if let Some(choice) = m.decision(private_epoch, &origin) {
                        drop(m);
                        args.SetState(choice.native())?;
                        return Ok(());
                    }
                    if !may_defer(
                        m.current(owner),
                        initiated.as_bool(),
                        m.pending.len(),
                        m.pending
                            .values()
                            .any(|pending| pending.owner == owner && pending.origin == origin),
                    ) {
                        drop(m);
                        args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY)?;
                        return Ok(());
                    }
                    drop(m);
                    let deferral = args.GetDeferral()?;
                    let mut m = manager.borrow_mut();
                    if !may_defer(
                        m.current(owner),
                        initiated.as_bool(),
                        m.pending.len(),
                        m.pending
                            .values()
                            .any(|pending| pending.owner == owner && pending.origin == origin),
                    ) {
                        drop(m);
                        let _ = args.SetState(COREWEBVIEW2_PERMISSION_STATE_DENY);
                        let _ = deferral.Complete();
                        return Ok(());
                    }
                    m.next_id += 1;
                    let id = m.next_id;
                    m.pending.insert(
                        id,
                        Pending {
                            owner,
                            origin,
                            args,
                            deferral: Some(deferral),
                            resolving: false,
                        },
                    );
                    drop(m);
                    changed(&permission_proxy);
                    Ok(())
                })),
                &mut token,
            )
            .map_err(|error| error.to_string())?;
        }
        if let Ok(notifications) = core.cast::<ICoreWebView2_24>() {
            let weak = Rc::downgrade(&manager);
            unsafe {
                notifications
                    .add_NotificationReceived(
                        &NotificationReceivedEventHandler::create(Box::new(move |sender, args| {
                            let (Some(sender), Some(args)) = (sender, args) else {
                                return Ok(());
                            };
                            let owner = weak.upgrade().and_then(|manager| {
                                let m = manager.borrow();
                                m.views
                                    .get(&tab)
                                    .filter(|view| view.nonce == nonce)
                                    .map(|view| Owner {
                                        tab,
                                        view: nonce,
                                        document: view.document,
                                        scope: view.scope,
                                    })
                            });
                            let metadata = (|| -> windows::core::Result<_> {
                                let mut raw = PWSTR::null();
                                args.SenderOrigin(&mut raw)?;
                                let origin = secure_origin(&take_pwstr(raw));
                                let mut raw = PWSTR::null();
                                sender.Source(&mut raw)?;
                                Ok((origin, secure_origin(&take_pwstr(raw))))
                            })();
                            let (origin, top) = match metadata {
                                Ok(values) => values,
                                Err(_) => {
                                    args.SetHandled(true)?;
                                    return Ok(());
                                }
                            };
                            let allowed = weak.upgrade().is_some_and(|manager| {
                                let m = manager.borrow();
                                owner.is_some_and(|owner| m.current(owner))
                                    && m.profiles.get(&private_epoch).is_some_and(|profile| {
                                        profile.initialized && profile.supported
                                    })
                                    && origin.as_ref().is_some_and(|origin| {
                                        Some(origin) == top.as_ref()
                                            && m.decision(private_epoch, origin)
                                                == Some(Decision::Allow)
                                    })
                            });
                            if !allowed {
                                args.SetHandled(true)?;
                            }
                            Ok(())
                        })),
                        &mut token,
                    )
                    .map_err(|error| error.to_string())?;
            }
        }
        Ok(())
    })();
    if let Err(error) = registration {
        detach(&manager, tab, &proxy);
        changed(&proxy);
        return Err(error);
    }
    changed(&proxy);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn secure_origins_are_exact_and_remote_http_credentials_and_opaque_urls_are_rejected() {
        assert_eq!(
            secure_origin("https://EXAMPLE.com:443/page?q=x#f").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            secure_origin("https://example.com:8443/a").as_deref(),
            Some("https://example.com:8443")
        );
        for input in [
            "http://example.com",
            "https://u:p@example.com",
            "file:///a",
            "data:text/html,x",
            "about:blank",
        ] {
            assert!(secure_origin(input).is_none(), "{input}")
        }
        for input in [
            "http://localhost:9397/a",
            "http://127.0.0.1:9397/a",
            "http://[::1]:9397/a",
        ] {
            assert!(secure_origin(input).is_some(), "{input}")
        }
        assert_ne!(
            secure_origin("https://example.com"),
            secure_origin("https://example.com.evil.test")
        );
    }
    #[test]
    fn policy_defaults_to_ask_and_never_loads_invalid_or_insecure_saved_allows() {
        let manager = Manager::new(&BTreeMap::from([
            ("https://good.test".into(), "allow".into()),
            ("http://bad.test".into(), "allow".into()),
            ("https://path.test/a".into(), "allow".into()),
            ("https://unknown.test".into(), "unknown".into()),
        ]));
        assert_eq!(
            manager.decision(None, "https://good.test"),
            Some(Decision::Allow)
        );
        assert!(manager.decision(None, "https://new.test").is_none());
        assert_eq!(manager.regular.len(), 1);
    }
    #[test]
    fn document_view_and_private_epoch_all_belong_to_a_prompt() {
        let mut manager = Manager::new(&BTreeMap::new());
        manager.private_epochs.insert(2);
        manager.views.insert(
            7,
            ViewState {
                nonce: 4,
                document: 9,
                scope: Some(2),
                ready: true,
            },
        );
        let owner = Owner {
            tab: 7,
            view: 4,
            document: 9,
            scope: Some(2),
        };
        assert!(manager.current(owner));
        for changed in [
            Owner { view: 5, ..owner },
            Owner {
                document: 10,
                ..owner
            },
            Owner {
                scope: Some(3),
                ..owner
            },
            Owner { tab: 8, ..owner },
        ] {
            assert!(!manager.current(changed))
        }
        manager.shutting_down = true;
        assert!(!manager.current(owner));
    }
    #[test]
    fn private_choices_are_separate_and_do_not_enter_regular_serializable_policy() {
        let mut manager = Manager::new(&BTreeMap::new());
        manager
            .private
            .insert((2, "https://private.test".into()), Decision::Allow);
        assert_eq!(
            manager.decision(Some(2), "https://private.test"),
            Some(Decision::Allow)
        );
        assert!(manager.decision(None, "https://private.test").is_none());
        assert!(manager.decision(Some(3), "https://private.test").is_none());
        assert!(manager.regular.is_empty());
    }
    fn transaction(scope: Scope, owner: Option<Owner>) -> Transaction {
        Transaction {
            scope,
            origin: "https://allowed.test".into(),
            choice: Some(Decision::Allow),
            owner,
            ready_to_save: true,
            cancelled: false,
        }
    }
    #[test]
    fn scope_teardown_removes_ownerless_private_resets_and_rejects_late_callbacks() {
        let mut manager = Manager::new(&BTreeMap::from([(
            "https://regular.test".into(),
            "block".into(),
        )]));
        manager.private_epochs.insert(2);
        manager
            .private
            .insert((2, "https://allowed.test".into()), Decision::Allow);
        let reset = Transaction {
            choice: None,
            ..transaction(Some(2), None)
        };
        assert!(manager.transaction_current(&reset));
        manager.transactions.insert(4, reset);
        manager.transactions.insert(5, transaction(None, None));
        assert!(manager.clear_private_state(2).is_none());
        assert!(!manager.transactions.contains_key(&4));
        assert!(manager.transactions.contains_key(&5));
        assert!(!manager.private_epochs.contains(&2));
        assert!(manager.private.is_empty());
        assert_eq!(manager.regular.len(), 1);
        assert!(!manager.operation_current(&Operation {
            origin: "https://allowed.test".into(),
            state: COREWEBVIEW2_PERMISSION_STATE_ALLOW,
            transaction: Some(4)
        }));
        assert!(!manager.transaction_current(&transaction(Some(2), None)));
    }
    #[test]
    fn queued_allow_is_rejected_after_cancel_navigation_or_shutdown() {
        let mut manager = Manager::new(&BTreeMap::new());
        let owner = Owner {
            tab: 7,
            view: 3,
            document: 1,
            scope: None,
        };
        manager.views.insert(
            7,
            ViewState {
                nonce: 3,
                document: 1,
                scope: None,
                ready: true,
            },
        );
        manager
            .transactions
            .insert(9, transaction(None, Some(owner)));
        let operation = Operation {
            origin: "https://allowed.test".into(),
            state: COREWEBVIEW2_PERMISSION_STATE_ALLOW,
            transaction: Some(9),
        };
        assert!(manager.operation_current(&operation));
        manager.views.get_mut(&7).unwrap().document += 1;
        assert!(!manager.operation_current(&operation));
        manager.views.get_mut(&7).unwrap().document = 1;
        manager.transactions.get_mut(&9).unwrap().cancelled = true;
        assert!(!manager.operation_current(&operation));
        manager.transactions.get_mut(&9).unwrap().cancelled = false;
        manager.shutting_down = true;
        assert!(!manager.operation_current(&operation));
        assert!(!manager.transaction_current(&transaction(None, None)));
    }
    #[test]
    fn failed_save_rolls_back_to_confirmed_policy_without_committing_allow() {
        let mut manager = Manager::new(&BTreeMap::from([(
            "https://allowed.test".into(),
            "block".into(),
        )]));
        manager.transactions.insert(9, transaction(None, None));
        let (scope, restore, pending) = manager.rollback_state(9).unwrap();
        assert!(scope.is_none() && pending.is_none());
        assert_eq!(restore.state, COREWEBVIEW2_PERMISSION_STATE_DENY);
        assert_eq!(
            manager.decision(None, "https://allowed.test"),
            Some(Decision::Block)
        );
        assert!(!manager.transactions.contains_key(&9));
        assert!(manager.rollback_state(9).is_none());
    }
    #[test]
    fn prompt_caps_deduplication_and_user_gesture_fail_closed() {
        assert!(may_defer(true, true, MAX_PENDING - 1, false));
        for (current, gesture, count, duplicate) in [
            (false, true, 0, false),
            (true, false, 0, false),
            (true, true, MAX_PENDING, false),
            (true, true, usize::MAX, false),
            (true, true, 0, true),
        ] {
            assert!(!may_defer(current, gesture, count, duplicate));
        }
        assert!(Decision::parse("invalid").is_none());
        let manager = Rc::new(RefCell::new(Manager::new(&BTreeMap::new())));
        assert!(commit_intent(&manager, 999).is_none());
        assert!(!prompt_belongs_to(&manager, 999, 7));
    }
    #[test]
    fn native_origin_aliases_cannot_override_canonical_saved_policy() {
        let aliases: HashSet<_> = [
            "https://ALLOWED.test:443/path/".into(),
            "https://allowed.test".into(),
        ]
        .into_iter()
        .map(native_origin)
        .collect();
        assert_eq!(aliases.len(), 1);
        assert!(aliases.contains("https://allowed.test"));
        assert_eq!(
            native_origin("http://remote.test/path".into()),
            "http://remote.test"
        );
        assert!(secure_origin("http://remote.test").is_none());
    }
    #[test]
    fn queued_rollback_uses_latest_confirmed_policy_instead_of_overriding_newer_reset() {
        let mut manager = Manager::new(&BTreeMap::from([(
            "https://allowed.test".into(),
            "allow".into(),
        )]));
        let old_restore = Operation {
            origin: "https://allowed.test".into(),
            state: COREWEBVIEW2_PERMISSION_STATE_DENY,
            transaction: None,
        };
        assert_eq!(
            manager.operation_state(None, &old_restore),
            COREWEBVIEW2_PERMISSION_STATE_ALLOW
        );
        manager.regular.remove("https://allowed.test");
        assert_eq!(
            manager.operation_state(None, &old_restore),
            COREWEBVIEW2_PERMISSION_STATE_DEFAULT
        );
        manager
            .session_blocks
            .insert((None, "https://allowed.test".into()));
        assert_eq!(
            manager.operation_state(None, &old_restore),
            COREWEBVIEW2_PERMISSION_STATE_DENY
        );
    }
    #[test]
    fn stealth_native_writes_only_reset_while_host_consent_remains_session_only() {
        let mut manager = Manager::new(&BTreeMap::new());
        manager.private_epochs.insert(7);
        for choice in [Decision::Allow, Decision::Block] {
            manager
                .private
                .insert((7, "https://private.test".into()), choice);
            assert_eq!(
                manager.decision(Some(7), "https://private.test"),
                Some(choice)
            );
            assert_eq!(
                manager.native_desired(Some(7), "https://private.test"),
                COREWEBVIEW2_PERMISSION_STATE_DEFAULT
            );
            for transaction in [None, Some(3)] {
                assert_eq!(
                    manager.operation_state(
                        Some(7),
                        &Operation {
                            origin: "https://private.test".into(),
                            state: choice.native(),
                            transaction,
                        }
                    ),
                    COREWEBVIEW2_PERMISSION_STATE_DEFAULT
                );
            }
            assert!(manager.regular.is_empty());
        }
        assert_eq!(manager.decision(Some(8), "https://private.test"), None);
    }
}
