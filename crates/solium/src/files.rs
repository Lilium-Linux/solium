//! Files from Lua without blocking the compositor (#473): `sol.read`,
//! `sol.write`, `sol.watch` and `sol.store`.
//!
//! The configuration runs on the compositor's own thread, so a read that
//! waits on a slow disk would stall every window. Here a read or a write
//! runs on a thread of its own, and what it came to is picked up in
//! [`settle`], once each time round the event loop, as `runs.rs` reads its
//! children: both loops wake at least every 16 ms, so a result waits no
//! longer than that to reach Lua.
//!
//! - **A read** goes to the end of the file rather than trusting its size,
//!   which `/proc` and `/sys` report as 0, and stops at `files.max_read`.
//! - **A write** is atomic: a temporary file beside the target, synced,
//!   renamed over it, and the directory synced, so a crash half-way leaves
//!   the old file whole. Writes to one path are applied in the order they
//!   were asked for: only one runs at a time, and the next waits for it.
//! - **A watch** is on the file's directory, through one non-blocking
//!   inotify instance read in [`settle`], so a file replaced by a rename,
//!   as editors and `sol.write` replace one, is still followed, and one that
//!   does not exist yet is seen when it appears.
//! - **A store** is a table the configuration keeps, loaded from
//!   `$XDG_STATE_HOME/solium/state/<name>.json` when it is first asked for,
//!   and written back through the same atomic write at most once a second
//!   when it changed, before a reload, and when the compositor stops.
//!
//! The Lua side, which keeps each callback under its id, is in `script.rs`.

use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    ffi::OsString,
    fs::{self, File},
    io::{self, Read as _, Write as _},
    path::{Path, PathBuf},
    sync::mpsc,
    thread::JoinHandle,
    time::Duration,
};

use crate::{json::Json, state::Solium};

/// How much `sol.read` reads when `files.max_read` does not say: 4 MiB.
pub(crate) const MAX_READ: usize = 4 << 20;
/// The largest store file that is read when `files.store_max` does not
/// say: 1 MiB. A store is settings and small state, read on the
/// compositor's own thread when it is first asked for, so a bigger one is
/// taken for a broken one.
pub(crate) const MAX_STORE: u64 = 1 << 20;
/// How often the stores are compared with what was last written when
/// `files.store_interval` does not say.
pub(crate) const STORE_INTERVAL: Duration = Duration::from_secs(1);
/// What a read past `files.max_read` is told.
pub(crate) const TOO_LONG: &str = "longer than files.max_read";

/// A path a configuration names, made absolute: `~/` is the home
/// directory, and anything that is neither absolute nor under `~/` is
/// refused, since the compositor's working directory is nobody's.
/// `tests::a_path_is_absolute_or_under_home`.
pub(crate) fn expand(path: &str) -> Result<PathBuf, String> {
    if let Some(rest) = path.strip_prefix("~/") {
        let home = std::env::var_os("HOME")
            .filter(|home| !home.is_empty())
            .ok_or_else(|| format!("{path}: HOME is not set, so `~/` names nothing"))?;
        return Ok(PathBuf::from(home).join(rest));
    }
    if path.starts_with('/') {
        return Ok(PathBuf::from(path));
    }
    Err(format!(
        "{path:?} is neither absolute nor under `~/`: a path is relative to nothing here"
    ))
}

/// Read `path` to its end, at most `max` bytes: the error, as the system
/// words it, when it cannot be read, and [`TOO_LONG`] past `max`.
/// `tests::a_proc_file_is_read_to_its_end`, `tests::a_read_stops_at_its_cap`.
pub(crate) fn read_capped(path: &Path, max: usize) -> Result<Vec<u8>, String> {
    let file = File::open(path).map_err(|err| err.to_string())?;
    let mut data = Vec::new();
    let limit = u64::try_from(max).unwrap_or(u64::MAX).saturating_add(1);
    file.take(limit)
        .read_to_end(&mut data)
        .map_err(|err| err.to_string())?;
    if data.len() > max {
        return Err(TOO_LONG.to_owned());
    }
    Ok(data)
}

/// Where a write to `target` is made before it is renamed over it: beside
/// it, so the rename stays on one filesystem and is atomic, and named for
/// this process, so two compositors never share one.
pub(crate) fn temporary(target: &Path) -> PathBuf {
    let mut name = target.file_name().map(OsString::from).unwrap_or_default();
    name.push(format!(".solium-tmp-{}", std::process::id()));
    target.with_file_name(name)
}

/// Write `data` to `path` atomically: to [`temporary`], synced, renamed over
/// `path`, and the directory synced, so the rename itself survives a power
/// cut. Until the rename `path` is the old file, whole; after it, the new
/// one. A symbolic link is followed, and the file it points at is the one
/// replaced. An existing file's permissions are kept. The directory must
/// exist. `tests::an_interrupted_write_leaves_the_old_file_whole`.
pub(crate) fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    let target = if fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink()) {
        fs::canonicalize(path)?
    } else {
        path.to_owned()
    };
    let Some(directory) = target.parent().filter(|dir| !dir.as_os_str().is_empty()) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a file to write needs a directory",
        ));
    };
    let temporary = temporary(&target);
    let written = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temporary)?;
        if let Ok(meta) = fs::metadata(&target) {
            file.set_permissions(meta.permissions())?;
        }
        file.write_all(data)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, &target)?;
        File::open(directory)?.sync_all()
    })();
    if written.is_err() {
        // Gone already when only the directory's sync failed.
        let _ = fs::remove_file(&temporary);
    }
    written
}

/// Where the stores are kept: `$XDG_STATE_HOME/solium/state`, or
/// `~/.local/state/solium/state`. `None` with neither set, when a store
/// lives only as long as the compositor.
///
/// Under test, never the user's own: a temporary directory a test may set
/// for its own thread with `tests::use_state_dir`.
pub(crate) fn state_dir() -> Option<PathBuf> {
    #[cfg(test)]
    {
        Some(tests::state_dir_for_this_test())
    }
    #[cfg(not(test))]
    {
        let base = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME")
                    .filter(|home| !home.is_empty())
                    .map(|home| PathBuf::from(home).join(".local/state"))
            })?;
        Some(base.join("solium").join("state"))
    }
}

