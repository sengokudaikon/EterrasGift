# eg-ui

The app's view: hosts over the pages' features, and the Slint views they show. The views are `eg-ui-view`
(`crates/ui-view/ui`), a crate of their own, so a change to the Rust here doesn't compile them again; this crate re-exports
what they generate. Slint is pinned to `=1.18.1` and used under the GPLv3, as the app is; Settings' About section says
"Made with Slint" and names the bundled fonts' and icons' licences.

## A page's view

- The page's root reads its global (`ForgeState`, `TrainerState`). The components under it take what they show as
  properties and say what happened through callbacks, so each shows what it is handed and nothing else.
- What travels together is one struct (`PickerView`, `CellInput`, `CellFocus`), and the gestures of a cell or a row are one
  callback carrying an action (`cell-acted(CellRef, CellAction, float)`, `acted(RowAction)`). Handing them down a level is
  then one line each.
- Rust sets such a struct whole on every render. Slint passes it on only when it changed, and a list inside it that is the
  same model as before keeps its rows (and their focus and scroll).

## Words

How the app's words read.

- **Labels** (buttons, headers, columns, toggles, field names) are short names in sentence case, no trailing period.
  "Add mod", not "Add a mod from its DLL".
- **Tooltips** say what the control does in one short phrase, or why it is disabled. No `;` chains, no restating the
  label, no background lore. "Start offline, without mods". A tooltip may carry the shortcut in parentheses.
- **Descriptions** under a title or a setting are one or two plain sentences. Say what it does for the player first;
  caveats only when they change what the player does.
- **Status and errors** are sentences: what happened, then what to do if anything. One of each at most.
- **No ` · ` joins.** Separate facts are separate text elements laid out side by side or stacked (a model row gets
  one field per fact), never one string glued with a middle dot. Ranges use an en dash; multiplication uses ×.
