# Views

Raw views are a tree. A view can handle events and render itself. Events travel
from the root to the leaves; a child can send an event back to its parent, and
an event that reaches the root without being captured is broadcast later. The
module comment in [`mod.rs`](mod.rs) is the rule for z-order and that walk.

Slint screens do not use that tree. Cadmus drives a generated window from the
view loop, then blits the captured frame. [`SlintHost`](slint/host.rs) owns the
session, Back/Home/menu, timers, and resize. A screen implements
[`SlintScreen`](slint/host.rs). Shared chrome is `InkWindow` plus `InkScaffold`;
screen size lives on `InkScreen`.

## Where a Slint piece lives

`ui/cadmus.slint` is the only compile root. It re-exports every component,
global, and struct Rust names.

Shared widgets live in `ui/widgets/`. A widget takes `in` properties and
callbacks. It does not know which screen placed it.

A screen's markup lives in its own folder. Settings is `ui/settings/`. Startup
is `ui/startup/`. The four shared overlays (option list, confirmation dialog,
file browser, text dialog) and the `*Io` globals that bind them live together in
`ui/widgets/overlays.slint`. `ui/cadmus.slint` re-exports those globals.
`ui/settings/window.slint` uses the overlays and does not export the globals
again, so a second screen imports them from the widgets file instead of
redeclaring them.

The confirmation dialog is a centered card, like the option list. A tap on the
page outside the card closes it. Download, re-download, and force-import
questions use that card.

Every screen draws its chrome with `InkScaffold`: the app bar, the back
button, the rule under it, and the body. A screen that is one message, like
the startup frame, uses `InkWindow` alone and has no app bar.

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
- [`Asked`](slint/overlay.rs) is a sheet and the control that opened it. The
  reason belongs with the sheet, so a screen does not keep a second list of
  what it is waiting for. Closing the sheet forgets the reason.
- [`settings_editor`](settings_editor/) is the settings screen. Category pages,
  the shell, the footer, the library editor, and the refresh-rate editor stay
  here. They edit settings data. A callback returns
  [`Step`](settings_editor/components/mod.rs), which names the sheet to open or
  the result to apply. `Waiting` names the control that opened the shared
  sheet, and the sheet keeps it. [`Flow`](slint/mod.rs) is only the renderer
  instruction (`Idle` or `Redraw`); `dispatch` reduces `Step` to it.
- [`ota`](ota/) is the Check for Updates screen. List, auth, and progress are
  components. It reuses the same overlays and host as settings.
- Another screen gets its own folder under `view/` and under `ui/`. It reuses
  the overlays by constructing them with its own row type and window. It
  does not grow a second copy of the file browser or the confirmation dialog.

[`Component`](slint/component.rs) is the callback loop: `wire`, `take`, `sync`,
`apply`. `Window` and `Effect` are associated types. Settings components return
`Step`. The shared overlays return their own outcome, and a `From` impl turns
that into `Step`. The future is `!Send` because a screen stays on the thread that
polls the view tree.

Sync only what is on screen. Settings copies the shell, the visible category
page, and the open overlays into the window. Models go through
[`changed_model`](slint/model.rs). A list of the same length is updated in
place, so one changed row does not rebuild the others or drop their widget
state. A different length replaces the model.

## Lists and paging

One list type and one pager. The shared list is
[`PagedList`](../../ui/widgets/paged_list.slint): it draws `PagedLine` rows and
owns the page. The settings pages, the file browser, the refresh-rate editor,
and the OTA list all use it. A row is built only when it is on the visible
page, so a long list does not build every row.

The page count and the clamp live in one place,
[`Paging`](../../ui/widgets/paging.slint). `shown-page` is the page the list
draws; it is never past the last page. The raw page follows it, one event-loop
turn later, because a Slint change callback runs after the frame. A list that
moves its page moves it from `shown-page`, so the raw page never drifts.

Rust moves the page with [`PagedIo`](slint/page.rs). The trait gives the
window's page and moves it by a delta, so the hardware page buttons and the
list's own arrows move the same page. The page itself lives in the window or
in the overlay's global; Rust reads it, it does not keep a copy.

A row is a [`ListRow`](slint/model.rs) in Rust. The fields are named, so a
call site says what it means: a row that cannot be tapped is `disabled`, and a
row that is still being checked is `busy`. A settings row also has an identity:
[`SettingRow`](settings_editor/components/setting.rs) names the setting, and
the page that shows it maps the tapped index back to that name. A library or
dictionary row is its index in the model.

The window size has one source: `InkScreen.window-height`. A list that needs
to know how many rows fit takes the height it may use, and the shared list
turns that into a page size. The scaffold reports the chrome it puts above the
body (`chrome-height`), so a screen does not re-derive the app bar.

Slint timers (press-and-hold) need the view loop awake. The host forwards one
event, applies queued callbacks, and queues dirty rectangles; its timer path
does that for `Event::SlintTimer`. Both call `Session::wake_for_timers`, which
posts that event when Slint's next timer is due. The wait is a detached task,
not a [`Job`](../runtime.rs): there is nothing to cancel, and a late event is
ignored once the screen is gone. Handle the event with `Session::tick`. Do not
add per-widget workarounds for a hold that fires late.

