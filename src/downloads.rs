use crate::{storage, Msg};
use serde::{Deserialize, Serialize};
use std::{
    cell::{Cell, RefCell},
    collections::{hash_map::DefaultHasher, HashMap, HashSet},
    hash::{Hash, Hasher},
    path::{Component, Path, PathBuf, Prefix},
    rc::Rc,
    time::{Duration, Instant},
};
use tao::{event_loop::EventLoopProxy, window::Window};
use webview2_com::{
    take_pwstr, BytesReceivedChangedEventHandler, DownloadStartingEventHandler,
    Microsoft::Web::WebView2::Win32::*, StateChangedEventHandler,
};
use windows::core::{Interface, HSTRING, PWSTR};
use wry::{WebView, WebViewExtWindows};

#[derive(Clone, Serialize)]
pub struct Record {
    pub id: usize,
    pub url: String,
    pub path: Option<String>,
    pub status: String,
    pub received: i64,
    pub total: i64,
    pub ok: bool,
    pub can_resume: bool,
    pub can_cancel: bool,
    pub bytes_per_second: f64,
    pub eta_seconds: Option<f64>,
    pub message: Option<String>,
    #[serde(skip)]
    owner_tab: usize,
    #[serde(skip)]
    owner_private: bool,
    #[serde(skip)]
    owner_view: u64,
    #[serde(skip)]
    generation: u64,
    #[serde(skip)]
    resume_pending: bool,
    #[serde(skip)]
    pause_requested: bool,
}
struct Running {
    tab_id: usize,
    op: ICoreWebView2DownloadOperation,
    tab_lease: TabLease,
    meter: ProgressMeter,
    tokens: Option<(i64, i64)>,
    // Keep native owners alive across resumed DownloadStarting operations.
    owners: Vec<ICoreWebView2DownloadOperation>,
    cancel_requested: bool,
}
// Cancellation tombstones may outlive a transfer to reject a late handoff.
// Only a live transfer needs to keep the source WebView resident.
struct TabLease {
    counter: Rc<Cell<usize>>,
    held: bool,
}
impl TabLease {
    fn new(counter: Rc<Cell<usize>>) -> Self {
        let mut lease = Self {
            counter,
            held: false,
        };
        lease.acquire();
        lease
    }
    fn acquire(&mut self) {
        if !self.held {
            self.counter.set(self.counter.get() + 1);
            self.held = true;
        }
    }
    fn release(&mut self) {
        if self.held {
            self.counter.set(self.counter.get().saturating_sub(1));
            self.held = false;
        }
    }
}
impl Drop for TabLease {
    fn drop(&mut self) {
        self.release();
    }
}
struct Pending {
    args: ICoreWebView2DownloadStartingEventArgs,
    deferral: Option<ICoreWebView2Deferral>,
    suggested: PathBuf,
}
impl Drop for Pending {
    fn drop(&mut self) {
        if let Some(d) = self.deferral.take() {
            unsafe {
                let _ = self.args.SetCancel(true);
                let _ = d.Complete();
            }
        }
    }
}
#[derive(Default)]
pub struct Manager {
    pub records: Vec<Record>,
    running: HashMap<usize, Running>,
    pending: HashMap<usize, Pending>,
    next: usize,
    next_view: u64,
    last_progress: Option<Instant>,
    next_regular: usize,
    ledger_path: Option<PathBuf>,
    ledger_loaded: bool,
    ledger_fingerprint: Option<u64>,
    last_saved: Option<Ledger>,
    last_save_attempt: Option<Instant>,
    last_save_structure: Option<u64>,
    last_save_error: Option<String>,
    ledger_needs_trim: bool,
    persisting: bool,
}
pub type Shared = Rc<RefCell<Manager>>;

// Profile identity only: fingerprints below cover the downloads record, never
// this shared database file or unrelated settings/vault transactions.
const LEDGER_FILE: &str = "yas.redb";
const LEDGER_VERSION: u32 = 1;
const HISTORY_LIMIT: usize = 500;
const MAX_LEDGER_BYTES: usize = 64 * 1024 * 1024;
const MAX_DISK_RECORDS: usize = 100_000;
const MAX_SAFE_ID: usize = 9_007_199_254_740_990;
const RESTORED_INTERRUPTED:&str="Yas stopped before this download finished. Start the download again; its previous transfer cannot be resumed after a full restart.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PersistPreflight {
    Unchanged,
    Throttled,
    FullWork,
}

// Disk records deliberately cannot restore native capabilities, ownership,
// callbacks, private sessions or saved runtime flags.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct LedgerRecord {
    id: usize,
    url: String,
    path: Option<String>,
    status: String,
    received: i64,
    total: i64,
    message: Option<String>,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Ledger {
    version: u32,
    next_id: usize,
    records: Vec<LedgerRecord>,
}
impl LedgerRecord {
    fn snapshot(record: &Record) -> Self {
        Self {
            id: record.id,
            url: record.url.clone(),
            path: record.path.clone(),
            status: record.status.clone(),
            received: record.received,
            total: record.total,
            message: record.message.clone(),
        }
    }
    fn restore(self) -> Record {
        let unfinished = matches!(
            self.status.as_str(),
            "choosing" | "active" | "paused" | "interrupted"
        );
        Record {
            id: self.id,
            url: self.url,
            path: self.path,
            status: if unfinished {
                "interrupted".into()
            } else {
                self.status.clone()
            },
            received: self.received,
            total: self.total,
            ok: self.status == "completed",
            can_resume: false,
            can_cancel: false,
            bytes_per_second: 0.,
            eta_seconds: None,
            message: if unfinished {
                Some(RESTORED_INTERRUPTED.into())
            } else {
                self.message
            },
            owner_tab: 0,
            owner_private: false,
            owner_view: 0,
            generation: 0,
            resume_pending: false,
            pause_requested: false,
        }
    }
}
fn valid_saved_path(path: &str) -> bool {
    if path.is_empty()
        || path.encode_utf16().count() > 32767
        || path.chars().any(|c| c.is_control())
    {
        return false;
    }
    let path = Path::new(path);
    if !path.is_absolute() || path.file_name().is_none() {
        return false;
    }
    let mut components = path.components();
    if !matches!(components.next(),Some(Component::Prefix(prefix)) if matches!(prefix.kind(),Prefix::Disk(_)|Prefix::VerbatimDisk(_)|Prefix::UNC(_,_)|Prefix::VerbatimUNC(_,_)))
    {
        return false;
    }
    components.all(|part| match part {
        Component::RootDir => true,
        Component::Normal(name) => !name.to_string_lossy().contains(':'),
        _ => false,
    })
}
fn validate_ledger(ledger: &Ledger) -> Result<(), String> {
    if ledger.version != LEDGER_VERSION
        || ledger.records.len() > MAX_DISK_RECORDS
        || ledger.next_id > MAX_SAFE_ID
    {
        return Err(
            "Saved download history has an unsupported format. The original file was preserved."
                .into(),
        );
    }
    let mut ids = HashSet::new();
    for record in &ledger.records {
        let valid_url = record.url.len() <= 2 * 1024 * 1024
            && !record.url.chars().any(|c| c.is_control())
            && url::Url::parse(&record.url).is_ok_and(|u| {
                matches!(u.scheme(), "http" | "https" | "blob" | "data" | "file")
                    && (!matches!(u.scheme(), "http" | "https") || u.host_str().is_some())
            });
        if record.id == 0
            || record.id > ledger.next_id
            || !ids.insert(record.id)
            || !valid_url
            || record.received < 0
            || record.total < -1
            || !matches!(
                record.status.as_str(),
                "choosing"
                    | "active"
                    | "paused"
                    | "interrupted"
                    | "completed"
                    | "cancelled"
                    | "failed"
            )
            || record.path.as_ref().is_some_and(|p| !valid_saved_path(p))
            || record.status == "completed" && record.path.is_none()
            || record
                .message
                .as_ref()
                .is_some_and(|m| m.len() > 4096 || m.contains('\0'))
        {
            return Err(
                "Saved download history is invalid. The original file was preserved.".into(),
            );
        }
    }
    Ok(())
}
fn fingerprint(bytes: &[u8]) -> u64 {
    storage::fingerprint(bytes)
}
fn structure(ledger: &Ledger) -> u64 {
    let mut hash = DefaultHasher::new();
    ledger.version.hash(&mut hash);
    ledger.next_id.hash(&mut hash);
    for record in &ledger.records {
        record.id.hash(&mut hash);
        record.url.hash(&mut hash);
        record.path.hash(&mut hash);
        record.status.hash(&mut hash);
        record.message.hash(&mut hash);
    }
    hash.finish()
}
fn encode_ledger(ledger: &Ledger, max_file_bytes: usize) -> Result<Vec<u8>, String> {
    let plain = postcard::to_allocvec(ledger).map_err(|e| e.to_string())?;
    protect_ledger(plain, max_file_bytes)
}
fn protect_ledger(mut plain: Vec<u8>, max_file_bytes: usize) -> Result<Vec<u8>, String> {
    if plain.len() > max_file_bytes {
        plain.fill(0);
        return Err("Download history is too large to save.".into());
    }
    let protected = storage::protect(&plain, false);
    plain.fill(0);
    let encrypted = protected?;
    // DPAPI adds an envelope, so a plaintext-only bound could save a record
    // that Yas refuses to reopen.
    if encrypted.len() > max_file_bytes {
        return Err("Download history is too large to save.".into());
    }
    Ok(encrypted)
}
fn decode_ledger(ciphertext: &[u8], legacy_json: bool) -> Result<Ledger, String> {
    if ciphertext.len() > MAX_LEDGER_BYTES {
        return Err("Saved download history is too large. The original data was preserved.".into());
    }
    let mut plain = storage::protect(ciphertext, true).map_err(|_| {
        "Could not decrypt download history. The original data was preserved.".to_string()
    })?;
    let parsed = if plain.len() > MAX_LEDGER_BYTES {
        Err("Saved download history is too large. The original data was preserved.".to_string())
    } else if legacy_json {
        serde_json::from_slice::<Ledger>(&plain).map_err(|_| {
            "Saved download history is invalid. The original data was preserved.".to_string()
        })
    } else {
        // from_bytes accepts a trailing payload. Require the entire versioned
        // record so extra/forged fields cannot be silently ignored.
        postcard::take_from_bytes::<Ledger>(&plain)
            .map_err(|_| {
                "Saved download history is invalid. The original data was preserved.".to_string()
            })
            .and_then(|(ledger, rest)| {
                if rest.is_empty() {
                    Ok(ledger)
                } else {
                    Err("Saved download history has trailing data. The original data was preserved.".into())
                }
            })
    };
    plain.fill(0);
    let ledger = parsed?;
    validate_ledger(&ledger)?;
    Ok(ledger)
}

/// Convert the complete legacy ledger without applying normal history pruning.
pub fn migrate_legacy_ledger(ciphertext: &[u8]) -> Result<Vec<u8>, String> {
    encode_ledger(&decode_ledger(ciphertext, true)?, MAX_LEDGER_BYTES)
}

/// Explicit compatibility export; the database remains the authoritative store.
pub fn export_legacy_ledger(ciphertext: &[u8]) -> Result<Vec<u8>, String> {
    let ledger = decode_ledger(ciphertext, false)?;
    let plain = serde_json::to_vec(&ledger).map_err(|e| e.to_string())?;
    protect_ledger(plain, MAX_LEDGER_BYTES)
}

