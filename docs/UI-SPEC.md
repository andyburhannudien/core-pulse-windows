# UI specification

The UI uses `egui`/`eframe` 0.37 with `egui_extras` (tables). This document
defines what the user sees so the implementation does not invent its own rules.

## 1. Principles

1. **Dense, not airy.** This is a diagnostic tool; information per pixel is
   worth more than whitespace.
2. **Honest.** An unknown value renders as `—` with a reason, never as `0`.
3. **Fast.** Every number is sortable and searchable with no perceptible delay.
4. **Unsurprising.** Destructive actions are always confirmed and always name
   the target plus its PID.
5. **Technical, not decorative.** Monospaced numerals, hairline borders, one
   accent colour. No gradients-for-their-own-sake, no rounded-everything.

## 2. Window frame

```
+----------+---------------------------------------------------------------+
|  ◢ core- | Processes                          Search [_________]  21:04:55 |
|    pulse | Running processes, CPU, memory, actions   211 processes  1.0 s |
+----------+---------------------------------------------------------------+
| ▤ Processes 212 | PID   NAME        PATH            CPU   MEM  I/O ...   |
| ▥ Performance   | 12552 Zed.exe     C:\...\Zed.exe  3.4%  141 MB  ...    |
| ◉ Sensors       |  1768 dwm.exe     C:\...\dwm.exe  1.3%  112 MB  ...    |
| ▲ Alerts        |                                                       |
| ⚙ Services  714 |                                                       |
| ↗ Startup     7 |                                                       |
| ◍ Users       2 |                                                       |
|                 |                                                       |
| ● All thresholds nominal                                               |
| 6 cores · 8 GB RAM                                                     |
+----------+---------------------------------------------------------------+
| CPU 24%  RAM 64%  DISK 31%  NET 1.8 KB/s  TEMP —   4 degraded sources    |
+-------------------------------------------------------------------------+
```

- Default size 1360×860, minimum 1000×620, and the window opens **maximised**
  (`--windowed` keeps it at the default size).
- The navigation rail is exactly 196 px and not resizable; the content area
  absorbs all remaining width.

## 3. Navigation rail

| Element | Behaviour |
|---|---|
| Brand | Pulse glyph plus wordmark; static |
| Items | Icon, label, and a right-aligned live counter where one makes sense |
| Selection | Tinted background, 2 px accent bar on the left edge, white label |
| Hover | `#252932` fill; no layout shift |
| Footer | Computer name and account type, core/RAM summary, alert state |

Counters are hidden when they would be zero (Alerts) or meaningless
(Performance, Sensors).

## 4. Command bar

Left: page title and a one-line description of the tab.

Centre, per tab:

| Tab | Controls |
|---|---|
| Processes | Search (name or PID), Clear, "System processes", "Signatures", "Details" |
| Services | Filter by service or display name, Clear |
| Sensors | "Only above 45 °C" |
| Others | none |

Right: the sample interval (`interval 999 ms`), the process count, and the
wall-clock time from the latest snapshot.

## 5. Status strip

Always visible, 34 px tall. Left to right:

`CPU <n>%` · `RAM <n>%` · `DISK <n>%` · `NET <rate>` · `TEMP <n> °C | —`

Right side: OS name, build, uptime, and a degraded-source count with a tooltip
listing each reason. Action feedback temporarily replaces the left side for six
seconds with a green `✓` or red `✕`.

When no temperature source exists the strip shows `TEMP —` and a tooltip
explaining that the firmware exposes no ACPI thermal zone and that disk SMART
needs the elevated helper.

## 6. Processes tab

### Columns

| Column | Width | Content |
|---|---|---|
| PID | 56 | Monospaced |
| Name | 190 | Status dot + name; amber when elevated |
| Executable path | flexible (≥ 120) | Middle-truncated, monospaced; `—` when unreadable |
| CPU | 76 | Percentage plus a 26 px inline bar |
| Memory | 92 | Digits and unit rendered separately for alignment |
| I/O total | 84 | Read + written bytes accumulated |
| GPU | 76 | `—` when not measured, never `0%` |
| Threads | 52 | |
| Handles | 44 | |
| Session | 46 | Terminal Services session id |
| Signature | 98 | Only while "Signatures" is enabled |

Sorting: PID, Name, CPU, Memory. Default is CPU descending. Clicking the active
column flips direction; a new column starts at its natural extreme.

### Behaviour

- Virtualised: only visible rows are painted.
- Single click selects; double click opens the detail panel; right click opens
  the context menu on any cell.
- The status dot encodes running (green), suspended (amber), not responding
  (red) and zombie (dim). Hovering a non-running dot explains it.
- Elevated processes are drawn amber; the detail panel states elevation
  explicitly.

### Detail panel

`Panel::bottom`, resizable, default 250 px. Two columns of key/value rows:

| Left | Right |
|---|---|
| Path, command line, user, integrity, elevated, signature | CPU time, memory (private/working set/shared), I/O total, threads/handles, session, running for, started at, efficiency mode |

Actions: "Open file location", "Copy command line", "Copy PID".

`…` means "not read yet"; `—` means "the source refused or is absent". The
distinction matters: they suggest different user actions.

## 7. Performance tab

Six KPI tiles: CPU, Memory, Disk, Network, GPU, Max temp. Each carries a
segmented meter, a primary number, and a one-line context. Max temp renders `—`
and "no sensor source" when no sensor exists.

