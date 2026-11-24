🇬🇧 English | [🇵🇱 Polski](README.pl.md)

# InfinityCanvas Android

**Ready-to-install app:** [`apk/InfinityCanvas-Android.apk`](apk/InfinityCanvas-Android.apk) — download it to an Android phone (8.0+, ARM64) and install it (you need to allow installing from unknown sources).

A drawing app with an infinite canvas and unlimited zoom in and out (in the spirit of
Endless Paper). Strokes are vectors re-rendered every frame, so they never get pixelated.
The same code runs on Android and on desktop Linux.

## How it works (in short)

- **Arbitrary-precision coordinates.** Canvas positions are numbers `mantissa · 2^exponent`
  with a mantissa of any length (`crates/core/src/coords.rs`). Addition and subtraction are
  exact, so there is no zoom limit in either direction.
- **Stroke scale level.** A stroke remembers `floor(log2 zoom)` from the moment it was drawn,
  an exact anchor (an integer in units of its level) and its points as small `f32` values
  relative to the anchor.
- **View rotation** is applied only after the exact subtraction `anchor − camera`, so it does not
  reduce precision at any depth.
- **Camera-relative transform.** Every frame computes `anchor − camera` exactly, and only the result
  goes into `f32` as an instance offset + scale on the GPU. Strokes viewed at huge magnification
  (> 2^6 relative to when they were drawn) are recomputed in `f64` on the CPU and clipped to the
  screen, so edges stay sharp and still.
- **The eraser** is a stroke too (colour with alpha 0): strokes are drawn into a transparent layer
  in which the eraser trail clears pixels, and the layer is then composited over the background.
- **Spatial index** per scale level (multi-resolution hashed grid), culling of strokes that are
  off-screen, smaller than 0.5 px or astronomically larger than the screen; tessellation cache
  (lyon) with levels of detail; 4× MSAA.

## Structure

```
crates/core   GPU-free logic: coordinates, camera, strokes, index, undo/redo, saving
crates/app    wgpu renderer, gestures, egui toolbar, launchers:
              desktop.rs (winit) and android.rs (NativeActivity / android-activity)
android/      AndroidManifest.xml and resources (icon, name)
build_android.sh
```

## Requirements

- Rust (stable) – `rustup`
- Desktop: a Vulkan or OpenGL driver (Mesa is enough)
- Android: Android SDK with `build-tools`, `platforms;android-36`, NDK, `platform-tools` (adb),
  `cargo install cargo-ndk`, `rustup target add aarch64-linux-android`, Java (a JRE is enough –
  for signing the APK). Neither Gradle nor a JDK is needed.

By default the script looks for the SDK in `~/Android/Sdk` (override with `ANDROID_HOME`).

## Desktop

```bash
cargo run --release                 # projects in ~/.local/share/infinite-canvas/projects/
cargo run --release -- other.icnv   # opens one specific file
```

| Action | Control |
|---|---|
| draw | left mouse button |
| pan | right or middle button + drag; touchpad scrolling |
| zoom around the cursor | mouse wheel (Ctrl + wheel = finer); Ctrl + scroll or pinch on the touchpad |
| rotate around the cursor | Alt + mouse wheel (7.5° per notch); the angle button on the toolbar straightens the canvas |
| undo / redo | Ctrl+Z / Ctrl+Shift+Z or Ctrl+Y |
| pen / eraser | P / E |
| back to the start | Home |

## Android

1. On the phone, enable **Developer options** (Settings → About phone → tap
   “Build number” 7 times), then turn on **USB debugging** in Developer options.
2. Connect the phone with a cable and accept the “Allow USB debugging?” prompt on it.
3. Check: `~/Android/Sdk/platform-tools/adb devices` – the phone should show as `device`.
4. Build and install:

```bash
./build_android.sh              # release, installs and launches
./build_android.sh --debug      # faster build
./build_android.sh --no-install # only target/android/infinite-canvas.apk
./build_android.sh --to-files   # also copies the APK to the phone: Download/InfinityCanvas-Android.apk
```

App logs: `~/Android/Sdk/platform-tools/adb logcat -s infinite_canvas:V`

### Brushes

The button with the brush name on the toolbar opens a list – each brush has a sample of how it
draws (in the current colour, rendered by the same engine as the drawing). Every brush remembers
its own width.