pub(crate) fn ledgers_equal(
    legacy_ciphertext: &[u8],
    db_ciphertext: &[u8],
) -> Result<bool, String> {
    Ok(decode_ledger(legacy_ciphertext, true)? == decode_ledger(db_ciphertext, false)?)
}
fn prune_history(records: &mut Vec<Record>, live: &HashSet<usize>) {
    let mut finished: Vec<_> = records
        .iter()
        .filter(|r| !r.owner_private && !live.contains(&r.id))
        .map(|r| r.id)
        .collect();
    finished.sort_unstable_by(|a, b| b.cmp(a));
    let keep: HashSet<_> = finished.into_iter().take(HISTORY_LIMIT).collect();
    records.retain(|r| r.owner_private || live.contains(&r.id) || keep.contains(&r.id));
}
fn history_needs_pruning(records: &[Record], mut is_live: impl FnMut(usize) -> bool) -> bool {
    records
        .iter()
        .filter(|row| !row.owner_private && !is_live(row.id))
        .take(HISTORY_LIMIT + 1)
        .count()
        > HISTORY_LIMIT
}
impl Manager {
    pub fn load(root: &Path) -> Result<Self, String> {
        let path = root.join(LEDGER_FILE);
        let encrypted = match storage::read_downloads(root)? {
            Some(value) => value,
            None => {
                return Ok(Self {
                    ledger_path: Some(path),
                    ledger_loaded: true,
                    ..Self::default()
                })
            }
        };
        let file_fingerprint = fingerprint(&encrypted);
        let mut ledger = decode_ledger(&encrypted, false)?;
        ledger.records.sort_by_key(|r| r.id);
        let trim = ledger.records.len() > HISTORY_LIMIT;
        if trim {
            ledger.records.drain(..ledger.records.len() - HISTORY_LIMIT);
        }
        let records = ledger
            .records
            .iter()
            .cloned()
            .map(LedgerRecord::restore)
            .collect();
        Ok(Self {
            records,
            next: ledger.next_id,
            next_regular: ledger.next_id,
            ledger_path: Some(path),
            ledger_loaded: true,
            ledger_fingerprint: Some(file_fingerprint),
            last_saved: Some(ledger),
            ledger_needs_trim: trim,
            ..Self::default()
        })
    }
    fn persist_preflight(&self, force: bool, now: Instant) -> PersistPreflight {
        if force
            || self.ledger_needs_trim
            || history_needs_pruning(&self.records, |id| {
                self.running.contains_key(&id) || self.pending.contains_key(&id)
            })
        {
            return PersistPreflight::FullWork;
        }
        let Some(saved) = &self.last_saved else {
            return PersistPreflight::FullWork;
        };
        let regular = || self.records.iter().filter(|row| !row.owner_private);
        let high_water = self
            .next_regular
            .max(regular().map(|row| row.id).max().unwrap_or(0));
        if saved.version != LEDGER_VERSION
            || saved.next_id != high_water
            || self.next_regular != high_water
            || self.next < high_water
            || saved.records.len() != regular().count()
        {
            return PersistPreflight::FullWork;
        }
        let mut progress_matches = true;
        for (row, old) in regular().zip(&saved.records) {
            // Exact borrowed equality inherits previous URL/path validation.
            // Changed structure or invalid progress must use full validation.
            if row.received < 0
                || row.total < -1
                || row.id != old.id
                || row.url != old.url
                || row.path != old.path
                || row.status != old.status
                || row.message != old.message
            {
                return PersistPreflight::FullWork;
            }
            progress_matches &= row.received == old.received && row.total == old.total;
        }
        if progress_matches {
            return PersistPreflight::Unchanged;
        }
        if self.last_save_structure == Some(structure(saved))
            && self
                .last_save_attempt
                .is_some_and(|at| now.saturating_duration_since(at) < Duration::from_secs(2))
        {
            PersistPreflight::Throttled
        } else {
            PersistPreflight::FullWork
        }
    }
    fn snapshot(&mut self) -> Ledger {
        self.next_regular = self.next_regular.max(
            self.records
                .iter()
                .filter(|r| !r.owner_private)
                .map(|r| r.id)
                .max()
                .unwrap_or(0),
        );
        self.next = self.next.max(self.next_regular);
        let live: HashSet<_> = self
            .running
            .keys()
            .chain(self.pending.keys())
            .copied()
            .collect();
        prune_history(&mut self.records, &live);
        Ledger {
            version: LEDGER_VERSION,
            next_id: self.next_regular,
            records: self
                .records
                .iter()
                .filter(|r| !r.owner_private)
                .map(LedgerRecord::snapshot)
                .collect(),
        }
    }
}

/// Persist regular history only. Progress is coalesced; structural changes and
/// explicit saves are immediate. Disk and DPAPI work happen outside the borrow.
pub fn persist(manager: &Shared, root: &Path, force: bool) -> Result<(), String> {
    let path = root.join(LEDGER_FILE);
    let (snapshot, expected, loaded) = {
        let mut m = manager.borrow_mut();
        if m.persisting {
            return Ok(());
        }
        if m.ledger_path.as_ref().is_some_and(|saved| saved != &path) {
            return Err("Download history belongs to another browser profile.".into());
        }
        match m.persist_preflight(force, Instant::now()) {
            PersistPreflight::Unchanged => return Ok(()),
            PersistPreflight::Throttled => {
                return m.last_save_error.clone().map_or(Ok(()), Err);
            }
            PersistPreflight::FullWork => {}
        }
        let snapshot = m.snapshot();
        validate_ledger(&snapshot)?;
        if !force && !m.ledger_needs_trim && m.last_saved.as_ref() == Some(&snapshot) {
            return Ok(());
        }
        let current_structure = structure(&snapshot);
        let transition = m.ledger_needs_trim || m.last_save_structure != Some(current_structure);
        if !force
            && !transition
            && m.last_save_attempt
                .is_some_and(|t| t.elapsed() < Duration::from_secs(2))
        {
            return m.last_save_error.clone().map_or(Ok(()), Err);
        }
        m.persisting = true;
        m.last_save_attempt = Some(Instant::now());
        m.last_save_structure = Some(current_structure);
        (snapshot, m.ledger_fingerprint, m.ledger_loaded)
    };
    let result: Result<Option<u64>, String> =
        (|| {
            match storage::read_downloads(root)? {
                Some(current) if !loaded || expected != Some(fingerprint(&current)) => return Err(
                    "Download history changed or was not loaded. The original data was preserved."
                        .into(),
                ),
                Some(_) => {}
                None if expected.is_none() => {}
                None => {
                    return Err(
                        "Download history was removed outside Yas. It was not recreated.".into(),
                    )
                }
            }
            // The common database may exist for bookmarks/passwords even when a
            // private-only session has never created a download ledger record.
            if snapshot.records.is_empty() && snapshot.next_id == 0 && expected.is_none() {
                return Ok(None);
            }
            if manager.borrow().last_saved.as_ref() == Some(&snapshot)
                && !manager.borrow().ledger_needs_trim
            {
                return Ok(expected);
            }
            let encrypted = encode_ledger(&snapshot, MAX_LEDGER_BYTES)?;
            storage::write_downloads_if_unchanged(root, expected, &encrypted)?;
            Ok(Some(fingerprint(&encrypted)))
        })();
    let mut m = manager.borrow_mut();
    m.persisting = false;
    match result {
        Ok(file_fingerprint) => {
            m.ledger_path = Some(path);
            m.ledger_loaded = true;
            m.ledger_fingerprint = file_fingerprint;
            m.last_saved = Some(snapshot);
            m.ledger_needs_trim = false;
            m.last_save_error = None;
            Ok(())
        }
        Err(error) => {
            m.last_save_error = Some(error.clone());
            Err(error)
        }
    }
}
pub fn needs_background(manager: &Shared, include_private: bool) -> bool {
    let m = manager.borrow();
    m.running.keys().any(|id| {
        m.records
            .iter()
            .any(|r| r.id == *id && (include_private || !r.owner_private))
    })
}
pub fn has_private_jobs(manager: &Shared) -> bool {
    let m = manager.borrow();
    m.running
        .keys()
        .any(|id| m.records.iter().any(|r| r.id == *id && r.owner_private))
}
pub fn can_prompt(manager: &Shared, id: usize) -> bool {
    let m = manager.borrow();
    m.pending.contains_key(&id)
        && m.running.get(&id).is_some_and(|job| !job.cancel_requested)
        && m.records
            .iter()
            .any(|r| r.id == id && r.status == "choosing")
}
pub fn pending_ids(manager: &Shared) -> Vec<usize> {
    let mut ids: Vec<_> = manager.borrow().pending.keys().copied().collect();
    ids.retain(|id| can_prompt(manager, *id));
    ids.sort_unstable();
    ids
}
pub fn clear_finished(manager: &Shared) -> usize {
    let mut m = manager.borrow_mut();
    m.next_regular = m.next_regular.max(
        m.records
            .iter()
            .filter(|r| !r.owner_private)
            .map(|r| r.id)
            .max()
            .unwrap_or(0),
    );
    let live: HashSet<_> = m.running.keys().chain(m.pending.keys()).copied().collect();
    let before = m.records.len();
    m.records.retain(|r| {
        live.contains(&r.id)
            || !matches!(
                r.status.as_str(),
                "completed" | "cancelled" | "failed" | "interrupted"
            )
    });
    before - m.records.len()
}
pub fn forget_private_finished(manager: &Shared) -> usize {
    let mut m = manager.borrow_mut();
    let live: HashSet<_> = m.running.keys().chain(m.pending.keys()).copied().collect();
    let before = m.records.len();
    m.records
        .retain(|r| !r.owner_private || live.contains(&r.id));
    before - m.records.len()
}
const SAVE_PATH_ERROR: &str =
    "Could not save to that location. Choose another folder and try again.";
const MONITORING_ERROR: &str = "Yas could not monitor this download. Start the download again.";

struct ProgressMeter {
    sampled_at: Instant,
    sampled_bytes: i64,
    rate: f64,
}
impl ProgressMeter {
    fn new() -> Self {
        Self {
            sampled_at: Instant::now(),
            sampled_bytes: 0,
            rate: 0.,
        }
    }
    fn reset(&mut self, now: Instant, received: i64) {
        self.sampled_at = now;
        self.sampled_bytes = received.max(0);
        self.rate = 0.;
    }
    fn sample(&mut self, now: Instant, received: i64, active: bool) -> f64 {
        let received = received.max(0);
        if !active || received < self.sampled_bytes {
            self.reset(now, received);
            return 0.;
        }
        let elapsed = now.saturating_duration_since(self.sampled_at).as_secs_f64();
        if elapsed >= 0.25 {
            let rate = received.saturating_sub(self.sampled_bytes) as f64 / elapsed;
            self.rate = if rate.is_finite() && rate >= 0. {
                rate
            } else {
                0.
            };
            self.sampled_at = now;
            self.sampled_bytes = received;
        }
        self.rate
    }
}