/// Whether `name` may name a store: letters, digits, `_` and `-`, so it is
/// always one file in the state directory.
pub(crate) fn store_name_ok(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// The file store `name` is kept in.
pub(crate) fn store_path(name: &str) -> Option<PathBuf> {
    state_dir().map(|dir| dir.join(format!("{name}.json")))
}

/// What a store's file held.
#[derive(Debug, PartialEq)]
pub(crate) enum StoreFile {
    /// No file yet, or nowhere to keep one.
    Missing,
    /// A JSON object or list.
    Found(Json),
    /// A file that cannot be used, at `path`, and why.
    Broken { path: PathBuf, why: String },
}

/// Store `name`'s file, read, at most `max` bytes of it
/// (`files.store_max`), and nothing changed: the caller decides what
/// happens to a broken one ([`set_aside`] in a session, a warning under
/// `--check`). One that is not JSON, not an object or a list, not UTF-8, or
/// larger than `max` is broken. `tests::a_broken_store_file_is_kept_as_broken`.
pub(crate) fn read_store(name: &str, max: u64) -> StoreFile {
    let Some(path) = store_path(name) else {
        return StoreFile::Missing;
    };
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return StoreFile::Missing,
        Err(err) => {
            return StoreFile::Broken {
                path,
                why: err.to_string(),
            };
        }
    };
    let mut bytes = Vec::new();
    let why = match file.take(max.saturating_add(1)).read_to_end(&mut bytes) {
        Err(err) => err.to_string(),
        Ok(_) if bytes.len() as u64 > max => format!("larger than files.store_max, {max} bytes"),
        Ok(_) => match String::from_utf8(bytes)
            .ok()
            .and_then(|text| Json::parse(&text))
        {
            Some(json @ (Json::Object(_) | Json::List(_))) => return StoreFile::Found(json),
            Some(_) => "neither a JSON object nor a list".to_owned(),
            None => "not JSON".to_owned(),
        },
    };
    StoreFile::Broken { path, why }
}

/// Move a store file that cannot be used out of the way, to
/// `<name>.json.broken`, so the next save does not overwrite it unseen.
pub(crate) fn set_aside(name: &str, path: &Path) {
    let broken = path.with_file_name(format!("{name}.json.broken"));
    match fs::rename(path, &broken) {
        Ok(()) => {
            tracing::warn!(store = name, kept = %broken.display(), "the unusable store file is kept beside it");
        }
        Err(err) => {
            tracing::warn!(store = name, %err, "the unusable store file could not be set aside");
        }
    }
}

/// A Lua value as the plain data a store keeps: strings, finite numbers,
/// booleans, and tables of them, a sequence as a list and string keys as an
/// object. Anything else is left out, and named in the log once per key
/// path (`warned` holds those already named). `where_it_is` is the path down
/// to `value`, from the store's name. `left` bounds the values read, since
/// the walk is Rust, which the handler deadline cannot stop: past it the
/// whole store is an error and is not saved.
/// `tests::only_plain_data_is_stored`.
pub(crate) fn plain(
    value: &mlua::Value,
    where_it_is: &str,
    depth: usize,
    left: &mut usize,
    warned: &mut HashSet<String>,
) -> Result<Option<Json>, String> {
    /// As deep as JSON text may nest (`json.rs`).
    const DEEPEST: usize = 64;
    *left = left
        .checked_sub(1)
        .ok_or_else(|| "more than 65536 values".to_owned())?;
    fn leave_out(
        warned: &mut HashSet<String>,
        where_it_is: &str,
        why: &str,
    ) -> Result<Option<Json>, String> {
        if warned.insert(where_it_is.to_owned()) {
            tracing::warn!(
                key = where_it_is,
                why,
                "a store keeps only strings, numbers, booleans and tables of them; this is left out"
            );
        }
        Ok(None)
    }
    match value {
        mlua::Value::Nil => Ok(None),
        mlua::Value::Boolean(yes) => Ok(Some(Json::Bool(*yes))),
        #[expect(
            clippy::cast_precision_loss,
            reason = "stored as JSON, whose numbers are doubles"
        )]
        mlua::Value::Integer(number) => Ok(Some(Json::Number(*number as f64))),
        mlua::Value::Number(number) if number.is_finite() => Ok(Some(Json::Number(*number))),
        mlua::Value::Number(_) => leave_out(warned, where_it_is, "a number that is not finite"),
        mlua::Value::String(text) => match text.to_str() {
            Ok(text) => Ok(Some(Json::Text(text.to_owned()))),
            Err(_) => leave_out(warned, where_it_is, "a string that is not UTF-8"),
        },
        mlua::Value::Table(table) => {
            if depth >= DEEPEST {
                return leave_out(
                    warned,
                    where_it_is,
                    "nested deeper than 64, or a table that contains itself",
                );
            }
            let pairs: Vec<(mlua::Value, mlua::Value)> = table
                .pairs::<mlua::Value, mlua::Value>()
                .filter_map(Result::ok)
                .collect();
            let length = table.raw_len();
            let is_list = length > 0
                && pairs.len() == length
                && pairs.iter().all(|(key, _)| {
                    matches!(key, mlua::Value::Integer(index)
                        if usize::try_from(*index).is_ok_and(|index| (1..=length).contains(&index)))
                });
            if is_list {
                let mut items = Vec::with_capacity(length);
                for index in 1..=length {
                    let item = table
                        .raw_get::<mlua::Value>(index)
                        .unwrap_or(mlua::Value::Nil);
                    let named = format!("{where_it_is}.{index}");
                    match plain(&item, &named, depth + 1, left, warned)? {
                        Some(item) => items.push(item),
                        // A list keeps its positions: what cannot be kept
                        // in one is `null`.
                        None => items.push(Json::Null),
                    }
                }
                return Ok(Some(Json::List(items)));
            }
            let mut fields = BTreeMap::new();
            for (key, item) in pairs {
                let key = match key {
                    mlua::Value::String(key) => match key.to_str() {
                        Ok(key) => key.to_owned(),
                        Err(_) => continue,
                    },
                    other => {
                        let named = format!("{where_it_is}.{}", key_name(&other));
                        if warned.insert(named.clone()) {
                            tracing::warn!(
                                key = named,
                                "a store's tables are lists or have string keys; this key is left out"
                            );
                        }
                        continue;
                    }
                };
                let named = format!("{where_it_is}.{key}");
                if let Some(item) = plain(&item, &named, depth + 1, left, warned)? {
                    fields.insert(key, item);
                }
            }
            Ok(Some(Json::Object(fields)))
        }
        other => leave_out(warned, where_it_is, other.type_name()),
    }
}