| Brush | Behaviour |
|---|---|
| Pen | width follows pressure |
| Fountain pen | angled nib: width depends on the direction of movement (and pressure) |
| Dynamic pen | fast movement = thinner line |
| Fixed width | always the same width |
| Soft / Hard pencil | semi-transparent, with paper grain; pressure = darker |
| Marker | semi-transparent, chisel tip, overlapping strokes get darker |
| Dotted | dots every ~2 widths |
| Ballpoint | thin and even |
| Brush | reacts strongly to pressure, tapered ends |
| Crayon / Charcoal | coarse grain, pressure = darker |
| Highlighter | wide, transparent, chisel tip |
| Airbrush | soft, blurred edges |
| Watercolour | transparent, soft, darker rim, light texture |
| Spray | scattered dots (the same at every zoom) |
| Fill | **tap** = paint bucket: fills an area enclosed by strokes (skipping islands inside); **drag** = draws a filled shape |

The same menu has **Shape**: Freehand / Line / Rectangle / Ellipse – dragging from corner to
corner draws the shape with the current brush (with “Fill” – filled). Lines snap to multiples of
45°, an almost-square to a square, an almost-circle to a circle.

All brushes are vector-based (the pencil grain is computed in the shader in stroke coordinates),
so they stay sharp when zooming. A semi-transparent stroke does not get darker where it overlaps
itself (stencil buffer).

### Photos

**More → Photos…** → **Add a photo** shows a grid of photos from the phone's gallery (the first
time Android asks for photo access; on a computer: images from `~/Obrazy`, `~/Pictures`,
`~/Pobrane` or a file dragged onto the window). After choosing one:

- **For tracing** – the photo lies under the drawing (the eraser doesn't erase it). You can place
  it right away: one finger moves it, two fingers scale and rotate it, “Done” or “Back” finishes.
  In the Photos window: opacity slider, “Move / scale”, “Hide”, “Delete”.
- **Turn into a drawing** – two styles:
  - **Colourful**: the photo's colours are reduced to 6–20 colours (k-means) and the patches turned
    into filled vector shapes (from largest to smallest), optionally with outlines on top – the
    result is a colourful illustration,
  - **Outlines only**: the photo's edges (Canny) as strokes in the current brush, colour and width.

  The “detail” slider controls the number of colours / fine lines. Optionally a faded photo stays
  underneath. One “Undo” removes it all. The Photos window shows a thumbnail next to each photo.

The photo is saved in the project (JPEG, max. 2048 px).

### Layers

**More → Layers…**: a list of layers (the top one at the top). Tapping a name selects the layer you
draw on (the eraser, fills and drawings made from photos go there too). Each layer has opacity
(slider), “Hide/Show”, “Up/Down”, renaming and deleting (with its strokes; they can be brought back
with “Undo”). The eraser only erases on its own layer. Each layer is rendered to a separate
transparent texture and composited with its own opacity.

### Bookmarks

**More → Bookmarks…**: “Add bookmark” remembers the current view (place, zoom, rotation).
Tapping a bookmark (or “Fly”) moves the camera smoothly: zoom out, fly over, zoom in – even between
places many orders of magnitude apart, with an exact landing. Touching the screen interrupts the
flight. Bookmarks are saved in the project.

**MP4 video** (in the Bookmarks window, from 2 bookmarks): a film flying through the bookmarks in
order (a pause at each, a smooth flight between them), 768 px wide, 30 fps, screen proportions.
On the phone it is hardware-encoded (MediaCodec H.264) and saved to the gallery (Movies →
“InfinityCanvas”), on a computer via `ffmpeg` to `~/Wideo/InfinityCanvas/`. Progress is shown at
the bottom; “Back” cancels.

### Recording

**More → Record video (MP4)** records the canvas (drawing, panning, zooming – without the toolbar
and windows), 30 fps, 768 px wide. A red button in the top-right corner shows the time; tapping it
stops the recording. The film goes to the same place as bookmark videos (gallery → Movies).

### Grid

**More → Grid**: a grid attached to the canvas (it rotates with it), always 24–48 pt on screen –
when zooming out/in it switches to a sparser/denser one (every 4th line stronger), exact at every
depth. When it is on, lines, rectangles and ellipses snap their corners to its intersections. The
grid is not included in exports.

### Export

**More → Export to PNG** saves what is visible on screen (without the toolbar) as a PNG at twice
the screen resolution: on the phone to the gallery (Pictures → “InfinityCanvas”), on a computer to
`~/Obrazy/InfinityCanvas/`.

### Gestures

| Action | Gesture |
|---|---|
| draw | stylus or one finger |
| pan + zoom | two fingers (pinch around the centre of the gesture), with inertia after letting go |
| rotate | twist with two fingers (starts after ~10°, so an ordinary pinch doesn't rotate); close to upright (< 6°) the canvas straightens itself; the angle button on the toolbar – straighten |
| eraser | the “Eraser” tool on the toolbar or the eraser end of a stylus – erases exactly the eraser trail (round, vector, sharp at every zoom); strokes drawn later are not erased |
| **Move** tool (toolbar, next to the colours) | one finger / stylus pans the canvas and draws nothing; double tap + drag down/up = zoom (like in Maps) – handy for showing and recording |
| **Select** tool (toolbar, next to the colours) | circle strokes with a finger (lasso); drag the frame – move, handle in the bottom-right corner – resize; bottom bar: Delete / Duplicate / Colour (current) / Done; every change is one “Undo” |
| **Text** tool | tap the canvas, type text (several lines), set the size → letters as vector shapes in the current colour (sharp at every zoom; selecting, erasing and undo work as with strokes) |
| **Bend** tool | drag over strokes – points in the circle under your finger move with a smooth falloff towards the edge (radius – slider); works on every brush, shapes and text; one “Undo” per drag |
| **Ruler** tool | drag – length on the canvas (in pt at 100 %, in powers of ten at huge scales) and angle; with the grid on, the ends snap |
| palm rejection | after the stylus is used for the first time, fingers no longer draw – one finger pans the canvas |
| ☰ menu and quick buttons | the whole menu hides under the ☰ (three lines) button in the top-left corner – tapping it opens the menu below, ✕ closes it; while drawing an open menu folds away by itself (turn off in More → “Hide the menu while drawing”); next to ☰ there are 5 squares – quick buttons, each chosen in More → “Quick buttons…” (Pen, Eraser, Move – finger icon, Select, Text, Ruler, Undo, Redo, Home, Grid, Colour, Photos, Layers or empty) |

A stroke started with a finger is cancelled if a second finger is added within a quarter of a
second (the start of a pinch).

## Languages and first launch

On first launch a welcome screen asks for the **language** (Polish, English, German, Russian,
Ukrainian, Spanish, French, Italian; the phone / system language is preselected) and the
**device**: *Phone* (the current layout) or *Tablet* (buttons and text 25 % larger).
The choice goes to `settings.txt` in the data folder; change it in **More → Settings…** or with the
**Settings…** button in the project gallery. Translations are in `crates/app/src/i18n.rs`
(the Polish text is the key, a missing translation → Polish).

After rotating the screen the canvas adapts to the new shape right away (the centre and scale stay),
all four orientations work and the rotation lock is respected.

## Projects

The app starts with the **project gallery**: tiles with a thumbnail, name and modification date,
plus a “+ New page” tile. Tapping a tile opens the project; the **Projects** button on the toolbar
(or “Back” on Android) saves it, refreshes the thumbnail and returns to the gallery. The “•••”
button under a tile: rename, duplicate, delete (with confirmation).

Each project is a `Name.icnv` file + a `Name.png` thumbnail in the `projects/` folder
(desktop: `~/.local/share/infinite-canvas/`, Android: the app's internal storage). The thumbnail
is rendered from the vectors when leaving the project and every ~20 s while drawing.

## Saving

The drawing saves automatically ~1.5 s after every change and when the app is closed/paused.
Binary format: an `ICNV` header, a version number (u16, currently 8; older versions still load),
data in `postcard`. Saving is atomic (temporary file + rename). A damaged file is not overwritten –
it is renamed to `*.broken`.

## Tests

```bash
cargo test                      # including offscreen GPU rendering tests
cargo clippy --all-targets
cargo test --release -p infinite_canvas --test render_offscreen -- --ignored --nocapture   # 10k-stroke performance
```

The tests cover, among other things, moving the camera down to 2^±300 and back, drawing at a depth
of ~10^90 and checking pixels, 0.5 px moves without jitter, edge accuracy at 2^20 magnification,
saving/loading huge coordinates, the eraser and undo.

## License

MIT OR Apache-2.0. The projects in `ref/` (afnopadd, InfiniPaint – both GPL-3.0) served only as
inspiration; their code is neither used nor copied.
