# ParaFlow TUI — Design Spec
Date: 2026-06-02

## Overview

An interactive terminal UI for the ParaFlow client. Upload-only scope, matching the current CLI. The CLI (`client upload --file ...`) remains intact for scripting. The TUI launches when `client` is run with no subcommand.

## Layout

Three fixed regions:

```
┌─ ParaFlow ────────────── ◉ Connected: 127.0.0.1:7878 ─┐
│  Queue: 3  Uploading: 1  Done: 2  Total: 2.3 GB        │  ← Header bar
├─────────────────────────────────────────────────────────┤
│ ▶ report.pdf    124MB   Uploading  [████████░░] 62%     │
│   video.mp4     2.1GB   Queued     [░░░░░░░░░░]  0%    │  ← Queue panel
│   archive.zip    45MB   Done ✓     [██████████]100%    │
├─────────────────────────────────────────────────────────┤
│  Workers — report.pdf                                   │
│  W0  chunk #3  [████████░░] 80%                        │  ← Worker panel (collapsible)
│  W1  chunk #7  [██████░░░░] 60%                        │
│  W2  chunk #1  Done ✓                                  │
├─────────────────────────────────────────────────────────┤
│ [a]dd  [Enter]workers  [q]uit  [?]help                  │  ← Footer
└─────────────────────────────────────────────────────────┘
```

- **Header bar**: connection status (patina teal dot), server address, aggregate stats (queue count, uploading count, done count, total bytes). Fixed, never scrolls.
- **Queue panel**: scrollable list of files. Each row: filename, size, status badge, inline progress bar. Always visible.
- **Worker panel**: hidden by default. Toggled with `Enter` on an uploading file. Shows per-thread chunk index and progress bar. Auto-closes when the selected file transitions to Done or Failed. Hidden when selected file is not uploading.
- **Footer**: context-sensitive keybinding hints. Updates based on active panel state.

Minimum terminal size: 80×24. Below that, display a "terminal too small" message.

## Color System

Rust-themed — forged metal aesthetic, warm darks with oxidized orange accents.

| Slot | Hex | Used For |
|---|---|---|
| `bg.base` | `#1C1A18` | App background |
| `bg.surface` | `#252220` | Queue panel |
| `bg.overlay` | `#2E2A26` | Worker panel |
| `bg.selection` | `#3D2A18` | Selected file row |
| `fg.default` | `#E8DDD0` | Body text |
| `fg.muted` | `#7A6E64` | Metadata, size, timestamps, footer |
| `fg.emphasis` | `#F5EDE3` | Headers, focused items |
| `accent.primary` | `#E8732A` | Rust orange — active progress bars, focus borders |
| `accent.secondary` | `#D4A843` | Copper — worker bars, secondary actions |
| `status.success` | `#6A8F3D` | Done ✓ |
| `status.error` | `#CC3333` | Failed, errors |
| `status.info` | `#5E9E8F` | Patina teal — connection status dot |

Rules:
- Focus border: `accent.primary`. Unfocused borders: `fg.muted`.
- Progress bars: uploading = `accent.primary`, workers = `accent.secondary`, done = `status.success`.
- Never hardcode hex in widget code — always reference semantic slot names.
- Must be usable in 16-color mode (true color enhances, never gates functionality).

## State Machine

**App states:**
```
Connecting ──(success)──► Ready ──(file added)──► Uploading ──(all done)──► Ready
     └──(fail)──► Error                                └──(fatal error)──► Error
```

**File states:**
```
Queued ──► Uploading ──► Done
                └──► Failed
```

## Data Flow

The TUI render loop runs on the main thread. Upload logic runs in a background thread. A `mpsc` channel bridges them.

```
TUI render loop (main thread)
  polls: crossterm events + progress_rx
  redraws on every event

Upload worker (spawned thread per file)
  reuses existing connect_and_auth() + chunk logic from main.rs
  sends ProgressEvent { file_id, worker_id, chunk_index, bytes_done } to progress_tx on every ChunkAck
```

The existing `connect_and_auth`, `read_chunk`, and chunk-sending logic from `client/src/main.rs` is extracted into a reusable function and called from the worker thread. No duplication.

## Config

File: `~/.paraflow.toml`

```toml
host = "127.0.0.1"
port = 7878
secret = "secret123"
threads = 4
```

- Loaded at startup using the `dirs` crate for the home directory path.
- Missing file: use defaults, show a hint in the status bar ("no config found, using defaults").
- CLI flags override config values when the CLI subcommand is used directly.

## Keybindings

| Key | Action |
|---|---|
| `a` | Add file — opens a path input popup |
| `j` / `↓` | Move selection down |
| `k` / `↑` | Move selection up |
| `Enter` | Toggle worker panel for selected file (only active when file is Uploading) |
| `q` | Quit — prompts confirmation if any upload is in progress |
| `Esc` | Dismiss popup / collapse worker panel |
| `?` | Toggle help overlay |

## New Dependencies

| Crate | Purpose |
|---|---|
| `ratatui` | TUI rendering framework |
| `crossterm` | Terminal backend for ratatui |
| `toml` | Config file parsing |
| `dirs` | Home directory resolution |

These are added to `client/Cargo.toml` only. The `clap` dependency remains.

## Out of Scope

- Download, file listing, delete — server protocol does not support these.
- Multi-user file isolation — single shared secret, no per-user identity.
- Config editor inside the TUI — edit `~/.paraflow.toml` directly.