struct NativeStatus {
    state: COREWEBVIEW2_DOWNLOAD_STATE,
    reason: COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON,
    received: i64,
    total: i64,
    can_resume: bool,
}
impl NativeStatus {
    fn read(op: &ICoreWebView2DownloadOperation) -> windows::core::Result<Self> {
        unsafe {
            let mut state = COREWEBVIEW2_DOWNLOAD_STATE_IN_PROGRESS;
            op.State(&mut state)?;
            let (mut received, mut total) = (0, -1);
            let _ = op.BytesReceived(&mut received);
            let _ = op.TotalBytesToReceive(&mut total);
            let mut reason = COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NONE;
            let _ = op.InterruptReason(&mut reason);
            let mut can_resume = windows::core::BOOL::default();
            let _ = op.CanResume(&mut can_resume);
            Ok(Self {
                state,
                reason,
                received: received.max(0),
                total,
                can_resume: can_resume.as_bool(),
            })
        }
    }
}

// A paused operation is interrupted in WebView2, but is still a live job.
// Its view must remain resident so Continue and Cancel keep working.
fn classify(native: &NativeStatus) -> (&'static str, bool) {
    if native.state == COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED {
        return ("completed", false);
    }
    if native.state != COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED {
        return ("active", true);
    }
    if native.reason == COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_CANCELED {
        return ("cancelled", false);
    }
    if native.reason == COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_PAUSED {
        return ("paused", true);
    }
    if native.can_resume || automatically_restarts(native.reason) {
        ("interrupted", true)
    } else {
        ("failed", false)
    }
}

fn automatically_restarts(reason: COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON) -> bool {
    matches!(
        reason,
        COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_NO_RANGE
            | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_HASH_MISMATCH
            | COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_TOO_SHORT
    )
}

fn native_cancelled(native: &NativeStatus) -> bool {
    native.state == COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED
        && native.reason == COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_CANCELED
}

fn reconcile(
    native: &NativeStatus,
    previous: &str,
    setup_failed: bool,
    cancel_requested: bool,
) -> (&'static str, bool) {
    if setup_failed {
        return (
            "failed",
            cancel_requested
                && !native_cancelled(native)
                && native.state != COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED,
        );
    }
    if previous == "completed" || native.state == COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED {
        return ("completed", false);
    }
    if previous == "cancelled" || cancel_requested {
        return ("cancelled", cancel_requested && !native_cancelled(native));
    }
    classify(native)
}

fn interrupted_message(native: &NativeStatus, status: &str) -> Option<&'static str> {
    match status {
        "interrupted" if !native.can_resume => {
            Some("Waiting for the server to restart this transfer.")
        }
        "interrupted" => Some("The transfer was interrupted. Continue to try again."),
        "failed" => Some(match native.reason {
            COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_UNAUTHORIZED => {
                "The server requires you to sign in before downloading."
            }
            COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_FORBIDDEN => {
                "The server did not allow this download."
            }
            COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_TIMEOUT => {
                "The server timed out. Try downloading again."
            }
            COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_DISCONNECTED => {
                "The network connection was lost. Try downloading again."
            }
            _ => "This download could not finish. Try downloading again.",
        }),
        _ => None,
    }
}

fn sync_status(manager: &Shared, id: usize, native: NativeStatus, force: bool) -> bool {
    let mut m = manager.borrow_mut();
    let live = m.running.contains_key(&id);
    let cancel_requested = m.running.get(&id).is_some_and(|job| job.cancel_requested);
    let setup_error =
        m.records
            .iter()
            .find(|r| r.id == id)
            .and_then(|r| match r.message.as_deref() {
                Some(message) if message == SAVE_PATH_ERROR => Some(SAVE_PATH_ERROR),
                Some(message) if message == MONITORING_ERROR => Some(MONITORING_ERROR),
                _ => None,
            });
    let setup_failed = setup_error.is_some();
    let previous = m
        .records
        .iter()
        .find(|r| r.id == id)
        .map(|r| r.status.as_str())
        .unwrap_or("");
    let freeze_totals = matches!(previous, "completed" | "cancelled")
        && native.state != COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED;
    let choosing_cancel = (cancel_requested || previous == "cancelled")
        && m.records
            .iter()
            .any(|record| record.id == id && record.path.is_none());
    let (status, keep) = if choosing_cancel
        && !setup_failed
        && native.state == COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED
    {
        ("cancelled", false)
    } else {
        reconcile(&native, previous, setup_failed, cancel_requested)
    };
    let choosing = m
        .records
        .iter()
        .any(|r| r.id == id && r.status == "choosing");
    let rate = m
        .running
        .get_mut(&id)
        .map(|job| {
            job.meter.sample(
                Instant::now(),
                native.received,
                !choosing && status == "active",
            )
        })
        .unwrap_or(0.);
    let mut changed = false;
    if let Some(record) = m.records.iter_mut().find(|r| r.id == id) {
        let can_cancel = live
            && !cancel_requested
            && (choosing || keep && matches!(status, "active" | "paused" | "interrupted"));
        changed |= record.can_cancel != can_cancel;
        record.can_cancel = can_cancel;
        if status == "interrupted" && automatically_restarts(native.reason) {
            record.resume_pending = true;
        }
        if native.state == COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED
            || native_cancelled(&native)
            || status == "failed" && !cancel_requested
        {
            record.resume_pending = false;
        }
        if !freeze_totals {
            changed = record.received != native.received || record.total != native.total;
            record.received = native.received;
            record.total = native.total;
        }
        if !choosing {
            let can_resume = live
                && !cancel_requested
                && matches!(status, "paused" | "interrupted")
                && native.can_resume;
            let eta = if status == "active" && rate > 0. && native.total > 0 {
                let seconds = native.total.saturating_sub(native.received).max(0) as f64 / rate;
                (seconds.is_finite() && seconds >= 0.).then_some(seconds)
            } else {
                None
            };
            let message = if setup_failed {
                setup_error
            } else {
                interrupted_message(&native, status)
            };
            changed |= record.status != status
                || record.can_resume != can_resume
                || record.bytes_per_second != rate
                || record.eta_seconds != eta
                || record.message.as_deref() != message;
            if record.status != status {
                record.status = status.into()
            }
            record.ok = status == "completed";
            record.can_resume = can_resume;
            record.bytes_per_second = rate;
            record.eta_seconds = eta;
            if record.message.as_deref() != message {
                record.message = message.map(str::to_owned)
            }
        }
    }
    if status == "cancelled" {
        if let Some(job) = m.running.get_mut(&id) {
            job.tab_lease.release();
        }
    }
    let released = if !choosing && !keep {
        m.running.remove(&id)
    } else {
        None
    };
    let notify = force
        || changed
            && m.last_progress
                .is_none_or(|t| t.elapsed() > Duration::from_millis(150));
    if notify {
        m.last_progress = Some(Instant::now());
    }
    drop(m);
    // Releasing COM objects can dispatch nested callbacks.
    drop(released);
    notify
}

fn update(
    manager: &Shared,
    id: usize,
    op: &ICoreWebView2DownloadOperation,
    terminal: bool,
    proxy: &EventLoopProxy<Msg>,
    generation: u64,
) -> windows::core::Result<()> {
    let native = NativeStatus::read(op)?;
    if is_latest(manager, id, generation) && sync_status(manager, id, native, terminal) {
        let _ = proxy.send_event(Msg::DownloadChanged);
    }
    Ok(())
}

fn is_latest(manager: &Shared, id: usize, generation: u64) -> bool {
    manager
        .borrow()
        .records
        .iter()
        .any(|r| r.id == id && r.generation == generation)
}

fn watch_operation(
    op: &ICoreWebView2DownloadOperation,
    manager: &Shared,
    id: usize,
    generation: u64,
    proxy: &EventLoopProxy<Msg>,
) -> windows::core::Result<(i64, i64)> {
    let weak = Rc::downgrade(manager);
    let progress_proxy = proxy.clone();
    let mut bytes_token = 0;
    unsafe {
        op.add_BytesReceivedChanged(
            &BytesReceivedChangedEventHandler::create(Box::new(move |sender, _| {
                if let (Some(m), Some(op)) = (weak.upgrade(), sender) {
                    update(&m, id, &op, false, &progress_proxy, generation)?;
                }
                Ok(())
            })),
            &mut bytes_token,
        )?;
        let weak = Rc::downgrade(manager);
        let state_proxy = proxy.clone();
        let mut state_token = 0;
        if let Err(error) = op.add_StateChanged(
            &StateChangedEventHandler::create(Box::new(move |sender, _| {
                if let (Some(m), Some(op)) = (weak.upgrade(), sender) {
                    update(&m, id, &op, true, &state_proxy, generation)?;
                }
                Ok(())
            })),
            &mut state_token,
        ) {
            let _ = op.remove_BytesReceivedChanged(bytes_token);
            return Err(error);
        }
        Ok((bytes_token, state_token))
    }
}

fn unwatch(op: &ICoreWebView2DownloadOperation, tokens: Option<(i64, i64)>) {
    if let Some((bytes, state)) = tokens {
        unsafe {
            let _ = op.remove_BytesReceivedChanged(bytes);
            let _ = op.remove_StateChanged(state);
        }
    }
}

fn refresh_latest(
    manager: &Shared,
    id: usize,
    proxy: Option<&EventLoopProxy<Msg>>,
) -> windows::core::Result<()> {
    let current = {
        let m = manager.borrow();
        m.running.get(&id).and_then(|job| {
            m.records
                .iter()
                .find(|r| r.id == id)
                .map(|r| (job.op.clone(), r.generation))
        })
    };
    if let Some((op, generation)) = current {
        let native = NativeStatus::read(&op)?;
        if is_latest(manager, id, generation) && sync_status(manager, id, native, true) {
            if let Some(proxy) = proxy {
                let _ = proxy.send_event(Msg::DownloadChanged);
            }
        }
    }
    Ok(())
}

