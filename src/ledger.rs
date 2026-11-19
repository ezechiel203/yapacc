use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    User,
    System,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LedgerPaths {
    pub root: PathBuf,
    pub journal: PathBuf,
    pub state: PathBuf,
    pub state_tmp: PathBuf,
    pub lock: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InstallRecord {
    pub id: String,
    pub name: String,
    pub format: String, // e.g. flatpak, appimage, arch-prefix, deb-extract
    pub source: String, // repo/url/local/aur/manual
    pub version: String,
    pub scope: String, // user|system
    pub install_root: String,
    pub paths: Vec<String>,
    pub status: String, // installed|removed|broken
    pub installed_at: u64,
    pub updated_at: u64,
    pub metadata: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LedgerState {
    pub schema_version: u32,
    pub head_seq: u64,
    pub head_hash: String,
    pub records: BTreeMap<String, InstallRecord>, // id -> record
}

impl Default for LedgerState {
    fn default() -> Self {
        Self {
            schema_version: 2,
            head_seq: 0,
            head_hash: "GENESIS".to_string(),
            records: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JournalEvent {
    pub seq: u64,
    pub prev_hash: String,
    pub event_type: String, // init|upsert|remove|checkpoint
    pub txid: String,
    pub timestamp: u64,
    pub payload: serde_json::Value,
    pub hash: String,
}

fn now_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn sha256_hex(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    hex(&h.finalize())
}

/// Legacy (pre-v2) non-cryptographic hash, kept only to verify old journals.
fn djb2_hex(s: &str) -> String {
    let mut h: u64 = 5381;
    for b in s.as_bytes() {
        h = ((h << 5).wrapping_add(h)).wrapping_add(*b as u64);
    }
    format!("{:016x}", h)
}

static ID_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Collision-safe record id: name + nanosecond timestamp + pid + in-process counter.
fn make_record_id(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let n = ID_COUNTER.fetch_add(1, AtomicOrdering::Relaxed);
    format!("pkg-{safe}-{}-{}-{n}", now_nanos(), std::process::id())
}

/// If the pre-rename (package_cleaner) ledger exists but the yapacc one does
/// not, keep using it so existing data survives the rename.
fn legacy_or_new(new: PathBuf, legacy: PathBuf) -> PathBuf {
    if new.exists() {
        return new;
    }
    if legacy.exists() {
        eprintln!(
            "note: using legacy ledger at {} (pre-rename); move it to {} to migrate",
            legacy.display(),
            new.display()
        );
        return legacy;
    }
    new
}

pub fn default_root(scope: &Scope) -> PathBuf {
    match scope {
        Scope::User => {
            if let Some(root) = crate::config::get().paths.ledger_user.clone() {
                return PathBuf::from(root);
            }
            if let Some(state) = crate::config::xdg_base("XDG_STATE_HOME", ".local/state") {
                return legacy_or_new(
                    state.join("yapacc/ledger"),
                    state.join("package_cleaner/ledger"),
                );
            }
            PathBuf::from(".yapacc/ledger-user")
        }
        Scope::System => crate::config::get()
            .paths
            .ledger_system
            .clone()
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                legacy_or_new(
                    PathBuf::from("/var/lib/yapacc/ledger"),
                    PathBuf::from("/var/lib/package_cleaner/ledger"),
                )
            }),
    }
}

pub fn paths_for(root: &Path) -> LedgerPaths {
    LedgerPaths {
        root: root.to_path_buf(),
        journal: root.join("journal.jsonl"),
        state: root.join("state.json"),
        state_tmp: root.join("state.json.tmp"),
        lock: root.join("ledger.lock"),
    }
}

/// Modes of what the ledger creates, `(directory, file)`.
///
/// A ledger written by an ordinary user is that user's business: it names
/// what they installed and where, so nobody else may read it (0700/0600).
/// A ledger written by root is the system-scope one, which every user reads
/// (`ledger list --scope system`, the interface's External tab), so it stays
/// readable and is writable by root only (0755/0644).
fn modes_for(root_user: bool) -> (u32, u32) {
    if root_user {
        (0o755, 0o644)
    } else {
        (0o700, 0o600)
    }
}

fn modes() -> (u32, u32) {
    modes_for(crate::util::is_root())
}

/// Remove the permission bits `path` has beyond `mode` (a ledger created by
/// an earlier version with the umask default). Best effort: a file owned by
/// someone else cannot be changed, and reading it still works.
fn restrict(path: &Path, mode: u32) {
    if let Ok(meta) = fs::metadata(path) {
        let current = meta.permissions().mode() & 0o7777;
        if current & !mode != 0 {
            let _ = fs::set_permissions(path, fs::Permissions::from_mode(current & mode));
        }
    }
}

/// Create the ledger directory (and what is missing above it) with the
/// ledger's mode. A directory that is already a ledger (it holds a journal)
/// is brought to that mode too; any other existing directory someone points
/// `--path` at keeps the mode its owner gave it.
fn ensure_dir(p: &Path) -> Result<(), String> {
    let (dir_mode, _) = modes();
    fs::DirBuilder::new()
        .recursive(true)
        .mode(dir_mode)
        .create(p)
        .map_err(|e| format!("failed to create {}: {e}", p.display()))?;
    if p.join("journal.jsonl").exists() {
        restrict(p, dir_mode);
    }
    Ok(())
}

/// Flush a directory entry (a file just created or renamed in `dir`) to
/// disk. Without it a power cut can leave fsynced contents under a name
/// the directory never recorded. Best effort: not every filesystem lets a
/// directory be synced.
fn sync_dir(dir: &Path) {
    if let Ok(d) = fs::File::open(dir) {
        let _ = d.sync_all();
    }
}

/// `state.json` is only a cache of the journal. `None` when it is missing
/// or unreadable. Nothing is ever answered from it: readers and writers
/// replay the journal, and `verify` compares the two.
fn read_state_cache(paths: &LedgerPaths) -> Option<LedgerState> {
    let data = fs::read_to_string(&paths.state).ok()?;
    serde_json::from_str(&data).ok()
}

fn write_state_atomic(paths: &LedgerPaths, state: &LedgerState) -> Result<(), String> {
    let (_, file_mode) = modes();
    let data = serde_json::to_string_pretty(state)
        .map_err(|e| format!("failed to serialize state: {e}"))?;
    let mut f = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(file_mode)
        .open(&paths.state_tmp)
        .map_err(|e| {
            format!(
                "failed to open temp state {}: {e}",
                paths.state_tmp.display()
            )
        })?;
    // a leftover temp file keeps the mode it was created with
    restrict(&paths.state_tmp, file_mode);
    f.write_all(data.as_bytes())
        .map_err(|e| format!("failed to write temp state: {e}"))?;
    f.write_all(b"\n")
        .map_err(|e| format!("failed to write temp state: {e}"))?;
    f.sync_all()
        .map_err(|e| format!("failed to fsync temp state: {e}"))?;
    fs::rename(&paths.state_tmp, &paths.state).map_err(|e| {
        format!(
            "failed to atomically replace state {}: {e}",
            paths.state.display()
        )
    })?;
    sync_dir(&paths.root);
    Ok(())
}

fn append_journal(paths: &LedgerPaths, event: &JournalEvent) -> Result<(), String> {
    let (_, file_mode) = modes();
    let line = serde_json::to_string(event)
        .map_err(|e| format!("failed to serialize journal event: {e}"))?;
    let created = !paths.journal.exists();
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(file_mode)
        .open(&paths.journal)
        .map_err(|e| format!("failed to open journal {}: {e}", paths.journal.display()))?;
    writeln!(f, "{}", line).map_err(|e| format!("failed to append journal: {e}"))?;
    f.sync_all()
        .map_err(|e| format!("failed to fsync journal: {e}"))?;
    if created {
        sync_dir(&paths.root);
    }
    Ok(())
}

fn compute_event_hash(
    seq: u64,
    prev_hash: &str,
    event_type: &str,
    txid: &str,
    timestamp: u64,
    payload: &serde_json::Value,
) -> String {
    let body = format!(
        "{}|{}|{}|{}|{}|{}",
        seq, prev_hash, event_type, txid, timestamp, payload
    );
    sha256_hex(&body)
}

// ---------------------------------------------------------------------------
// Locking
// ---------------------------------------------------------------------------

/// Exclusive advisory lock (flock) around journal/state mutations. The
/// kernel releases it when the holder exits, so there are no stale locks
/// to take over and no time limit on how long a holder may work.
pub struct LedgerLock {
    _file: fs::File,
}

const LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

fn acquire_lock_within(
    paths: &LedgerPaths,
    wait: std::time::Duration,
) -> Result<LedgerLock, String> {
    let f = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(modes().1)
        .open(&paths.lock)
        .map_err(|e| format!("cannot open lock {}: {e}", paths.lock.display()))?;
    let deadline = std::time::Instant::now() + wait;
    loop {
        match f.try_lock() {
            Ok(()) => return Ok(LedgerLock { _file: f }),
            Err(fs::TryLockError::WouldBlock) => {
                if std::time::Instant::now() >= deadline {
                    return Err(format!(
                        "ledger is locked by another yapacc process ({}); try again when it finishes",
                        paths.lock.display()
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(fs::TryLockError::Error(e)) => {
                return Err(format!("cannot lock {}: {e}", paths.lock.display()));
            }
        }
    }
}

fn acquire_lock(paths: &LedgerPaths) -> Result<LedgerLock, String> {
    acquire_lock_within(paths, LOCK_WAIT)
}

// ---------------------------------------------------------------------------
// Replay: the journal is the source of truth
// ---------------------------------------------------------------------------

/// Result of reading the whole journal and checking its chain.
struct Replayed {
    state: LedgerState,
    /// hash of event `seq` at index `seq - 1`
    hashes: Vec<String>,
    legacy_events: u64,
    /// byte length of the valid prefix
    good_len: u64,
    /// the file ends in an incomplete line (a write was interrupted)
    torn_tail: bool,
    /// the last valid line has no trailing newline
    unterminated: bool,
}

fn integrity(msg: String) -> crate::Error {
    crate::Error::integrity(msg)
}

fn apply_event(state: &mut LedgerState, ev: &JournalEvent) -> Result<(), crate::Error> {
    match ev.event_type.as_str() {
        "upsert" => {
            let rec: InstallRecord = serde_json::from_value(ev.payload.clone()).map_err(|e| {
                integrity(format!("journal seq {}: bad record payload: {e}", ev.seq))
            })?;
            state.records.insert(rec.id.clone(), rec);
        }
        "remove" => {
            if let Some(id) = ev.payload.get("id").and_then(|v| v.as_str()) {
                state.records.remove(id);
            }
        }
        // init and any future event types carry no record state
        _ => {}
    }
    state.head_seq = ev.seq;
    state.head_hash = ev.hash.clone();
    Ok(())
}

fn replay(paths: &LedgerPaths) -> Result<Replayed, crate::Error> {
    let mut out = Replayed {
        state: LedgerState::default(),
        hashes: Vec::new(),
        legacy_events: 0,
        good_len: 0,
        torn_tail: false,
        unterminated: false,
    };
    let data = match fs::read(&paths.journal) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => {
            return Err(crate::Error::from(format!(
                "failed to read journal {}: {e}",
                paths.journal.display()
            )));
        }
    };
    let mut offset = 0usize;
    let mut line_no = 0usize;
    while offset < data.len() {
        let (end, terminated) = match data[offset..].iter().position(|b| *b == b'\n') {
            Some(i) => (offset + i, true),
            None => (data.len(), false),
        };
        let line = &data[offset..end];
        let next = if terminated { end + 1 } else { end };
        line_no += 1;
        if line.iter().all(|b| b.is_ascii_whitespace()) {
            offset = next;
            if out.good_len as usize == offset - line.len() - usize::from(terminated) {
                out.good_len = offset as u64;
            }
            continue;
        }
        let ev: JournalEvent = match serde_json::from_slice(line) {
            Ok(ev) => ev,
            Err(_) if !terminated => {
                // an append was cut short; the event was never acknowledged
                out.torn_tail = true;
                break;
            }
            Err(e) => {
                return Err(integrity(format!(
                    "journal parse error at line {line_no}: {e}"
                )));
            }
        };
        let expected_seq = out.state.head_seq + 1;
        if ev.seq != expected_seq {
            return Err(integrity(format!(
                "sequence gap at line {line_no} (expected {expected_seq}, got {})",
                ev.seq
            )));
        }
        if ev.prev_hash != out.state.head_hash {
            return Err(integrity(format!(
                "prev_hash mismatch at seq {} (line {line_no})",
                ev.seq
            )));
        }
        let expect = compute_event_hash(
            ev.seq,
            &ev.prev_hash,
            &ev.event_type,
            &ev.txid,
            ev.timestamp,
            &ev.payload,
        );
        if ev.hash != expect {
            let legacy = djb2_hex(&format!(
                "{}|{}|{}|{}|{}|{}",
                ev.seq, ev.prev_hash, ev.event_type, ev.txid, ev.timestamp, ev.payload
            ));
            if ev.hash == legacy {
                out.legacy_events += 1;
            } else {
                return Err(integrity(format!(
                    "hash mismatch at seq {} (line {line_no})",
                    ev.seq
                )));
            }
        }
        apply_event(&mut out.state, &ev)?;
        out.hashes.push(ev.hash.clone());
        out.good_len = next as u64;
        out.unterminated = !terminated;
        offset = next;
    }
    Ok(out)
}

/// A state cache with no journal beside it. The journal is the record; a
/// cache on its own is either what is left after the journal was deleted or
/// a file someone wrote by hand, and neither may be answered from.
fn orphan_cache(paths: &LedgerPaths) -> Option<crate::Error> {
    (!paths.journal.exists() && paths.state.exists()).then(|| {
        integrity(format!(
            "ledger {}: state.json is present but journal.jsonl is missing. The journal is the record and the cache is not trusted without it. Restore journal.jsonl from a backup, or move state.json away to start an empty ledger.",
            paths.root.display()
        ))
    })
}

/// How the state cache stands against the replayed journal.
#[derive(Debug, PartialEq)]
enum Cache {
    /// same head, same contents
    Current,
    /// no cache, an unreadable one, or one written before the last event(s)
    /// (the process died between the journal append and the cache write)
    AbsentOrBehind,
    /// it cannot have been produced from this journal: the message says how
    Contradicts(String),
}

fn records_json(st: &LedgerState) -> serde_json::Value {
    serde_json::to_value(&st.records).unwrap_or_default()
}

/// Compare the whole cache with what the journal replays to — not only the
/// head, which is all an editor of `state.json` needs to leave alone.
fn compare_cache(paths: &LedgerPaths, r: &Replayed) -> Cache {
    let Some(cache) = read_state_cache(paths) else {
        return Cache::AbsentOrBehind;
    };
    let head = &r.state;
    if cache.head_seq == head.head_seq && cache.head_hash == head.head_hash {
        // (the schema number is not compared: a cache written by an earlier
        // version carries its own and is otherwise right)
        if records_json(&cache) != records_json(head) {
            return Cache::Contradicts(format!(
                "state.json does not match the journal: it carries the journal's head (seq {}) but different records. It was edited, or written by something other than yapacc",
                head.head_seq
            ));
        }
        return Cache::Current;
    }
    // behind: its head is an earlier point of this very chain
    let on_chain = if cache.head_seq == 0 {
        cache.head_hash == "GENESIS"
    } else {
        cache.head_seq < head.head_seq
            && r.hashes.get((cache.head_seq - 1) as usize) == Some(&cache.head_hash)
    };
    if on_chain {
        return Cache::AbsentOrBehind;
    }
    Cache::Contradicts(format!(
        "state.json records seq {} with a hash the journal does not have at that point (the journal ends at seq {}): the journal was truncated or rewritten after the cache was written",
        cache.head_seq, head.head_seq
    ))
}

/// Current state for readers: always what the journal replays to, so that
/// no command acts on a record the chain does not contain. Never writes.
fn load(paths: &LedgerPaths) -> Result<LedgerState, crate::Error> {
    if let Some(e) = orphan_cache(paths) {
        return Err(e);
    }
    Ok(replay(paths)?.state)
}

/// Current state for writers (call with the lock held): the replayed
/// journal. If the cache disagrees with it — the process died between the
/// journal append and the cache write, the cache was deleted or edited —
/// the cache is rewritten, and an interrupted final line is discarded.
fn load_for_update(paths: &LedgerPaths) -> Result<LedgerState, String> {
    if let Some(e) = orphan_cache(paths) {
        return Err(e.message);
    }
    let r = replay(paths)?;
    let cache = compare_cache(paths, &r);
    if cache == Cache::Current && !r.torn_tail && !r.unterminated {
        return Ok(r.state);
    }
    if r.torn_tail || r.unterminated {
        let f = OpenOptions::new()
            .write(true)
            .open(&paths.journal)
            .map_err(|e| format!("failed to open journal for repair: {e}"))?;
        f.set_len(r.good_len)
            .map_err(|e| format!("failed to drop interrupted journal line: {e}"))?;
        if r.unterminated {
            let mut f = OpenOptions::new()
                .append(true)
                .open(&paths.journal)
                .map_err(|e| format!("failed to open journal for repair: {e}"))?;
            f.write_all(b"\n")
                .map_err(|e| format!("failed to terminate journal line: {e}"))?;
        }
        f.sync_all().ok();
    }
    if let Cache::Contradicts(why) = &cache {
        eprintln!(
            "warning: ledger {}: {why}. The journal is the record; the cache is rewritten from it.",
            paths.root.display()
        );
    } else if paths.journal.exists() {
        eprintln!(
            "ledger: state cache rebuilt from the journal ({} event(s){})",
            r.state.head_seq,
            if r.torn_tail {
                ", one interrupted write discarded"
            } else {
                ""
            }
        );
    }
    refresh_cache(paths, &r.state);
    Ok(r.state)
}

/// Write the cache. It is never needed for a correct answer (readers and
/// writers replay the journal), so a failure is reported and does not fail
/// the caller: after a journal append in particular, the event is committed,
/// and reporting a failure (disk full) would make the caller undo or repeat
/// work the ledger already holds. A cache left behind is rewritten by the
/// next change.
fn refresh_cache(paths: &LedgerPaths, st: &LedgerState) {
    if let Err(e) = write_state_atomic(paths, st) {
        let _ = fs::remove_file(&paths.state_tmp);
        eprintln!(
            "warning: ledger {}: the journal is complete (seq {}), but the state cache could not be written: {e}. It will be rebuilt on the next change.",
            paths.root.display(),
            st.head_seq
        );
    }
}

// ---------------------------------------------------------------------------
// Witnesses: what makes tampering detectable
// ---------------------------------------------------------------------------
//
// A hash chain on its own only detects accidental corruption: whoever can
// write the journal can rewrite all of it and recompute every hash. It
// becomes tamper-EVIDENT only relative to a copy of the head that the
// attacker cannot also rewrite. A witness is such a copy.

/// Special witness name: the systemd journal (entries a user logs cannot be
/// edited or removed by that user afterwards).
pub const WITNESS_JOURNALD: &str = "journald";
const WITNESS_TAG: &str = "yapacc-ledger";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Anchor {
    pub root: String,
    pub seq: u64,
    pub hash: String,
    #[serde(default)]
    pub ts: u64,
}

/// Record the current head with every configured witness (best effort: a
/// witness that cannot be reached is reported, the ledger change stands).
pub fn witness_head(root: &Path, seq: u64, hash: &str, witnesses: &[String]) {
    if witnesses.is_empty() {
        return;
    }
    let anchor = Anchor {
        root: root.display().to_string(),
        seq,
        hash: hash.to_string(),
        ts: now_ts(),
    };
    let Ok(line) = serde_json::to_string(&anchor) else {
        return;
    };
    for w in witnesses {
        let res: Result<(), String> = if w == WITNESS_JOURNALD {
            std::process::Command::new("logger")
                .args(["-t", WITNESS_TAG, "--"])
                .arg(&line)
                .status()
                .map_err(|e| e.to_string())
                .and_then(|s| {
                    s.success()
                        .then_some(())
                        .ok_or_else(|| format!("logger exited with {s}"))
                })
        } else {
            OpenOptions::new()
                .create(true)
                .append(true)
                .mode(modes().1)
                .open(w)
                .and_then(|mut f| writeln!(f, "{line}"))
                .map_err(|e| e.to_string())
        };
        if let Err(e) = res {
            eprintln!("warning: ledger witness '{w}' not updated: {e}");
        }
    }
}

/// The uid whose journald entries count as this ledger's witness: the owner
/// of the journal (of the ledger directory before the first event), else
/// this process's own.
fn ledger_owner_uid(root: &Path) -> Option<u32> {
    let paths = paths_for(root);
    fs::metadata(&paths.journal)
        .or_else(|_| fs::metadata(&paths.root))
        .map(|m| m.uid())
        .ok()
        .or_else(crate::util::real_uid)
}

/// `journalctl` arguments that read the witness entries logged by `uid` or
/// by root, and by nobody else. `_UID` is set by journald from the sender's
/// credentials, so another local user cannot log an entry that passes: a
/// fake anchor from them would otherwise make `verify` report a rewrite
/// that never happened. Matches on one field are alternatives.
fn journald_args(uid: u32) -> Vec<String> {
    let mut args = vec!["-t".to_string(), WITNESS_TAG.to_string()];
    args.push(format!("_UID={uid}"));
    if uid != 0 {
        args.push("_UID=0".to_string());
    }
    args.extend(["-o", "cat", "--no-pager", "-q"].map(String::from));
    args
}

fn read_witness(root: &Path, witness: &str) -> Result<Vec<Anchor>, String> {
    let text = if witness == WITNESS_JOURNALD {
        let uid = ledger_owner_uid(root)
            .ok_or("journald: the uid that owns this ledger could not be determined")?;
        let out = std::process::Command::new("journalctl")
            .args(journald_args(uid))
            .output()
            .map_err(|e| format!("journalctl: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "journalctl: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        String::from_utf8_lossy(&out.stdout).to_string()
    } else {
        match fs::read_to_string(witness) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e.to_string()),
        }
    };
    let me = root.display().to_string();
    Ok(text
        .lines()
        .filter_map(|l| serde_json::from_str::<Anchor>(l.trim()).ok())
        .filter(|a| a.root == me)
        .collect())
}

/// The head of a ledger with no event, as `yapacc ledger head` prints it.
const EMPTY_HEAD_HASH: &str = "GENESIS";

/// Parse a `SEQ:HASH` anchor as printed by `yapacc ledger head`, including
/// the `0:GENESIS` it prints for a ledger with nothing recorded yet.
pub fn parse_anchor(root: &Path, s: &str) -> Result<Anchor, String> {
    let (seq, hash) = s
        .split_once(':')
        .ok_or_else(|| format!("invalid anchor '{s}' (expected SEQ:HASH)"))?;
    let seq: u64 = seq
        .trim()
        .parse()
        .map_err(|_| format!("invalid anchor sequence in '{s}'"))?;
    let hash = hash.trim();
    if seq == 0 && hash == EMPTY_HEAD_HASH {
        return Ok(Anchor {
            root: root.display().to_string(),
            seq: 0,
            hash: EMPTY_HEAD_HASH.to_string(),
            ts: 0,
        });
    }
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!(
            "invalid anchor hash in '{s}' (expected 64 hex digits)"
        ));
    }
    Ok(Anchor {
        root: root.display().to_string(),
        seq,
        hash: hash.to_lowercase(),
        ts: 0,
    })
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Create the ledger if needed. Only mutating paths call this; reads never
/// create anything on disk.
pub fn init(root: &Path) -> Result<LedgerPaths, String> {
    ensure_dir(root)?;
    let paths = paths_for(root);
    // a new journal is never started over a cache that has lost its own
    if let Some(e) = orphan_cache(&paths) {
        return Err(e.message);
    }
    // files of a ledger made by an earlier version: same modes as new ones
    let (_, file_mode) = modes();
    for file in [&paths.journal, &paths.state, &paths.lock] {
        restrict(file, file_mode);
    }
    if !paths.journal.exists() {
        let _guard = acquire_lock(&paths)?;
        if !paths.journal.exists() {
            let payload = serde_json::json!({"root": root.display().to_string()});
            let ts = now_ts();
            let hash = compute_event_hash(1, "GENESIS", "init", "init", ts, &payload);
            let ev = JournalEvent {
                seq: 1,
                prev_hash: "GENESIS".into(),
                event_type: "init".into(),
                txid: "init".into(),
                timestamp: ts,
                payload,
                hash,
            };
            append_journal(&paths, &ev)?;
            let mut st = LedgerState::default();
            apply_event(&mut st, &ev)?;
            refresh_cache(&paths, &st);
        }
    }
    Ok(paths)
}

/// Current `(seq, hash)` head — the value to keep somewhere safe as an
/// anchor for later `ledger verify --anchor`.
pub fn head(root: &Path) -> Result<(u64, String), crate::Error> {
    let st = load(&paths_for(root))?;
    Ok((st.head_seq, st.head_hash))
}

/// A broken chain, a cache without its journal, or a cache that contradicts
/// the journal is an integrity error (exit 5), here as in `verify`.
pub fn status(root: &Path) -> Result<String, crate::Error> {
    let paths = paths_for(root);
    if let Some(e) = orphan_cache(&paths) {
        return Err(e);
    }
    let r = replay(&paths)?;
    let cache = match compare_cache(&paths, &r) {
        Cache::Contradicts(why) => {
            return Err(integrity(format!("ledger {}: {why}", paths.root.display())));
        }
        other => other,
    };
    let st = r.state;
    Ok(format!(
        "ledger root: {}\njournal: {}\nstate cache: {}\nschema_version: {}\nhead_seq: {}\nhead_hash: {}\nrecords: {}\nwitnesses: {}",
        paths.root.display(),
        if paths.journal.exists() {
            "present"
        } else {
            "absent (nothing recorded yet)"
        },
        if cache == Cache::Current {
            "current"
        } else {
            "absent or behind (rebuilt from the journal on next change)"
        },
        st.schema_version,
        st.head_seq,
        st.head_hash,
        st.records.len(),
        {
            let w = &crate::config::get().ledger.witnesses;
            if w.is_empty() {
                "none configured".to_string()
            } else {
                w.join(", ")
            }
        }
    ))
}

/// The records the journal replays to (never the cache's).
pub fn list_records(root: &Path) -> Result<Vec<InstallRecord>, crate::Error> {
    Ok(load(&paths_for(root))?.records.into_values().collect())
}

#[derive(Clone, Debug, Default)]
pub struct VerifyOptions {
    /// Treat pre-v2 (djb2) events as "unverified" instead of failing.
    pub accept_legacy: bool,
    /// Extra anchors supplied by the user (`SEQ:HASH`).
    pub anchors: Vec<Anchor>,
}

#[derive(Clone, Debug, Serialize)]
pub struct VerifyReport {
    pub events: u64,
    pub head_seq: u64,
    pub head_hash: String,
    pub legacy_events: u64,
    pub torn_tail: bool,
    pub cache_current: bool,
    /// number of independent anchor records that matched the journal
    pub anchors_matched: usize,
    /// highest sequence number confirmed by an anchor
    pub anchored_through: Option<u64>,
    pub witness_errors: Vec<String>,
}

impl VerifyReport {
    pub fn render(&self) -> String {
        if self.events == 0 {
            return "verify: nothing recorded yet (no journal)".into();
        }
        let mut s = format!(
            "verify: chain OK ({} events, head_seq={}, head_hash={})",
            self.events, self.head_seq, self.head_hash
        );
        match self.anchored_through {
            Some(n) => s.push_str(&format!(
                "\ntamper evidence: {} anchor record(s) matched; history up to seq {n} is confirmed unmodified",
                self.anchors_matched
            )),
            None => s.push_str(
                "\ntamper evidence: NONE — no witness or anchor to compare against. The chain alone only detects accidental corruption; whoever can write the journal can rewrite it. Configure [ledger] witnesses or keep `yapacc ledger head` somewhere safe and pass it back with --anchor.",
            ),
        }
        if self.legacy_events > 0 {
            s.push_str(&format!(
                "\nnote: {} pre-v2 event(s) accepted as UNVERIFIED (--accept-legacy)",
                self.legacy_events
            ));
        }
        if self.torn_tail {
            s.push_str("\nnote: the journal ends in an interrupted write; it will be discarded on the next change");
        }
        if !self.cache_current {
            s.push_str("\nnote: state cache is absent or behind the journal; it will be rebuilt on the next change");
        }
        for e in &self.witness_errors {
            s.push_str(&format!("\nwarning: witness unreadable: {e}"));
        }
        s
    }
}

/// Verify with the witnesses from the configuration.
pub fn verify(root: &Path, opts: &VerifyOptions) -> Result<VerifyReport, crate::Error> {
    verify_with(root, opts, &crate::config::get().ledger.witnesses)
}

pub fn verify_with(
    root: &Path,
    opts: &VerifyOptions,
    witnesses: &[String],
) -> Result<VerifyReport, crate::Error> {
    let paths = paths_for(root);
    if let Some(e) = orphan_cache(&paths) {
        return Err(integrity(format!("verify failed: {e}")));
    }
    let r = replay(&paths).map_err(|e| crate::Error::new(e.kind, format!("verify failed: {e}")))?;
    if r.legacy_events > 0 && !opts.accept_legacy {
        return Err(integrity(format!(
            "verify failed: {} event(s) use the pre-v2 non-cryptographic hash (djb2), which is trivial to forge. Re-run with --accept-legacy to treat them as unverified.",
            r.legacy_events
        )));
    }
    let mut anchors = opts.anchors.clone();
    let mut witness_errors = Vec::new();
    for w in witnesses {
        match read_witness(root, w) {
            Ok(mut a) => anchors.append(&mut a),
            Err(e) => witness_errors.push(format!("{w}: {e}")),
        }
    }
    let mut anchored_through = None;
    let mut anchors_matched = 0;
    for a in &anchors {
        if a.seq == 0 && a.hash == EMPTY_HEAD_HASH {
            // the head of an empty ledger: true of every journal, so it
            // neither contradicts this one nor confirms any of it
            continue;
        }
        if a.seq == 0 || a.seq > r.state.head_seq {
            return Err(integrity(format!(
                "verify failed: the journal was TRUNCATED — an anchor recorded seq {} but the journal ends at seq {}",
                a.seq, r.state.head_seq
            )));
        }
        if r.hashes[(a.seq - 1) as usize] != a.hash {
            return Err(integrity(format!(
                "verify failed: the journal was REWRITTEN — seq {} no longer has the hash an anchor recorded",
                a.seq
            )));
        }
        anchors_matched += 1;
        anchored_through = anchored_through.max(Some(a.seq));
    }
    // The cache is what an earlier version answered from, and what an
    // editor would change: it must be exactly what the journal replays to.
    let cache_current = match compare_cache(&paths, &r) {
        Cache::Current => true,
        Cache::AbsentOrBehind => false,
        Cache::Contradicts(why) => return Err(integrity(format!("verify failed: {why}"))),
    };
    Ok(VerifyReport {
        events: r.state.head_seq,
        head_seq: r.state.head_seq,
        head_hash: r.state.head_hash.clone(),
        legacy_events: r.legacy_events,
        torn_tail: r.torn_tail,
        cache_current,
        anchors_matched,
        anchored_through,
        witness_errors,
    })
}

fn make_txid(prefix: &str) -> String {
    format!("{}-{}-{}", prefix, now_ts(), std::process::id())
}

/// Append one event and refresh the cache. The journal append is the
/// commit point: if the process dies after it, the next reader or writer
/// rebuilds the cache from the journal.
fn commit(
    paths: &LedgerPaths,
    st: &mut LedgerState,
    event_type: &str,
    txid: String,
    ts: u64,
    payload: serde_json::Value,
) -> Result<(), String> {
    let seq = st.head_seq + 1;
    let hash = compute_event_hash(seq, &st.head_hash, event_type, &txid, ts, &payload);
    let ev = JournalEvent {
        seq,
        prev_hash: st.head_hash.clone(),
        event_type: event_type.into(),
        txid,
        timestamp: ts,
        payload,
        hash,
    };
    append_journal(paths, &ev)?;
    apply_event(st, &ev)?;
    // the event is committed from here on: see `refresh_cache`
    refresh_cache(paths, st);
    witness_head(
        &paths.root,
        st.head_seq,
        &st.head_hash,
        &crate::config::get().ledger.witnesses,
    );
    Ok(())
}

pub fn upsert_record(root: &Path, mut rec: InstallRecord) -> Result<InstallRecord, String> {
    let paths = init(root)?;
    let _guard = acquire_lock(&paths)?;
    let mut st = load_for_update(&paths)?;
    let ts = now_ts();
    if rec.id.is_empty() {
        rec.id = make_record_id(&rec.name);
    }
    if rec.installed_at == 0 {
        rec.installed_at = ts;
    }
    rec.updated_at = ts;
    let payload =
        serde_json::to_value(&rec).map_err(|e| format!("failed to encode record: {e}"))?;
    commit(&paths, &mut st, "upsert", make_txid("tx"), ts, payload)?;
    Ok(rec)
}

/// Drop a record from the ledger (maintenance; installs update records via status instead).
#[allow(dead_code)]
pub fn remove_record(root: &Path, id: &str) -> Result<(), String> {
    let paths = init(root)?;
    let _guard = acquire_lock(&paths)?;
    let mut st = load_for_update(&paths)?;
    if !st.records.contains_key(id) {
        return Err(format!("record not found: {id}"));
    }
    let payload = serde_json::json!({"id": id});
    commit(
        &paths,
        &mut st,
        "remove",
        make_txid("rm"),
        now_ts(),
        payload,
    )
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PathConflict {
    pub path: String,
    pub kind: String, // ledger|filesystem|pacman
    pub detail: String,
}

pub fn check_path_conflicts(
    root: &Path,
    paths_to_check: &[String],
) -> Result<Vec<PathConflict>, String> {
    let lpaths = paths_for(root);
    let st = load(&lpaths)?;
    let on_pacman = host_is_pacman();
    let mut conflicts = Vec::new();
    for p in paths_to_check {
        // ledger ownership (active records)
        for rec in st.records.values() {
            if rec.status == "removed" {
                continue;
            }
            if rec.install_root == *p || rec.paths.iter().any(|x| x == p) {
                conflicts.push(PathConflict {
                    path: p.clone(),
                    kind: "ledger".into(),
                    detail: format!("tracked by {} ({}, {})", rec.id, rec.name, rec.format),
                });
            }
        }
        // existing filesystem path
        let pp = Path::new(p);
        if pp.exists() {
            conflicts.push(PathConflict {
                path: p.clone(),
                kind: "filesystem".into(),
                detail: "path already exists on filesystem".into(),
            });
            // pacman ownership (best-effort; only where pacman is the
            // system's package manager, and the path after `--`)
            if on_pacman
                && let Ok(out) = std::process::Command::new("pacman")
                    .args(["-Qo", "--"])
                    .arg(p)
                    .output()
                && out.status.success()
            {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !s.is_empty() {
                    conflicts.push(PathConflict {
                        path: p.clone(),
                        kind: "pacman".into(),
                        detail: s,
                    });
                }
            }
        }
    }
    Ok(conflicts)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuditFinding {
    pub kind: String, // path-overlap|provides-collision|missing-dep-hint|metadata
    pub subject: String,
    pub detail: String,
}

/// True when pacman is this system's package manager. Elsewhere "not known
/// to pacman" says nothing about a path or a dependency, so pacman is not
/// asked at all.
fn host_is_pacman() -> bool {
    crate::platform::native_manager().1 == Some(crate::platform::PkgManager::Pacman)
}

/// `pacman -Q -- NAME`: the name after `--`, so that a dependency token
/// taken from record metadata can never be read as an option.
fn pacman_query_args(name: &str) -> [&str; 3] {
    ["-Q", "--", name]
}

fn pacman_has(name: &str) -> bool {
    std::process::Command::new("pacman")
        .args(pacman_query_args(name))
        .stdin(std::process::Stdio::null())
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn audit(root: &Path) -> Result<Vec<AuditFinding>, crate::Error> {
    let recs = list_records(root)?;
    let on_pacman = host_is_pacman();
    Ok(audit_records(
        recs,
        on_pacman.then_some(&pacman_has as &dyn Fn(&str) -> bool),
    ))
}

/// The audit itself. `host_has` answers "is this package installed on the
/// host"; `None` where that cannot be asked (not a pacman system), and then
/// no missing-dependency hint is given. Only tokens that are valid package
/// names are passed to it.
fn audit_records(
    recs: Vec<InstallRecord>,
    host_has: Option<&dyn Fn(&str) -> bool>,
) -> Vec<AuditFinding> {
    let active: Vec<_> = recs.into_iter().filter(|r| r.status != "removed").collect();
    let mut findings = Vec::new();
    // path ownership overlaps across different active records
    for i in 0..active.len() {
        for j in (i + 1)..active.len() {
            let a = &active[i];
            let b = &active[j];
            if a.install_root == b.install_root {
                findings.push(AuditFinding {
                    kind: "path-overlap".into(),
                    subject: a.id.clone(),
                    detail: format!(
                        "install_root overlaps with {} ({}, {}) root={}",
                        b.id, b.name, b.format, a.install_root
                    ),
                });
            }
            // check tracked file overlaps (sample/any exact)
            for pa in &a.paths {
                if b.paths.iter().any(|pb| pb == pa) {
                    findings.push(AuditFinding {
                        kind: "path-overlap".into(),
                        subject: a.id.clone(),
                        detail: format!(
                            "tracked path '{}' also owned by {} ({})",
                            pa, b.id, b.name
                        ),
                    });
                    break;
                }
            }
        }
    }
    // provides/name collisions and missing dep hints (best-effort, ledger-local)
    let mut provided: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new(); // token -> owner id
    for r in &active {
        let name_key = r.name.to_lowercase();
        provided
            .entry(name_key.clone())
            .or_insert_with(|| r.id.clone());
        if let Some(p) = r.metadata.get("provides") {
            for tok in p.split([',', '|', ' ', '\n']) {
                let t = tok.trim().to_lowercase();
                if t.is_empty() {
                    continue;
                }
                if let Some(owner) = provided.get(&t) {
                    if owner != &r.id {
                        findings.push(AuditFinding {
                            kind: "provides-collision".into(),
                            subject: r.id.clone(),
                            detail: format!("provides/name '{}' collides with {}", t, owner),
                        });
                    }
                } else {
                    provided.insert(t, r.id.clone());
                }
            }
        }
    }
    for r in &active {
        if let Some(depstr) = r
            .metadata
            .get("depends")
            .or_else(|| r.metadata.get("Requires"))
        {
            // sample first few tokens as hints (not full resolver)
            for tok in depstr.split([',', '|']) {
                let t = tok.trim();
                if t.is_empty() {
                    continue;
                }
                let base = t.split_whitespace().next().unwrap_or("").to_lowercase();
                if base.is_empty() {
                    continue;
                }
                if provided.contains_key(&base) {
                    continue;
                }
                // host presence hint
                let Some(host_has) = host_has else {
                    continue;
                };
                if !crate::util::valid_pkg_name(&base) {
                    continue;
                }
                if !host_has(&base) {
                    findings.push(AuditFinding {
                        kind: "missing-dep-hint".into(),
                        subject: r.id.clone(),
                        detail: format!("declared dependency '{}' not found in ledger-provides or installed pacman packages (best-effort)", t.trim()),
                    });
                }
            }
        }
        if r.format.ends_with("-extract")
            && !r.metadata.contains_key("depends")
            && !r.metadata.contains_key("analysis_notes")
        {
            findings.push(AuditFinding {
                kind: "metadata".into(),
                subject: r.id.clone(),
                detail: "no dependency/analysis metadata recorded".into(),
            });
        }
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_root(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "pc-ledger-test-{tag}-{}-{}",
            std::process::id(),
            now_nanos()
        ))
    }

    fn sample_rec(name: &str, format: &str) -> InstallRecord {
        InstallRecord {
            id: String::new(),
            name: name.into(),
            format: format.into(),
            source: "test".into(),
            version: "1.0".into(),
            scope: "user".into(),
            install_root: format!("/tmp/x/{name}"),
            paths: vec![],
            status: "installed".into(),
            installed_at: 0,
            updated_at: 0,
            metadata: BTreeMap::new(),
        }
    }

    fn ok(root: &Path) -> VerifyReport {
        verify_with(root, &VerifyOptions::default(), &[]).unwrap()
    }

    #[test]
    fn init_upsert_verify_roundtrip() {
        let root = tmp_root("roundtrip");
        init(&root).unwrap();
        let rec = upsert_record(&root, sample_rec("foo", "appimage")).unwrap();
        assert!(rec.id.starts_with("pkg-foo-"));
        let recs = list_records(&root).unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].name, "foo");
        // init event + upsert event
        let v = ok(&root);
        assert_eq!(v.events, 2);
        assert!(v.cache_current);
        assert!(
            v.anchored_through.is_none() && v.render().contains("tamper evidence: NONE"),
            "an unanchored chain must not be presented as tamper-evident: {}",
            v.render()
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn reads_never_create_a_ledger() {
        let root = tmp_root("readonly");
        assert!(list_records(&root).unwrap().is_empty());
        assert_eq!(head(&root).unwrap().0, 0);
        status(&root).unwrap();
        assert_eq!(ok(&root).events, 0);
        assert!(!root.exists(), "read paths must leave no trace on disk");
    }

    #[test]
    fn ids_unique_for_same_name_same_second() {
        let root = tmp_root("ids");
        init(&root).unwrap();
        let a = upsert_record(&root, sample_rec("foo", "appimage")).unwrap();
        let b = upsert_record(&root, sample_rec("foo", "deb-extract")).unwrap();
        assert_ne!(a.id, b.id, "same-name same-second ids must not collide");
        assert_eq!(list_records(&root).unwrap().len(), 2);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn tampered_journal_fails_verify() {
        let root = tmp_root("tamper");
        init(&root).unwrap();
        upsert_record(&root, sample_rec("foo", "appimage")).unwrap();
        let journal = root.join("journal.jsonl");
        // journal is JSONL: tamper with the LAST line only
        let lines: Vec<String> = fs::read_to_string(&journal)
            .unwrap()
            .lines()
            .map(|l| l.to_string())
            .collect();
        let last = lines.last().unwrap().clone();
        let ev: serde_json::Value = serde_json::from_str(&last).unwrap();
        let mut forged = ev.clone();
        forged["event_type"] = serde_json::json!("checkpoint");
        let mut out = lines.clone();
        *out.last_mut().unwrap() = forged.to_string();
        fs::write(&journal, out.join("\n") + "\n").unwrap();
        let res = verify_with(&root, &VerifyOptions::default(), &[]);
        let err = res.expect_err("tampered journal must fail verify");
        assert_eq!(err.kind, crate::Kind::Integrity);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn remove_record_works_and_verifies() {
        let root = tmp_root("remove");
        init(&root).unwrap();
        let rec = upsert_record(&root, sample_rec("bar", "flatpak")).unwrap();
        remove_record(&root, &rec.id).unwrap();
        assert!(list_records(&root).unwrap().is_empty());
        // init + upsert + remove = 3 events
        assert_eq!(ok(&root).events, 3);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn lock_is_exclusive_and_released_on_drop() {
        let root = tmp_root("lock");
        init(&root).unwrap();
        let paths = paths_for(&root);
        let short = std::time::Duration::from_millis(120);
        let g = acquire_lock_within(&paths, short).unwrap();
        assert!(
            acquire_lock_within(&paths, short).is_err(),
            "second lock must fail while the first is held"
        );
        drop(g);
        assert!(acquire_lock_within(&paths, short).is_ok());
        let _ = fs::remove_dir_all(&root);
    }

    /// The crash the old code could not survive: the journal line is on
    /// disk, the process dies before the cache is rewritten.
    #[test]
    fn crash_between_journal_and_cache_is_recovered() {
        let root = tmp_root("crash");
        init(&root).unwrap();
        upsert_record(&root, sample_rec("one", "appimage")).unwrap();
        let state = root.join("state.json");
        let before = fs::read(&state).unwrap();
        upsert_record(&root, sample_rec("two", "appimage")).unwrap();
        fs::write(&state, &before).unwrap(); // cache as it was before the "crash"

        // readers already see the truth, without writing anything
        assert_eq!(list_records(&root).unwrap().len(), 2);
        assert!(!ok(&root).cache_current);

        // the next change continues the chain instead of forking it
        upsert_record(&root, sample_rec("three", "appimage")).unwrap();
        let v = ok(&root);
        assert_eq!(v.events, 4);
        assert!(v.cache_current);
        let names: Vec<String> = list_records(&root)
            .unwrap()
            .into_iter()
            .map(|r| r.name)
            .collect();
        for n in ["one", "two", "three"] {
            assert!(names.iter().any(|x| x == n), "{n} lost: {names:?}");
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn deleted_cache_is_rebuilt_from_the_journal() {
        let root = tmp_root("nocache");
        init(&root).unwrap();
        upsert_record(&root, sample_rec("one", "appimage")).unwrap();
        fs::remove_file(root.join("state.json")).unwrap();
        assert_eq!(list_records(&root).unwrap().len(), 1);
        upsert_record(&root, sample_rec("two", "appimage")).unwrap();
        assert_eq!(ok(&root).events, 3);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn interrupted_append_is_discarded_not_fatal() {
        let root = tmp_root("torn");
        init(&root).unwrap();
        upsert_record(&root, sample_rec("one", "appimage")).unwrap();
        let journal = root.join("journal.jsonl");
        let mut f = OpenOptions::new().append(true).open(&journal).unwrap();
        f.write_all(br#"{"seq":3,"prev_hash":"abc","event_ty"#)
            .unwrap();
        drop(f);
        let v = ok(&root);
        assert!(v.torn_tail);
        assert_eq!(v.events, 2);
        upsert_record(&root, sample_rec("two", "appimage")).unwrap();
        let v = ok(&root);
        assert!(!v.torn_tail);
        assert_eq!(v.events, 3);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn legacy_djb2_events_fail_verify_unless_accepted() {
        let root = tmp_root("legacy");
        fs::create_dir_all(&root).unwrap();
        let payload = serde_json::json!({"root": "x"});
        let body = format!("1|GENESIS|init|init|7|{payload}");
        let ev = JournalEvent {
            seq: 1,
            prev_hash: "GENESIS".into(),
            event_type: "init".into(),
            txid: "init".into(),
            timestamp: 7,
            payload,
            hash: djb2_hex(&body),
        };
        fs::write(
            root.join("journal.jsonl"),
            serde_json::to_string(&ev).unwrap() + "\n",
        )
        .unwrap();
        let strict = verify_with(&root, &VerifyOptions::default(), &[]).unwrap_err();
        assert_eq!(strict.kind, crate::Kind::Integrity);
        let lenient = verify_with(
            &root,
            &VerifyOptions {
                accept_legacy: true,
                anchors: vec![],
            },
            &[],
        )
        .unwrap();
        assert_eq!(lenient.legacy_events, 1);
        let _ = fs::remove_dir_all(&root);
    }

    /// The attack the bare chain cannot see: rewrite history and recompute
    /// every hash. A witness that kept the old head exposes it.
    #[test]
    fn witness_detects_a_fully_recomputed_rewrite_and_truncation() {
        let root = tmp_root("witness");
        let witness = tmp_root("witness-file").with_extension("log");
        let w = vec![witness.display().to_string()];
        init(&root).unwrap();
        upsert_record(&root, sample_rec("honest", "appimage")).unwrap();
        let (seq, hash) = head(&root).unwrap();
        witness_head(&root, seq, &hash, &w);
        let good = verify_with(&root, &VerifyOptions::default(), &w).unwrap();
        assert_eq!(good.anchored_through, Some(2));

        // a manual anchor works the same way
        let anchor = parse_anchor(&root, &format!("{seq}:{hash}")).unwrap();
        let opts = VerifyOptions {
            accept_legacy: false,
            anchors: vec![anchor],
        };
        assert!(verify_with(&root, &opts, &[]).is_ok());

        // attacker: start over with a different history, all hashes valid
        let saved = fs::read(root.join("journal.jsonl")).unwrap();
        fs::remove_dir_all(&root).unwrap();
        init(&root).unwrap();
        upsert_record(&root, sample_rec("evil", "appimage")).unwrap();
        assert!(
            verify_with(&root, &VerifyOptions::default(), &[]).is_ok(),
            "without a witness the forged chain is indistinguishable"
        );
        let e = verify_with(&root, &VerifyOptions::default(), &w).unwrap_err();
        assert_eq!(e.kind, crate::Kind::Integrity);
        assert!(e.to_string().contains("REWRITTEN"), "{e}");
        assert!(verify_with(&root, &opts, &[]).is_err());

        // attacker: truncate back to genesis
        let first_line = saved.split(|b| *b == b'\n').next().unwrap().to_vec();
        fs::write(
            root.join("journal.jsonl"),
            [first_line, b"\n".to_vec()].concat(),
        )
        .unwrap();
        let e = verify_with(&root, &VerifyOptions::default(), &w).unwrap_err();
        assert!(e.to_string().contains("TRUNCATED"), "{e}");
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_file(&witness);
    }

    fn kind_of<T>(res: Result<T, crate::Error>) -> crate::Kind {
        match res {
            Ok(_) => panic!("expected an error"),
            Err(e) => e.kind,
        }
    }

    /// Review finding 19: only `state.json` is edited (the head is left
    /// alone). No command may act on the edited record, and verify fails.
    #[test]
    fn an_edited_cache_is_never_served_and_fails_verify() {
        let root = tmp_root("edited-cache");
        init(&root).unwrap();
        upsert_record(&root, sample_rec("honest", "appimage")).unwrap();
        let state = root.join("state.json");
        let edited = fs::read_to_string(&state)
            .unwrap()
            .replace("\"honest\"", "\"evil\"")
            .replace("/tmp/x/honest", "/home/victim/Documents");
        assert!(edited.contains("/home/victim/Documents"));
        fs::write(&state, edited).unwrap();

        // what `remove appimage` / `remove external` would act on
        let recs = list_records(&root).unwrap();
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].name, "honest");
        assert_eq!(recs[0].install_root, "/tmp/x/honest");

        let e = verify_with(&root, &VerifyOptions::default(), &[]).unwrap_err();
        assert_eq!(e.kind, crate::Kind::Integrity);
        assert!(
            e.to_string()
                .contains("state.json does not match the journal"),
            "{e}"
        );
        assert_eq!(kind_of(status(&root)), crate::Kind::Integrity);

        // the next change rewrites the cache from the journal
        upsert_record(&root, sample_rec("next", "appimage")).unwrap();
        let v = ok(&root);
        assert!(v.cache_current);
        assert!(status(&root).unwrap().contains("state cache: current"));
        assert!(
            !fs::read_to_string(&state).unwrap().contains("victim"),
            "the edited record must not survive"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_cache_without_its_journal_is_an_integrity_error() {
        let root = tmp_root("orphan-cache");
        init(&root).unwrap();
        upsert_record(&root, sample_rec("one", "appimage")).unwrap();
        fs::remove_file(root.join("journal.jsonl")).unwrap();

        assert_eq!(kind_of(list_records(&root)), crate::Kind::Integrity);
        assert_eq!(kind_of(head(&root)), crate::Kind::Integrity);
        assert_eq!(kind_of(status(&root)), crate::Kind::Integrity);
        assert_eq!(kind_of(audit(&root)), crate::Kind::Integrity);
        let e = verify_with(&root, &VerifyOptions::default(), &[]).unwrap_err();
        assert_eq!(e.kind, crate::Kind::Integrity);
        assert!(e.to_string().contains("journal.jsonl is missing"), "{e}");

        // and a change does not quietly start a new journal over it
        let e = upsert_record(&root, sample_rec("two", "appimage")).unwrap_err();
        assert!(e.contains("journal.jsonl is missing"), "{e}");
        assert!(!root.join("journal.jsonl").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_cache_ahead_of_the_journal_fails_verify() {
        let root = tmp_root("cache-ahead");
        init(&root).unwrap();
        upsert_record(&root, sample_rec("one", "appimage")).unwrap();
        let journal = root.join("journal.jsonl");
        let before = fs::read(&journal).unwrap();
        upsert_record(&root, sample_rec("two", "appimage")).unwrap();
        // the journal loses its last event; the cache still knows it
        fs::write(&journal, &before).unwrap();
        let e = verify_with(&root, &VerifyOptions::default(), &[]).unwrap_err();
        assert_eq!(e.kind, crate::Kind::Integrity);
        assert!(e.to_string().contains("truncated or rewritten"), "{e}");
        // readers answer from the journal alone
        assert_eq!(list_records(&root).unwrap().len(), 1);
        let _ = fs::remove_dir_all(&root);
    }

    fn mode_of(p: &Path) -> u32 {
        fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn a_ledger_written_by_an_ordinary_user_is_private() {
        assert_eq!(modes_for(false), (0o700, 0o600));
        assert_eq!(modes_for(true), (0o755, 0o644));
        if crate::util::is_root() {
            return; // root writes the system-scope ledger, readable by all
        }
        let base = tmp_root("modes");
        let root = base.join("state/yapacc/ledger");
        let witness = base.join("witness.log");
        init(&root).unwrap();
        upsert_record(&root, sample_rec("one", "appimage")).unwrap();
        let (seq, hash) = head(&root).unwrap();
        witness_head(&root, seq, &hash, &[witness.display().to_string()]);
        for dir in [&root, &base.join("state/yapacc"), &base.join("state")] {
            assert_eq!(mode_of(dir), 0o700, "{}", dir.display());
        }
        for file in ["journal.jsonl", "state.json", "ledger.lock"] {
            assert_eq!(mode_of(&root.join(file)), 0o600, "{file}");
        }
        assert_eq!(mode_of(&witness), 0o600);

        // a ledger an earlier version created with the umask default is
        // tightened by the next change, and stays readable
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        for file in ["journal.jsonl", "state.json", "ledger.lock"] {
            fs::set_permissions(root.join(file), fs::Permissions::from_mode(0o644)).unwrap();
        }
        assert_eq!(list_records(&root).unwrap().len(), 1);
        upsert_record(&root, sample_rec("two", "appimage")).unwrap();
        assert_eq!(mode_of(&root), 0o700);
        for file in ["journal.jsonl", "state.json", "ledger.lock"] {
            assert_eq!(mode_of(&root.join(file)), 0o600, "{file}");
        }
        assert_eq!(ok(&root).events, 3);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn the_journald_witness_reads_only_the_owner_and_root() {
        assert_eq!(
            journald_args(1000),
            vec![
                "-t",
                "yapacc-ledger",
                "_UID=1000",
                "_UID=0",
                "-o",
                "cat",
                "--no-pager",
                "-q"
            ]
        );
        assert_eq!(
            journald_args(0),
            vec![
                "-t",
                "yapacc-ledger",
                "_UID=0",
                "-o",
                "cat",
                "--no-pager",
                "-q"
            ]
        );
        // the owner is whoever owns the journal: this process, in a test
        let root = tmp_root("owner");
        init(&root).unwrap();
        let me = fs::metadata(root.join("journal.jsonl")).unwrap().uid();
        assert_eq!(ledger_owner_uid(&root), Some(me));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn the_head_of_an_empty_ledger_is_a_valid_anchor() {
        let root = tmp_root("empty-anchor");
        let (seq, hash) = head(&root).unwrap();
        assert_eq!((seq, hash.as_str()), (0, "GENESIS"));
        let anchor = parse_anchor(&root, &format!("{seq}:{hash}")).unwrap();
        let opts = VerifyOptions {
            accept_legacy: false,
            anchors: vec![anchor],
        };
        // true of the empty ledger and of any later one; confirms nothing
        let v = verify_with(&root, &opts, &[]).unwrap();
        assert_eq!((v.events, v.anchors_matched), (0, 0));
        init(&root).unwrap();
        upsert_record(&root, sample_rec("one", "appimage")).unwrap();
        let v = verify_with(&root, &opts, &[]).unwrap();
        assert_eq!(v.anchored_through, None);
        assert!(v.render().contains("tamper evidence: NONE"));

        for bad in ["0:abc", "1:GENESIS", "GENESIS", "0:", ":GENESIS"] {
            assert!(parse_anchor(&root, bad).is_err(), "{bad}");
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn audit_asks_the_host_only_about_valid_names_and_only_on_pacman() {
        assert_eq!(pacman_query_args("ripgrep"), ["-Q", "--", "ripgrep"]);
        let mut rec = sample_rec("tool", "deb-extract");
        rec.id = "pkg-tool-1".into();
        rec.metadata.insert(
            "depends".into(),
            "--config=/x, -y, ripgrep (>= 1), libfoo | a;b, --root=/, tool".into(),
        );
        let asked = std::cell::RefCell::new(Vec::<String>::new());
        let probe = |name: &str| {
            asked.borrow_mut().push(name.to_string());
            name == "ripgrep"
        };
        let findings = audit_records(vec![rec.clone()], Some(&probe));
        assert_eq!(
            *asked.borrow(),
            vec!["ripgrep", "libfoo"],
            "option-like and malformed tokens never reach the package manager"
        );
        let hints: Vec<&AuditFinding> = findings
            .iter()
            .filter(|f| f.kind == "missing-dep-hint")
            .collect();
        assert_eq!(hints.len(), 1, "{findings:?}");
        assert!(hints[0].detail.contains("'libfoo'"), "{:?}", hints[0]);

        // not a pacman system: nothing is asked and nothing is called missing
        let findings = audit_records(vec![rec], None);
        assert!(
            findings.iter().all(|f| f.kind != "missing-dep-hint"),
            "{findings:?}"
        );
    }

    /// Disk full after the journal append: the event is committed, so the
    /// change must not be reported as failed.
    #[test]
    fn a_cache_that_cannot_be_written_does_not_fail_a_committed_change() {
        let root = tmp_root("cache-unwritable");
        init(&root).unwrap();
        upsert_record(&root, sample_rec("one", "appimage")).unwrap();
        // make the temp file impossible to create
        fs::create_dir(root.join("state.json.tmp")).unwrap();
        let rec = upsert_record(&root, sample_rec("two", "appimage")).unwrap();
        assert!(rec.id.starts_with("pkg-two-"));
        assert_eq!(list_records(&root).unwrap().len(), 2);
        let v = ok(&root);
        assert_eq!(v.events, 3);
        assert!(!v.cache_current, "the cache is one event behind");
        // once the cache can be written again, the next change repairs it
        fs::remove_dir(root.join("state.json.tmp")).unwrap();
        upsert_record(&root, sample_rec("three", "appimage")).unwrap();
        assert!(ok(&root).cache_current);
        let _ = fs::remove_dir_all(&root);
    }
}
