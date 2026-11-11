// Data Exporter: saves what happens in the game to files on the player's computer.
// This file is the mod's Cargo library (its tests) and the module the bundle adds to the
// game. It knows nothing of the game: `glue.rs` turns the game's hooks into `emit` calls.

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const ID: &str = "data_exporter";
pub const NAME: &str = "Data Exporter";
pub const VERSION: &str = "0.1.1";
pub const DESCRIPTION: &str = "Saves what happens in your game and your statistics to files on your computer, ready to analyze. Nothing is sent over the network.";

/// The profile's file name in the game's `config/` folder.
pub const PROFILE_FILE: &str = "data_exporter.json";

const SCHEMA: i64 = 1;
/// Records waiting for the writer thread. Past this the game drops records, never waits.
const QUEUE: usize = 65_536;
const FLUSH_EVERY: Duration = Duration::from_secs(1);
const CHECK_PROFILE_EVERY: Duration = Duration::from_secs(1);
/// While the disk limit is reached, how often the folder is measured again, so that saving
/// resumes once the player frees space.
const RECHECK_FULL_EVERY: Duration = Duration::from_secs(5);

/// One kind of data: its id (in the profile and in the folders), and its group of buttons.
pub struct Kind {
    pub id: &'static str,
    pub stats: bool,
    pub group: usize,
}

pub const GROUPS: [&str; 4] = [
    "Session & player",
    "Blocks & combat",
    "Items & progress",
    "Statistics",
];

const fn kind(id: &'static str, group: usize, stats: bool) -> Kind {
    Kind { id, stats, group }
}

pub const KINDS: [Kind; 11] = [
    kind("session", 0, false),
    kind("dimension_change", 0, false),
    kind("player_state", 0, false),
    kind("block_broken", 1, false),
    kind("block_placed", 1, false),
    kind("player_deaths", 1, false),
    kind("mobs_killed", 1, false),
    kind("crafting", 2, false),
    kind("pickups", 2, false),
    kind("advancements", 2, false),
    kind("stat_awards", 3, true),
];

fn kinds() -> impl Iterator<Item = &'static Kind> {
    KINDS.iter()
}

fn find_kind(id: &str) -> Option<&'static Kind> {
    kinds().find(|k| k.id == id)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Format {
    Jsonl,
    Csv,
}