fn key_name(key: &mlua::Value) -> String {
    match key {
        mlua::Value::Integer(number) => number.to_string(),
        mlua::Value::Number(number) => number.to_string(),
        other => format!("<{}>", other.type_name()),
    }
}

/// What a watch saw happen to its file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    /// Written and closed, or another file renamed onto it.
    Changed,
    Created,
    /// Deleted, or renamed away.
    Removed,
}

/// What [`Files::pump`] heard, for the configuration.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Heard {
    /// A read ended: what it read, or why it could not.
    Read {
        id: u64,
        result: Result<Vec<u8>, String>,
    },
    /// A write ended, with the error when it failed.
    Wrote { id: u64, error: Option<String> },
    /// A watched file changed.
    Watched { id: u64, kind: Kind, path: String },
    /// A watch ended because its directory went away: its function is let
    /// go. Told after the `removed` it was owed.
    WatchEnded { id: u64 },
}

/// `sol.read`: read `path`, up to `max` bytes, and tell read `id`.
#[derive(Clone, Debug)]
pub(crate) struct ReadRequest {
    pub(crate) id: u64,
    pub(crate) path: PathBuf,
    pub(crate) max: usize,
}

/// `sol.write`: write `data` to `path`, and tell write `id` when `report`.
#[derive(Clone, Debug)]
pub(crate) struct WriteRequest {
    pub(crate) id: u64,
    pub(crate) path: PathBuf,
    pub(crate) data: Vec<u8>,
    pub(crate) report: bool,
}

/// One write, waiting or running.
#[derive(Debug)]
struct Job {
    path: PathBuf,
    data: Vec<u8>,
    /// Who is told: a `sol.write`'s id and the configuration it came from,
    /// or the store it saves.
    by: By,
}

/// One read, waiting or running, of the configuration of `generation`.
#[derive(Debug)]
struct ReadJob {
    id: u64,
    path: PathBuf,
    max: usize,
    generation: u64,
}

/// What waits on a path behind the write running there: a later write, or
/// a read that must see what the writes before it wrote.
#[derive(Debug)]
enum Queued {
    Write(Job),
    Read(ReadJob),
}

#[derive(Clone, Debug)]
enum By {
    /// A `sol.write`, told when `report`.
    Lua {
        id: u64,
        generation: u64,
        report: bool,
    },
    /// A store's save, of the text it was saved with.
    Store { name: String },
}

/// What a worker thread sends back.
#[derive(Debug)]
enum Finished {
    Read {
        id: u64,
        generation: u64,
        result: Result<Vec<u8>, String>,
    },
    Wrote {
        path: PathBuf,
        by: By,
        text: Vec<u8>,
        error: Option<String>,
    },
}

/// One file `sol.watch` follows.
#[derive(Debug)]
struct Watch {
    directory: inotify::WatchDescriptor,
    name: OsString,
    /// The path as the configuration wrote it, for its events.
    shown: String,
    /// Whether the last thing it was told was `removed`, so the end of its
    /// directory does not tell it twice.
    removed: bool,
}

/// How many reads run at once. More wait, in the order they were asked
/// for, so a configuration that reads in a loop cannot start a thread for
/// each. Not a setting: it bounds the compositor's own threads, and four
/// keep a slow file from holding the others up.
const READS_AT_ONCE: usize = 4;

/// How long the compositor's end waits for writes still under way, in all.
/// Not a setting: it is how long a session's end may be held, and a write
/// to a local disk takes far less. A write still pending then is named in
/// the log and given up; the file it was replacing stays whole.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(2);

/// The configuration's reads, writes, watches and stores.
#[derive(Debug)]
pub(crate) struct Files {
    sender: mpsc::Sender<Finished>,
    receiver: mpsc::Receiver<Finished>,
    /// Counts reloads: what a read or a write of an older configuration
    /// comes to is told to nobody.
    generation: u64,
    /// Paths with a write running, and what waits behind it there.
    writing: HashMap<PathBuf, VecDeque<Queued>>,
    /// The threads of the writes running, so the compositor can wait for
    /// them before it stops.
    threads: Vec<JoinHandle<()>>,
    /// How many reads are running, at most [`READS_AT_ONCE`].
    reading: usize,
    /// Reads waiting for one of those to end, in order.
    reads_waiting: VecDeque<ReadJob>,
    /// What could not reach a thread: a thread that could not start.
    heard: Vec<Heard>,
    /// One instance for every watch, made with the first.
    inotify: Option<inotify::Inotify>,
    /// Each watched directory's watches.
    directories: HashMap<inotify::WatchDescriptor, Vec<u64>>,
    watches: HashMap<u64, Watch>,
    /// Each store's text as it was last saved, or loaded: a store is saved
    /// only when it differs.
    stored: HashMap<String, String>,
    /// When the stores are next compared, on the compositor's clock.
    next_save: Duration,
    /// How often the configuration is asked for its stores:
    /// `files.store_interval`.
    pub(crate) store_interval: Duration,
}

impl Default for Files {
    fn default() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            sender,
            receiver,
            generation: 0,
            writing: HashMap::new(),
            threads: Vec::new(),
            reading: 0,
            reads_waiting: VecDeque::new(),
            heard: Vec::new(),
            inotify: None,
            directories: HashMap::new(),
            watches: HashMap::new(),
            stored: HashMap::new(),
            next_save: Duration::ZERO,
            store_interval: STORE_INTERVAL,
        }
    }
}

/// The watch mask: what happens to a name in a directory, the directory
/// itself going, and `ONLYDIR`, since a watch is always on its file's
/// directory.
const MASK: inotify::WatchMask = inotify::WatchMask::CLOSE_WRITE
    .union(inotify::WatchMask::MOVED_TO)
    .union(inotify::WatchMask::CREATE)
    .union(inotify::WatchMask::DELETE)
    .union(inotify::WatchMask::MOVED_FROM)
    .union(inotify::WatchMask::DELETE_SELF)
    .union(inotify::WatchMask::ONLYDIR);

impl Files {
    /// Read a file: after the writes asked for before it on the same path,
    /// so a configuration reads back what it wrote, and on a thread of its
    /// own once fewer than [`READS_AT_ONCE`] are running.
    /// `tests::a_read_after_a_write_sees_what_was_written`.
    pub(crate) fn read(&mut self, request: ReadRequest) {
        let ReadRequest { id, path, max } = request;
        let job = ReadJob {
            id,
            path,
            max,
            generation: self.generation,
        };
        match self.writing.get_mut(&job.path) {
            Some(waiting) => waiting.push_back(Queued::Read(job)),
            None => self.read_when_free(job),
        }
    }

