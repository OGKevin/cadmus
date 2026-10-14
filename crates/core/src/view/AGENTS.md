# Views

Raw views are a tree. A view can handle events and render itself. Events travel
from the root to the leaves; a child can send an event back to its parent, and
an event that reaches the root without being captured is broadcast later. The
module comment in [`mod.rs`](mod.rs) is the rule for z-order and that walk.

Slint screens do not use that tree. Cadmus drives a generated window from the
view loop, then blits the captured frame. The shared host is [`slint`](slint/):
opening a session, forwarding device events, and the callback queue.

## Where a Slint piece lives

`ui/cadmus.slint` is the only compile root. It re-exports every component,
global, and struct Rust names.

Shared widgets live in `ui/widgets/`. A widget takes `in` properties and
callbacks. It does not know which screen placed it.

A screen's markup lives in its own folder. Settings is `ui/settings/`. Startup
is `ui/startup/`. The four shared overlays (option list, confirmation dialog,
file browser, text dialog) and the `*Io` globals that bind them live together in
`ui/widgets/overlays.slint`, so a second screen imports them instead of
redeclaring them.

The confirmation dialog is a centered card, like the option list. A tap on the
page outside the card closes it. Download, re-download, and force-import
questions use that card.

Rust follows the same split:

- [`slint/components`](slint/components/) holds reusable overlay state: the
  option list, the confirmation dialog, the file browser, and the text dialog.
  The option list is generic over the row id. Every overlay is generic over the
  window that draws it, bounded by `W: ComponentHandle + 'static` and
  `for<'a> XxxIo<'a>: slint::Global<'a, W>`. Overlays read and write those
  globals directly. Settings constructs `ChoiceSheet<EntryId, SettingsWindow>`
  (and `FileBrowser<SettingsWindow>`, `TextSheet<SettingsWindow>`,
  `ConfirmSheet<SettingsWindow>`). An overlay does not store which control
  opened it. `WindowOf<W>` only puts `W` in the overlay's type.
- [`settings_editor`](settings_editor/) is the settings screen. Category pages,
  the shell, the footer, the library editor, and the refresh-rate editor stay
  here. They edit settings data. A callback returns
  [`Step`](settings_editor/components/mod.rs), which names the sheet to open or
  the result to apply. `Waiting` remembers which control opened the shared
  sheet. [`Flow`](slint/mod.rs) is only the renderer instruction (`Idle` or
  `Redraw`); `dispatch` reduces `Step` to it.
- The next screen gets its own folder under `view/` and under `ui/`. It reuses
  the overlays by constructing them with its own row type and window. It
  does not grow a second copy of the file browser or the confirmation dialog.

[`Component`](slint/component.rs) is the callback loop: `wire`, `take`, `sync`,
`apply`. `Window` and `Effect` are associated types. Settings components return
`Step`. The shared overlays return their own outcome, and a `From` impl turns
that into `Step`. The future is `!Send` because a screen stays on the thread that
polls the view tree.

Sync only what is on screen. Settings copies the shell, the visible category
page, and the open overlays into the window. Models go through
[`changed_model`](slint/model.rs), which keeps the current model when the rows
did not change, so an unchanged list is not re-set and does not dirty the frame.

Slint timers (press-and-hold) need the view loop awake. After input and after
each tick the screen calls `Session::wake_for_timers`, which sends
`Event::SlintTimer` when Slint's next timer is due. Handle that event with
`Session::tick`. Do not add per-widget workarounds for a hold that fires late.

The text dialog's keyboard is as tall as the reader keyboard and sits just
above the bottom edge. The field floats above it. A pressed key is drawn
black for that finger-down frame. The dialog decides that refresh
(`frame_refresh`, `skip_press_frame`); the host does not pick the waveform.
The key caps come from the selected keyboard layout, including Russian,
terminal, and any layout loaded from disk. Holding a character key opens
the accented forms of that character above the keys. The combine key is
not drawn.

A screen lists its pieces once. Settings uses `each_part` in
[`settings_editor/components`](settings_editor/components/mod.rs) to wire, sync,
and drain them. Add a piece to that list rather than repeating the walk.

## Translations

Static copy drawn by a `.slint` file is `@tr("English")`. `cadmus-translate`
extracts those strings into `ui/lang`. Crowdin owns the translations. Do not
hand-edit the `.po` files.

Strings chosen at runtime, including setting rows and notifications, use
`fl!`. The rules are in [`i18n/AGENTS.md`](../../i18n/AGENTS.md).

## Widgets

Do not import the upstream Material component library. Copy a widget into
`ui/widgets/` and change it for this panel. Glyphs copied from Material keep
the MIT notice in [`ui/icons`](../../ui/icons/MATERIAL-LICENSE.md).

## Tooling

`devenv` links the upstream Slint skill at `.agents/skills/slint`. It writes
`.cursor/mcp.json` when that file is absent, and it writes `opencode.jsonc`
with the same two servers. None of those files is committed. treefmt excludes
the skill directory and those two files. A tree walk also skips the symlink.
`cadmus-slint-view` previews the settings window and serves the viewer's MCP
endpoint on `127.0.0.1:9315`. That server is disabled until the preview is
running. The same config includes the Slint docs MCP, which stays enabled.