impl Format {
    fn parse(text: &str) -> Option<Self> {
        match text {
            "jsonl" => Some(Self::Jsonl),
            "csv" => Some(Self::Csv),
            _ => None,
        }
    }
    fn id(self) -> &'static str {
        match self {
            Self::Jsonl => "jsonl",
            Self::Csv => "csv",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Field {
    Text(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Null,
}

impl Field {
    fn json(&self) -> String {
        match self {
            Self::Text(text) => serde_json::Value::from(text.as_str()).to_string(),
            Self::Int(value) => value.to_string(),
            Self::Float(value) if value.is_finite() => value.to_string(),
            Self::Bool(value) => value.to_string(),
            Self::Float(_) | Self::Null => "null".to_owned(),
        }
    }
    fn csv(&self) -> String {
        match self {
            Self::Text(text) => csv_escape(text),
            Self::Int(value) => value.to_string(),
            Self::Float(value) if value.is_finite() => value.to_string(),
            Self::Bool(value) => value.to_string(),
            Self::Float(_) | Self::Null => String::new(),
        }
    }
}

fn csv_escape(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_owned()
    }
}

pub type Fields = Vec<(&'static str, Field)>;

fn json_line(fields: &[(&'static str, Field)]) -> String {
    let body: Vec<String> = fields
        .iter()
        .map(|(name, value)| format!("\"{name}\":{}", value.json()))
        .collect();
    format!("{{{}}}\n", body.join(","))
}

fn csv_header(fields: &[(&'static str, Field)]) -> String {
    let names: Vec<&str> = fields.iter().map(|(name, _)| *name).collect();
    format!("{}\n", names.join(","))
}

fn csv_line(fields: &[(&'static str, Field)]) -> String {
    let cells: Vec<String> = fields.iter().map(|(_, value)| value.csv()).collect();
    format!("{}\n", cells.join(","))
}

/// Where one kind of data goes instead of the profile's folder and format.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Output {
    pub folder: Option<String>,
    pub format: Option<Format>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub profile: String,
    pub folder: String,
    pub format: Format,
    pub capture: BTreeMap<String, bool>,
    pub outputs: BTreeMap<String, Output>,
    pub include_other_players: bool,
    pub player_state_seconds: u64,
    pub max_file_mb: u64,
    pub max_total_mb: u64,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            profile: "default".to_owned(),
            folder: "data-exporter".to_owned(),
            format: Format::Jsonl,
            capture: kinds().map(|k| (k.id.to_owned(), true)).collect(),
            outputs: BTreeMap::new(),
            include_other_players: false,
            player_state_seconds: 5,
            max_file_mb: 64,
            max_total_mb: 1024,
        }
    }
}

fn positive(root: &serde_json::Value, key: &str, default: u64) -> Result<u64, String> {
    match root.get(key) {
        None => Ok(default),
        Some(value) => match value.as_u64() {
            Some(number) if number >= 1 => Ok(number),
            _ => Err(format!("\"{key}\" must be a whole number of at least 1")),
        },
    }
}

impl Profile {
    pub fn parse(text: &str) -> Result<Self, String> {
        let root: serde_json::Value =
            serde_json::from_str(text).map_err(|e| format!("not valid JSON: {e}"))?;
        if !root.is_object() {
            return Err("the profile must be a JSON object".to_owned());
        }
        let mut profile = Self::default();
        if let Some(value) = root.get("profile") {
            profile.profile = value.as_str().ok_or("\"profile\" must be text")?.to_owned();
        }
        if let Some(value) = root.get("folder") {
            let folder = value.as_str().ok_or("\"folder\" must be text")?;
            if folder.trim().is_empty() {
                return Err("\"folder\" can't be empty".to_owned());
            }
            profile.folder = folder.to_owned();
        }
        if let Some(value) = root.get("format") {
            profile.format = parse_format(value)?;
        }
        if let Some(value) = root.get("include_other_players") {
            profile.include_other_players = value
                .as_bool()
                .ok_or("\"include_other_players\" must be true or false")?;
        }
        profile.player_state_seconds =
            positive(&root, "player_state_seconds", profile.player_state_seconds)?;
        profile.max_file_mb = positive(&root, "max_file_mb", profile.max_file_mb)?;
        profile.max_total_mb = positive(&root, "max_total_mb", profile.max_total_mb)?;
        if let Some(value) = root.get("capture") {
            let map = value.as_object().ok_or("\"capture\" must be an object")?;
            for (id, on) in map {
                if find_kind(id).is_none() {
                    return Err(format!("\"capture\" has an unknown kind of data: \"{id}\""));
                }
                let on = on
                    .as_bool()
                    .ok_or_else(|| format!("\"capture\".\"{id}\" must be true or false"))?;
                profile.capture.insert(id.clone(), on);
            }
        }
        if let Some(value) = root.get("outputs") {
            let map = value.as_object().ok_or("\"outputs\" must be an object")?;
            for (id, output) in map {
                if find_kind(id).is_none() {
                    return Err(format!("\"outputs\" has an unknown kind of data: \"{id}\""));
                }
                let output = output
                    .as_object()
                    .ok_or_else(|| format!("\"outputs\".\"{id}\" must be an object"))?;
                let folder = match output.get("folder") {
                    None => None,
                    Some(folder) => Some(
                        folder
                            .as_str()
                            .ok_or_else(|| format!("\"outputs\".\"{id}\".folder must be text"))?
                            .to_owned(),
                    ),
                };
                let format = output.get("format").map(parse_format).transpose()?;
                profile
                    .outputs
                    .insert(id.clone(), Output { folder, format });
            }
        }
        Ok(profile)
    }

    /// The profile as its file, in a fixed, readable order.
    pub fn to_json(&self) -> String {
        let text = |value: &str| serde_json::Value::from(value).to_string();
        let capture: Vec<String> = kinds()
            .map(|k| format!("    \"{}\": {}", k.id, self.captures(k.id)))
            .collect();
        let outputs: Vec<String> = self
            .outputs
            .iter()
            .map(|(id, output)| {
                let mut parts = Vec::new();
                if let Some(folder) = &output.folder {
                    parts.push(format!("\"folder\": {}", text(folder)));
                }
                if let Some(format) = output.format {
                    parts.push(format!("\"format\": \"{}\"", format.id()));
                }
                format!("    \"{id}\": {{ {} }}", parts.join(", "))
            })
            .collect();
        let outputs = if outputs.is_empty() {
            "{}".to_owned()
        } else {
            format!("{{\n{}\n  }}", outputs.join(",\n"))
        };
        format!(
            "{{\n  \"profile\": {},\n  \"folder\": {},\n  \"format\": \"{}\",\n  \"capture\": {{\n{}\n  }},\n  \"outputs\": {},\n  \"include_other_players\": {},\n  \"player_state_seconds\": {},\n  \"max_file_mb\": {},\n  \"max_total_mb\": {}\n}}\n",
            text(&self.profile),
            text(&self.folder),
            self.format.id(),
            capture.join(",\n"),
            outputs,
            self.include_other_players,
            self.player_state_seconds,
            self.max_file_mb,
            self.max_total_mb,
        )
    }

    pub fn captures(&self, id: &str) -> bool {
        self.capture.get(id).copied().unwrap_or(false)
    }

    /// Whether every kind of the group is on (`Some(true)`), off (`Some(false)`), or mixed.
    pub fn group(&self, group: usize) -> Option<bool> {
        let mut states = kinds()
            .filter(|k| k.group == group)
            .map(|k| self.captures(k.id));
        let first = states.next()?;
        states.all(|state| state == first).then_some(first)
    }

    /// Turns the whole group on, unless it is all on already.
    pub fn flip_group(&mut self, group: usize) {
        let on = self.group(group) != Some(true);
        for kind in kinds().filter(|k| k.group == group) {
            self.capture.insert(kind.id.to_owned(), on);
        }
    }

    /// The folder and format of one kind of data.
    fn destination(&self, kind: &Kind, game_dir: &Path) -> (PathBuf, Format) {
        let output = self.outputs.get(kind.id);
        let format = output.and_then(|o| o.format).unwrap_or(self.format);
        let folder = match output.and_then(|o| o.folder.as_deref()) {
            Some(folder) => resolve(folder, game_dir),
            None => resolve(&self.folder, game_dir)
                .join(if kind.stats { "stats" } else { "events" })
                .join(kind.id),
        };
        (folder, format)
    }
}

fn parse_format(value: &serde_json::Value) -> Result<Format, String> {
    value
        .as_str()
        .and_then(Format::parse)
        .ok_or_else(|| "a format must be \"jsonl\" or \"csv\"".to_owned())
}

/// `~` is the home folder; a relative path is inside the game's folder.
fn resolve(folder: &str, game_dir: &Path) -> PathBuf {
    let home = || {
        std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
    };
    if folder == "~" {
        return home().unwrap_or_else(|| game_dir.to_path_buf());
    }
    if let Some(rest) = folder
        .strip_prefix("~/")
        .or_else(|| folder.strip_prefix("~\\"))
        && let Some(home) = home()
    {
        return home.join(rest);
    }
    let path = PathBuf::from(folder);
    if path.is_absolute() {
        path
    } else {
        game_dir.join(path)
    }
}

/// Counters the game adds to on every tick: their rows would say nothing the clock doesn't.
pub fn is_tick_counter(stat_type: &str, key: &str) -> bool {
    stat_type == "custom"
        && matches!(
            key.strip_prefix("minecraft:").unwrap_or(key),
            "play_time" | "total_world_time" | "time_since_death" | "time_since_rest"
        )
}

/// A statistic's type with its namespace: the game passes `used`, the stat files say `minecraft:used`.
pub fn namespaced(id: &str) -> String {
    if id.contains(':') {
        id.to_owned()
    } else {
        format!("minecraft:{id}")
    }
}

/// The canonical text form of a UUID.
pub fn uuid_text(uuid: u128) -> String {
    let hex = format!("{uuid:032x}");
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

/// Year, month and day of a day count since 1970 (Howard Hinnant's `civil_from_days`).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

fn parts(ms: i64) -> (i64, u32, u32, i64, i64, i64, i64) {
    let (year, month, day) = civil(ms.div_euclid(86_400_000));
    let rest = ms.rem_euclid(86_400_000);
    (
        year,
        month,
        day,
        rest / 3_600_000,
        rest / 60_000 % 60,
        rest / 1000 % 60,
        rest % 1000,
    )
}

/// `2026-11-09T18:32:05.123Z`.
pub fn iso(ms: i64) -> String {
    let (y, mo, d, h, mi, s, milli) = parts(ms);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{milli:03}Z")
}

fn file_stamp(ms: i64) -> String {
    let (y, mo, d, h, mi, s, _) = parts(ms);
    format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}")
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |t| t.as_millis() as i64)
}

/// A world's name as a piece of a file name, on every system.
fn safe_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let cleaned = cleaned.trim().trim_end_matches('.').to_owned();
    if cleaned.is_empty() {
        "world".to_owned()
    } else {
        cleaned
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct FileKey {
    dir: PathBuf,
    event: &'static str,
    world: String,
    format: Format,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Limits {
    max_file: u64,
    max_total: u64,
}

struct Open {
    file: BufWriter<File>,
    bytes: u64,
    part: u32,
    name: String,
}

/// The files being written. Lives on the writer thread, so the game never waits for the disk.
struct Writer {
    files: HashMap<FileKey, Open>,
    root: Option<PathBuf>,
    used: u64,
    /// The disk limit was reached: records are dropped until a new measure of the folder, or a
    /// higher limit, makes room again.
    full: bool,
    /// The limit `full` was decided against: a raised one is measured again at once.
    full_limit: u64,
    /// When the folder was last measured while `full`.
    measured: Option<Instant>,
    recheck_every: Duration,
}

/// Bytes under `dir`, counting only the files this mod writes.
fn folder_size(dir: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let mut total = 0;
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            total += folder_size(&entry.path());
        } else if entry
            .path()
            .extension()
            .is_some_and(|e| e == "jsonl" || e == "csv")
        {
            total += metadata.len();
        }
    }
    total
}

/// What became of a record.
#[derive(Debug, PartialEq, Eq)]
enum Wrote {
    Saved,
    /// The disk limit: nothing is saved until the player makes room or raises the limit.
    Full,
}

impl Writer {
    fn new() -> Self {
        Self {
            files: HashMap::new(),
            root: None,
            used: 0,
            full: false,
            full_limit: 0,
            measured: None,
            recheck_every: RECHECK_FULL_EVERY,
        }
    }

    /// While full: measures the folder again when the limit changed or a while has passed, and
    /// lifts `full` when the folder has room under the limit again.
    fn recheck(&mut self, root: &Path, max_total: u64) {
        let due = max_total != self.full_limit
            || self
                .measured
                .is_none_or(|at| at.elapsed() >= self.recheck_every);
        if !due {
            return;
        }
        // What waits in the buffers counts too.
        self.flush();
        self.used = folder_size(root);
        self.measured = Some(Instant::now());
        self.full_limit = max_total;
        self.full = self.used >= max_total;
    }

    fn open(key: &FileKey, part: u32, ms: i64) -> io::Result<Open> {
        fs::create_dir_all(&key.dir)?;
        let suffix = if part == 0 {
            String::new()
        } else {
            format!("_part{part}")
        };
        let stem = format!("{}_{}{suffix}", safe_name(&key.world), file_stamp(ms));
        // Two sessions in one second must not write into one file.
        for attempt in 0..1000 {
            let unique = if attempt == 0 {
                String::new()
            } else {
                format!("_{attempt}")
            };
            let name = format!("{stem}{unique}.{}", key.format.id());
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(key.dir.join(&name))
            {
                Ok(file) => {
                    return Ok(Open {
                        file: BufWriter::new(file),
                        bytes: 0,
                        part,
                        name,
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e),
            }
        }
        Err(io::ErrorKind::AlreadyExists.into())
    }

    fn write(
        &mut self,
        root: &Path,
        key: FileKey,
        fields: &[(&'static str, Field)],
        limits: Limits,
        ms: i64,
    ) -> io::Result<Wrote> {
        if self.root.as_deref() != Some(root) {
            self.root = Some(root.to_path_buf());
            self.used = folder_size(root);
            self.full = false;
        }
        if self.full {
            self.recheck(root, limits.max_total);
        }
        let line = match key.format {
            Format::Jsonl => json_line(fields),
            Format::Csv => csv_line(fields),
        };
        let header = (key.format == Format::Csv).then(|| csv_header(fields));
        let new_file = !self.files.contains_key(&key);
        let extra = header.as_ref().filter(|_| new_file).map_or(0, String::len) as u64;
        if self.full || self.used + line.len() as u64 + extra > limits.max_total {
            if !self.full {
                self.full = true;
                self.full_limit = limits.max_total;
                self.measured = Some(Instant::now());
            }
            return Ok(Wrote::Full);
        }
        if let Some(open) = self.files.get(&key)
            && open.bytes > 0
            && open.bytes + line.len() as u64 > limits.max_file
        {
            let part = open.part + 1;
            if let Some(mut old) = self.files.remove(&key) {
                old.file.flush()?;
            }
            let opened = Self::open(&key, part, ms)?;
            self.files.insert(key.clone(), opened);
        }
        if !self.files.contains_key(&key) {
            let opened = Self::open(&key, 0, ms)?;
            self.files.insert(key.clone(), opened);
        }
        let open = self.files.get_mut(&key).expect("just opened");
        if open.bytes == 0
            && let Some(header) = &header
        {
            open.file.write_all(header.as_bytes())?;
            open.bytes += header.len() as u64;
            self.used += header.len() as u64;
        }
        open.file.write_all(line.as_bytes())?;
        open.bytes += line.len() as u64;
        self.used += line.len() as u64;
        Ok(Wrote::Saved)
    }

    fn flush(&mut self) {
        for open in self.files.values_mut() {
            if let Err(e) = open.file.flush() {
                eprintln!("[data_exporter] can't write {}: {e}", open.name);
            }
        }
    }
}

enum Message {
    Record {
        root: PathBuf,
        key: FileKey,
        fields: Fields,
        limits: Limits,
        ms: i64,
    },
    Flush,
}

struct Shared {
    path: PathBuf,
    game_dir: PathBuf,
    profile: RwLock<Arc<Profile>>,
    modified: Mutex<Option<SystemTime>>,
    checked: Mutex<Option<Instant>>,
    error: Mutex<Option<String>>,
    sender: SyncSender<Message>,
    saved: Arc<AtomicU64>,
    dropped: Arc<AtomicU64>,
    full: Arc<AtomicBool>,
    samples: Mutex<HashMap<u128, Instant>>,
}

static SHARED: OnceLock<Shared> = OnceLock::new();
/// The world owner, kept from when the game could say: it can't during a player's packet.
static OWNER: Mutex<Option<u128>> = Mutex::new(None);

pub fn remember_owner(uuid: u128) {
    *OWNER.lock().unwrap_or_else(|e| e.into_inner()) = Some(uuid);
}

pub fn is_owner(uuid: u128) -> bool {
    *OWNER.lock().unwrap_or_else(|e| e.into_inner()) == Some(uuid)
}

fn run_writer(
    receiver: Receiver<Message>,
    saved: Arc<AtomicU64>,
    dropped: Arc<AtomicU64>,
    full: Arc<AtomicBool>,
) {
    let mut writer = Writer::new();
    let mut failed_once = false;
    loop {
        match receiver.recv_timeout(FLUSH_EVERY) {
            Ok(Message::Record {
                root,
                key,
                fields,
                limits,
                ms,
            }) => match writer.write(&root, key, &fields, limits, ms) {
                Ok(Wrote::Saved) => {
                    saved.fetch_add(1, Ordering::Relaxed);
                    if full.swap(false, Ordering::Relaxed) {
                        eprintln!(
                            "[data_exporter] room again under the disk limit: saving resumed"
                        );
                    }
                }
                Ok(Wrote::Full) => {
                    if !full.swap(true, Ordering::Relaxed) {
                        eprintln!(
                            "[data_exporter] disk limit reached: nothing is saved until there is room again"
                        );
                    }
                    dropped.fetch_add(1, Ordering::Relaxed);
                }
                Err(_) => {
                    if !failed_once {
                        failed_once = true;
                        eprintln!("[data_exporter] can't write to {}", root.display());
                    }
                    dropped.fetch_add(1, Ordering::Relaxed);
                }
            },
            Ok(Message::Flush) => writer.flush(),
            Err(RecvTimeoutError::Timeout) => writer.flush(),
            Err(RecvTimeoutError::Disconnected) => {
                writer.flush();
                return;
            }
        }
    }
}

fn modified_at(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Writes through a temporary file, so a crash never leaves half a profile.
fn save_text(path: &Path, text: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, text)?;
    fs::rename(&temporary, path)
}

fn load_or_create(path: &Path) -> (Profile, Option<String>) {
    match fs::read_to_string(path) {
        Ok(text) => match Profile::parse(&text) {
            Ok(profile) => (profile, None),
            // The player's file stays as it is, for them to fix.
            Err(error) => (Profile::default(), Some(error)),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let profile = Profile::default();
            let error = save_text(path, &profile.to_json())
                .err()
                .map(|e| format!("can't create the profile: {e}"));
            (profile, error)
        }
        Err(e) => (
            Profile::default(),
            Some(format!("can't read the profile: {e}")),
        ),
    }
}

fn start(config_dir: PathBuf) -> Shared {
    let path = config_dir.join(PROFILE_FILE);
    let game_dir = config_dir
        .parent()
        .map_or_else(|| config_dir.clone(), Path::to_path_buf);
    let (profile, error) = load_or_create(&path);
    if let Some(error) = &error {
        eprintln!("[data_exporter] profile: {error}; using the defaults");
    }
    let (sender, receiver) = sync_channel(QUEUE);
    let saved = Arc::new(AtomicU64::new(0));
    let dropped = Arc::new(AtomicU64::new(0));
    let full = Arc::new(AtomicBool::new(false));
    let thread = (saved.clone(), dropped.clone(), full.clone());
    let spawned = std::thread::Builder::new()
        .name("data-exporter".to_owned())
        .spawn(move || run_writer(receiver, thread.0, thread.1, thread.2));
    if let Err(e) = spawned {
        eprintln!("[data_exporter] can't start the writer: {e}");
    }
    Shared {
        modified: Mutex::new(modified_at(&path)),
        path,
        game_dir,
        profile: RwLock::new(Arc::new(profile)),
        checked: Mutex::new(None),
        error: Mutex::new(error),
        sender,
        saved,
        dropped,
        full,
        samples: Mutex::new(HashMap::new()),
    }
}

/// Starts the mod the first time; false while the game's folder isn't known.
pub fn ensure(config_dir: impl FnOnce() -> Option<PathBuf>) -> bool {
    if SHARED.get().is_some() {
        return true;
    }
    let Some(dir) = config_dir() else {
        return false;
    };
    let _ = SHARED.set(start(dir));
    true
}

fn current(shared: &Shared) -> Arc<Profile> {
    shared
        .profile
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

fn replace(shared: &Shared, profile: Profile) {
    *shared.profile.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(profile);
}

/// Whether the player wants this kind of data, before building its fields.
pub fn wants(id: &str) -> bool {
    SHARED.get().is_some_and(|s| current(s).captures(id))
}

pub fn others_allowed() -> bool {
    SHARED
        .get()
        .is_some_and(|s| current(s).include_other_players)
}

/// Picks up an edited profile, at most once a second. A broken file keeps the last good one.
pub fn poll_profile() {
    let Some(shared) = SHARED.get() else {
        return;
    };
    {
        let mut checked = shared.checked.lock().unwrap_or_else(|e| e.into_inner());
        if checked.is_some_and(|at| at.elapsed() < CHECK_PROFILE_EVERY) {
            return;
        }
        *checked = Some(Instant::now());
    }
    reload(shared);
}

fn reload(shared: &Shared) {
    let now = modified_at(&shared.path);
    {
        let mut modified = shared.modified.lock().unwrap_or_else(|e| e.into_inner());
        if *modified == now {
            return;
        }
        *modified = now;
    }
    let result = fs::read_to_string(&shared.path)
        .map_err(|e| format!("can't read the profile: {e}"))
        .and_then(|text| Profile::parse(&text));
    let mut error = shared.error.lock().unwrap_or_else(|e| e.into_inner());
    match result {
        Ok(profile) => {
            replace(shared, profile);
            *error = None;
        }
        Err(message) => {
            eprintln!("[data_exporter] profile: {message}; keeping the last good one");
            *error = Some(message);
        }
    }
}

/// Queues a record, unless the player doesn't want this kind. The game never waits.
pub fn emit(world: &str, id: &'static str, fields: Fields) {
    let Some(shared) = SHARED.get() else {
        return;
    };
    let profile = current(shared);
    let Some(kind) = find_kind(id).filter(|k| profile.captures(k.id)) else {
        return;
    };
    let (dir, format) = profile.destination(kind, &shared.game_dir);
    let ms = now_ms();
    let mut all: Fields = vec![
        ("schema", Field::Int(SCHEMA)),
        ("ts", Field::Text(iso(ms))),
        ("event", Field::Text(id.to_owned())),
        ("world", Field::Text(world.to_owned())),
    ];
    all.extend(fields);
    let message = Message::Record {
        root: resolve(&profile.folder, &shared.game_dir),
        key: FileKey {
            dir,
            event: kind.id,
            world: world.to_owned(),
            format,
        },
        fields: all,
        limits: Limits {
            max_file: profile.max_file_mb.saturating_mul(1 << 20),
            max_total: profile.max_total_mb.saturating_mul(1 << 20),
        },
        ms,
    };
    if let Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) =
        shared.sender.try_send(message)
    {
        shared.dropped.fetch_add(1, Ordering::Relaxed);
    }
}

/// Asks the writer to save what it holds now, as when a player leaves the world.
pub fn flush() {
    if let Some(shared) = SHARED.get() {
        let _ = shared.sender.try_send(Message::Flush);
    }
}

/// True once per `player_state_seconds` for each player.
pub fn sample_due(player: u128) -> bool {
    let Some(shared) = SHARED.get() else {
        return false;
    };
    let every = Duration::from_secs(current(shared).player_state_seconds);
    let mut samples = shared.samples.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    match samples.get(&player) {
        Some(last) if now.duration_since(*last) < every => false,
        _ => {
            samples.insert(player, now);
            true
        }
    }
}

const BUTTON_FORMAT: usize = GROUPS.len();
const BUTTON_OPEN: usize = BUTTON_FORMAT + 1;
const BUTTON_COPY: usize = BUTTON_FORMAT + 2;

/// The buttons of the mod's page in Options > Mods.
pub fn labels() -> Vec<String> {
    let Some(shared) = SHARED.get() else {
        return Vec::new();
    };
    reload(shared);
    let profile = current(shared);
    let mut buttons: Vec<String> = GROUPS
        .iter()
        .enumerate()
        .map(|(group, name)| {
            let state = match profile.group(group) {
                Some(true) => "ON",
                Some(false) => "OFF",
                None => "MIXED",
            };
            format!("{name}: {state}")
        })
        .collect();
    buttons.push(format!("Format: {}", profile.format.id().to_uppercase()));
    buttons.push("Open profile location".to_owned());
    buttons.push("Copy profile path".to_owned());
    buttons.push(status(shared));
    buttons
}

fn status(shared: &Shared) -> String {
    if shared
        .error
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some()
    {
        return "Status: the profile has an error".to_owned();
    }
    if shared.full.load(Ordering::Relaxed) {
        return "Status: disk limit reached".to_owned();
    }
    format!(
        "Status: {} saved, {} dropped",
        shared.saved.load(Ordering::Relaxed),
        shared.dropped.load(Ordering::Relaxed)
    )
}

/// What a click asks the game to do.
#[derive(Debug, PartialEq, Eq)]
pub enum Click {
    Nothing,
    Open(PathBuf),
    Copy(String),
    Failed(String),
}

fn edit_profile(shared: &Shared, change: impl FnOnce(&mut Profile)) -> Click {
    let mut profile = (*current(shared)).clone();
    change(&mut profile);
    if let Err(e) = save_text(&shared.path, &profile.to_json()) {
        return Click::Failed(format!("can't save the profile: {e}"));
    }
    *shared.modified.lock().unwrap_or_else(|e| e.into_inner()) = modified_at(&shared.path);
    *shared.error.lock().unwrap_or_else(|e| e.into_inner()) = None;
    replace(shared, profile);
    Click::Nothing
}

pub fn click(button: usize) -> Click {
    let Some(shared) = SHARED.get() else {
        return Click::Nothing;
    };
    match button {
        group if group < GROUPS.len() => edit_profile(shared, |p| p.flip_group(group)),
        BUTTON_FORMAT => edit_profile(shared, |p| {
            p.format = match p.format {
                Format::Jsonl => Format::Csv,
                Format::Csv => Format::Jsonl,
            }
        }),
        BUTTON_OPEN => Click::Open(shared.path.clone()),
        BUTTON_COPY => Click::Copy(shared.path.display().to_string()),
        _ => Click::Nothing,
    }
}

/// Shows `path` in the system's file manager.
pub fn reveal(path: &Path) -> io::Result<()> {
    let mut command = reveal_command(path);
    let mut child = command.spawn()?;
    // Reaped on its own thread, so the game's frame doesn't wait for it.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

#[cfg(target_os = "macos")]
fn reveal_command(path: &Path) -> std::process::Command {
    let mut command = std::process::Command::new("open");
    command.arg("-R").arg(path);
    command
}

#[cfg(windows)]
fn reveal_command(path: &Path) -> std::process::Command {
    let mut command = std::process::Command::new("explorer");
    command.arg(format!("/select,{}", path.display()));
    command
}

#[cfg(not(any(target_os = "macos", windows)))]
fn reveal_command(path: &Path) -> std::process::Command {
    let mut command = std::process::Command::new("xdg-open");
    command.arg(path.parent().unwrap_or(path));
    command
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    static NEXT: AtomicU32 = AtomicU32::new(0);

    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "data-exporter-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn limits() -> Limits {
        Limits {
            max_file: 1 << 20,
            max_total: 1 << 30,
        }
    }

    fn key(dir: &Path, format: Format) -> FileKey {
        FileKey {
            dir: dir.to_path_buf(),
            event: "session",
            world: "My World".to_owned(),
            format,
        }
    }

    fn record(text: &str) -> Fields {
        vec![
            ("event", Field::Text(text.to_owned())),
            ("count", Field::Int(3)),
            ("ratio", Field::Float(0.5)),
            ("ok", Field::Bool(true)),
            ("missing", Field::Null),
        ]
    }

    fn only_file(dir: &Path) -> String {
        let mut files: Vec<_> = fs::read_dir(dir).unwrap().flatten().collect();
        assert_eq!(files.len(), 1, "{files:?}");
        fs::read_to_string(files.remove(0).path()).unwrap()
    }

    #[test]
    fn the_default_profile_survives_its_own_file() {
        let profile = Profile::default();
        assert_eq!(Profile::parse(&profile.to_json()).unwrap(), profile);
    }

    #[test]
    fn a_profile_with_outputs_survives_its_own_file() {
        let mut profile = Profile {
            folder: "~/Data \"mine\"".to_owned(),
            format: Format::Csv,
            ..Profile::default()
        };
        profile.capture.insert("pickups".to_owned(), false);
        profile.outputs.insert(
            "player_deaths".to_owned(),
            Output {
                folder: Some("~/Analysis/deaths".to_owned()),
                format: Some(Format::Jsonl),
            },
        );
        assert_eq!(Profile::parse(&profile.to_json()).unwrap(), profile);
    }

    #[test]
    fn an_empty_object_is_the_default_profile() {
        assert_eq!(Profile::parse("{}").unwrap(), Profile::default());
    }

    #[test]
    fn a_broken_profile_says_what_is_wrong() {
        assert!(Profile::parse("{").unwrap_err().contains("not valid JSON"));
        assert!(Profile::parse("[]").unwrap_err().contains("object"));
        assert!(
            Profile::parse(r#"{"format":"parquet"}"#)
                .unwrap_err()
                .contains("jsonl")
        );
        assert!(
            Profile::parse(r#"{"capture":{"deaths":true}}"#)
                .unwrap_err()
                .contains("unknown kind")
        );
        assert!(
            Profile::parse(r#"{"max_total_mb":0}"#)
                .unwrap_err()
                .contains("at least 1")
        );
    }

    #[test]
    fn the_keyboard_and_other_players_are_not_captured_by_default() {
        let profile = Profile::default();
        assert!(!profile.include_other_players);
        assert!(kinds().all(|k| !k.id.contains("input") && !k.id.contains("chat")));
    }

    #[test]
    fn a_group_button_turns_all_of_a_mixed_group_on_then_off() {
        let mut profile = Profile::default();
        profile.capture.insert("block_placed".to_owned(), false);
        assert_eq!(profile.group(1), None);
        profile.flip_group(1);
        assert_eq!(profile.group(1), Some(true));
        profile.flip_group(1);
        assert_eq!(profile.group(1), Some(false));
        assert_eq!(profile.group(0), Some(true));
    }

    #[test]
    fn events_and_statistics_go_to_their_own_folders() {
        let profile = Profile::default();
        let game = Path::new("/game");
        let event = find_kind("crafting").unwrap();
        let stat = find_kind("stat_awards").unwrap();
        assert_eq!(
            profile.destination(event, game),
            (
                PathBuf::from("/game/data-exporter/events/crafting"),
                Format::Jsonl
            )
        );
        assert_eq!(
            profile.destination(stat, game).0,
            PathBuf::from("/game/data-exporter/stats/stat_awards")
        );
    }

    #[test]
    fn an_output_overrides_the_folder_and_format_of_its_kind_only() {
        let mut profile = Profile::default();
        profile.outputs.insert(
            "crafting".to_owned(),
            Output {
                folder: Some("/elsewhere".to_owned()),
                format: Some(Format::Csv),
            },
        );
        let game = Path::new("/game");
        assert_eq!(
            profile.destination(find_kind("crafting").unwrap(), game),
            (PathBuf::from("/elsewhere"), Format::Csv)
        );
        assert_eq!(
            profile.destination(find_kind("pickups").unwrap(), game).1,
            Format::Jsonl
        );
    }

    #[test]
    fn json_lines_are_valid_and_keep_their_order() {
        let line = json_line(&record("a \"quoted\"\nname"));
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["event"], "a \"quoted\"\nname");
        assert_eq!(value["count"], 3);
        assert!(value["missing"].is_null());
        assert!(line.find("\"event\"") < line.find("\"count\""));
    }

    #[test]
    fn csv_quotes_only_what_needs_it() {
        assert_eq!(csv_line(&record("plain")), "plain,3,0.5,true,\n");
        assert_eq!(
            csv_line(&record("a,\"b\"")),
            "\"a,\"\"b\"\"\",3,0.5,true,\n"
        );
        assert_eq!(csv_header(&record("x")), "event,count,ratio,ok,missing\n");
    }

    #[test]
    fn a_number_json_cant_hold_is_null() {
        assert_eq!(Field::Float(f64::NAN).json(), "null");
        assert_eq!(Field::Float(f64::INFINITY).csv(), "");
    }

    #[test]
    fn times_are_utc_iso_8601() {
        assert_eq!(iso(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso(1_772_323_200_123), "2026-03-01T00:00:00.123Z");
        assert_eq!(file_stamp(1_772_323_200_123), "20260301-000000");
    }

    #[test]
    fn only_clock_like_statistics_are_left_out() {
        assert!(is_tick_counter("custom", "minecraft:play_time"));
        assert!(is_tick_counter("custom", "time_since_rest"));
        assert!(!is_tick_counter("custom", "minecraft:jump"));
        assert!(!is_tick_counter("mined", "minecraft:play_time"));
        assert_eq!(namespaced("used"), "minecraft:used");
        assert_eq!(namespaced("mod:thing"), "mod:thing");
    }

    #[test]
    fn uuids_have_the_usual_shape() {
        assert_eq!(
            uuid_text(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef),
            "01234567-89ab-cdef-0123-456789abcdef"
        );
    }

    #[test]
    fn a_world_name_is_safe_in_a_file_name() {
        assert_eq!(safe_name("New World (6)"), "New World (6)");
        assert_eq!(safe_name("a/b:c?"), "a_b_c_");
        assert_eq!(safe_name(" . "), "world");
    }

    #[test]
    fn a_csv_file_has_one_header_then_its_rows() {
        let dir = scratch();
        let mut writer = Writer::new();
        for _ in 0..2 {
            let wrote = writer
                .write(&dir, key(&dir, Format::Csv), &record("x"), limits(), 0)
                .unwrap();
            assert_eq!(wrote, Wrote::Saved);
        }
        writer.flush();
        assert_eq!(
            only_file(&dir),
            "event,count,ratio,ok,missing\nx,3,0.5,true,\nx,3,0.5,true,\n"
        );
    }

    #[test]
    fn a_file_past_its_size_continues_in_a_new_part() {
        let dir = scratch();
        let mut writer = Writer::new();
        let small = Limits {
            max_file: 60,
            max_total: 1 << 30,
        };
        for _ in 0..4 {
            writer
                .write(&dir, key(&dir, Format::Jsonl), &record("x"), small, 0)
                .unwrap();
        }
        writer.flush();
        let names: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(names.len() > 1, "{names:?}");
        assert!(names.iter().any(|n| n.contains("_part")), "{names:?}");
    }

    #[test]
    fn the_disk_limit_stops_saving_and_deletes_nothing() {
        let dir = scratch();
        fs::write(dir.join("earlier.jsonl"), vec![b'x'; 200]).unwrap();
        let mut writer = Writer::new();
        let tight = Limits {
            max_file: 1 << 20,
            max_total: 260,
        };
        let first = writer
            .write(&dir, key(&dir, Format::Jsonl), &record("x"), tight, 0)
            .unwrap();
        assert_eq!(first, Wrote::Full);
        writer.flush();
        assert_eq!(fs::metadata(dir.join("earlier.jsonl")).unwrap().len(), 200);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
    }

    fn tight(max_total: u64) -> Limits {
        Limits {
            max_file: 1 << 20,
            max_total,
        }
    }

    #[test]
    fn a_raised_limit_resumes_saving_at_once() {
        let dir = scratch();
        fs::write(dir.join("earlier.jsonl"), vec![b'x'; 200]).unwrap();
        let mut writer = Writer::new();
        let line = || record("x");
        assert_eq!(
            writer
                .write(&dir, key(&dir, Format::Jsonl), &line(), tight(260), 0)
                .unwrap(),
            Wrote::Full
        );
        assert_eq!(
            writer
                .write(&dir, key(&dir, Format::Jsonl), &line(), tight(260), 0)
                .unwrap(),
            Wrote::Full
        );
        assert_eq!(
            writer
                .write(&dir, key(&dir, Format::Jsonl), &line(), tight(1 << 20), 0)
                .unwrap(),
            Wrote::Saved
        );
    }

    #[test]
    fn freeing_space_in_the_same_folder_resumes_saving() {
        let dir = scratch();
        fs::write(dir.join("earlier.jsonl"), vec![b'x'; 200]).unwrap();
        let mut writer = Writer::new();
        writer.recheck_every = Duration::ZERO;
        assert_eq!(
            writer
                .write(&dir, key(&dir, Format::Jsonl), &record("x"), tight(260), 0)
                .unwrap(),
            Wrote::Full
        );
        fs::remove_file(dir.join("earlier.jsonl")).unwrap();
        assert_eq!(
            writer
                .write(&dir, key(&dir, Format::Jsonl), &record("x"), tight(260), 0)
                .unwrap(),
            Wrote::Saved
        );
    }

    #[test]
    fn a_full_folder_is_not_measured_on_every_record() {
        let dir = scratch();
        fs::write(dir.join("earlier.jsonl"), vec![b'x'; 200]).unwrap();
        let mut writer = Writer::new();
        assert_eq!(
            writer
                .write(&dir, key(&dir, Format::Jsonl), &record("x"), tight(260), 0)
                .unwrap(),
            Wrote::Full
        );
        // Room again, but the folder isn't measured until the recheck is due.
        fs::remove_file(dir.join("earlier.jsonl")).unwrap();
        assert_eq!(
            writer
                .write(&dir, key(&dir, Format::Jsonl), &record("x"), tight(260), 0)
                .unwrap(),
            Wrote::Full
        );
    }

    #[test]
    fn a_second_writer_never_shares_a_file_with_the_first() {
        let dir = scratch();
        let (mut one, mut two) = (Writer::new(), Writer::new());
        for writer in [&mut one, &mut two] {
            writer
                .write(&dir, key(&dir, Format::Jsonl), &record("x"), limits(), 0)
                .unwrap();
            writer.flush();
        }
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 2);
    }

    #[test]
    fn a_missing_profile_is_created_and_a_broken_one_is_left_alone() {
        let dir = scratch();
        let path = dir.join(PROFILE_FILE);
        let (profile, error) = load_or_create(&path);
        assert_eq!((profile, error), (Profile::default(), None));
        assert_eq!(
            Profile::parse(&fs::read_to_string(&path).unwrap()).unwrap(),
            Profile::default()
        );
        fs::write(&path, "{ broken").unwrap();
        let (profile, error) = load_or_create(&path);
        assert_eq!(profile, Profile::default());
        assert!(error.unwrap().contains("not valid JSON"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "{ broken");
    }

    #[test]
    fn a_relative_folder_is_inside_the_game_folder() {
        let game = Path::new("/game");
        assert_eq!(resolve("out", game), PathBuf::from("/game/out"));
        assert_eq!(resolve("/abs", game), PathBuf::from("/abs"));
    }
}
