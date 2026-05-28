//! Tails Path of Exile 2's `Client.txt` and emits parsed events.
//!
//! Mirrors `python/log_watcher.py` for event parsing and ignore lists, with
//! one additional behavior: on startup, scan back through the tail of the
//! file to find the most recent zone signal so the overlay can show current
//! state immediately instead of a blank "waiting" state.

use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::sync::mpsc::{Sender, channel};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use regex::Regex;

static ZONE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"You have entered (.+)\.").unwrap());
static SCENE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[SCENE\] Set Source \[(.+)\]").unwrap());
static DEATH_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"You have been slain\.").unwrap());
static LEVEL_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"is now level (\d+)").unwrap());

const IGNORED_SCENES: &[&str] = &["(null)", "(unknown)"];

const IGNORED_ZONE_NAMES: &[&str] = &[
    "Act 1",
    "Act 2",
    "Act 3",
    "Act 4",
    "Ziggurat Encampment",
    "Clearfell Encampment",
    "The Ardura Caravan",
    "Kingsmarch",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogEvent {
    ZoneEntered(String),
    SceneSet(String),
    Died,
    LevelUp(u32),
}

pub fn parse_line(line: &str) -> Option<LogEvent> {
    if let Some(caps) = ZONE_RE.captures(line) {
        let zone = caps.get(1).unwrap().as_str();
        return (!is_ignored(zone)).then(|| LogEvent::ZoneEntered(zone.to_string()));
    }
    if let Some(caps) = SCENE_RE.captures(line) {
        let scene = caps.get(1).unwrap().as_str();
        return (!is_ignored(scene)).then(|| LogEvent::SceneSet(scene.to_string()));
    }
    if DEATH_RE.is_match(line) {
        return Some(LogEvent::Died);
    }
    if let Some(caps) = LEVEL_RE.captures(line)
        && let Ok(n) = caps[1].parse::<u32>()
    {
        return Some(LogEvent::LevelUp(n));
    }
    None
}

fn is_ignored(name: &str) -> bool {
    // Hideouts (e.g. "Plateau of the Gods Hideout") aren't campaign zones, so
    // popping to one mid-run shouldn't blank the guide — drop them and keep the
    // last real zone showing.
    name.contains("Hideout") || IGNORED_SCENES.contains(&name) || IGNORED_ZONE_NAMES.contains(&name)
}

pub fn default_search_paths() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();

    #[cfg(target_os = "linux")]
    if let Some(home) = dirs::home_dir() {
        out.push(home.join(".local/share/Steam/steamapps/common/Path of Exile 2/logs/Client.txt"));
        out.push(home.join(
            ".steam/steam/steamapps/compatdata/2694490/pfx/drive_c/users/steamuser/AppData/Local/Path of Exile 2/logs/Client.txt",
        ));
        out.push(home.join(
            ".var/app/com.valvesoftware.Steam/data/Steam/steamapps/compatdata/2694490/pfx/drive_c/users/steamuser/AppData/Local/Path of Exile 2/logs/Client.txt",
        ));
    }

    #[cfg(target_os = "macos")]
    if let Some(home) = dirs::home_dir() {
        out.push(home.join("Library/Application Support/Path of Exile 2/logs/Client.txt"));
    }

    #[cfg(target_os = "windows")]
    {
        // Steam's default install location for the PoE2 client. Users who
        // move Steam to a secondary library aren't auto-detected — `--log`
        // covers that case.
        for root in [r"C:\Program Files (x86)\Steam", r"C:\Program Files\Steam"] {
            out.push(PathBuf::from(root).join(r"steamapps\common\Path of Exile 2\logs\Client.txt"));
        }
        // Standalone (GGG) client. The exact path is UNCONFIRMED on a real
        // install — this mirrors PoE1's `%LOCALAPPDATA%\Path of Exile 2`
        // pattern and PoE2 may differ. `--log` is the fallback until verified
        // on a Windows box.
        if let Some(local) = dirs::data_local_dir() {
            out.push(local.join(r"Path of Exile 2\logs\Client.txt"));
        }
    }

    out
}

