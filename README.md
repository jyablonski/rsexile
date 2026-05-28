# rsexile

[![CI/CD](https://github.com/jyablonski/rsexile/actions/workflows/ci_cd.yaml/badge.svg)](https://github.com/jyablonski/rsexile/actions/workflows/ci_cd.yaml)

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

Coming Soon.

## Campaign data

Zone and quest data lives in `data/poe2_campaign.json`. Each zone entry contains:

- `tasks` — ordered list of things to do before leaving, with rewards noted
- `optional` flag — marks tasks that can be skipped on a fast run
- `next_zone` — where to go after

To add personal notes without changing the shared guide, copy `data/poe2_campaign.local.example.json` to your per-user data dir as `poe2_campaign.local.json`:

- Linux/macOS: `$XDG_DATA_HOME/rsexile/poe2_campaign.local.json` (typically `~/.local/share/rsexile/`)
- Windows: `%APPDATA%\rsexile\poe2_campaign.local.json`

Local entries take precedence by normalized zone name — if your local file contains a `Clearfell` entry, it replaces the default `Clearfell` entry entirely. You can also add zones that are not in the default guide. The overlay picks up changes on next launch.