- Name things the way the game does (affix, tier, seal, Weaver's Will, legendary potential) and the way Windows does
  (folder, File Explorer, Settings).
- Avoid: "so that", "once", "the next time you play with mods" repeated on every control; say it once on the page.

## Slint: known pain points

- **Churn:**
  - The MSRV rises nearly every release.
  - `KeyEvent` became `#[non_exhaustive]` in a patch release (1.16.1).
  - `viewport-*` was renamed to `content-*`.
  - The 1.19 layout fix creates new binding-loop errors in existing code (#13972).
  - The testing crate must be pinned to an exact version.
- **Text:** vertical-centering regression since 1.16 (#11589, reported by LibrePCB); no OpenType features (#11960);
  text jitter fixed only in 1.19 (#13146).
- **Popups and tooltips:** clipped to the window on winit; positioned only near the pointer; native menus on Windows
  can't be themed.
- **Lists:** virtualization fights keyboard focus and accessibility (#12645, #13425).
- **Input:** no mouse capture or filter phase (#10702); no pointer delta (#13189).
- **Missing platform features:** file dialogs (#9781), Mica (#4121), reduced-motion and high-contrast detection
  (#13630).
- **Build:** prefer `build.rs` over the `slint!` macro, which re-parses the std widgets on every expansion (#12096).

## Slint quirks met here (1.18.1)

Each of these was hit and worked around in `crates/ui-view/ui`.

- **Inside a component that has a `max` property, `max(...)` fails** with "The expression is not a function". Use
  `Math.max`.
- **Wrapped text inside `if` or `for` makes a layout's `preferred-height` (and `min-height`) too small.** The repeated
  items are counted as one line each, but they still lay out and draw at their real height. A `ScrollView` sized from
  the layout therefore stops short of the end. Probe: a `for` of two 3-line texts reports 28 px instead of 84 px.
  - Workaround: put a zero-height marker right after the repeater, and bind `content-height` to the sum of the `y`
    positions up to that marker (`pages/settings.slint`).
  - Give the body a height of its own (`height: self.preferred-height`). If its height follows the content height,
    the marker binding becomes a compile-time binding loop.
  - Worth reporting upstream.
- **A component whose root has no base element gets zero size; it does not fill its parent.**
  - A focus ring drawn from such a root became a 4×4 px dot.
  - A tooltip trigger anchored to the centre of the control.
  - Fix: inherit `Rectangle`, which fills its parent.
- **A component root cannot refer to `parent`.** Put the geometry in a child element instead.
- **A component that inherits `PopupWindow` fails anywhere under an `if` or a `for` in the same component**, with
  "PopupWindow cannot be directly repeated or conditional", though a plain `PopupWindow` is fine there. From outside the
  popup, nothing can call its functions or read what is inside it (such as a `FocusScope`'s `has-focus`). So
  `OptionMenu` is the popup's content, and the caller keeps the `PopupWindow`.
- **`padding-*` are built-in property names on every element**, not only on layouts: `in property <length>
  padding-left` fails with "Cannot override property". `OptionMenu` names its own `inset-*`.
- **A callback named `start` hides the `start` enum value inside its component**: `alignment: start` then fails with
  "Callback must be called". Name it something else (`OptionMenu`'s is `initial`).
- **`row` is a built-in property name** (grid placement), so `in property <NavRow> row` fails with "Cannot override
  property". Other names that are taken: `col`, `x`, `y`.
- **Inside a `PopupWindow`, `absolute-position` is measured from the popup**, and the window's own elements draw under
  every popup. A window-level tooltip raised from a flyout landed at the window's top-left, then (offset) under the
  flyout. Tips raised in a flyout set `Tip.in-flyout`, and the flyout draws the bubble as its last child.
- **A `PopupWindow` needs an explicit width and height.** Elements inside it can't be read from outside (#4438), and
  without a size the popup is placed wrongly.
- **Window-level `capture-key-pressed` only runs once something has focus.** The first Tab is invisible to it, so each
  focusable widget also turns keyboard mode on in `focus-gained(FocusReason.tab-navigation)`.
- **`image-fit` must be set to `contain` on icons.** The icon SVGs use the square round their path's bounds as the
  view box. With the bare bounds (17×16 for the warning mark), the capture renderer drew a 12–14px
  icon a pixel taller than its box and cut off its foot (found by the 2026-10-20 parity audit).
- **`animation-tick()` in a binding repaints every frame while that binding is live.** The page entrance reads it only
  while `entering` is true, and a `Timer` ends that state.
- **Wheel scrolling in the headless window works with `WindowEvent::PointerScrolled`.**
- **No opacity masks.** An image can't be faded by a gradient or clipped to a soft circle. Eterra's band (its reveal from
  the eye and its dissolving ends) is composited in Rust into a `SharedPixelBuffer` and shown as an `Image`
  (`crates/ui/src/decorations.rs`). In a debug build that takes about 21 ms per frame for the 1536×304 cut, against
  about 1 ms in release.
- **Painting the ground colour over the ground isn't a no-op in the software renderer.** A ground-to-transparent gradient
  over the ground came out one level darker (13 against 14), which left faint boxes on the screen. Fade the image
  itself instead.
- **`line-height-factor` (1.18) does not shrink a single-line `Text`'s box.** Setting Cinzel 28px to a 34px line
  (factor = 34 / (ascent − descent), measured on a hidden probe) moved the glyphs up 2px but left the preferred height
  as it was, so titles still sit about 4px taller than a 34px line. Reading `font-metrics` on the same `Text` that sets
  its `line-height-factor` panics with "Recursion detected". Left as is.
- **A `PopupWindow` as a child of a component instance without `@children` panics the 1.18.1 Rust generator.** Wrap it
  in a plain `Rectangle` (`pages/forge/`).
- **A hidden layout used only to measure widths still constrains its parent** unless its children set preferred sizes
  rather than fixed ones.
- **`FlexboxLayout` reports a taller preferred height than it lays out**, so the forge's status strip measures its
  one-line width separately.
- **Transforms (`transform-scale-*`, rotation) are not available in the software renderer**, so captures can't show
  them; an elliptical gradient is an SVG instead (`crates/ui-view/ui/gradients/`).
- **Slint surfaces no window activation**, so the Windows settings it can't read itself ("Animation effects", high
  contrast) are polled every 2 s (`eg_ui::follow_system_settings`, started by the app and not by captures).
- **`slint-viewer` can't run headless with MCP.** `SLINT_BACKEND=headless`
  fails with "headless backend requested but it is not available".
  - This holds for the prebuilt binary and for `cargo install --features mcp`.
  - The viewer always picks its backend through `BackendSelector::select()`. `BackendSelector` has no `headless`
    arm; only the default platform path (`i-slint-backend-selector` `lib.rs`) has one. Master is the same, and no
    issue was filed as of 2026-10-17.
  - An app that lets Slint pick its platform does get it: `mise run live` works headless.
  - The viewer's MCP server works in a window (`mise run live:viewer`), and `--screenshot` stays headless.
- **The live app's MCP build** needs `SLINT_EMIT_DEBUG_INFO=1` at build time (element IDs) and `--features slint/mcp`
  on the command line, never in `Cargo.toml`. It builds into `target/mcp` so the normal cache survives.
  - `get_element_tree` takes an `elementHandle` (the window's `rootElementHandle`), not the `windowHandle`.
- **The standard ScrollView's thumb is nearly invisible on this ground**, so `widgets/scroll-view.slint` replaces it
  with a Fluent-style scroll bar (Flickable plus a drawn bar). A component named `ListView` that has `content-y`,
  `content-height`, `content-width`, `visible-height` and `visible-width` lengths keeps the compiler's virtualised rows
  (`object_tree.rs` checks the name and those properties), so the picker's `ListView` inherits the new `ScrollView`.
- **These worked first time:**
  - typing into a `TextInput` through `WindowEvent::KeyPressed`;
  - `two-way` (`<=>`) bindings from a toggle to a global;
  - `changed <prop> =>` handlers;
  - repeated `PopupWindow`s.