// Resume may create a new native operation and fire DownloadStarting again.
// Reuse the explicit pending row instead of asking for a second destination.
#[expect(
    clippy::too_many_arguments,
    reason = "Explicit native identity and runtime ownership checks remain together in this callback"
)]
unsafe fn rebind_resume(
    manager: &Shared,
    tab_id: usize,
    view_nonce: u64,
    url: &str,
    op: &ICoreWebView2DownloadOperation,
    args: &ICoreWebView2DownloadStartingEventArgs,
    deferral: &ICoreWebView2Deferral,
    proxy: &EventLoopProxy<Msg>,
) -> windows::core::Result<bool> {
    let candidate = pending_resume(&manager.borrow().records, tab_id, view_nonce, url);
    let Some(id) = candidate else {
        return Ok(false);
    };
    let (generation, path, paused, old) = {
        let mut m = manager.borrow_mut();
        let live = m.running.get(&id).is_some_and(|job| !job.cancel_requested);
        let record = m.records.iter_mut().find(|r| r.id == id).unwrap();
        record.resume_pending = false;
        let paused = record.pause_requested;
        let path = if live && matches!(record.status.as_str(), "active" | "paused" | "interrupted")
        {
            record.path.clone()
        } else {
            None
        };
        if path.is_some() {
            record.generation = record.generation.wrapping_add(1);
        }
        let generation = record.generation;
        let old = if path.is_some() {
            let job = m.running.get_mut(&id).unwrap();
            job.meter.reset(Instant::now(), 0);
            job.owners.push(job.op.clone());
            Some((
                std::mem::replace(&mut job.op, op.clone()),
                job.tokens.take(),
            ))
        } else {
            None
        };
        (generation, path, paused, old)
    };
    if let Some((old, tokens)) = old {
        unwatch(&old, tokens);
        drop(old);
    }
    let Some(path) = path else {
        args.SetCancel(true)?;
        deferral.Complete()?;
        return Ok(true);
    };
    let tokens = match watch_operation(op, manager, id, generation, proxy) {
        Ok(tokens) => tokens,
        Err(error) => {
            let _ = args.SetCancel(true);
            let _ = deferral.Complete();
            let _ = cancel(manager, id);
            return Err(error);
        }
    };
    if let Some(job) = manager.borrow_mut().running.get_mut(&id) {
        job.tokens = Some(tokens);
    }
    if let Err(error) = args
        .SetResultFilePath(&HSTRING::from(path))
        .and_then(|_| args.SetCancel(false))
    {
        let _ = args.SetCancel(true);
        let _ = deferral.Complete();
        let _ = cancel(manager, id);
        return Err(error);
    }
    deferral.Complete()?;
    if paused {
        let _ = op.Pause();
    }
    refresh_latest(manager, id, Some(proxy))?;
    Ok(true)
}

fn pending_resume(records: &[Record], tab_id: usize, view_nonce: u64, url: &str) -> Option<usize> {
    records
        .iter()
        .find(|r| {
            r.owner_tab == tab_id && r.owner_view == view_nonce && r.url == url && r.resume_pending
        })
        .map(|r| r.id)
}