Settings uses the host for every event except the category and page buttons.
The press that changes the category or the page remembers that button and
swallows only its release and repeats. Forwarding that release turns it into
an arrow key and opens the first row. A button whose press was forwarded is
released normally, so the keyboard is not left held down.

The host owns the frame loop. A screen only says how a frame reaches the
panel: its `refresh` hook fills a [`RefreshPlan`](slint/host.rs) with regions
and their waveform, and the host queues them. A progress bar reports the
rectangle it occupies in the `InkBar` global. The screen scales that rectangle
to physical pixels with [`pixel_scale`](slint/metrics.rs) and hands it to
[`FastInk`](slint/waveform.rs), which keeps the physical one. The text dialog's keyboard is as
tall as the reader keyboard and sits just above the bottom edge. The field
floats above it. A pressed key is drawn black for that finger-down frame. The
dialog decides that refresh (`frame_refresh`, `skip_press_frame`); the host
does not pick the waveform. The key caps come from the selected keyboard
layout, including Russian, terminal, and any layout loaded from disk. Holding
a character key opens the accented forms of that character above the keys. The
combine key is not drawn.

## The host and the screen

[`SlintHost`](slint/host.rs) owns the session, the event loop, the resize, and
the panel updates. A screen implements [`SlintScreen`](slint/host.rs):

- `open` and `wire` build the window and connect its callbacks.
- `start` runs once, before the first sync.
- `sync` copies the screen's state into the window. The host redraws
  afterwards, so a screen never captures a frame itself.
- `apply_callbacks` applies what the window queued and returns [`Flow`].
- `consume_back`, `on_event`, and `take_device` are the screen's own
  shortcuts. The screen that takes an event says so, and the host syncs.
- `refresh` plans how the dirty rectangles reach the panel.
- `stop_jobs` and `take_background_jobs` hand the screen's jobs over.

[`OverlayStack`](slint/overlay.rs) is the list of open sheets, front-most
first. It closes the top sheet and moves the page of the top sheet that pages.
A sheet that is closed does not rebuild its rows.

[`PagerBar`](../../ui/widgets/pager_bar.slint) draws the page arrows and the
page label. It emits callbacks; the list moves the page.

Tests use the harness in [`slint/testing.rs`](slint/testing.rs):
`install_ui_font` writes the app fonts, `open_screen` mounts a screen on a test
device, and `RecordingFramebuffer` records the panel updates.

A screen lists its pieces once. Settings uses `each_part` in
[`settings_editor/components`](settings_editor/components/mod.rs) to wire, sync,
and drain them. Add a piece to that list rather than repeating the walk.

## Translations

Static copy drawn by a `.slint` file is `@tr("English")`. `cadmus-translate`
extracts those strings into `ui/lang`. Crowdin owns the translations. Do not
hand-edit the `.po` files. The category names and the file chooser's "Up" and
"Select this folder" stay `@tr`. They have no Fluent twin.

Strings chosen at runtime, including setting rows and notifications, use
`fl!`. A static category page is a row model: the label is
[`SettingKind::label`](settings_editor/kinds/mod.rs), not a second `@tr`
string. Cancel and OK on the confirmation dialog and the text dialog, and the
library and refresh-rate labels that already have `fl!` keys, are set from
Rust (`cancel`, `ok`, and the `settings-*` keys). The rules are in
[`i18n/AGENTS.md`](../../i18n/AGENTS.md).

## Widgets

Do not import the upstream Material component library. Copy a widget into
`ui/widgets/` and change it for this panel. Glyphs copied from Material keep
the MIT notice in [`ui/icons`](../../ui/icons/MATERIAL-LICENSE.md).

A layout length is dp: `Npx * InkMetrics.dp` from
[`ui/widgets/ink/metrics.slint`](../../ui/widgets/ink/metrics.slint). One dp
is 1/160 inch. Screen size and the reader-keyboard block stay logical pixels.
Rust writes those from the framebuffer and from the reader keyboard.

`InkMetrics.hairline`, `frame`, `radius`, `mark`, and `mark-radius` are logical pixels too. Rust writes a length that the framebuffer scale turns into a whole physical pixel. Slint's software renderer truncates a border to whole physical pixels and rounds a rectangle's position, so anything else draws an uneven stroke.

## Tooling

`devenv` links the upstream Slint skill at `.agents/skills/slint`. It writes
`.cursor/mcp.json` when that file is absent, and it writes `opencode.jsonc`
with the same two servers. None of those files is committed. treefmt excludes
the skill directory and those two files. A tree walk also skips the symlink.
`cadmus-slint-view` previews the settings window and serves the viewer's MCP
endpoint on `127.0.0.1:9315`. That server is disabled until the preview is
running. The same config includes the Slint docs MCP, which stays enabled.