    /// Start `job`, or wait it behind the reads running.
    fn read_when_free(&mut self, job: ReadJob) {
        if self.reading >= READS_AT_ONCE {
            self.reads_waiting.push_back(job);
            return;
        }
        let ReadJob {
            id,
            path,
            max,
            generation,
        } = job;
        let sender = self.sender.clone();
        let started = std::thread::Builder::new()
            .name("solium-read".to_owned())
            .spawn(move || {
                let result = read_capped(&path, max);
                let _ = sender.send(Finished::Read {
                    id,
                    generation,
                    result,
                });
            });
        match started {
            Ok(_) => self.reading += 1,
            Err(err) => {
                tracing::warn!(%err, "could not start a thread to read a file");
                if generation == self.generation {
                    self.heard.push(Heard::Read {
                        id,
                        result: Err(err.to_string()),
                    });
                }
            }
        }
    }

    /// A read ended: the next one waiting starts.
    fn read_ended(&mut self) {
        self.reading = self.reading.saturating_sub(1);
        while self.reading < READS_AT_ONCE
            && let Some(job) = self.reads_waiting.pop_front()
        {
            self.read_when_free(job);
        }
    }

    /// Queue a `sol.write`.
    pub(crate) fn write(&mut self, request: WriteRequest) {
        let by = By::Lua {
            id: request.id,
            generation: self.generation,
            report: request.report,
        };
        self.queue(Job {
            path: request.path,
            data: request.data,
            by,
        });
    }

    /// Start `job`, or wait it behind the write running on its path.
    fn queue(&mut self, job: Job) {
        if let Some(waiting) = self.writing.get_mut(&job.path) {
            waiting.push_back(Queued::Write(job));
            return;
        }
        self.writing.insert(job.path.clone(), VecDeque::new());
        self.start(job);
    }

    fn start(&mut self, job: Job) {
        let sender = self.sender.clone();
        let Job { path, data, by } = job;
        let failed = (path.clone(), by.clone());
        let started = std::thread::Builder::new()
            .name("solium-write".to_owned())
            .spawn(move || {
                // A store's directory is the compositor's to make; a
                // `sol.write`'s must already be there.
                let made = match (&by, path.parent()) {
                    (By::Store { .. }, Some(directory)) => fs::create_dir_all(directory),
                    _ => Ok(()),
                };
                let error = made
                    .and_then(|()| write_atomic(&path, &data))
                    .err()
                    .map(|err| err.to_string());
                let _ = sender.send(Finished::Wrote {
                    path,
                    by,
                    text: data,
                    error,
                });
            });
        match started {
            Ok(thread) => self.threads.push(thread),
            Err(err) => {
                tracing::warn!(%err, "could not start a thread to write a file");
                let (path, by) = failed;
                self.wrote(path, by, &[], Some(err.to_string()));
            }
        }
    }

    /// A write on `path` ended: tell whoever asked, let the reads waiting
    /// for it go, and start the next write waiting there.
    fn wrote(&mut self, path: PathBuf, by: By, text: &[u8], error: Option<String>) {
        match by {
            By::Lua {
                id,
                generation,
                report,
            } => {
                if let Some(err) = &error {
                    tracing::warn!(path = %path.display(), err, "sol.write failed");
                }
                if report && generation == self.generation {
                    self.heard.push(Heard::Wrote { id, error });
                }
            }
            By::Store { name } => {
                if let Some(err) = &error {
                    tracing::warn!(store = name, path = %path.display(), err, "a store could not be saved; trying again");
                    // Compared afresh next time, so it is tried again.
                    if self.stored.get(&name).map(String::as_bytes) == Some(text) {
                        self.stored.remove(&name);
                    }
                }
            }
        }
        loop {
            let next = self.writing.get_mut(&path).and_then(VecDeque::pop_front);
            match next {
                Some(Queued::Read(job)) => self.read_when_free(job),
                Some(Queued::Write(job)) => {
                    self.start(job);
                    break;
                }
                None => {
                    self.writing.remove(&path);
                    break;
                }
            }
        }
    }

    /// Follow the file at `path`, whose directory exists, for watch `id`.
    pub(crate) fn watch(&mut self, id: u64, path: &Path, shown: String) {
        let (Some(directory), Some(name)) = (path.parent(), path.file_name()) else {
            tracing::warn!(path = %path.display(), "sol.watch: nothing to watch there");
            return;
        };
        if self.inotify.is_none() {
            match inotify::Inotify::init() {
                Ok(inotify) => self.inotify = Some(inotify),
                Err(err) => {
                    tracing::warn!(%err, "cannot watch files: inotify refused");
                    return;
                }
            }
        }
        let Some(inotify) = &self.inotify else {
            return;
        };
        let directory_watch = match inotify.watches().add(directory, MASK) {
            Ok(watch) => watch,
            Err(err) => {
                tracing::warn!(directory = %directory.display(), %err, "sol.watch: cannot watch the directory");
                return;
            }
        };
        self.directories
            .entry(directory_watch.clone())
            .or_default()
            .push(id);
        self.watches.insert(
            id,
            Watch {
                directory: directory_watch,
                name: name.to_owned(),
                shown,
                removed: false,
            },
        );
    }

    /// End watch `id`, and its directory's watch with the last one there.
    pub(crate) fn cancel_watch(&mut self, id: u64) {
        let Some(watch) = self.watches.remove(&id) else {
            return;
        };
        let Some(ids) = self.directories.get_mut(&watch.directory) else {
            return;
        };
        ids.retain(|each| *each != id);
        if ids.is_empty() {
            self.directories.remove(&watch.directory);
            if let Some(inotify) = &self.inotify {
                let _ = inotify.watches().remove(watch.directory);
            }
        }
    }

    /// The text store `name` was just loaded with, so it is not saved again
    /// until it changes.
    pub(crate) fn loaded_store(&mut self, name: String, text: String) {
        self.stored.insert(name, text);
    }

