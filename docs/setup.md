# Setup

## Prerequisites

* **Rust** — to build the companion (`cargo`, 1.74+).
* **OpenCode V2** — the `opencode2` CLI. The companion talks to the background
  service over HTTP; it discovers the URL and password from
  `~/.local/state/opencode/service.json` (written by the service). You do not
  need to start a server yourself, but the service must be running and you must
  have authenticated any providers whose models you want to use.
* **World of Warcraft** — windowed or borderless, with the strip visible.
* **Python 3** — only for the optional development tools.

Verify the server first:

```sh
ocw ping        # prints the URL, health and version
ocw models      # lists available models (provider/model)
ocw projects    # lists projects the server already knows about
```

## 1. Build

```sh
cd companion
cargo build --release
```

The resulting binary is `companion/target/release/ocw`.

## 2. Install the addon

```sh
ocw install --addon-dir "/Applications/World of Warcraft/_classic_beta_/Interface/AddOns/OCWow"
```

This writes the Lua files and creates the font bank (`Fonts/fontreplyNNNN.ttf`,
4096 slots by default, hard-linked to one baseline font). Use `--slots` to
change the size and `--force` to rewrite existing slots — **never** use
`--force` while the game is running.

The addon directory is auto-detected for common WoW installations on macOS,
Windows and Linux (Proton). `ocw paths` shows what was detected.

Enable the addon in game, then `/reload`. If the client does not discover the
new font files, restart it fully once.

## 3. Calibrate the strip

The addon draws the strip in the top-left corner of the UI. Its screen position
depends on your window position, monitor layout and UI scale.

In game:

```
/ocw calibrate
```

This paints a fixed, valid frame so the strip is easy to find. Then:

```sh
ocw probe
```

`probe` captures your configured region (whole screen by default on macOS) and
searches for the strip, printing a suggested crop:

```
found strip: cell=4px, offset=(12, 34), frame type=control
suggested: --crop 20,42,512,16
```

Save that crop in your config or pass it to `run`:

```sh
ocw run --crop 20,42,512,16 --cell 0
```

`--cell 0` auto-detects the cell size from the captured width, which handles
Retina and scaled displays. Run `/ocw calibrate` again to stop the fixed frame.

### macOS permissions

`probe` and `run` need **Screen Recording** permission for the terminal
application you run them from (Terminal, iTerm, VS Code, …). Grant it in
System Settings → Privacy & Security → Screen Recording, then restart the
terminal. Without it, `screencapture` fails or returns a blank image.

## 4. Run

```sh
ocw run --project ~/code/my-project --model opencode/mimo-v2.6-flash-free
```

Useful flags:

| Flag | Meaning |
|------|---------|
| `--mock` | local echo backend; verifies the transport without a model |
| `--project DIR` | default project directory for new sessions |
| `--model provider/model` | default model |
| `--crop x,y,w,h` | strip location (`full` for the whole screen) |
| `--cell N` | cell size in pixels (`0` = auto) |
| `--capture-cmd TEMPLATE` | custom capture command |
| `--poll-ms N` | strip sample interval (default 120 ms) |
| `--duration N` | stop after N seconds |
| `--verbose` | log decoded frames and slot writes |

In game, open the panel with `/ocw` and start typing.

## Cross-platform capture

Capture is delegated to an external command so the companion stays portable. A
template may use `{x} {y} {w} {h} {x2} {y2} {out}`.

**macOS** (default):

```
screencapture -x -t png -R{x},{y},{w},{h} "{out}"
```

**Linux / Wayland** (requires `grim`):

```
grim -g "{x},{y} {w}x{h}" "{out}"
```

**Linux / X11** (ImageMagick):

```
import -window root -crop {w}x{h}+{x}+{y} "{out}"
```

**Windows** (PowerShell + System.Drawing):

```
powershell -NoProfile -Command "Add-Type -AssemblyName System.Drawing; \
$b=New-Object Drawing.Bitmap {w},{h}; \
$g=[Drawing.Graphics]::FromImage($b); \
$g.CopyFromScreen({x},{y},0,0,$b.Size); \
$b.Save('{out}',[Drawing.Imaging.ImageFormat]::Png)"
```

Pass it with `--capture-cmd '...'`. The captured PNG must be 8-bit RGB or RGBA,
non-interlaced.

## Updating

`ocw install` deploys the addon; the sources are embedded in the binary so it
works from anywhere. What to do depends on what changed:

| Changed | Rebuild? | Font bank? |
|---------|----------|------------|
| Addon Lua / TOC | no (`--from`) | no (`--no-bank`) |
| Wire protocol / font format | yes | yes (`--force`) |

**Editing the addon** (fastest loop, no rebuild):

```sh
ocw install --from addon/OCWow --no-bank
```

**Normal update:**

```sh
cargo build --release
./target/release/ocw install
```

Then `/reload` in game.

**If the font bank changed** (only after editing `companion/src/fonts/`):

```sh
# with the game CLOSED
ocw install --force
```

`--force` rewrites all slots. Never run it while the client is running: the
client caches font files on first use, and a slot it has already loaded cannot
be changed safely.

**After a game patch**, update `## Interface:` in `addon/OCWow/OCWow.toc` to the
new build number (currently `16001`), or the addon is flagged out of date.

## Troubleshooting

**`no strip found`**
* Run `/ocw calibrate` in game first.
* Make sure the WoW window is not fullscreen-exclusive and the strip is not
  covered by another window.
* Grant Screen Recording permission and restart the terminal.
* Try `ocw probe --crop full`.

**`transport stalled` in game**
* The companion is not running, or its crop is wrong.
* Check `ocw run --verbose`: it logs each decoded frame and slot write.
* If the strip is on a second monitor, recalibrate with the window where it is.

**`font bank exhausted`**
* Restart the game client and the companion. The companion detects the new
  process and rewinds the counter. Increase `--slots` if this happens often.

**Answers never arrive but prompts do**
* The inbound channel is the empirical part. Confirm the font bank exists at
  `ocw paths`, and that `/reload` happened after installation.
* `ocw dump --slot N` decodes a slot; `tools/validate_font.py` checks a font
  against macOS CoreText.

**`ocw ping` fails**
* The OpenCode service is not running: `opencode2 service status`.
* `ocw paths` prints the discovered service URL.