Below: two charts per row (Processor + Memory pressure, Disk activity + Network
throughput); each is a 60-point, 1 Hz sparkline with a gradient fill and a
25/50/75% grid.

Then, in two columns:

- **Logical processors** — per-core bars with a value column, plus physical
  cores, logical cores, base clock, total threads and process count.
- **Volumes** — mount point, usage bar (green < 85%, amber < 95%, red above),
  free space; hover shows label, filesystem and exact usage.

Then **Memory composition**, a two-column key/value grid (in use, available,
standby, modified, free, cached, commit, paged/nonpaged pool, pressure).

Then **Disks** and **GPUs**: per-device dot, model, media type, busy meter,
read/write rates, queue depth and average response; GPU adds utilization meter
and memory split.

Finally **Degraded sources**, one line per reason.

## 8. Sensors tab

Three KPI tiles: temperature sensor count, hottest reading, helper state
(`ACTIVE` / `NOT RUNNING` / `ERROR` / `UNKNOWN`).

Then two columns:

- **Temperature** — one row per sensor: dot, label, value right-aligned in
  monospace, confidence marker (`≈` derived, `◇` third party), a 110 °C-scaled
  meter, and a source/min/max/avg line. Empty state explains *why* nothing is
  available.
- **Provider status** — one row per provider with state and reason, then the
  helper block: status line plus "Start helper (UAC)" and
  "Install service (UAC)". There is no automatic elevation.

Then two cards: "Why CPU temperature is often missing" and "Other sensors"
(fan, voltage, power, wear) with polling interval and reading counts.

## 9. Alerts tab

When nothing is active: a calm green card stating that thresholds are evaluated
every second and listing them. When something is active: one card per alert
with severity badge, title, detail, value and threshold. Below, a **Recent
events** card with up to 20 raised/cleared entries and their timestamps.

An alert banner appears above any tab body whenever at least one alert is
active. It shows the highest-severity alert, the count of others, and a
"View alerts" button.

## 10. Services, Startup, Users

All three are virtualised tables with a one-line summary above and an
explanatory note below when a limitation applies.

- Services: name, display name, state (dot + label), startup type, PID.
- Startup: entry, source, target present/missing, 32-bit flag; hover shows the
  command line.
- Users: session id, user (`—` when unreadable), state, process count, CPU,
  memory.

## 11. Interaction summary

| Input | Result |
|---|---|
| `Ctrl+F` | Focus process search |
| `Ctrl+1`…`Ctrl+7` | Switch tabs |
| `Esc` | Close detail panel or confirmation dialog |
| Double click row | Open detail panel |
| Right click row | Context menu |

## 12. Theme

| Token | Value |
|---|---|
| Background | `#111317` |
| Panel | `#17191F` |
| Elevated surface | `#1D2027` |
| Hover | `#252932` |
| Border | `#2B3039`, strong `#3A414D` |
| Text | `#E4E8EF`, dim `#9AA3B2`, faint `#6C7585` |
| Accent | `#4C9EFF` |
| Success / warning / danger | `#4FC386` / `#E3A83E` / `#E5605A` |
| Chart series | CPU blue, RAM green, disk amber, network cyan, GPU purple |

### Type scale

Exactly five steps. Every explicit size in the UI comes from `theme::size`; ad
hoc `.size(9.0)`/`.size(10.0)` values are not allowed because five sizes inside
a 2 px range look identical but make the layout feel unsettled.

| Token | Size | Use |
|---|---|---|
| `MICRO` | 9.5 | table headers, footnotes, unit suffixes |
| `SMALL` | 10.5 | secondary labels and metadata |
| `BODY` | 12.0 | default table cell text, monospace numerals |
| `STRONG` | 12.5 | section titles, process names |
| `TITLE` | 16.0 | page title in the command bar |
| `METRIC` | 22.0 | KPI tile numerals |

Fonts: Segoe UI (prose), Consolas (numerals), and their bold faces. egui's
`RichText::strong()` only brightens the colour, so emphasis uses
`theme::bold(size)` / `theme::bold_mono(size)`, which select real bold families
and fall back to the regular face when a bold file is absent.

### Icons

Navigation icons are **painted geometry**, not glyphs. Unicode picks such as `⚙`
and `▶` are claimed by the Windows emoji font, so they rendered as full-colour
emoji next to monochrome text glyphs — two items in the rail glowed purple and
blue. Drawing the shapes gives every item identical weight and colour and needs
no font.

Status and value symbols elsewhere come from `ui::icons`, one character each:

| Constant | Glyph | Meaning |
|---|---|---|
| `DOT_ON` / `DOT_OFF` | `●` / `○` | active / inactive |
| `UP` / `DOWN` | `▲` / `▼` | raised / cleared |
| `CHECK` / `CROSS` | `✓` / `✕` | success / failure |
| `DASH` | `—` | unknown or unavailable |
| `PENDING` | `…` | not read yet |
| `APPROX` | `≈` | derived value |
| `THIRD_PARTY` | `◇` | third-party source |
| `BULLET` | `•` | prose list item |

`DASH` and `PENDING` are deliberately different: a dash means the source
refused or is absent, pending means nobody has looked yet.

## 13. Accessibility

- Colour is never the only signal: status always has a text label or a glyph.
- All numbers use a monospaced face so digits do not jitter as values change.
- Hover tooltips explain every abbreviated state.
- Contrast of body text against the panel background exceeds 7:1.
