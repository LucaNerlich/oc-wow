# Setup

## Prerequisites

* **Rust** — to build the companion (`cargo`, 1.74+).
* **OpenCode V2** — the `opencode2` CLI. The companion discovers the background
  service's URL and password from `~/.local/state/opencode/service.json`.
* **World of Warcraft** — **windowed or borderless**; exclusive fullscreen blocks
  screen capture.
* **macOS** — Screen Recording permission for the terminal you run `ocw` from
  (System Settings → Privacy & Security → Screen Recording). Restart the terminal
  after granting it.

Verify the server first:

```sh
ocw ping        # prints the URL, health and version
ocw models      # lists available models (provider/model)
```

## 1. Build

```sh
cd companion
cargo build --release
```

## 2. Install the addon and the slot bank

```sh
ocw install --addon-dir "/Applications/World of Warcraft/_classic_beta_/Interface/AddOns/OCWow"
```

This writes the Lua files, creates 200 `OCWow_Snnn` load-on-demand addons beside
them, and pre-creates the signal files (`sig/`, `ack/`, `ctl/`) **empty** — they
must exist when the client launches.

**Fully quit and relaunch WoW.** The client only discovers addon files that
existed at startup. On the AddOns screen enable **OCWow** and leave the
`OCWow Slot …` entries disabled; they are transport files, not features.

`ocw paths` shows what was detected, including the strip rectangle.

## 3. Find the strip

In game, open the panel and run:

```
/ocw test
```

That puts a strip on screen and holds it there. Then, in a terminal:

```sh
ocw probe
```

`probe` captures the screen, finds the strip by its fixed magic bytes, validates
the frame, and saves the rectangle to the config:

```
strip found: cell 4px at image (612, 148), capture scale 2
screen rectangle: 848x240 points at 282,50
saved to ~/.config/ocw/config.json
```

If it reports no strip: check the window is windowed or borderless and on screen,
that Screen Recording permission is granted, and that `/ocw test` actually drew
something (the strip is a small band of coloured squares in the game window's
top-left corner).

## 4. Run

```sh
ocw run --project ~/code/my-project --model opencode/mimo-v2.6-flash-free
```

| Flag | Meaning |
|------|---------|
| `--mock` | local echo backend; verifies the transport without a model |
| `--project DIR` | working directory the agent runs in |
| `--model provider/model` | default model |
| `--capture-cmd TEMPLATE` | custom capture command |
| `--poll-ms N` | strip sampling interval (default 250 ms) |
| `--duration N` | stop after N seconds |
| `--verbose` | log decoded frames and publishes |

In game: `/ocw`, type a prompt, press Enter. The first thing to try is
`ocw run --mock`, which echoes locally and proves the whole path.

## Updating

| Changed | Rebuild? | Re-run install? | Restart WoW? |
|---------|----------|-----------------|--------------|
| Addon Lua | no | yes, `--no-slots` | no, `/reload` |
| Wire protocol / slots | yes | yes | yes |
| Game patch | no | no | bump `## Interface:` in `OCWow.toc` |

Fast loop while editing the addon:

```sh
ocw install --from addon/OCWow --no-slots
```

## Cross-platform capture

Capture is delegated to an external command so the companion stays portable. A
template may use `{x} {y} {w} {h} {x2} {y2} {out}`, and the region is in screen
**points**.

**macOS** (default): `screencapture -x -t png -R{x},{y},{w},{h} "{out}"`

**Linux / Wayland** (`grim`): `grim -g "{x},{y} {w}x{h}" "{out}"`

**Linux / X11** (ImageMagick):
`import -window root -crop {w}x{h}+{x}+{y} "{out}"`

**Windows** (PowerShell + System.Drawing): see the `CopyFromScreen` approach in
the reference implementation; pass it with `--capture-cmd`.

The captured PNG must be 8-bit RGB or RGBA, non-interlaced.

## Troubleshooting

**`probe` finds no strip**
* `/ocw test` in game first — the strip is only on screen while a message is
  unacknowledged or after `/ocw test`.
* WoW must be windowed or borderless.
* Grant Screen Recording permission and restart the terminal.
* `--capture-cmd` if `screencapture` is not what you want.

**In game: "bridge not answering"**
* The companion is not running, or the strip is not being seen. Run
  `ocw run --verbose`: it logs `strip frame N: … bytes` when it decodes one.
* Re-run `ocw probe` if the game window moved.

**"slots missing"**
* `ocw install` was not run, or WoW has not been restarted since. The slot addons
  must exist at launch.

**"slot pool used up"**
* 200 slots are consumed by slot loads. `/reload` frees them all.

**Replies never arrive but the strip is decoded**
* `/ocw status` in game shows the signal channel and slot state.
* Check the slot addons exist (`ocw paths`).

**Encoded messages visible on screen**
* The strip is only up briefly after a send and is re-shown at most three times.
* `/ocw transport off` disables it entirely.
