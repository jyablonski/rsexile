//! Disk-backed UI state: window position and lock flag.
//!
//! On Linux/macOS these live in `$XDG_STATE_HOME/rsexile/` (typically
//! `~/.local/state/rsexile/`), mirroring `python/overlay.py`'s `_STATE_DIR`
//! layout so users migrating from the Python build keep their position. On
//! Windows, where there's no XDG_STATE_HOME, they live under
//! `%LOCALAPPDATA%\rsexile\` (see `default_state_dir`).
//!
//! All I/O is best-effort — failures are swallowed by the public wrappers
//! since a missing or unwritable state dir should never crash the overlay.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

const APP_DIR_NAME: &str = "rsexile";
const POSITION_FILE: &str = "position.json";
const LOCK_FILE: &str = "lock.json";
const COLLAPSE_FILE: &str = "collapse.json";

pub const DEFAULT_POSITION: [f32; 2] = [20.0, 90.0];

#[derive(Debug, Serialize, Deserialize)]
struct PositionFile {
    x: f32,
    y: f32,
}

#[derive(Debug, Serialize, Deserialize)]
struct LockFile {
    locked: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct CollapseFile {
    collapsed: bool,
}

pub fn default_state_dir() -> Option<PathBuf> {
    #[cfg(not(target_os = "windows"))]
    {
        dirs::state_dir().map(|d| d.join(APP_DIR_NAME))
    }
    // Windows has no XDG_STATE_HOME equivalent and `dirs::state_dir()` returns
    // None there, which would silently disable position/lock persistence.
    // %LOCALAPPDATA% is the conventional per-user app-state location, e.g.
    // C:\Users\<user>\AppData\Local\rsexile.
    #[cfg(target_os = "windows")]
    {
        dirs::data_local_dir().map(|d| d.join(APP_DIR_NAME))
    }
}

/// Initial window position. Priority: `RSEXILE_X`/`RSEXILE_Y` env vars (both
/// required), then the saved position file, then [`DEFAULT_POSITION`].
pub fn initial_position() -> [f32; 2] {
    if let Some(pos) = position_from_env() {
        return pos;
    }
    if let Some(dir) = default_state_dir()
        && let Some(pos) = load_position_in(&dir)
    {
        return pos;
    }
    DEFAULT_POSITION
}

pub fn load_lock_state() -> bool {
    default_state_dir()
        .as_deref()
        .and_then(load_lock_in)
        .unwrap_or(false)
}

pub fn load_collapse_state() -> bool {
    default_state_dir()
        .as_deref()
        .and_then(load_collapse_in)
        .unwrap_or(false)
}

pub fn save_position(pos: [f32; 2]) -> Result<()> {
    let Some(dir) = default_state_dir() else {
        return Ok(());
    };
    save_position_in(&dir, pos)
}

pub fn save_lock_state(locked: bool) -> Result<()> {
    let Some(dir) = default_state_dir() else {
        return Ok(());
    };
    save_lock_in(&dir, locked)
}

pub fn save_collapse_state(collapsed: bool) -> Result<()> {
    let Some(dir) = default_state_dir() else {
        return Ok(());
    };
    save_collapse_in(&dir, collapsed)
}

fn position_from_env() -> Option<[f32; 2]> {
    position_from(|k| std::env::var(k).ok())
}

/// Parameterized inner so tests can inject a fake env getter without
/// touching process-wide env state (which races under parallel tests).
fn position_from<F>(get: F) -> Option<[f32; 2]>
where
    F: Fn(&str) -> Option<String>,
{
    let x = get("RSEXILE_X")?.parse::<f32>().ok()?;
    let y = get("RSEXILE_Y")?.parse::<f32>().ok()?;
    Some([x, y])
}

pub fn load_position_in(dir: &Path) -> Option<[f32; 2]> {
    let raw = fs::read_to_string(dir.join(POSITION_FILE)).ok()?;
    let parsed: PositionFile = serde_json::from_str(&raw).ok()?;
    Some([parsed.x, parsed.y])
}

pub fn save_position_in(dir: &Path, pos: [f32; 2]) -> Result<()> {
    fs::create_dir_all(dir)?;
    let body = serde_json::to_string(&PositionFile {
        x: pos[0],
        y: pos[1],
    })?;
    fs::write(dir.join(POSITION_FILE), body)?;
    Ok(())
}

pub fn load_lock_in(dir: &Path) -> Option<bool> {
    let raw = fs::read_to_string(dir.join(LOCK_FILE)).ok()?;
    let parsed: LockFile = serde_json::from_str(&raw).ok()?;
    Some(parsed.locked)
}

pub fn save_lock_in(dir: &Path, locked: bool) -> Result<()> {
    fs::create_dir_all(dir)?;
    let body = serde_json::to_string(&LockFile { locked })?;
    fs::write(dir.join(LOCK_FILE), body)?;
    Ok(())
}

pub fn load_collapse_in(dir: &Path) -> Option<bool> {
    let raw = fs::read_to_string(dir.join(COLLAPSE_FILE)).ok()?;
    let parsed: CollapseFile = serde_json::from_str(&raw).ok()?;
    Some(parsed.collapsed)
}

pub fn save_collapse_in(dir: &Path, collapsed: bool) -> Result<()> {
    fs::create_dir_all(dir)?;
    let body = serde_json::to_string(&CollapseFile { collapsed })?;
    fs::write(dir.join(COLLAPSE_FILE), body)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TempDir(PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    impl TempDir {
        fn path(&self) -> &Path {
            &self.0
        }
    }

    fn tempdir() -> TempDir {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let scope = module_path!().replace("::", "-");
        let path = std::env::temp_dir().join(format!("rsexile-test-{scope}-{pid}-{n}"));
        fs::create_dir_all(&path).unwrap();
        TempDir(path)
    }

    #[test]
    fn missing_position_file_returns_none() {
        let tmp = tempdir();
        assert_eq!(load_position_in(tmp.path()), None);
    }

    #[test]
    fn missing_lock_file_returns_none() {
        let tmp = tempdir();
        assert_eq!(load_lock_in(tmp.path()), None);
    }

    #[test]
    fn position_roundtrip() {
        let tmp = tempdir();
        save_position_in(tmp.path(), [123.0, 456.0]).unwrap();
        assert_eq!(load_position_in(tmp.path()), Some([123.0, 456.0]));
    }

    #[test]
    fn lock_roundtrip() {
        let tmp = tempdir();
        save_lock_in(tmp.path(), true).unwrap();
        assert_eq!(load_lock_in(tmp.path()), Some(true));

        save_lock_in(tmp.path(), false).unwrap();
        assert_eq!(load_lock_in(tmp.path()), Some(false));
    }

    #[test]
    fn collapse_roundtrip() {
        let tmp = tempdir();
        assert_eq!(load_collapse_in(tmp.path()), None);
        save_collapse_in(tmp.path(), true).unwrap();
        assert_eq!(load_collapse_in(tmp.path()), Some(true));
        save_collapse_in(tmp.path(), false).unwrap();
        assert_eq!(load_collapse_in(tmp.path()), Some(false));
    }

    #[test]
    fn save_creates_parent_directory() {
        let tmp = tempdir();
        let nested = tmp.path().join("nested/dir");
        save_position_in(&nested, [1.0, 2.0]).unwrap();
        assert_eq!(load_position_in(&nested), Some([1.0, 2.0]));
    }

    #[test]
    fn corrupt_position_file_returns_none() {
        let tmp = tempdir();
        fs::write(tmp.path().join(POSITION_FILE), "{ not json").unwrap();
        assert_eq!(load_position_in(tmp.path()), None);
    }

    #[test]
    fn corrupt_lock_file_returns_none() {
        let tmp = tempdir();
        fs::write(tmp.path().join(LOCK_FILE), "broken").unwrap();
        assert_eq!(load_lock_in(tmp.path()), None);
    }

    fn fake_env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k: &str| {
            pairs
                .iter()
                .find(|(name, _)| *name == k)
                .map(|(_, v)| (*v).to_string())
        }
    }

    #[test]
    fn position_from_env_returns_some_when_both_set() {
        let get = fake_env(&[("RSEXILE_X", "150.0"), ("RSEXILE_Y", "275.5")]);
        assert_eq!(position_from(get), Some([150.0, 275.5]));
    }

    #[test]
    fn position_from_env_returns_none_when_x_missing() {
        let get = fake_env(&[("RSEXILE_Y", "275.5")]);
        assert_eq!(position_from(get), None);
    }

    #[test]
    fn position_from_env_returns_none_when_y_missing() {
        let get = fake_env(&[("RSEXILE_X", "150.0")]);
        assert_eq!(position_from(get), None);
    }

    #[test]
    fn position_from_env_returns_none_when_unparseable() {
        let get = fake_env(&[("RSEXILE_X", "not-a-number"), ("RSEXILE_Y", "275.5")]);
        assert_eq!(position_from(get), None);
    }

    #[test]
    fn position_from_env_accepts_integers() {
        let get = fake_env(&[("RSEXILE_X", "100"), ("RSEXILE_Y", "200")]);
        assert_eq!(position_from(get), Some([100.0, 200.0]));
    }
}