    /// Save each of `stores` whose text differs from what it was last
    /// saved with.
    pub(crate) fn save<'a>(&mut self, stores: impl IntoIterator<Item = (&'a String, &'a Json)>) {
        for (name, value) in stores {
            let text = value.render();
            if self.stored.get(name) == Some(&text) {
                continue;
            }
            let Some(path) = store_path(name) else {
                tracing::warn!(
                    store = name,
                    "no XDG_STATE_HOME or HOME: the store is not saved"
                );
                self.stored.insert(name.clone(), text);
                continue;
            };
            let mut data = text.clone().into_bytes();
            data.push(b'\n');
            self.stored.insert(name.clone(), text);
            self.queue(Job {
                path,
                data,
                by: By::Store { name: name.clone() },
            });
        }
    }

    /// Whether it is time to compare the stores, at `now`; and if it is,
    /// the next time is `every` on (`files.store_interval`).
    fn save_due(&mut self, now: Duration, every: Duration) -> bool {
        if now < self.next_save {
            return false;
        }
        self.next_save = now.saturating_add(every);
        true
    }

    /// The old configuration's watches end, and nothing it asked to read or
    /// write is told to the new one: for a reload. Writes under way still
    /// land, in order.
    pub(crate) fn reload(&mut self) {
        self.generation += 1;
        self.watches.clear();
        self.directories.clear();
        // Closing the instance ends every watch it holds.
        self.inotify = None;
        self.heard.clear();
        self.reads_waiting.clear();
    }

    /// One thread's result.
    fn finished(&mut self, finished: Finished) {
        match finished {
            Finished::Read {
                id,
                generation,
                result,
            } => {
                if generation == self.generation {
                    self.heard.push(Heard::Read { id, result });
                }
                self.read_ended();
            }
            Finished::Wrote {
                path,
                by,
                text,
                error,
            } => self.wrote(path, by, &text, error),
        }
    }

    /// What reads, writes and watches came to since the last call, in the
    /// order heard.
    pub(crate) fn pump(&mut self) -> Vec<Heard> {
        while let Ok(finished) = self.receiver.try_recv() {
            self.finished(finished);
        }
        self.threads.retain(|thread| !thread.is_finished());
        self.watched();
        std::mem::take(&mut self.heard)
    }

    /// What inotify has buffered, for the watches it concerns.
    fn watched(&mut self) {
        let Some(inotify) = &mut self.inotify else {
            return;
        };
        let mut buffer = [0_u8; 4096];
        let mut seen = Vec::new();
        loop {
            let Ok(events) = inotify.read_events(&mut buffer) else {
                break;
            };
            let before = seen.len();
            seen.extend(
                events.map(|event| (event.wd.clone(), event.mask, event.name.map(OsString::from))),
            );
            if seen.len() == before {
                break;
            }
        }
        for (directory, mask, name) in seen {
            if mask.contains(inotify::EventMask::Q_OVERFLOW) {
                // What was lost cannot be known: every file may have
                // changed.
                tracing::warn!("too many file events at once; telling every watch it changed");
                let mut ids: Vec<u64> = self.watches.keys().copied().collect();
                ids.sort_unstable();
                for id in ids {
                    self.tell(id, Kind::Changed);
                }
                continue;
            }
            if mask.intersects(inotify::EventMask::DELETE_SELF | inotify::EventMask::IGNORED) {
                self.directory_gone(&directory);
                continue;
            }
            let Some(name) = name else {
                continue;
            };
            let kind = if mask.contains(inotify::EventMask::CREATE) {
                Kind::Created
            } else if mask.intersects(inotify::EventMask::DELETE | inotify::EventMask::MOVED_FROM) {
                Kind::Removed
            } else {
                Kind::Changed
            };
            let ids: Vec<u64> = self
                .directories
                .get(&directory)
                .map(|ids| {
                    ids.iter()
                        .copied()
                        .filter(|id| self.watches.get(id).is_some_and(|watch| watch.name == name))
                        .collect()
                })
                .unwrap_or_default();
            for id in ids {
                self.tell(id, kind);
            }
        }
    }

    /// Tell watch `id` its file was `kind`.
    fn tell(&mut self, id: u64, kind: Kind) {
        if let Some(watch) = self.watches.get_mut(&id) {
            watch.removed = kind == Kind::Removed;
            self.heard.push(Heard::Watched {
                id,
                kind,
                path: watch.shown.clone(),
            });
        }
    }

    /// A watched directory was removed or unmounted: each of its watches is
    /// told its file is `removed`, unless that was the last thing it was
    /// told, and ends. `tests::a_watch_ends_when_its_directory_goes`.
    fn directory_gone(&mut self, directory: &inotify::WatchDescriptor) {
        let Some(ids) = self.directories.remove(directory) else {
            return;
        };
        for id in ids {
            let Some(watch) = self.watches.remove(&id) else {
                continue;
            };
            tracing::info!(
                path = watch.shown,
                "a watched file's directory went away; its watch ends"
            );
            if !watch.removed {
                self.heard.push(Heard::Watched {
                    id,
                    kind: Kind::Removed,
                    path: watch.shown,
                });
            }
            self.heard.push(Heard::WatchEnded { id });
        }
    }

    /// Whether there is nothing to pick up.
    fn is_idle(&self) -> bool {
        self.writing.is_empty()
            && self.heard.is_empty()
            && self.inotify.is_none()
            && self.threads.is_empty()
            && self.reading == 0
    }

    /// Wait for the writes asked for to land, the ones waiting behind
    /// others too, for [`SHUTDOWN_WAIT`] at most: for the compositor's end,
    /// after the last save. A write still pending then is named in the log,
    /// by its path, and given up.
    /// `tests::the_end_waits_for_writes_but_not_for_ever`.
    pub(crate) fn wait_for_writes(&mut self) {
        self.wait_for_writes_within(SHUTDOWN_WAIT);
    }

    fn wait_for_writes_within(&mut self, limit: Duration) {
        let deadline = std::time::Instant::now() + limit;
        while !self.writing.is_empty() {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                break;
            }
            match self.receiver.recv_timeout(left) {
                Ok(finished) => self.finished(finished),
                Err(_) => break,
            }
        }
        for (path, waiting) in self.writing.drain() {
            let writes = 1 + waiting
                .iter()
                .filter(|queued| matches!(queued, Queued::Write(_)))
                .count();
            for _ in 0..writes {
                tracing::warn!(
                    path = %path.display(),
                    "a write had not landed when the compositor stopped; giving it up"
                );
            }
        }
        // Whatever is still running is left to finish or die with the
        // process.
        self.threads.clear();
    }

    /// How many watches are set, for a test.
    #[cfg(test)]
    pub(crate) fn watching(&self) -> usize {
        self.watches.len()
    }

    /// How many paths have a write running, for a test.
    #[cfg(test)]
    pub(crate) fn writing(&self) -> usize {
        self.writing.len()
    }
}

