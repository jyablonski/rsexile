# rsexile

A Path of Exile 2 in-game campaign-guide overlay for Linux and Windows. As you play, rsexile follows your zone transitions and shows what to do, what to skip, and what rewards to look for in the current zone.

## What it does

- Tails PoE2's `Client.txt` log file and listens for zone-change events.
- Renders a small always-on-top overlay window beside the game showing the current zone's tasks, optional steps, rewards, and next destination.
- Picks up changes instantly — when you enter a new zone, the overlay updates within a frame.
- Ships with a default campaign guide baked into the binary. You can override individual zones with a personal file without touching the shared guide.

The overlay is fully external to the game: it never reads game memory, never injects input, never automates anything, and never talks to GGG's servers. It only reads a log file the game already writes to disk and draws a desktop window next to it.

## Why this is not ban-worthy

GGG's third-party tool policy explicitly permits tools that "would work if run on a second computer" — i.e. tools that only consume what the client already writes to disk and do not interact with the game process. rsexile sits firmly in that category:

- No memory reads of the PoE2 process.
- No input injection or keystroke automation.
- No modification of game files or assets.
- No network traffic to GGG.
- The only inputs are the client log file and the user's own keyboard/mouse on the overlay window itself.

If you can imagine the same workflow done by glancing at a printed walkthrough on your desk, rsexile is the same thing — just on the same screen.

## Install

Pre-built binaries for Linux and Windows are published on GitHub Releases.

### Linux

```bash
curl -fL -o /tmp/rsexile.tar.gz \
    "https://github.com/jyablonski/rsexile/releases/latest/download/rsexile-x86_64-unknown-linux-gnu.tar.gz"
mkdir -p "${HOME}/.local/bin"
tar -xzf /tmp/rsexile.tar.gz -C "${HOME}/.local/bin"
chmod +x "${HOME}/.local/bin/rsexile"
rm /tmp/rsexile.tar.gz
```

Make sure `${HOME}/.local/bin` is on your `PATH`, then run:

```bash
rsexile
```

### Windows

In PowerShell:

```powershell
$url = "https://github.com/jyablonski/rsexile/releases/latest/download/rsexile-x86_64-pc-windows-msvc.zip"
$out = "$env:LOCALAPPDATA\rsexile"
New-Item -ItemType Directory -Force -Path $out | Out-Null
Invoke-WebRequest -Uri $url -OutFile "$out\rsexile.zip"
Expand-Archive -Force -Path "$out\rsexile.zip" -DestinationPath $out
Remove-Item "$out\rsexile.zip"
```

Then run it directly, or add `$env:LOCALAPPDATA\rsexile` to your user `PATH`:

```powershell
& "$env:LOCALAPPDATA\rsexile\rsexile.exe"
```

The first launch may trip Windows SmartScreen ("Windows protected your PC") because the binary is unsigned — choose More info -> Run anyway.

### Updating

To update later, re-run the install snippet for your platform, or use the built-in updater:

```bash
rsexile self-update
```

### Build from source

If you'd rather build locally:

```bash
cargo build --release --locked
./target/release/rsexile
```

- Linux build dependencies (Debian/Ubuntu names): `libxkbcommon-dev libxcb1-dev libgl1-mesa-dev libwayland-dev pkg-config`.
- Windows needs the [MSVC build tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) (Visual Studio Build Tools 2019/2022 with the C++ workload). No extra system libraries, eframe's `glow` backend uses the WGL/OpenGL that ships with Windows.

## Setting it up with PoE2

Set PoE2's Display Mode to "Windowed Fullscreen" (borderless). An external always-on-top window like rsexile cannot draw over a game running in *exclusive* fullscreen — the overlay will be hidden behind it. Borderless/windowed mode lets the desktop compositor put rsexile on top. (In PoE2: Options -> Graphics -> Display Mode -> Windowed Fullscreen.)

You don't need to enable anything in-game for logging, PoE2 writes `Client.txt` on its own, and rsexile auto-detects it for a default Steam install. If your copy lives somewhere else, point rsexile at the log explicitly (see [Finding your Client.txt](#finding-your-clienttxt)).

### Launching alongside the game

For Linux the `scripts/steam-launch.sh` wrapper starts rsexile alongside PoE2 from Steam and cleans it up when the game exits. Set your PoE2 launch options in Steam to:

```
/path/to/rsexile/scripts/steam-launch.sh %command%
```

For Windows there is no launch wrapper yet. Start `rsexile.exe` manually before or after launching PoE2 (or add it to your Startup folder / Task Scheduler). The overlay attaches to whichever PoE2 instance writes to `Client.txt`, so launch order doesn't matter beyond having rsexile running when you want the overlay.

### Using the overlay

The overlay is controlled entirely with the mouse — there are no in-game hotkeys, so it never interferes with your binds:

- Move it — drag the panel body to reposition. The window manager handles the drag; the new position is saved automatically a moment after you let go.
- Lock it — click the L / U button (top-right). When locked (L), dragging is disabled so you can't nudge it by accident; click again to unlock (U).
- Collapse it — click the ▾ button to shrink the overlay to a small nub; click the nub to expand it again.

Position, lock, and collapse state persist across restarts.

### Finding your Client.txt

rsexile auto-detects `Client.txt` for a default Steam install:

- Linux: `~/.local/share/Steam/steamapps/common/Path of Exile 2/logs/Client.txt` (Proton compatdata and Flatpak Steam variants are also checked).
- Windows: `C:\Program Files (x86)\Steam\steamapps\common\Path of Exile 2\logs\Client.txt` (and the `C:\Program Files\Steam` variant).

If your install is elsewhere — Steam on a second drive, or the GGG standalone client (whose Windows log path is not yet verified) — pass the path explicitly:

```bash
# Linux
rsexile --log "/path/to/Path of Exile 2/logs/Client.txt"
```

```powershell
# Windows
& "$env:LOCALAPPDATA\rsexile\rsexile.exe" --log "D:\Games\Path of Exile 2\logs\Client.txt"
```

rsexile still launches in an idle state if it can't find the log, so you can start it first and pass `--log` once you've located the file.

## State and config locations

Window position and lock/collapse state are saved per-user:

- Linux/macOS: `$XDG_STATE_HOME/rsexile/` (typically `~/.local/state/rsexile/`)
- Windows: `%LOCALAPPDATA%\rsexile\`

## Campaign data

Zone and quest data lives in `data/poe2_campaign.json`. Each zone entry contains:

- `tasks` — ordered list of things to do before leaving, with rewards noted
- `optional` flag — marks tasks that can be skipped on a fast run
- `next_zone` — where to go after

To add personal notes without changing the shared guide, copy `data/poe2_campaign.local.example.json` to your per-user data dir as `poe2_campaign.local.json`:

- Linux/macOS: `$XDG_DATA_HOME/rsexile/poe2_campaign.local.json` (typically `~/.local/share/rsexile/`)
- Windows: `%APPDATA%\rsexile\poe2_campaign.local.json`

Local entries take precedence by normalized zone name — if your local file contains a `Clearfell` entry, it replaces the default `Clearfell` entry entirely. You can also add zones that are not in the default guide. The overlay picks up changes on next launch.