pub fn resolve_log_path(override_path: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(p) = override_path {
        if !p.exists() {
            bail!("--log path does not exist: {}", p.display());
        }
        return Ok(p);
    }
    let candidates = default_search_paths();
    for p in &candidates {
        if p.exists() {
            return Ok(p.clone());
        }
    }
    let listed = candidates
        .iter()
        .map(|p| format!("  - {}", p.display()))
        .collect::<Vec<_>>()
        .join("\n");
    bail!("Could not find Client.txt. Pass --log <PATH> or place it at one of:\n{listed}");
}

/// Reads lines appended to a file since the last call. Starts at EOF on
/// construction so existing history is not replayed.
pub struct Tailer {
    path: PathBuf,
    pos: u64,
}

impl Tailer {
    pub fn at_end(path: PathBuf) -> Result<Self> {
        let pos = fs::metadata(&path)
            .with_context(|| format!("stat log file: {}", path.display()))?
            .len();
        Ok(Self { path, pos })
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn pos(&self) -> u64 {
        self.pos
    }

    /// Reads any newly appended *complete* lines and invokes `on_line` for
    /// each. Advances the internal position only past newline-terminated
    /// lines, so a partial line still being written (no trailing `\n` yet)
    /// is left unconsumed and re-read in full on the next call. Without this,
    /// a line flushed in two writes would be split across two reads and both
    /// halves would fail to parse, silently dropping the event.
    pub fn read_new_lines<F: FnMut(&str)>(&mut self, mut on_line: F) -> Result<()> {
        let mut file = File::open(&self.path)
            .with_context(|| format!("opening log file: {}", self.path.display()))?;
        let len = file.metadata()?.len();
        // If the file shrank (rotation/truncation), restart from 0.
        if len < self.pos {
            self.pos = 0;
        }
        let start = self.pos;
        file.seek(SeekFrom::Start(start))?;
        let mut reader = BufReader::new(file);
        let mut line = String::new();
        let mut consumed: u64 = 0;
        loop {
            line.clear();
            let n = reader.read_line(&mut line)?;
            if n == 0 {
                break;
            }
            // A read that doesn't end in '\n' is a partial line at EOF.
            // Leave it for the next call rather than emitting a fragment.
            if !line.ends_with('\n') {
                break;
            }
            consumed += n as u64;
            on_line(&line);
        }
        self.pos = start + consumed;
        Ok(())
    }
}

/// Scans the tail of the log for the most recent zone or scene signal.
/// Kept around (and tested) for possible future use behind an opt-in
/// flag, but not currently wired into `LogWatcher::start` — `Client.txt`
/// persists across PoE2 sessions, so pre-seeding from history usually
/// shows stale zone data from the prior play session.
#[cfg_attr(not(test), allow(dead_code))]
pub fn scan_for_current_zone(path: &Path) -> Option<LogEvent> {
    const TAIL_BYTES: u64 = 64 * 1024;
    let len = fs::metadata(path).ok()?.len();
    let start = len.saturating_sub(TAIL_BYTES);
    let mut file = File::open(path).ok()?;
    file.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = String::new();
    BufReader::new(file).read_to_string(&mut buf).ok()?;
    for line in buf.lines().rev() {
        if let Some(ev) = parse_line(line)
            && matches!(ev, LogEvent::ZoneEntered(_) | LogEvent::SceneSet(_))
        {
            return Some(ev);
        }
    }
    None
}

/// Owns the filesystem watcher and the worker thread that translates
/// filesystem events into `LogEvent`s. Drop to stop watching.
pub struct LogWatcher {
    _watcher: RecommendedWatcher,
    _handle: JoinHandle<()>,
}

impl LogWatcher {
    pub fn start(path: PathBuf, events_tx: Sender<LogEvent>) -> Result<Self> {
        // Canonicalize up front so directory-watch events (which report
        // real, symlink-resolved paths) compare equal to our stored path.
        // Without this, a symlinked log path (e.g. a Steam compatdata prefix
        // reached through a symlink, or a non-canonical --log) would make the
        // `event.paths` equality check in `run_worker` always fail, silently
        // dropping every update.
        //
        // dunce::canonicalize, not std::fs::canonicalize: on Windows the std
        // version returns an extended-length `\\?\C:\...` (verbatim UNC) path,
        // while notify reports plain `C:\...` paths — they'd never compare
        // equal, reintroducing the exact silent-drop bug this guards against.
        // dunce strips the `\\?\` prefix when safe and is a no-op on Linux.
        let path = dunce::canonicalize(&path)
            .with_context(|| format!("resolving log path: {}", path.display()))?;

        // No pre-seeding from history: PoE2's log persists across sessions,
        // so the "most recent zone" in the file is usually from a previous
        // play session, not the current one. Wait for a live zone-change
        // event after startup before showing anything.
        let tailer = Tailer::at_end(path.clone())?;
        let parent = path
            .parent()
            .ok_or_else(|| anyhow!("log path has no parent directory: {}", path.display()))?
            .to_path_buf();

        let (fs_tx, fs_rx) = channel::<notify::Result<notify::Event>>();
        let mut watcher: RecommendedWatcher = notify::recommended_watcher(move |res| {
            let _ = fs_tx.send(res);
        })?;
        watcher.watch(&parent, RecursiveMode::NonRecursive)?;

        let handle = thread::Builder::new()
            .name("rsexile-log-watcher".into())
            .spawn(move || {
                run_worker(path, tailer, fs_rx, events_tx);
            })?;

        Ok(Self {
            _watcher: watcher,
            _handle: handle,
        })
    }
}

fn run_worker(
    path: PathBuf,
    mut tailer: Tailer,
    fs_rx: std::sync::mpsc::Receiver<notify::Result<notify::Event>>,
    events_tx: Sender<LogEvent>,
) {
    let drain = |tailer: &mut Tailer| {
        if let Err(e) = tailer.read_new_lines(|line| {
            if let Some(ev) = parse_line(line) {
                let _ = events_tx.send(ev);
            }
        }) {
            eprintln!("log watcher: read failed: {e}");
        }
    };

    loop {
        match fs_rx.recv_timeout(Duration::from_millis(500)) {
            Ok(Ok(event)) => {
                if !event.paths.iter().any(|p| p == &path) {
                    continue;
                }
                drain(&mut tailer);
            }
            Ok(Err(_)) => {}
            // Safety net: notify backends can coalesce or miss inotify events
            // under heavy I/O. Re-read on each idle tick so a missed event is
            // picked up within the timeout instead of waiting for the next one.
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => drain(&mut tailer),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn p(line: &str) -> Option<LogEvent> {
        parse_line(line)
    }

    #[test]
    fn zone_change_parses() {
        assert_eq!(
            p("2025/01/01 12:00:00 [INFO Client 1234] : You have entered Clearfell."),
            Some(LogEvent::ZoneEntered("Clearfell".into()))
        );
    }

    #[test]
    fn zone_with_spaces() {
        assert_eq!(
            p("2025/01/01 12:00:00 [INFO Client 1234] : You have entered Ogham Farmlands."),
            Some(LogEvent::ZoneEntered("Ogham Farmlands".into()))
        );
    }

    #[test]
    fn zone_with_apostrophe() {
        assert_eq!(
            p("2025/01/01 12:00:00 [INFO Client 1234] : You have entered Jiquani's Machinarium."),
            Some(LogEvent::ZoneEntered("Jiquani's Machinarium".into()))
        );
    }

    #[test]
    fn zone_with_numbers() {
        assert_eq!(
            p("2025/01/01 12:00:00 [INFO Client 1234] : You have entered Zone42."),
            Some(LogEvent::ZoneEntered("Zone42".into()))
        );
    }

    #[test]
    fn scene_set_parses() {
        assert_eq!(
            p("2026/05/27 19:00:25 12825619 [INFO Client 340] [SCENE] Set Source [The Azak Bog]"),
            Some(LogEvent::SceneSet("The Azak Bog".into()))
        );
    }

    #[test]
    fn scene_null_ignored() {
        assert_eq!(p("[SCENE] Set Source [(null)]"), None);
    }

    #[test]
    fn scene_unknown_ignored() {
        assert_eq!(p("[SCENE] Set Source [(unknown)]"), None);
    }

    #[test]
    fn ignored_zone_names_drop() {
        for name in IGNORED_ZONE_NAMES {
            let line = format!("[INFO] You have entered {name}.");
            assert_eq!(p(&line), None, "expected ignore for {name}");
        }
    }

    #[test]
    fn hideout_zones_are_ignored() {
        assert_eq!(
            p("[INFO Client 1234] : You have entered Plateau of the Gods Hideout."),
            None
        );
        assert_eq!(p("[SCENE] Set Source [Felled Hideout]"), None);
    }

    #[test]
    fn death_parses() {
        assert_eq!(
            p("2025/01/01 12:00:00 [INFO Client 1234] : You have been slain."),
            Some(LogEvent::Died)
        );
    }

    #[test]
    fn level_up_parses() {
        assert_eq!(
            p("2025/01/01 12:00:00 [INFO Client 1234] PlayerName is now level 42"),
            Some(LogEvent::LevelUp(42))
        );
    }

    #[test]
    fn level_up_level_1() {
        assert_eq!(
            p("2025/01/01 12:00:00 [INFO Client 1234] PlayerName is now level 1"),
            Some(LogEvent::LevelUp(1))
        );
    }

    #[test]
    fn level_up_high_level() {
        assert_eq!(
            p("2025/01/01 12:00:00 [INFO Client 1234] PlayerName is now level 100"),
            Some(LogEvent::LevelUp(100))
        );
    }

    #[test]
    fn irrelevant_line_parses_to_none() {
        assert_eq!(
            p("2025/01/01 12:00:00 [INFO Client 1234] Connecting to instance server..."),
            None
        );
    }

    #[test]
    fn empty_line_parses_to_none() {
        assert_eq!(p(""), None);
    }

    #[test]
    fn zone_line_is_not_a_death() {
        assert_eq!(
            p("2025/01/01 12:00:00 [INFO Client 1234] : You have entered Clearfell."),
            Some(LogEvent::ZoneEntered("Clearfell".into()))
        );
    }

    #[test]
    fn death_line_is_not_a_zone() {
        assert_eq!(
            p("2025/01/01 12:00:00 [INFO Client 1234] : You have been slain."),
            Some(LogEvent::Died)
        );
    }

    #[test]
    fn tailer_starts_at_eof() {
        let tmp = tempdir();
        let log = tmp.path().join("Client.txt");
        fs::write(&log, "existing line\n").unwrap();
        let tailer = Tailer::at_end(log.clone()).unwrap();
        assert_eq!(tailer.pos(), fs::metadata(&log).unwrap().len());
    }

    #[test]
    fn tailer_reads_appended_lines() {
        let tmp = tempdir();
        let log = tmp.path().join("Client.txt");
        fs::write(&log, "").unwrap();
        let mut tailer = Tailer::at_end(log.clone()).unwrap();

        append(
            &log,
            "2025/01/01 12:00:00 [INFO Client 1234] : You have entered Clearfell.\n",
        );

        let mut events = Vec::new();
        tailer
            .read_new_lines(|line| {
                if let Some(ev) = parse_line(line) {
                    events.push(ev);
                }
            })
            .unwrap();

        assert_eq!(events, vec![LogEvent::ZoneEntered("Clearfell".into())]);
    }

    #[test]
    fn tailer_advances_position() {
        let tmp = tempdir();
        let log = tmp.path().join("Client.txt");
        fs::write(&log, "").unwrap();
        let mut tailer = Tailer::at_end(log.clone()).unwrap();
        let initial = tailer.pos();

        append(&log, "added\n");
        tailer.read_new_lines(|_| {}).unwrap();

        assert!(tailer.pos() > initial);
    }

    #[test]
    fn tailer_does_not_re_emit_old_content() {
        let tmp = tempdir();
        let log = tmp.path().join("Client.txt");
        fs::write(
            &log,
            "2025/01/01 12:00:00 [INFO Client 1234] : You have entered OldZone.\n",
        )
        .unwrap();
        let mut tailer = Tailer::at_end(log).unwrap();

        let mut events = Vec::new();
        tailer
            .read_new_lines(|line| {
                if let Some(ev) = parse_line(line) {
                    events.push(ev);
                }
            })
            .unwrap();

        assert!(events.is_empty());
    }

    #[test]
    fn tailer_defers_partial_line_until_newline_arrives() {
        // A line flushed in two writes must not be split into two fragments
        // (which would both fail to parse). The partial is held back until the
        // terminating newline shows up, then emitted as one complete line.
        let tmp = tempdir();
        let log = tmp.path().join("Client.txt");
        fs::write(&log, "").unwrap();
        let mut tailer = Tailer::at_end(log.clone()).unwrap();

        // First write: the line without its trailing newline.
        append(
            &log,
            "2025/01/01 12:00:00 [INFO Client 1234] : You have entered Clear",
        );
        let pos_before = tailer.pos();
        let mut events = Vec::new();
        tailer
            .read_new_lines(|line| {
                if let Some(ev) = parse_line(line) {
                    events.push(ev);
                }
            })
            .unwrap();
        assert!(
            events.is_empty(),
            "partial line should not emit an event yet"
        );
        assert_eq!(
            tailer.pos(),
            pos_before,
            "position must not advance past an unterminated line"
        );

        // Second write: the rest of the line plus the newline.
        append(&log, "fell.\n");
        tailer
            .read_new_lines(|line| {
                if let Some(ev) = parse_line(line) {
                    events.push(ev);
                }
            })
            .unwrap();
        assert_eq!(events, vec![LogEvent::ZoneEntered("Clearfell".into())]);
    }

    #[test]
    fn tailer_handles_truncation() {
        let tmp = tempdir();
        let log = tmp.path().join("Client.txt");
        fs::write(&log, "0123456789\n").unwrap();
        let mut tailer = Tailer::at_end(log.clone()).unwrap();
        assert!(tailer.pos() > 0);

        fs::write(
            &log,
            "2025/01/01 12:00:00 [INFO Client 1234] : You have entered Clearfell.\n",
        )
        .unwrap();

        let mut events = Vec::new();
        tailer
            .read_new_lines(|line| {
                if let Some(ev) = parse_line(line) {
                    events.push(ev);
                }
            })
            .unwrap();

        assert_eq!(events, vec![LogEvent::ZoneEntered("Clearfell".into())]);
    }

    #[test]
    fn scan_for_current_zone_finds_most_recent() {
        let tmp = tempdir();
        let log = tmp.path().join("Client.txt");
        let contents = "\
2025/01/01 12:00:00 [INFO Client 1234] : You have entered Clearfell.
2025/01/01 12:00:05 [INFO Client 1234] : You have entered Hunting Grounds.
2025/01/01 12:00:10 [INFO Client 1234] : You have entered Freythorn.
";
        fs::write(&log, contents).unwrap();
        assert_eq!(
            scan_for_current_zone(&log),
            Some(LogEvent::ZoneEntered("Freythorn".into())),
        );
    }

    #[test]
    fn scan_for_current_zone_skips_ignored() {
        let tmp = tempdir();
        let log = tmp.path().join("Client.txt");
        let contents = "\
2025/01/01 12:00:00 [INFO Client 1234] : You have entered Clearfell.
2025/01/01 12:00:05 [INFO Client 1234] : You have entered Act 1.
2025/01/01 12:00:06 [INFO Client 1234] [SCENE] Set Source [(null)]
";
        fs::write(&log, contents).unwrap();
        assert_eq!(
            scan_for_current_zone(&log),
            Some(LogEvent::ZoneEntered("Clearfell".into())),
        );
    }

    #[test]
    fn scan_for_current_zone_returns_none_for_empty() {
        let tmp = tempdir();
        let log = tmp.path().join("Client.txt");
        fs::write(&log, "").unwrap();
        assert_eq!(scan_for_current_zone(&log), None);
    }

    #[test]
    fn scan_for_current_zone_returns_scene_when_only_scene_present() {
        let tmp = tempdir();
        let log = tmp.path().join("Client.txt");
        fs::write(
            &log,
            "2026/05/27 19:00:25 12825619 [INFO Client 340] [SCENE] Set Source [The Azak Bog]\n",
        )
        .unwrap();
        assert_eq!(
            scan_for_current_zone(&log),
            Some(LogEvent::SceneSet("The Azak Bog".into())),
        );
    }

    #[test]
    fn resolve_log_path_uses_override() {
        let tmp = tempdir();
        let log = tmp.path().join("Client.txt");
        fs::write(&log, "").unwrap();
        let resolved = resolve_log_path(Some(log.clone())).unwrap();
        assert_eq!(resolved, log);
    }

    #[test]
    fn resolve_log_path_rejects_missing_override() {
        let missing = PathBuf::from("/does/not/exist/Client.txt");
        let err = resolve_log_path(Some(missing.clone())).unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn default_search_paths_returns_client_txt_candidates() {
        let paths = default_search_paths();
        assert!(
            !paths.is_empty(),
            "default_search_paths should return at least one candidate"
        );
        for p in &paths {
            assert_eq!(
                p.file_name().and_then(|n| n.to_str()),
                Some("Client.txt"),
                "expected all candidates to end with Client.txt, got {}",
                p.display()
            );
        }
    }

    #[test]
    fn log_watcher_emits_event_when_zone_line_appended() {
        let tmp = tempdir();
        let log = tmp.path().join("Client.txt");
        fs::write(&log, "").unwrap();

        let (tx, rx) = channel::<LogEvent>();
        let _watcher = LogWatcher::start(log.clone(), tx).expect("watcher starts");

        // Give the notify backend a moment to register the watch before
        // we make the change. Without this, the inotify subscription can
        // miss the very next write on some kernels.
        thread::sleep(Duration::from_millis(150));

        append(
            &log,
            "2025/01/01 12:00:00 [INFO Client 1234] : You have entered Clearfell.\n",
        );

        let event = rx
            .recv_timeout(Duration::from_secs(3))
            .expect("expected ZoneEntered event within 3s");
        assert_eq!(event, LogEvent::ZoneEntered("Clearfell".into()));
    }

    #[test]
    fn log_watcher_does_not_pre_seed_from_history() {
        // The overlay should stay idle until a new zone-change event lands
        // *after* startup. Historical entries in Client.txt (from prior
        // sessions) must not surface — they're stale and confusing.
        let tmp = tempdir();
        let log = tmp.path().join("Client.txt");
        fs::write(
            &log,
            "2025/01/01 12:00:00 [INFO Client 1234] : You have entered Clearfell.\n",
        )
        .unwrap();

        let (tx, rx) = channel::<LogEvent>();
        let _watcher = LogWatcher::start(log, tx).expect("watcher starts");

        let result = rx.recv_timeout(Duration::from_millis(300));
        assert!(
            result.is_err(),
            "expected no pre-seeded event, got: {result:?}"
        );
    }

    #[test]
    fn log_watcher_rejects_path_with_no_parent() {
        // A root-only path has no parent directory for notify to watch.
        // LogWatcher doesn't impl Debug, so use a match instead of unwrap_err.
        let (tx, _rx) = channel::<LogEvent>();
        match LogWatcher::start(PathBuf::from("/"), tx) {
            Ok(_) => panic!("expected error for path with no parent"),
            Err(err) => {
                let msg = err.to_string();
                assert!(
                    msg.contains("parent")
                        || msg.contains("stat")
                        || msg.contains("Is a directory"),
                    "unexpected error: {msg}"
                );
            }
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn canonicalized_log_path_has_no_verbatim_prefix() {
        // On Windows, std::fs::canonicalize yields a `\\?\` (verbatim UNC)
        // path that notify's plain `C:\...` event paths never match, silently
        // dropping every update. dunce::canonicalize (used in start()) must
        // strip that prefix so the equality check in run_worker holds.
        let tmp = tempdir();
        let log = tmp.path().join("Client.txt");
        fs::write(&log, "").unwrap();
        let canon = dunce::canonicalize(&log).unwrap();
        assert!(
            !canon.to_string_lossy().starts_with(r"\\?\"),
            "canonical path unexpectedly verbatim: {}",
            canon.display()
        );
    }

    fn append(path: &Path, contents: &str) {
        let mut f = fs::OpenOptions::new().append(true).open(path).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
    }

    struct TempDir(PathBuf);

    impl TempDir {
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn tempdir() -> TempDir {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let scope = module_path!().replace("::", "-");
        let path = std::env::temp_dir().join(format!("rsexile-test-{scope}-{pid}-{n}"));
        fs::create_dir_all(&path).expect("create tempdir");
        TempDir(path)
    }
}