/// Once a loop: tell the configuration what its reads, writes and watches
/// came to, and every `files.store_interval` save the stores that changed.
pub(crate) fn settle(state: &mut Solium) {
    let now = state.clock.now();
    settle_at(state, now);
}

/// [`settle`], at `now` on the compositor's clock, which a test chooses.
pub(crate) fn settle_at(state: &mut Solium, now: Duration) {
    let every = state.files.store_interval;
    if state.scripts.is_some() && state.files.save_due(now, every) {
        save_stores(state);
    }
    if state.files.is_idle() {
        return;
    }
    let heard = state.files.pump();
    if heard.is_empty() {
        return;
    }
    let snapshot = state.snapshot();
    let Some(mut scripts) = state.scripts.take() else {
        return;
    };
    let outcome = scripts.files_heard(&heard, snapshot);
    state.scripts = Some(scripts);
    state.apply(outcome);
}

/// At the compositor's end: save the stores that changed, and wait a
/// bounded time for every write to land.
pub(crate) fn finish(state: &mut Solium) {
    save_stores(state);
    state.files.wait_for_writes();
}

/// Ask the configuration for the stores that changed, and save them.
pub(crate) fn save_stores(state: &mut Solium) {
    let snapshot = state.snapshot();
    let Some(mut scripts) = state.scripts.take() else {
        return;
    };
    let outcome = scripts.save_stores(snapshot);
    state.scripts = Some(scripts);
    state.apply(outcome);
}

#[cfg(test)]
pub(crate) mod tests {
    use std::{
        cell::RefCell,
        time::{Duration, Instant},
    };

    use super::*;

    thread_local! {
        /// The state directory a test chose for its own thread.
        static STATE_DIR: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    }

    /// Keep this test's stores in `dir`. Each test runs on a thread of its
    /// own, so this is its alone.
    pub(crate) fn use_state_dir(dir: &Path) {
        STATE_DIR.with(|chosen| *chosen.borrow_mut() = Some(dir.to_owned()));
    }

    /// The directory [`use_state_dir`] chose, or one in the temporary
    /// directory for this process: never the user's.
    pub(crate) fn state_dir_for_this_test() -> PathBuf {
        STATE_DIR
            .with(|chosen| chosen.borrow().clone())
            .unwrap_or_else(|| {
                std::env::temp_dir().join(format!("solium-test-state-{}", std::process::id()))
            })
    }