/// Reconcile native truth even when a resumed operation misses an event. The
/// Win32 SDK has no CanResumeChanged event. Use the browser's existing idle tick.
pub fn poll(manager: &Shared, proxy: &EventLoopProxy<Msg>) {
    let jobs: Vec<_> = {
        let m = manager.borrow();
        m.running
            .iter()
            .filter(|(id, job)| {
                job.cancel_requested
                    || m.records.iter().any(|r| {
                        r.id == **id
                            && matches!(r.status.as_str(), "active" | "paused" | "interrupted")
                    })
            })
            .filter_map(|(id, job)| {
                m.records
                    .iter()
                    .find(|r| r.id == *id)
                    .map(|r| (*id, job.op.clone(), r.generation))
            })
            .collect()
    };
    for (id, op, generation) in jobs {
        let _ = update(manager, id, &op, false, proxy, generation);
    }
}
fn fail_start(manager: &Shared, id: usize) {
    let (pending, mut released) = {
        let mut m = manager.borrow_mut();
        if let Some(record) = m.records.iter_mut().find(|r| r.id == id) {
            record.status = "failed".into();
            record.ok = false;
            record.can_resume = false;
            record.can_cancel = false;
            record.resume_pending = false;
            record.pause_requested = false;
            record.bytes_per_second = 0.;
            record.eta_seconds = None;
            record.message = Some(MONITORING_ERROR.into());
        }
        (m.pending.remove(&id), m.running.remove(&id))
    };
    // Neither deferral completion nor native Cancel may run under the
    // Manager borrow. Running::drop releases the tab counter exactly once.
    if let Some(job) = released.as_mut() {
        job.cancel_requested = true;
    }
    drop(pending);
    if let Some(job) = &released {
        unsafe {
            let _ = job.op.Cancel();
        }
    }
    drop(released);
}
pub fn attach(
    view: &WebView,
    manager: Shared,
    tab_id: usize,
    private: bool,
    tab_counter: Rc<Cell<usize>>,
    proxy: EventLoopProxy<Msg>,
) -> windows::core::Result<()> {
    let view_nonce = {
        let mut m = manager.borrow_mut();
        m.next_view = m.next_view.wrapping_add(1);
        m.next_view
    };
    let weak = Rc::downgrade(&manager);
    unsafe {
        let mut token = 0;
        view.webview()
            .cast::<ICoreWebView2_4>()?
            .add_DownloadStarting(
                &DownloadStartingEventHandler::create(Box::new(move |_, args| {
                    let Some(args) = args else { return Ok(()) };
                    let Some(manager) = weak.upgrade() else {
                        args.SetCancel(true)?;
                        return Ok(());
                    };
                    // Defer before returning from WebView2. Modal UI runs later in Tao's event loop.
                    let deferral = args.GetDeferral()?;
                    let op = args.DownloadOperation()?;
                    if NativeStatus::read(&op).is_ok_and(|native| {
                        native.reason == COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_CANCELED
                    }) {
                        args.SetCancel(true)?;
                        deferral.Complete()?;
                        return Ok(());
                    }
                    let mut raw = PWSTR::null();
                    args.ResultFilePath(&mut raw)?;
                    let suggested = PathBuf::from(take_pwstr(raw));
                    let mut raw = PWSTR::null();
                    op.Uri(&mut raw)?;
                    let url = take_pwstr(raw);
                    args.SetHandled(true)?;
                    if rebind_resume(
                        &manager, tab_id, view_nonce, &url, &op, &args, &deferral, &proxy,
                    )? {
                        return Ok(());
                    }
                    let id = {
                        let mut m = manager.borrow_mut();
                        m.next += 1;
                        let id = m.next;
                        if !private {
                            m.next_regular = id;
                        }
                        m.records.push(Record {
                            id,
                            url,
                            path: None,
                            status: "choosing".into(),
                            received: 0,
                            total: -1,
                            ok: false,
                            can_resume: false,
                            can_cancel: true,
                            bytes_per_second: 0.,
                            eta_seconds: None,
                            message: None,
                            owner_tab: tab_id,
                            owner_private: private,
                            owner_view: view_nonce,
                            generation: 0,
                            resume_pending: false,
                            pause_requested: false,
                        });
                        m.running.insert(
                            id,
                            Running {
                                tab_id,
                                op: op.clone(),
                                tab_lease: TabLease::new(tab_counter.clone()),
                                meter: ProgressMeter::new(),
                                tokens: None,
                                owners: Vec::new(),
                                cancel_requested: false,
                            },
                        );
                        m.pending.insert(
                            id,
                            Pending {
                                args,
                                deferral: Some(deferral),
                                suggested,
                            },
                        );
                        id
                    };
                    let tokens = match watch_operation(&op, &manager, id, 0, &proxy) {
                        Ok(tokens) => tokens,
                        Err(error) => {
                            fail_start(&manager, id);
                            let _ = proxy.send_event(Msg::DownloadChanged);
                            return Err(error);
                        }
                    };
                    if let Some(job) = manager.borrow_mut().running.get_mut(&id) {
                        job.tokens = Some(tokens);
                    }
                    let _ = proxy.send_event(Msg::RefreshUrl(tab_id));
                    let _ = proxy.send_event(Msg::DownloadPrompt(id));
                    Ok(())
                })),
                &mut token,
            )?;
    }
    Ok(())
}
pub fn prompt(manager: &Shared, id: usize, window: &Window, proxy: &EventLoopProxy<Msg>) {
    if !can_prompt(manager, id) {
        return;
    }
    let Some(mut pending) = manager.borrow_mut().pending.remove(&id) else {
        return;
    };
    let filename = pending
        .suggested
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("download");
    let mut dialog = rfd::FileDialog::new()
        .set_parent(window)
        .set_title("Save download — Yas")
        .set_file_name(filename);
    if let Some(parent) = pending.suggested.parent().filter(|p| p.is_dir()) {
        dialog = dialog.set_directory(parent);
    }
    let destination = dialog.save_file();
    // A close/cancel dispatched by a nested native modal loop must not revive a job.
    let destination = if manager
        .borrow()
        .running
        .get(&id)
        .is_some_and(|job| !job.cancel_requested)
    {
        destination
    } else {
        None
    };
    let accepted = destination.is_some();
    {
        let mut m = manager.borrow_mut();
        if let Some(record) = m.records.iter_mut().find(|r| r.id == id) {
            record.status = if accepted { "active" } else { "cancelled" }.into();
            record.path = destination
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned());
            record.can_resume = false;
            record.can_cancel = accepted;
            record.bytes_per_second = 0.;
            record.eta_seconds = None;
        }
        if !accepted {
            if let Some(job) = m.running.get_mut(&id) {
                job.cancel_requested = true;
            }
        }
    }
    unsafe {
        let result = if let Some(path) = destination {
            pending
                .args
                .SetResultFilePath(&HSTRING::from(path.as_os_str()))
                .and_then(|_| pending.args.SetCancel(false))
        } else {
            pending.args.SetCancel(true)
        };
        if result.is_err() {
            {
                let mut m = manager.borrow_mut();
                if let Some(r) = m.records.iter_mut().find(|r| r.id == id) {
                    r.status = "failed".into();
                    r.can_cancel = false;
                    r.message = Some(SAVE_PATH_ERROR.into());
                }
                if let Some(job) = m.running.get_mut(&id) {
                    job.cancel_requested = true;
                }
            }
            let _ = pending.args.SetCancel(true);
        }
        if let Some(d) = pending.deferral.take() {
            let _ = d.Complete();
        }
    }
    // Preserve native ownership until cancellation acknowledgement. A tiny body
    // can also complete while the save dialog is open.
    let _ = refresh_latest(manager, id, Some(proxy));
    let _ = proxy.send_event(Msg::DownloadChanged);
}
pub fn pause(manager: &Shared, id: usize) -> Result<(), String> {
    {
        let mut m = manager.borrow_mut();
        if !m.records.iter().any(|r| r.id == id && r.status == "active") {
            return Err("Only a running download can be paused.".into());
        }
        if let Some(record) = m.records.iter_mut().find(|r| r.id == id) {
            record.pause_requested = true;
        }
    }
    // A COM property read can dispatch the resumed DownloadStarting and replace
    // the operation. Re-read the new generation before controlling it.
    for _ in 0..3 {
        let (op, generation) = {
            let m = manager.borrow();
            m.running
                .get(&id)
                .and_then(|job| {
                    m.records
                        .iter()
                        .find(|r| r.id == id)
                        .map(|record| (job.op.clone(), record.generation))
                })
                .ok_or("This download is no longer active")?
        };
        let current = NativeStatus::read(&op).map_err(|e| e.to_string())?;
        if !is_latest(manager, id, generation) {
            continue;
        }
        if current.state != COREWEBVIEW2_DOWNLOAD_STATE_IN_PROGRESS {
            let _ = refresh_latest(manager, id, None);
            return Err("This download is not running.".into());
        }
        if let Some(job) = manager.borrow_mut().running.get_mut(&id) {
            job.meter.reset(Instant::now(), current.received);
        }
        let result = unsafe { op.Pause() };
        if !is_latest(manager, id, generation) {
            continue;
        }
        result.map_err(|e| format!("Could not pause this download: {e}"))?;
        refresh_latest(manager, id, None).map_err(|e| e.to_string())?;
        return Ok(());
    }
    Err("The download changed while pausing. Try again.".into())
}
fn begin_resume(record: &mut Record, generation: u64, cancel_requested: Option<bool>) -> bool {
    if record.generation != generation
        || cancel_requested != Some(false)
        || !matches!(record.status.as_str(), "paused" | "interrupted")
    {
        return false;
    }
    record.resume_pending = true;
    record.pause_requested = false;
    record.status = "active".into();
    record.ok = false;
    record.can_resume = false;
    record.can_cancel = true;
    record.bytes_per_second = 0.;
    record.eta_seconds = None;
    record.message = None;
    true
}
fn clear_failed_resume(record: &mut Record, generation: u64, cancel_requested: Option<bool>) {
    if record.generation == generation
        && cancel_requested == Some(false)
        && record.status == "active"
    {
        record.resume_pending = false;
    }
}
pub fn resume(manager: &Shared, id: usize) -> Result<(), String> {
    for _ in 0..3 {
        let (op, generation) = {
            let m = manager.borrow();
            let record = m
                .records
                .iter()
                .find(|r| r.id == id && matches!(r.status.as_str(), "paused" | "interrupted"))
                .ok_or("This download is not paused or waiting to continue.")?;
            let job = m
                .running
                .get(&id)
                .filter(|job| !job.cancel_requested)
                .ok_or("This download is no longer available to continue")?;
            (job.op.clone(), record.generation)
        };
        // COM reads can pump callbacks that cancel, complete, close or rebind
        // this operation. Revalidate its live owner and generation before the
        // Active transition, under one borrow with no intervening COM calls.
        let current = NativeStatus::read(&op).map_err(|e| e.to_string())?;
        if !is_latest(manager, id, generation) {
            continue;
        }
        if current.state != COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED || !current.can_resume {
            let _ = refresh_latest(manager, id, None);
            return Err("This download cannot be continued. Try downloading it again.".into());
        }
        let began = {
            let mut m = manager.borrow_mut();
            let cancellation = m.running.get(&id).map(|job| job.cancel_requested);
            let began = m
                .records
                .iter_mut()
                .find(|r| r.id == id)
                .is_some_and(|record| begin_resume(record, generation, cancellation));
            if began {
                if let Some(job) = m.running.get_mut(&id) {
                    job.meter.reset(Instant::now(), current.received);
                }
            }
            began
        };
        if !began {
            continue;
        }
        if let Err(error) = unsafe { op.Resume() } {
            // A newer handoff owns its own pending marker. A failed call on
            // the old native operation must not clear that newer generation.
            {
                let mut m = manager.borrow_mut();
                let cancellation = m.running.get(&id).map(|job| job.cancel_requested);
                if let Some(record) = m.records.iter_mut().find(|r| r.id == id) {
                    clear_failed_resume(record, generation, cancellation);
                }
            }
            let _ = refresh_latest(manager, id, None);
            return Err(format!("Could not continue this download: {error}"));
        }
        refresh_latest(manager, id, None).map_err(|e| e.to_string())?;
        return Ok(());
    }
    Err("The download changed while continuing. Try again.".into())
}
pub fn cancel(manager: &Shared, id: usize) -> Result<(), String> {
    let (op, choosing, generation, previous, pending_cancel) = {
        let m = manager.borrow();
        (
            m.running
                .get(&id)
                .map(|r| r.op.clone())
                .ok_or("This download is no longer active")?,
            m.records
                .iter()
                .any(|r| r.id == id && r.status == "choosing"),
            m.records
                .iter()
                .find(|r| r.id == id)
                .map(|r| r.generation)
                .unwrap_or(0),
            m.records
                .iter()
                .find(|r| r.id == id)
                .map(|r| r.status.clone())
                .unwrap_or_default(),
            m.running.get(&id).is_some_and(|job| job.cancel_requested),
        )
    };
    if pending_cancel {
        let _ = refresh_latest(manager, id, None);
        return Ok(());
    }
    // Latch cancellation before even reading COM properties, because those
    // reads can dispatch a nested resumed DownloadStarting.
    {
        let mut m = manager.borrow_mut();
        if let Some(job) = m.running.get_mut(&id) {
            job.cancel_requested = true;
        }
        if let Some(record) = m.records.iter_mut().find(|r| r.id == id) {
            record.status = "cancelled".into();
            record.ok = false;
            record.can_resume = false;
            record.can_cancel = false;
            record.bytes_per_second = 0.;
            record.eta_seconds = None;
            record.message = None;
        }
    }
    if choosing {
        let pending = manager.borrow_mut().pending.remove(&id);
        drop(pending);
    }
    if let Ok(before) = NativeStatus::read(&op) {
        if !choosing
            && before.state == COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED
            && is_latest(manager, id, generation)
        {
            sync_status(manager, id, before, true);
            return Err("This download has already finished.".into());
        }
    }
    let result = unsafe { op.Cancel() };
    let after = NativeStatus::read(&op).ok();
    let acknowledged = after.as_ref().is_some_and(|native| {
        native_cancelled(native) || native.state == COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED
    });
    if let Some(after) = after {
        if !choosing
            && after.state == COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED
            && is_latest(manager, id, generation)
        {
            sync_status(manager, id, after, true);
            return Err("This download has already finished.".into());
        }
        if (result.is_ok() || acknowledged) && is_latest(manager, id, generation) {
            sync_status(manager, id, after, true);
        }
    }
    if let Err(error) = result {
        if !acknowledged {
            if is_latest(manager, id, generation) {
                let mut m = manager.borrow_mut();
                if let Some(job) = m.running.get_mut(&id) {
                    job.cancel_requested = false;
                    job.tab_lease.acquire();
                }
                if let Some(record) = m
                    .records
                    .iter_mut()
                    .find(|r| r.id == id && r.status == "cancelled")
                {
                    record.status = previous;
                    record.can_cancel = matches!(
                        record.status.as_str(),
                        "choosing" | "active" | "paused" | "interrupted"
                    );
                }
            }
            let _ = refresh_latest(manager, id, None);
            return Err(error.to_string());
        }
    }
    Ok(())
}
pub fn cancel_all(manager: &Shared) {
    let ids: Vec<_> = manager.borrow().running.keys().copied().collect();
    for id in ids {
        let _ = cancel(manager, id);
    }
    // Deferrals can invoke native callbacks, so drop them outside RefCell borrows.
    let pending: Vec<_> = manager
        .borrow_mut()
        .pending
        .drain()
        .map(|(_, p)| p)
        .collect();
    drop(pending);
    let remaining: Vec<_> = manager
        .borrow_mut()
        .running
        .drain()
        .map(|(_, r)| r)
        .collect();
    {
        let mut m = manager.borrow_mut();
        for record in &mut m.records {
            record.can_cancel = false;
            record.can_resume = false;
        }
    }
    drop(remaining);
}
pub fn cancel_tab(manager: &Shared, tab_id: usize) {
    let ids: Vec<_> = manager
        .borrow()
        .running
        .iter()
        .filter(|(_, job)| job.tab_id == tab_id)
        .map(|(id, _)| *id)
        .collect();
    for id in ids {
        let _ = cancel(manager, id);
        let pending = manager.borrow_mut().pending.remove(&id);
        drop(pending);
        // The owning view is being destroyed, so it cannot support Continue or
        // deliver an acknowledgement. Its nonce still rejects future views.
        let released = {
            let mut m = manager.borrow_mut();
            if let Some(record) = m.records.iter_mut().find(|r| r.id == id) {
                if !matches!(record.status.as_str(), "completed" | "failed" | "cancelled") {
                    record.status = "cancelled".into();
                    record.message = None;
                }
                record.ok = record.status == "completed";
                record.can_resume = false;
                record.can_cancel = false;
                record.bytes_per_second = 0.;
                record.eta_seconds = None;
            }
            m.running.remove(&id)
        };
        drop(released);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancelled_tombstone_releases_tab_once_and_failed_cancel_can_reacquire() {
        let counter = Rc::new(Cell::new(0));
        let mut cancelled = TabLease::new(counter.clone());
        let other = TabLease::new(counter.clone());
        assert_eq!(counter.get(), 2);
        cancelled.release();
        cancelled.release();
        assert_eq!(counter.get(), 1);
        cancelled.acquire();
        cancelled.acquire();
        assert_eq!(counter.get(), 2);
        cancelled.release();
        drop(cancelled);
        assert_eq!(counter.get(), 1);
        drop(other);
        assert_eq!(counter.get(), 0);
    }
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };
    struct TestDirectory {
        path: PathBuf,
        store: Option<storage::ProfileStore>,
    }
    impl TestDirectory {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let tick = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "yas-download-ledger-test-{}-{tick}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            // Match Browser's profile lifetime, including rejection and
            // preservation checks after an externally corrupted ledger row.
            let store = Some(storage::open_profile(&path).unwrap());
            Self { path, store }
        }
        fn path(&self) -> &Path {
            &self.path
        }
    }
    impl Drop for TestDirectory {
        fn drop(&mut self) {
            self.store.take();
            // Only remove this test's explicit, uniquely generated temp child.
            if self.path.parent() == Some(std::env::temp_dir().as_path())
                && self
                    .path
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("yas-download-ledger-test-"))
            {
                let _ = fs::remove_dir_all(&self.path);
            }
        }
    }
    fn saved_record(root: &Path, id: usize, status: &str, private: bool) -> Record {
        let mut row = record();
        row.id = id;
        row.status = status.into();
        row.ok = status == "completed";
        row.url = format!("https://example.test/file/{id}?signed=fixture-token-{id}");
        row.path = (status != "choosing").then(|| {
            root.join(format!("fixture-{id}.bin"))
                .to_string_lossy()
                .into_owned()
        });
        row.owner_private = private;
        row.can_resume = matches!(status, "paused" | "interrupted");
        row.can_cancel = matches!(status, "choosing" | "active" | "paused" | "interrupted");
        row
    }
    fn test_manager(root: &Path, records: Vec<Record>) -> Shared {
        let mut manager = Manager::load(root).unwrap();
        manager.records = records;
        Rc::new(RefCell::new(manager))
    }
    fn read_test_ledger(root: &Path) -> Ledger {
        decode_ledger(&read_test_ciphertext(root), false).unwrap()
    }
    fn read_test_ciphertext(root: &Path) -> Vec<u8> {
        storage::read_downloads(root).unwrap().unwrap()
    }
    fn write_test_ciphertext(root: &Path, ciphertext: &[u8]) {
        let current = storage::read_downloads(root).unwrap();
        let expected = current.as_deref().map(fingerprint);
        storage::write_downloads_if_unchanged(root, expected, ciphertext).unwrap();
    }
    fn legacy_ciphertext(ledger: &Ledger) -> Vec<u8> {
        protect_ledger(serde_json::to_vec(ledger).unwrap(), MAX_LEDGER_BYTES).unwrap()
    }
    fn record() -> Record {
        Record {
            id: 7,
            url: "https://example.test/file".into(),
            path: Some("file.bin".into()),
            status: "cancelled".into(),
            received: 2048,
            total: 10000,
            ok: false,
            can_resume: false,
            can_cancel: false,
            bytes_per_second: 0.,
            eta_seconds: None,
            message: None,
            owner_tab: 3,
            owner_private: false,
            owner_view: 11,
            generation: 2,
            resume_pending: true,
            pause_requested: false,
        }
    }
    #[test]
    fn resumed_events_require_the_pending_owner_and_url_and_are_consumed_once() {
        let mut records = vec![record()];
        assert_eq!(
            pending_resume(&records, 3, 11, "https://example.test/file"),
            Some(7)
        );
        assert_eq!(
            pending_resume(&records, 4, 11, "https://example.test/file"),
            None
        );
        assert_eq!(
            pending_resume(&records, 3, 11, "https://example.test/other"),
            None
        );
        assert_eq!(
            pending_resume(&records, 3, 12, "https://example.test/file"),
            None
        );
        // Even a cancelled row is matched so its late restart is cancelled,
        // rather than creating a fresh row and opening another Save dialog.
        records[0].resume_pending = false;
        assert_eq!(
            pending_resume(&records, 3, 11, "https://example.test/file"),
            None
        );
    }
    #[test]
    fn latest_generation_survives_release_but_stale_operations_cannot_overwrite_it() {
        let manager = Rc::new(RefCell::new(Manager {
            records: vec![record()],
            ..Manager::default()
        }));
        assert!(manager.borrow().running.is_empty());
        assert!(!is_latest(&manager, 7, 1));
        assert!(is_latest(&manager, 7, 2));
        let mut native = interrupted(COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NONE, false);
        native.state = COREWEBVIEW2_DOWNLOAD_STATE_IN_PROGRESS;
        sync_status(&manager, 7, native, true);
        assert_eq!(manager.borrow().records[0].status, "cancelled");
        let mut completed = interrupted(COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NONE, false);
        completed.state = COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED;
        sync_status(&manager, 7, completed, true);
        assert_eq!(manager.borrow().records[0].status, "completed");
    }
    fn interrupted(
        reason: COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON,
        can_resume: bool,
    ) -> NativeStatus {
        NativeStatus {
            state: COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED,
            reason,
            received: 2048,
            total: 10000,
            can_resume,
        }
    }
    #[test]
    fn pausing_keeps_the_native_job_even_before_resume_is_available() {
        assert_eq!(
            classify(&interrupted(
                COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_PAUSED,
                false
            )),
            ("paused", true)
        );
        assert_eq!(
            classify(&interrupted(
                COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_PAUSED,
                true
            )),
            ("paused", true)
        );
        assert_eq!(
            classify(&interrupted(
                COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_CANCELED,
                true
            )),
            ("cancelled", false)
        );
    }
    #[test]
    fn resumable_and_automatic_restarts_keep_the_job_but_fatal_errors_release_it() {
        assert_eq!(
            classify(&interrupted(
                COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NETWORK_TIMEOUT,
                true
            )),
            ("interrupted", true)
        );
        for reason in [
            COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_NO_RANGE,
            COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_HASH_MISMATCH,
            COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_FILE_TOO_SHORT,
        ] {
            assert_eq!(classify(&interrupted(reason, false)), ("interrupted", true));
        }
        assert_eq!(
            classify(&interrupted(
                COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_SERVER_FORBIDDEN,
                false
            )),
            ("failed", false)
        );
        let mut done = interrupted(COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_PAUSED, true);
        done.state = COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED;
        assert_eq!(classify(&done), ("completed", false));
    }
    #[test]
    fn speed_excludes_paused_time_and_resets_when_a_transfer_restarts() {
        let start = Instant::now();
        let mut meter = ProgressMeter::new();
        meter.reset(start, 0);
        assert_eq!(
            meter.sample(start + Duration::from_secs(1), 1000, true),
            1000.
        );
        assert_eq!(
            meter.sample(start + Duration::from_secs(10), 1000, false),
            0.
        );
        assert_eq!(
            meter.sample(start + Duration::from_secs(11), 2000, true),
            1000.
        );
        assert_eq!(meter.sample(start + Duration::from_secs(12), 10, true), 0.);
        assert_eq!(
            meter.sample(start + Duration::from_secs(13), 1010, true),
            1000.
        );
    }
    #[test]
    fn cancelled_progress_cannot_revive_a_job_and_native_completion_wins_the_cancel_race() {
        let mut native = interrupted(COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_NONE, false);
        native.state = COREWEBVIEW2_DOWNLOAD_STATE_IN_PROGRESS;
        assert_eq!(
            reconcile(&native, "cancelled", false, false),
            ("cancelled", false)
        );
        native.state = COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED;
        assert_eq!(
            reconcile(&native, "cancelled", false, false),
            ("completed", false)
        );
        assert_eq!(reconcile(&native, "active", true, false), ("failed", false));
        native.state = COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED;
        native.reason = COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_CANCELED;
        assert_eq!(
            reconcile(&native, "completed", false, false),
            ("completed", false)
        );
    }
    #[test]
    fn cancellation_retains_owners_until_native_acknowledgement() {
        let mut native = interrupted(COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_PAUSED, true);
        assert_eq!(
            reconcile(&native, "cancelled", false, true),
            ("cancelled", true)
        );
        native.state = COREWEBVIEW2_DOWNLOAD_STATE_IN_PROGRESS;
        assert_eq!(
            reconcile(&native, "cancelled", false, true),
            ("cancelled", true)
        );
        assert_eq!(reconcile(&native, "failed", true, true), ("failed", true));
        native.state = COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED;
        native.reason = COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_CANCELED;
        assert_eq!(
            reconcile(&native, "cancelled", false, true),
            ("cancelled", false)
        );
        assert_eq!(reconcile(&native, "failed", true, true), ("failed", false));
        native.state = COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED;
        assert_eq!(
            reconcile(&native, "cancelled", false, true),
            ("completed", false)
        );
    }
    #[test]
    fn acknowledged_cancellation_retires_the_marker_for_a_same_view_retry() {
        let manager = Rc::new(RefCell::new(Manager {
            records: vec![record()],
            ..Manager::default()
        }));
        let native = interrupted(COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_CANCELED, false);
        sync_status(&manager, 7, native, true);
        let m = manager.borrow();
        assert_eq!(m.records[0].status, "cancelled");
        assert_eq!(
            pending_resume(&m.records, 3, 11, "https://example.test/file"),
            None
        );
    }
    #[test]
    fn unselected_cancelled_save_cannot_become_completed_after_ownership_release() {
        let mut cancelled = record();
        cancelled.path = None;
        cancelled.resume_pending = false;
        let manager = Rc::new(RefCell::new(Manager {
            records: vec![cancelled],
            ..Manager::default()
        }));
        let mut native = interrupted(COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_CANCELED, false);
        native.state = COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED;
        sync_status(&manager, 7, native, true);
        assert_eq!(manager.borrow().records[0].status, "cancelled");
        assert!(!manager.borrow().records[0].ok);
    }
    #[test]
    fn resume_commit_cannot_revive_cancelled_completed_closed_or_rebound_operations() {
        for (status, generation, cancellation) in [
            ("cancelled", 2, Some(false)),
            ("completed", 2, Some(false)),
            ("failed", 2, Some(false)),
            ("paused", 2, Some(true)),
            ("paused", 2, None),
            ("paused", 3, Some(false)),
        ] {
            let mut row = record();
            row.status = status.into();
            row.generation = generation;
            row.resume_pending = false;
            let before = serde_json::to_value(&row).unwrap();
            assert!(!begin_resume(&mut row, 2, cancellation));
            assert_eq!(serde_json::to_value(&row).unwrap(), before);
            assert!(!row.resume_pending);
        }
    }
    #[test]
    fn resume_commit_uses_only_the_current_live_owner_and_resets_stale_progress() {
        for status in ["paused", "interrupted"] {
            let mut row = record();
            row.status = status.into();
            row.resume_pending = false;
            row.pause_requested = true;
            row.can_resume = true;
            row.can_cancel = false;
            row.bytes_per_second = 1500.;
            row.eta_seconds = Some(5.);
            row.message = Some("Waiting for a previous attempt".into());
            assert!(begin_resume(&mut row, 2, Some(false)));
            assert_eq!(row.status, "active");
            assert!(row.resume_pending && !row.pause_requested && row.can_cancel);
            assert!(!row.can_resume);
            assert_eq!(row.bytes_per_second, 0.);
            assert!(row.eta_seconds.is_none() && row.message.is_none());
            assert_eq!((row.received, row.total), (2048, 10000));
        }
    }
    #[test]
    fn an_old_resume_failure_cannot_retire_newer_or_cancel_pending_handoff_markers() {
        for (status, generation, cancellation, should_keep) in [
            ("active", 2, Some(false), false),
            ("active", 3, Some(false), true),
            ("cancelled", 2, Some(true), true),
            ("cancelled", 2, None, true),
        ] {
            let mut row = record();
            row.status = status.into();
            row.generation = generation;
            row.resume_pending = true;
            clear_failed_resume(&mut row, 2, cancellation);
            assert_eq!(row.resume_pending, should_keep);
            assert_eq!(row.status, status);
        }
    }
    #[test]
    fn encrypted_history_limit_includes_dpapi_overhead_before_any_file_write() {
        let root = TestDirectory::new();
        let ledger = Ledger {
            version: LEDGER_VERSION,
            next_id: 7,
            records: vec![LedgerRecord::snapshot(&saved_record(
                root.path(),
                7,
                "completed",
                false,
            ))],
        };
        let plain_size = postcard::to_allocvec(&ledger).unwrap().len();
        let encrypted = encode_ledger(&ledger, MAX_LEDGER_BYTES).unwrap();
        assert!(encrypted.len() > plain_size);
        // A scaled boundary exercises real DPAPI without allocating a64MiB
        // fixture. This plaintext fits, but its encrypted envelope does not.
        assert!(encode_ledger(&ledger, encrypted.len() - 1).is_err());
        assert_eq!(
            encode_ledger(&ledger, encrypted.len()).unwrap().len(),
            encrypted.len()
        );
        assert!(storage::read_downloads(root.path()).unwrap().is_none());
    }
    #[test]
    fn legacy_migration_and_explicit_export_preserve_every_row_order_and_counter() {
        let root = TestDirectory::new();
        let mut rows: Vec<_> = (1..=HISTORY_LIMIT + 7)
            .rev()
            .map(|id| LedgerRecord::snapshot(&saved_record(root.path(), id, "completed", false)))
            .collect();
        // Duplicate URLs and non-ASCII metadata must not be deduplicated or
        // normalized by migration. Retention limits belong to Manager::load.
        rows[1].url = rows[0].url.clone();
        rows[1].message = Some("  UAE دبي fixture  ".into());
        rows[2].status = "paused".into();
        rows[2].total = -1;
        let ledger = Ledger {
            version: LEDGER_VERSION,
            next_id: 900,
            records: rows,
        };
        let legacy = legacy_ciphertext(&ledger);
        let original = legacy.clone();
        let migrated = migrate_legacy_ledger(&legacy).unwrap();
        assert!(decode_ledger(&migrated, false).unwrap() == ledger);
        assert!(ledgers_equal(&legacy, &migrated).unwrap());
        assert_eq!(legacy, original);
        let exported = export_legacy_ledger(&migrated).unwrap();
        assert!(decode_ledger(&exported, true).unwrap() == ledger);
        assert!(ledgers_equal(&exported, &migrated).unwrap());
        assert!(storage::read_downloads(root.path()).unwrap().is_none());

        write_test_ciphertext(root.path(), &migrated);
        let restored = Manager::load(root.path()).unwrap();
        assert_eq!(restored.records.len(), HISTORY_LIMIT);
        assert_eq!(restored.next_regular, 900);
        assert!(restored.ledger_needs_trim);
        // Loading applies the existing history policy only in memory.
        assert!(decode_ledger(&read_test_ciphertext(root.path()), false).unwrap() == ledger);
    }
    #[test]
    fn ledger_codecs_reject_trailing_bytes_invalid_versions_and_mixed_formats() {
        let root = TestDirectory::new();
        let ledger = Ledger {
            version: LEDGER_VERSION,
            next_id: 7,
            records: vec![LedgerRecord::snapshot(&saved_record(
                root.path(),
                7,
                "completed",
                false,
            ))],
        };
        let legacy = legacy_ciphertext(&ledger);
        let migrated = migrate_legacy_ledger(&legacy).unwrap();
        assert!(export_legacy_ledger(&legacy).is_err());
        assert!(migrate_legacy_ledger(&migrated).is_err());
        let mut trailing = postcard::to_allocvec(&ledger).unwrap();
        trailing.push(1);
        let trailing = protect_ledger(trailing, MAX_LEDGER_BYTES).unwrap();
        assert!(decode_ledger(&trailing, false).is_err());
        assert!(export_legacy_ledger(&trailing).is_err());
        assert!(ledgers_equal(&legacy, &trailing).is_err());
        let mut unsupported = ledger.clone();
        unsupported.version += 1;
        assert!(migrate_legacy_ledger(&legacy_ciphertext(&unsupported)).is_err());
        assert!(decode_ledger(
            &encode_ledger(&unsupported, MAX_LEDGER_BYTES).unwrap(),
            false
        )
        .is_err());
        let mut different = ledger;
        different.records[0].received += 1;
        assert!(!ledgers_equal(
            &legacy,
            &encode_ledger(&different, MAX_LEDGER_BYTES).unwrap()
        )
        .unwrap());
    }
    #[test]
    fn ledger_record_cas_rejects_a_stale_writer_without_touching_the_newer_ciphertext() {
        let root = TestDirectory::new();
        let first = test_manager(
            root.path(),
            vec![saved_record(root.path(), 7, "active", false)],
        );
        persist(&first, root.path(), true).unwrap();
        let stale = fingerprint(&read_test_ciphertext(root.path()));
        let second = Rc::new(RefCell::new(Manager::load(root.path()).unwrap()));
        first.borrow_mut().records[0].status = "paused".into();
        persist(&first, root.path(), true).unwrap();
        let newer = read_test_ciphertext(root.path());
        assert!(
            storage::write_downloads_if_unchanged(root.path(), Some(stale), b"stale fixture")
                .is_err()
        );
        second.borrow_mut().records[0].received += 1;
        assert!(persist(&second, root.path(), true).is_err());
        assert_eq!(read_test_ciphertext(root.path()), newer);
    }
    #[test]
    fn unrelated_database_settings_do_not_conflict_with_the_download_record() {
        let root = TestDirectory::new();
        let manager = test_manager(
            root.path(),
            vec![saved_record(root.path(), 7, "active", false)],
        );
        persist(&manager, root.path(), true).unwrap();
        let original = read_test_ciphertext(root.path());
        let mut settings = storage::read_settings(root.path()).unwrap();
        settings.custom_filters = "! synthetic settings transaction".into();
        storage::write_settings(root.path(), &settings).unwrap();
        persist(&manager, root.path(), true).unwrap();
        assert_eq!(read_test_ciphertext(root.path()), original);
        manager.borrow_mut().records[0].status = "paused".into();
        persist(&manager, root.path(), true).unwrap();
        assert_eq!(read_test_ledger(root.path()).records[0].status, "paused");
        assert_eq!(
            storage::read_settings(root.path()).unwrap().custom_filters,
            settings.custom_filters
        );
    }
    #[test]
    fn monitor_registration_failure_stays_failed_after_late_native_completion_or_cancel() {
        let mut row = record();
        row.status = "choosing".into();
        row.path = None;
        row.can_cancel = true;
        let manager = Rc::new(RefCell::new(Manager {
            records: vec![row],
            ..Manager::default()
        }));
        fail_start(&manager, 7);
        for state in [
            COREWEBVIEW2_DOWNLOAD_STATE_IN_PROGRESS,
            COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED,
            COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED,
        ] {
            let mut native =
                interrupted(COREWEBVIEW2_DOWNLOAD_INTERRUPT_REASON_USER_CANCELED, false);
            native.state = state;
            sync_status(&manager, 7, native, true);
            let m = manager.borrow();
            let row = &m.records[0];
            assert_eq!(row.status, "failed");
            assert_eq!(row.message.as_deref(), Some(MONITORING_ERROR));
            assert!(!row.ok && !row.can_resume && !row.can_cancel && !row.resume_pending);
        }
        assert!(!needs_background(&manager, true));
        assert!(pending_ids(&manager).is_empty());
    }
    #[test]
    fn regular_history_is_encrypted_and_private_records_never_enter_the_ledger() {
        let root = TestDirectory::new();
        let manager = test_manager(
            root.path(),
            vec![
                saved_record(root.path(), 7, "completed", false),
                saved_record(root.path(), 99, "completed", true),
            ],
        );
        persist(&manager, root.path(), false).unwrap();
        let encrypted = read_test_ciphertext(root.path());
        assert!(serde_json::from_slice::<serde_json::Value>(&encrypted).is_err());
        assert!(!encrypted
            .windows(b"fixture-token".len())
            .any(|part| part == b"fixture-token"));
        let ledger = read_test_ledger(root.path());
        assert_eq!(ledger.next_id, 7);
        assert_eq!(ledger.records.len(), 1);
        assert_eq!(
            ledger.records[0].url,
            "https://example.test/file/7?signed=fixture-token-7"
        );
        assert!(ledger.records.iter().all(|r| r.id != 99));
        // Closing a private tab does not turn its old download rows into regular rows.
        manager.borrow_mut().records[1].owner_tab = 0;
        persist(&manager, root.path(), true).unwrap();
        assert_eq!(read_test_ledger(root.path()).records.len(), 1);
    }
    #[test]
    fn a_private_only_session_creates_no_history_record_or_persisted_id_counter() {
        let root = TestDirectory::new();
        let manager = test_manager(
            root.path(),
            vec![saved_record(root.path(), 99, "failed", true)],
        );
        persist(&manager, root.path(), true).unwrap();
        assert!(storage::read_downloads(root.path()).unwrap().is_none());
        assert_eq!(Manager::load(root.path()).unwrap().next, 0);
        assert_eq!(forget_private_finished(&manager), 1);
        assert!(manager.borrow().records.is_empty());
    }
    #[test]
    fn restart_restores_terminal_history_but_never_native_controls_or_background_jobs() {
        let root = TestDirectory::new();
        let statuses = [
            "completed",
            "cancelled",
            "failed",
            "choosing",
            "active",
            "paused",
            "interrupted",
        ];
        let manager = test_manager(
            root.path(),
            statuses
                .iter()
                .enumerate()
                .map(|(n, status)| saved_record(root.path(), n + 1, status, false))
                .collect(),
        );
        persist(&manager, root.path(), true).unwrap();
        let restored = Rc::new(RefCell::new(Manager::load(root.path()).unwrap()));
        let m = restored.borrow();
        for (n, row) in m.records.iter().enumerate() {
            assert_eq!(row.status, if n < 3 { statuses[n] } else { "interrupted" });
            assert_eq!(row.ok, n == 0);
            assert!(!row.can_resume && !row.can_cancel);
            assert_eq!(row.bytes_per_second, 0.);
            assert!(row.eta_seconds.is_none());
            assert_eq!((row.owner_tab, row.owner_view, row.generation), (0, 0, 0));
            assert!(!row.owner_private && !row.resume_pending && !row.pause_requested);
            if n >= 3 {
                assert_eq!(row.message.as_deref(), Some(RESTORED_INTERRUPTED));
            }
        }
        assert!(m.running.is_empty() && m.pending.is_empty());
        drop(m);
        assert!(!needs_background(&restored, true));
        assert!(!has_private_jobs(&restored));
        assert!(pending_ids(&restored).is_empty());
        assert!(!can_prompt(&restored, 1));
        assert_eq!(clear_finished(&restored), 7);
    }
    #[test]
    fn clearing_and_reloading_history_preserves_monotonic_regular_ids() {
        let root = TestDirectory::new();
        let manager = test_manager(
            root.path(),
            vec![saved_record(root.path(), 42, "completed", false)],
        );
        persist(&manager, root.path(), true).unwrap();
        assert_eq!(clear_finished(&manager), 1);
        persist(&manager, root.path(), true).unwrap();
        let mut restored = Manager::load(root.path()).unwrap();
        assert!(restored.records.is_empty());
        assert_eq!(restored.next, 42);
        assert_eq!(restored.next_regular, 42);
        restored.next += 1;
        let id = restored.next;
        restored
            .records
            .push(saved_record(root.path(), id, "cancelled", false));
        let restored = Rc::new(RefCell::new(restored));
        persist(&restored, root.path(), true).unwrap();
        assert_eq!(Manager::load(root.path()).unwrap().records[0].id, 43);
    }
    #[test]
    fn progress_saves_are_coalesced_but_path_and_state_changes_are_immediate() {
        let root = TestDirectory::new();
        let manager = test_manager(
            root.path(),
            vec![saved_record(root.path(), 7, "active", false)],
        );
        persist(&manager, root.path(), false).unwrap();
        let first = read_test_ciphertext(root.path());
        manager.borrow_mut().records[0].received += 10;
        persist(&manager, root.path(), false).unwrap();
        assert_eq!(first, read_test_ciphertext(root.path()));
        manager.borrow_mut().records[0].status = "paused".into();
        persist(&manager, root.path(), false).unwrap();
        assert_eq!(read_test_ledger(root.path()).records[0].status, "paused");
        manager.borrow_mut().records[0].path = Some(
            root.path()
                .join("changed.bin")
                .to_string_lossy()
                .into_owned(),
        );
        persist(&manager, root.path(), false).unwrap();
        assert!(read_test_ledger(root.path()).records[0]
            .path
            .as_deref()
            .unwrap()
            .ends_with("changed.bin"));
        manager.borrow_mut().records[0].received += 10;
        manager.borrow_mut().last_save_attempt = Some(Instant::now() - Duration::from_secs(3));
        persist(&manager, root.path(), false).unwrap();
        assert_eq!(read_test_ledger(root.path()).records[0].received, 2068);
        manager.borrow_mut().records[0].received += 10;
        persist(&manager, root.path(), true).unwrap();
        assert_eq!(read_test_ledger(root.path()).records[0].received, 2078);
    }
    #[test]
    fn corrupt_history_and_an_unloaded_default_manager_cannot_overwrite_the_original() {
        let root = TestDirectory::new();
        let original = b"deliberately invalid encrypted fixture";
        write_test_ciphertext(root.path(), original);
        assert!(Manager::load(root.path()).is_err());
        let manager = Rc::new(RefCell::new(Manager {
            records: vec![saved_record(root.path(), 7, "completed", false)],
            ..Manager::default()
        }));
        assert!(persist(&manager, root.path(), true).is_err());
        assert_eq!(read_test_ciphertext(root.path()), original);
    }
    #[test]
    fn forged_runtime_fields_and_external_changes_are_rejected_without_replacing_ciphertext() {
        let root = TestDirectory::new();
        let manager = test_manager(
            root.path(),
            vec![saved_record(root.path(), 7, "completed", false)],
        );
        persist(&manager, root.path(), true).unwrap();
        let ledger = read_test_ledger(root.path());
        let mut forged = serde_json::to_value(&ledger).unwrap();
        forged["records"][0]["can_resume"] = true.into();
        let encrypted = storage::protect(&serde_json::to_vec(&forged).unwrap(), false).unwrap();
        // Legacy JSON is not a valid database codec, and its forged runtime
        // fields must also prevent legacy migration.
        assert!(migrate_legacy_ledger(&encrypted).is_err());
        write_test_ciphertext(root.path(), &encrypted);
        assert!(Manager::load(root.path()).is_err());
        assert!(persist(&manager, root.path(), true).is_err());
        assert_eq!(read_test_ciphertext(root.path()), encrypted);
    }
    #[test]
    fn ledger_validation_rejects_unsafe_paths_duplicate_ids_and_unknown_states() {
        let root = TestDirectory::new();
        let row = LedgerRecord::snapshot(&saved_record(root.path(), 7, "completed", false));
        let ledger = Ledger {
            version: LEDGER_VERSION,
            next_id: 7,
            records: vec![row.clone()],
        };
        assert!(validate_ledger(&ledger).is_ok());
        for path in [
            "relative.bin",
            "C:\\fixture.bin:secret",
            "C:\\folder\\..\\fixture.bin",
            "\\\\.\\PhysicalDrive0",
        ] {
            let mut bad = ledger.clone();
            bad.records[0].path = Some(path.into());
            assert!(validate_ledger(&bad).is_err(), "{path}");
        }
        let mut bad = ledger.clone();
        bad.records.push(row);
        assert!(validate_ledger(&bad).is_err());
        let mut bad = ledger.clone();
        bad.records[0].status = "running-forever".into();
        assert!(validate_ledger(&bad).is_err());
        let mut bad = ledger.clone();
        bad.records[0].received = -1;
        assert!(validate_ledger(&bad).is_err());
        let mut bad = ledger;
        bad.next_id = 6;
        assert!(validate_ledger(&bad).is_err());
    }
    #[test]
    fn history_cap_keeps_the_newest_five_hundred_regular_rows_and_every_live_owner() {
        let root = TestDirectory::new();
        let mut records: Vec<_> = (1..=510)
            .map(|id| saved_record(root.path(), id, "completed", false))
            .collect();
        let mut private = saved_record(root.path(), 999, "completed", true);
        private.owner_tab = 0;
        records.push(private);
        let live = HashSet::from([1, 2]);
        prune_history(&mut records, &live);
        assert_eq!(records.len(), 503);
        assert!(records.iter().any(|r| r.id == 1));
        assert!(records.iter().any(|r| r.id == 2));
        assert!(!records.iter().any(|r| r.id == 10));
        assert!(records.iter().any(|r| r.id == 11));
        assert!(records.iter().any(|r| r.id == 999));
        let manager = test_manager(
            root.path(),
            (1..=510)
                .map(|id| saved_record(root.path(), id, "completed", false))
                .collect(),
        );
        persist(&manager, root.path(), true).unwrap();
        let loaded = Manager::load(root.path()).unwrap();
        assert_eq!(loaded.records.len(), HISTORY_LIMIT);
        assert_eq!(loaded.records[0].id, 11);
        assert_eq!(loaded.next, 510);
    }

    #[test]
    fn persistence_preflight_prunes_after_an_owner_retires_without_row_changes() {
        let root = TestDirectory::new();
        let rows: Vec<_> = (1..=501)
            .map(|id| saved_record(root.path(), id, "completed", false))
            .collect();
        assert!(!history_needs_pruning(&rows, |id| id == 1));
        assert!(history_needs_pruning(&rows, |_| false));
        assert!(!history_needs_pruning(&rows[..500], |_| false));
        let mut with_private = rows.clone();
        with_private[0].owner_private = true;
        assert!(!history_needs_pruning(&with_private, |_| false));

        // A validated snapshot could contain 500 finished rows plus a live
        // owner. Reproduce its retirement without fabricating any COM owner.
        let ledger = Ledger {
            version: LEDGER_VERSION,
            next_id: 501,
            records: rows.iter().map(LedgerRecord::snapshot).collect(),
        };
        let encrypted = encode_ledger(&ledger, MAX_LEDGER_BYTES).unwrap();
        let path = root.path().join(LEDGER_FILE);
        write_test_ciphertext(root.path(), &encrypted);
        let manager = Rc::new(RefCell::new(Manager {
            records: rows,
            next: 501,
            next_regular: 501,
            ledger_path: Some(path),
            ledger_loaded: true,
            ledger_fingerprint: Some(fingerprint(&encrypted)),
            last_save_structure: Some(structure(&ledger)),
            last_saved: Some(ledger),
            last_save_attempt: Some(Instant::now()),
            ..Manager::default()
        }));
        assert_eq!(
            manager.borrow().persist_preflight(false, Instant::now()),
            PersistPreflight::FullWork
        );
        persist(&manager, root.path(), false).unwrap();
        let saved = read_test_ledger(root.path());
        assert_eq!(saved.records.len(), HISTORY_LIMIT);
        assert_eq!(saved.records[0].id, 2);
        assert_eq!(saved.next_id, 501);
    }

    #[test]
    fn persistence_preflight_preserves_boundaries_validation_and_id_bookkeeping() {
        let root = TestDirectory::new();
        let manager = test_manager(
            root.path(),
            vec![saved_record(root.path(), 7, "active", false)],
        );
        persist(&manager, root.path(), true).unwrap();
        let now = Instant::now();
        let mut m = manager.borrow_mut();
        assert_eq!(m.persist_preflight(false, now), PersistPreflight::Unchanged);
        assert_eq!(m.persist_preflight(true, now), PersistPreflight::FullWork);
        m.records[0].received += 1;
        m.last_save_attempt = Some(now - Duration::from_millis(1999));
        assert_eq!(m.persist_preflight(false, now), PersistPreflight::Throttled);
        m.last_save_attempt = Some(now - Duration::from_secs(2));
        assert_eq!(m.persist_preflight(false, now), PersistPreflight::FullWork);
        m.last_save_attempt = Some(now - Duration::from_millis(2001));
        assert_eq!(m.persist_preflight(false, now), PersistPreflight::FullWork);
        m.last_save_attempt = Some(now);
        m.records[0].total = -1;
        assert_eq!(m.persist_preflight(false, now), PersistPreflight::Throttled);
        m.records[0].total = -2;
        assert_eq!(m.persist_preflight(false, now), PersistPreflight::FullWork);
        m.records[0].total = m.last_saved.as_ref().unwrap().records[0].total;
        m.records[0].received = -1;
        assert_eq!(m.persist_preflight(false, now), PersistPreflight::FullWork);
        m.records[0].received = m.last_saved.as_ref().unwrap().records[0].received;
        let original = m.records[0].clone();
        for changed in ["url", "path", "status", "message", "id"] {
            m.records[0] = original.clone();
            match changed {
                "url" => m.records[0].url = "not a valid URI".into(),
                "path" => m.records[0].path = Some("relative.bin".into()),
                "status" => m.records[0].status = "paused".into(),
                "message" => m.records[0].message = Some("changed".into()),
                "id" => m.records[0].id = 0,
                _ => unreachable!(),
            }
            assert_eq!(
                m.persist_preflight(false, now),
                PersistPreflight::FullWork,
                "{changed} must receive full validation"
            );
        }
        m.records[0] = original;
        m.next_regular = 0;
        m.next = 0;
        assert_eq!(m.persist_preflight(false, now), PersistPreflight::FullWork);
        drop(m);
        persist(&manager, root.path(), false).unwrap();
        assert_eq!(manager.borrow().next_regular, 7);
        assert_eq!(manager.borrow().next, 7);
    }

    #[test]
    fn persistence_preflight_replays_errors_and_keeps_forced_and_structural_saves() {
        let root = TestDirectory::new();
        let manager = test_manager(
            root.path(),
            vec![saved_record(root.path(), 7, "active", false)],
        );
        persist(&manager, root.path(), true).unwrap();
        let initial = read_test_ciphertext(root.path());
        {
            let mut m = manager.borrow_mut();
            m.records[0].received += 10;
            m.last_save_attempt = Some(Instant::now());
            m.last_save_error = Some("fixture write failure".into());
        }
        assert_eq!(
            persist(&manager, root.path(), false),
            Err("fixture write failure".into())
        );
        assert_eq!(read_test_ciphertext(root.path()), initial);
        manager.borrow_mut().records[0].status = "paused".into();
        persist(&manager, root.path(), false).unwrap();
        assert_eq!(read_test_ledger(root.path()).records[0].status, "paused");
        assert!(manager.borrow().last_save_error.is_none());
        manager.borrow_mut().records[0].path = Some(
            root.path()
                .join("changed.bin")
                .to_string_lossy()
                .into_owned(),
        );
        persist(&manager, root.path(), false).unwrap();
        assert!(read_test_ledger(root.path()).records[0]
            .path
            .as_ref()
            .unwrap()
            .ends_with("changed.bin"));
        manager.borrow_mut().records[0].received += 1;
        persist(&manager, root.path(), true).unwrap();
        assert_eq!(
            read_test_ledger(root.path()).records[0].received,
            manager.borrow().records[0].received
        );
        let foreign = TestDirectory::new();
        assert!(persist(&manager, foreign.path(), false)
            .unwrap_err()
            .contains("another browser profile"));
        assert!(storage::read_downloads(foreign.path()).unwrap().is_none());
        let unchanged = read_test_ciphertext(root.path());
        manager.borrow_mut().records[0].received = -1;
        assert!(persist(&manager, root.path(), false).is_err());
        assert_eq!(read_test_ciphertext(root.path()), unchanged);
    }

    #[test]
    fn private_progress_stays_out_of_fast_saves_and_force_rechecks_ciphertext() {
        let root = TestDirectory::new();
        let manager = test_manager(
            root.path(),
            vec![saved_record(root.path(), 7, "completed", false)],
        );
        persist(&manager, root.path(), true).unwrap();
        let initial = read_test_ciphertext(root.path());
        let mut private = saved_record(root.path(), 999, "active", true);
        private.received += 10;
        manager.borrow_mut().records.push(private);
        assert_eq!(
            manager.borrow().persist_preflight(false, Instant::now()),
            PersistPreflight::Unchanged
        );
        persist(&manager, root.path(), false).unwrap();
        assert_eq!(read_test_ciphertext(root.path()), initial);
        assert_eq!(manager.borrow().next_regular, 7);
        assert_eq!(read_test_ledger(root.path()).records.len(), 1);
        let mut externally_changed = initial;
        externally_changed.push(1);
        write_test_ciphertext(root.path(), &externally_changed);
        assert!(persist(&manager, root.path(), true).is_err());
        assert_eq!(read_test_ciphertext(root.path()), externally_changed);
    }
}