    /// A temporary directory of this test's own, emptied first.
    pub(crate) fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("solium-files-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a temporary directory");
        dir
    }

    /// Pump `files` until `done` says what was heard is enough.
    fn pump_until(files: &mut Files, mut done: impl FnMut(&[Heard]) -> bool) -> Vec<Heard> {
        let started = Instant::now();
        let mut heard = Vec::new();
        loop {
            heard.extend(files.pump());
            if done(&heard) {
                return heard;
            }
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "heard only {heard:?}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// **A path is absolute, or under `~/`**; anything else is refused.
    #[test]
    fn a_path_is_absolute_or_under_home() {
        assert_eq!(expand("/etc/hostname"), Ok(PathBuf::from("/etc/hostname")));
        if let Some(home) = std::env::var_os("HOME").filter(|home| !home.is_empty()) {
            assert_eq!(expand("~/a/b"), Ok(PathBuf::from(home).join("a/b")));
        }
        for refused in ["a/b", "./a", "~user/a", "", "~"] {
            assert!(expand(refused).is_err(), "{refused:?} was taken");
        }
    }

    /// **A `/proc` file is read to its end**, though its size says 0.
    #[test]
    fn a_proc_file_is_read_to_its_end() {
        let path = Path::new("/proc/self/status");
        assert_eq!(
            fs::metadata(path).map(|meta| meta.len()).ok(),
            Some(0),
            "the premise"
        );
        let data = read_capped(path, MAX_READ).expect("reading /proc/self/status");
        let text = String::from_utf8_lossy(&data);
        assert!(
            text.starts_with("Name:") && text.contains("\nPid:"),
            "{text}"
        );
        let mut files = Files::default();
        files.read(ReadRequest {
            id: 7,
            path: path.to_owned(),
            max: MAX_READ,
        });
        let heard = pump_until(&mut files, |heard| !heard.is_empty());
        assert!(
            matches!(&heard[..], [Heard::Read { id: 7, result: Ok(data) }] if data.starts_with(b"Name:")),
            "{heard:?}"
        );
    }

    /// **A read stops at its cap**, which a file exactly that long is not
    /// past; and a missing file says why, as the system words it.
    #[test]
    fn a_read_stops_at_its_cap() {
        let dir = scratch("cap");
        let path = dir.join("ten");
        fs::write(&path, "0123456789").expect("writing");
        assert_eq!(read_capped(&path, 10), Ok(b"0123456789".to_vec()));
        assert_eq!(read_capped(&path, 9), Err(TOO_LONG.to_owned()));
        let missing = read_capped(&dir.join("missing"), 10).expect_err("a missing file");
        assert!(missing.contains("No such file"), "{missing}");
        let _ = fs::remove_dir_all(dir);
    }

    /// **An interrupted write leaves the old file whole** (#473): a write
    /// that died after its temporary file was half written, and before the
    /// rename, leaves the target as it was; the next write replaces both.
    #[test]
    fn an_interrupted_write_leaves_the_old_file_whole() {
        let dir = scratch("interrupted");
        let path = dir.join("settings.json");
        write_atomic(&path, b"{\"old\":true}").expect("the first write");
        // The crash: the temporary file half written, never renamed.
        fs::write(temporary(&path), b"{\"new\":").expect("the half write");
        assert_eq!(
            fs::read(&path).ok().as_deref(),
            Some(&b"{\"old\":true}"[..])
        );
        write_atomic(&path, b"{\"new\":true}").expect("the next write");
        assert_eq!(
            fs::read(&path).ok().as_deref(),
            Some(&b"{\"new\":true}"[..])
        );
        assert!(!temporary(&path).exists(), "the temporary file was left");
        let names: Vec<_> = fs::read_dir(&dir)
            .expect("listing")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name())
            .collect();
        assert_eq!(names, ["settings.json"]);
        let _ = fs::remove_dir_all(dir);
    }

    /// **A write keeps the file's permissions, follows a link, and needs its
    /// directory**: it is not made.
    #[test]
    fn a_write_keeps_permissions_follows_a_link_and_needs_its_directory() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = scratch("write-details");
        let real = dir.join("real");
        fs::write(&real, "a").expect("writing");
        fs::set_permissions(&real, fs::Permissions::from_mode(0o600)).expect("chmod");
        let link = dir.join("link");
        std::os::unix::fs::symlink(&real, &link).expect("linking");
        write_atomic(&link, b"b").expect("writing through the link");
        assert!(fs::symlink_metadata(&link).is_ok_and(|meta| meta.file_type().is_symlink()));
        assert_eq!(fs::read(&real).ok().as_deref(), Some(&b"b"[..]));
        assert_eq!(
            fs::metadata(&real)
                .map(|meta| meta.permissions().mode() & 0o777)
                .ok(),
            Some(0o600)
        );
        let err = write_atomic(&dir.join("no/such/dir/file"), b"c").expect_err("no directory");
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        assert!(!dir.join("no").exists());
        let _ = fs::remove_dir_all(dir);
    }

    /// **Writes to one path land in the order they were asked for**, each
    /// told, and only one runs there at a time.
    #[test]
    fn writes_to_one_path_land_in_order() {
        let dir = scratch("order");
        let path = dir.join("log");
        let mut files = Files::default();
        for id in 1..=20_u64 {
            files.write(WriteRequest {
                id,
                path: path.clone(),
                data: id.to_string().into_bytes(),
                report: true,
            });
        }
        assert_eq!(files.writing(), 1, "one path, one write at a time");
        let heard = pump_until(&mut files, |heard| heard.len() == 20);
        let told: Vec<u64> = heard
            .iter()
            .map(|each| match each {
                Heard::Wrote { id, error: None } => *id,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(told, (1..=20).collect::<Vec<_>>());
        assert_eq!(fs::read_to_string(&path).ok().as_deref(), Some("20"));
        assert_eq!(files.writing(), 0);
        let _ = fs::remove_dir_all(dir);
    }

    /// **A file replaced by a rename is still watched** (#473): the watch is
    /// on its directory, so a write, a rename over it, a removal and a new
    /// file under its name are each told, and other names are not.
    #[test]
    fn a_file_replaced_by_a_rename_is_still_watched() {
        let dir = scratch("watch");
        let path = dir.join("watched");
        let mut files = Files::default();
        files.watch(1, &path, "watched-as-given".to_owned());
        let kinds = |heard: &[Heard]| -> Vec<Kind> {
            heard
                .iter()
                .map(|each| match each {
                    Heard::Watched { id: 1, kind, path } if path == "watched-as-given" => *kind,
                    other => panic!("{other:?}"),
                })
                .collect()
        };
        // Not there yet: its creation is seen.
        fs::write(&path, "1").expect("writing");
        let heard = pump_until(&mut files, |heard| heard.len() >= 2);
        assert_eq!(kinds(&heard), [Kind::Created, Kind::Changed]);
        // Replaced by a rename, as `sol.write` and editors replace a file.
        write_atomic(&path, b"2").expect("replacing");
        let heard = pump_until(&mut files, |heard| !heard.is_empty());
        assert_eq!(kinds(&heard), [Kind::Changed]);
        // And still followed after it.
        fs::write(&path, "3").expect("writing again");
        let heard = pump_until(&mut files, |heard| !heard.is_empty());
        assert_eq!(kinds(&heard), [Kind::Changed]);
        fs::write(dir.join("another"), "x").expect("writing another");
        fs::remove_file(&path).expect("removing");
        let heard = pump_until(&mut files, |heard| !heard.is_empty());
        assert_eq!(kinds(&heard), [Kind::Removed]);
        files.cancel_watch(1);
        assert_eq!(files.watching(), 0);
        fs::write(&path, "4").expect("writing after the cancel");
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(files.pump(), []);
        let _ = fs::remove_dir_all(dir);
    }

    /// **A reload ends the watches and silences the old reads**, and a
    /// write under way still lands.
    #[test]
    fn a_reload_ends_the_watches_and_silences_the_old_reads() {
        let dir = scratch("reload");
        let path = dir.join("watched");
        let mut files = Files::default();
        files.watch(1, &path, "w".to_owned());
        files.read(ReadRequest {
            id: 2,
            path: PathBuf::from("/proc/self/status"),
            max: MAX_READ,
        });
        files.write(WriteRequest {
            id: 3,
            path: path.clone(),
            data: b"landed".to_vec(),
            report: true,
        });
        files.reload();
        assert_eq!(files.watching(), 0);
        let started = Instant::now();
        while files.writing() > 0 {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "the write never ended"
            );
            assert_eq!(files.pump(), [], "the old configuration was told");
            std::thread::sleep(Duration::from_millis(5));
        }
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(files.pump(), []);
        assert_eq!(fs::read_to_string(&path).ok().as_deref(), Some("landed"));
        let _ = fs::remove_dir_all(dir);
    }

    /// **A broken store file is kept as `.broken`** (#473), never written
    /// over: not JSON, not an object, or past the cap.
    #[test]
    fn a_broken_store_file_is_kept_as_broken() {
        let dir = scratch("broken-store");
        use_state_dir(&dir);
        let big = format!(
            "{{\"a\":\"{}\"}}",
            "x".repeat(usize::try_from(MAX_STORE).unwrap_or(0))
        );
        for (name, text) in [
            ("prefs", "{ not json"),
            ("number", "5"),
            ("big", big.as_str()),
        ] {
            fs::write(dir.join(format!("{name}.json")), text).expect("writing");
            assert!(
                matches!(read_store(name, MAX_STORE), StoreFile::Broken { .. }),
                "{name}"
            );
            assert!(
                dir.join(format!("{name}.json")).exists(),
                "reading changed {name}"
            );
            set_aside(name, &dir.join(format!("{name}.json")));
            assert!(
                !dir.join(format!("{name}.json")).exists(),
                "{name} was left in place"
            );
            assert_eq!(
                fs::read_to_string(dir.join(format!("{name}.json.broken")))
                    .ok()
                    .as_deref(),
                Some(text),
                "{name}"
            );
        }
        fs::write(dir.join("good.json"), r#"{"volume": 60}"#).expect("writing");
        let StoreFile::Found(good) = read_store("good", MAX_STORE) else {
            panic!("a good store")
        };
        assert_eq!(good.get("volume"), Some(&Json::Number(60.0)));
        assert!(
            matches!(read_store("good", 5), StoreFile::Broken { .. }),
            "files.store_max"
        );
        assert_eq!(read_store("missing", MAX_STORE), StoreFile::Missing);
        let _ = fs::remove_dir_all(dir);
    }

    /// **A read after a write sees what was written** (#473): a read of a
    /// path waits for the writes asked for before it there, and a later
    /// write does not wait for the read.
    #[test]
    fn a_read_after_a_write_sees_what_was_written() {
        let dir = scratch("read-after-write");
        let path = dir.join("f");
        fs::write(&path, "old").expect("writing");
        let mut files = Files::default();
        for round in 0..20_u64 {
            let text = format!("new {round}");
            files.write(WriteRequest {
                id: round * 2,
                path: path.clone(),
                data: text.clone().into_bytes(),
                report: false,
            });
            files.read(ReadRequest {
                id: round * 2 + 1,
                path: path.clone(),
                max: MAX_READ,
            });
            let heard = pump_until(&mut files, |heard| !heard.is_empty());
            assert_eq!(
                heard,
                [Heard::Read {
                    id: round * 2 + 1,
                    result: Ok(text.into_bytes())
                }]
            );
        }
        let _ = fs::remove_dir_all(dir);
    }

    /// **At most four reads run at once**, and the rest are started in the
    /// order they were asked for as those end.
    #[test]
    fn at_most_four_reads_run_at_once() {
        let mut files = Files::default();
        for id in 1..=10 {
            files.read(ReadRequest {
                id,
                path: PathBuf::from("/proc/self/status"),
                max: MAX_READ,
            });
        }
        assert_eq!(
            (files.reading, files.reads_waiting.len()),
            (READS_AT_ONCE, 6)
        );
        let heard = pump_until(&mut files, |heard| heard.len() == 10);
        let mut ids: Vec<u64> = heard
            .iter()
            .map(|each| match each {
                Heard::Read { id, result: Ok(_) } => *id,
                other => panic!("{other:?}"),
            })
            .collect();
        ids.sort_unstable();
        assert_eq!(ids, (1..=10).collect::<Vec<_>>());
        assert_eq!((files.reading, files.reads_waiting.len()), (0, 0));
    }

    /// **A watch ends when its directory goes** (#473): told `removed` once,
    /// even when its file was removed with the directory, and then ended.
    #[test]
    fn a_watch_ends_when_its_directory_goes() {
        let dir = scratch("directory-gone");
        let mut files = Files::default();
        // One file that is there, and one that never was.
        fs::write(dir.join("here"), "x").expect("writing");
        files.watch(1, &dir.join("here"), "here".to_owned());
        files.watch(2, &dir.join("never"), "never".to_owned());
        fs::remove_dir_all(&dir).expect("removing the directory");
        let heard = pump_until(&mut files, |heard| {
            heard
                .iter()
                .filter(|each| matches!(each, Heard::WatchEnded { .. }))
                .count()
                == 2
        });
        let told = |id: u64| -> Vec<&Heard> {
            heard
                .iter()
                .filter(|each| {
                    matches!(each, Heard::Watched { id: each, .. } | Heard::WatchEnded { id: each } if *each == id)
                })
                .collect()
        };
        for id in [1, 2] {
            let path = if id == 1 { "here" } else { "never" };
            assert_eq!(
                told(id),
                [
                    &Heard::Watched {
                        id,
                        kind: Kind::Removed,
                        path: path.to_owned()
                    },
                    &Heard::WatchEnded { id }
                ]
            );
        }
        assert_eq!(files.watching(), 0);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(files.pump(), []);
    }

    /// **The end waits for writes, but not for ever**: one that lands in
    /// time is waited for, and past the bound the rest are given up.
    #[test]
    fn the_end_waits_for_writes_but_not_for_ever() {
        let dir = scratch("end");
        let mut files = Files::default();
        files.write(WriteRequest {
            id: 1,
            path: dir.join("landed"),
            data: b"yes".to_vec(),
            report: false,
        });
        files.wait_for_writes_within(Duration::from_secs(10));
        assert_eq!(
            fs::read_to_string(dir.join("landed")).ok().as_deref(),
            Some("yes")
        );
        // Its temporary file a FIFO with no reader, so the write's open
        // waits for ever.
        let held = dir.join("held");
        let made = std::process::Command::new("mkfifo")
            .arg(temporary(&held))
            .status();
        assert!(made.is_ok_and(|status| status.success()), "mkfifo");
        let mut files = Files::default();
        files.write(WriteRequest {
            id: 2,
            path: held.clone(),
            data: b"x".to_vec(),
            report: false,
        });
        let started = Instant::now();
        files.wait_for_writes_within(Duration::from_millis(200));
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "waited past the bound"
        );
        assert_eq!(files.writing(), 0, "a write that never landed was kept");
        // Let the stuck thread go.
        let _ = fs::OpenOptions::new().read(true).open(temporary(&held));
        let _ = fs::remove_dir_all(dir);
    }

    /// **Only plain data is stored**: a function, userdata and a key that is
    /// neither a string nor a list's are left out, each named once.
    #[test]
    fn only_plain_data_is_stored() {
        let lua = mlua::Lua::new();
        let value: mlua::Value = lua
            .load(
                r#"return { n = 1, f = 0.5, s = "x", b = true, list = { "a", print, "c" },
                           map = { deep = { k = 2 } }, fn = print, [3] = "odd", empty = {} }"#,
            )
            .eval()
            .expect("a table");
        let mut warned = HashSet::new();
        let mut left = 65_536;
        let json = plain(&value, "s", 0, &mut left, &mut warned)
            .expect("in budget")
            .expect("a value");
        assert_eq!(
            json.render(),
            r#"{"b":true,"empty":{},"f":0.5,"list":["a",null,"c"],"map":{"deep":{"k":2}},"n":1,"s":"x"}"#
        );
        let mut named: Vec<&str> = warned.iter().map(String::as_str).collect();
        named.sort_unstable();
        assert_eq!(named, ["s.3", "s.fn", "s.list.2"]);
        let mut left = 3;
        assert!(
            plain(&value, "s", 0, &mut left, &mut warned).is_err(),
            "the budget"
        );
    }
}
