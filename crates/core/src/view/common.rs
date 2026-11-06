use super::menu::{Menu, MenuKind};
use super::notification::Notification;
use super::{AppCmd, EntryId, EntryKind, RenderData, RenderQueue, View, ViewId};
use crate::device::AppContext;
use crate::device::DeviceHardware as _;
use crate::device::battery::Battery as _;
use crate::device::{DeviceCapabilities as _, DevicePaths as _, DeviceRotation as _};
use crate::fl;
use crate::framebuffer::Framebuffer as _;
use crate::framebuffer::UpdateMode;
use crate::geom::{Point, Rectangle};
use crate::settings::{ButtonScheme, RotationLock, RotationMode};
use chrono::Local;

pub fn shift(view: &mut dyn View, delta: Point) {
    *view.rect_mut() += delta;
    for child in view.children_mut().iter_mut() {
        shift(child.as_mut(), delta);
    }
}

pub fn locate<T: View>(view: &dyn View) -> Option<usize> {
    for (index, child) in view.children().iter().enumerate() {
        if child.as_ref().is::<T>() {
            return Some(index);
        }
    }
    None
}

pub fn rlocate<T: View>(view: &dyn View) -> Option<usize> {
    for (index, child) in view.children().iter().enumerate().rev() {
        if child.as_ref().is::<T>() {
            return Some(index);
        }
    }
    None
}

pub fn locate_by_id(view: &dyn View, id: ViewId) -> Option<usize> {
    view.children()
        .iter()
        .position(|c| c.view_id().map_or(false, |i| i == id))
}

pub fn overlapping_rectangle(view: &dyn View) -> Rectangle {
    let mut rect = *view.rect();
    for child in view.children() {
        rect.absorb(&overlapping_rectangle(child.as_ref()));
    }
    rect
}

// Transfer the notifications from the view1 to the view2.
pub fn transfer_notifications(
    view1: &mut dyn View,
    view2: &mut dyn View,
    rq: &mut RenderQueue,
    context: &mut AppContext,
) {
    for index in (0..view1.len()).rev() {
        if view1.child(index).is::<Notification>() {
            let mut child = view1.children_mut().remove(index);
            if view2.rect() != view1.rect() {
                let (tx, _rx) = crate::view::hub_channel();
                child.resize(*view2.rect(), &tx, rq, context);
            }
            view2.children_mut().push(child);
        }
    }
}

/// Recursively searches the view tree for a notification with the given ViewId.
///
/// # Arguments
///
/// * `view` - The root view to start searching from
/// * `id` - The ViewId to search for
///
/// # Returns
///
/// A mutable reference to the Notification if found, or `None` if not found.
///
/// # Note
///
/// This function performs a depth-first search through the entire view hierarchy.
/// It will find the first notification that matches the given id.
pub fn find_notification_mut(view: &mut dyn View, id: ViewId) -> Option<&mut Notification> {
    if view.is::<Notification>() && view.view_id() == Some(id) {
        return view.downcast_mut::<Notification>();
    }

    for child in view.children_mut() {
        if let Some(notif) = find_notification_mut(child.as_mut(), id) {
            return Some(notif);
        }
    }

    None
}

/// Entries of the main menu's Rotation submenu.
///
/// The gyro auto modes (when the device has a gyroscope), the fixed angles and
/// the button scheme (when the device has page turn buttons) form separate
/// radio groups separated by dividers. A fixed angle implies ignoring the
/// gyroscope; without a gyroscope the lock is vestigial and the current angle
/// is the active option.
fn rotation_entries(context: &AppContext) -> Vec<EntryKind> {
    let mut entries = Vec::new();
    let has_gyroscope = context.device.has_gyroscope();

    if has_gyroscope {
        let lock = context.settings.rotation_lock;
        for (label_id, mode, active) in [
            (
                fl!("top-menu-rotation-auto"),
                RotationMode::Auto,
                lock.is_none(),
            ),
            (
                fl!("top-menu-rotation-auto-portrait"),
                RotationMode::AutoPortrait,
                lock == Some(RotationLock::Portrait),
            ),
            (
                fl!("top-menu-rotation-auto-landscape"),
                RotationMode::AutoLandscape,
                lock == Some(RotationLock::Landscape),
            ),
        ] {
            entries.push(EntryKind::RadioButton(
                label_id.to_string(),
                EntryId::SetRotation(mode),
                active,
            ));
        }
        entries.push(EntryKind::Separator);
    }

    let rotation = context.device.to_canonical(context.display.rotation);
    for n in 0..4 {
        let fixed = context.device.to_native(n);
        let locked =
            !has_gyroscope || context.settings.rotation_lock == Some(RotationLock::Current);
        let degrees: i16 = (n as i16) * 90;
        entries.push(EntryKind::RadioButton(
            fl!("settings-rotation-angle", degrees = degrees).to_string(),
            EntryId::SetRotation(RotationMode::Fixed(fixed)),
            locked && n == rotation,
        ));
    }

    if context.device.has_page_turn_buttons() {
        let button_scheme = context.settings.button_scheme;
        entries.push(EntryKind::Separator);
        entries.extend(button_scheme_entries(button_scheme));
    }

    entries
}

/// Button scheme radio group of the main menu's Rotation submenu.
fn button_scheme_entries(button_scheme: ButtonScheme) -> Vec<EntryKind> {
    [
        (
            fl!("settings-rotation-button-scheme-natural"),
            ButtonScheme::Natural,
        ),
        (
            fl!("settings-rotation-button-scheme-inverted"),
            ButtonScheme::Inverted,
        ),
    ]
    .into_iter()
    .map(|(label, scheme)| {
        EntryKind::RadioButton(
            label.to_string(),
            EntryId::SetButtonScheme(scheme),
            button_scheme == scheme,
        )
    })
    .collect()
}

pub fn toggle_main_menu(
    view: &mut dyn View,
    rect: Rectangle,
    enable: Option<bool>,
    rq: &mut RenderQueue,
    context: &mut AppContext,
) {
    if let Some(index) = locate_by_id(view, ViewId::MainMenu) {
        if let Some(true) = enable {
            return;
        }
        rq.add(RenderData::expose(
            *view.child(index).rect(),
            UpdateMode::Gui,
        ));
        view.children_mut().remove(index);
    } else {
        if let Some(false) = enable {
            return;
        }

        let apps = vec![
            EntryKind::Command(
                "Dictionary".to_string(),
                EntryId::Launch(AppCmd::Dictionary {
                    query: "".to_string(),
                    language: "".to_string(),
                }),
            ),
            EntryKind::Command(
                "Calculator".to_string(),
                EntryId::Launch(AppCmd::Calculator),
            ),
            EntryKind::Command("Sketch".to_string(), EntryId::Launch(AppCmd::Sketch)),
            EntryKind::Separator,
            EntryKind::Command(
                "Touch Events".to_string(),
                EntryId::Launch(AppCmd::TouchEvents),
            ),
            EntryKind::Command(
                "Rotation Values".to_string(),
                EntryId::Launch(AppCmd::RotationValues),
            ),
        ];
        let mut entries = vec![
            EntryKind::Command("About".to_string(), EntryId::About),
            EntryKind::Command("System Info".to_string(), EntryId::SystemInfo),
            EntryKind::Command(
                "Settings".to_string(),
                EntryId::Launch(AppCmd::SettingsEditor),
            ),
            EntryKind::Command("Check for Updates".to_string(), EntryId::CheckForUpdates),
            EntryKind::Separator,
        ];

        entries.push(EntryKind::SubMenu(
            fl!("top-menu-rotation").to_string(),
            rotation_entries(&*context),
        ));

        entries.extend(vec![
            EntryKind::CheckBox(
                "Invert Colors".to_string(),
                EntryId::ToggleInverted,
                context.device.framebuffer().inverted(),
            ),
            EntryKind::SubMenu(
                fl!("top-menu-wifi").to_string(),
                vec![
                    EntryKind::RadioButton(
                        fl!("settings-wifi-mode-off").to_string(),
                        EntryId::SetWifiMode(crate::settings::WifiMode::Off),
                        context.settings.wifi == crate::settings::WifiMode::Off,
                    ),
                    EntryKind::RadioButton(
                        fl!("settings-wifi-mode-always-on").to_string(),
                        EntryId::SetWifiMode(crate::settings::WifiMode::AlwaysOn),
                        context.settings.wifi == crate::settings::WifiMode::AlwaysOn,
                    ),
                    EntryKind::RadioButton(
                        fl!("settings-wifi-mode-auto").to_string(),
                        EntryId::SetWifiMode(crate::settings::WifiMode::Auto),
                        context.settings.wifi == crate::settings::WifiMode::Auto,
                    ),
                ],
            ),
            EntryKind::Separator,
            EntryKind::Command("Take Screenshot".to_string(), EntryId::TakeScreenshot),
            EntryKind::Separator,
            EntryKind::SubMenu("Applications".to_string(), apps),
            EntryKind::Separator,
            EntryKind::SubMenu(fl!("top-menu-exit").to_string(), {
                let mut exit_entries = Vec::new();
                if let Some(peer) = context.device.peer_installs().first() {
                    let build = match peer.kind {
                        crate::version::BuildKind::Standard => fl!("build-kind-main"),
                        crate::version::BuildKind::Test => fl!("build-kind-test"),
                    };
                    exit_entries.push(EntryKind::Command(
                        fl!("top-menu-switch-to", build = build.as_str()).to_string(),
                        EntryId::SwitchInstall,
                    ));
                    exit_entries.push(EntryKind::Separator);
                }
                exit_entries.extend([
                    EntryKind::Command(fl!("top-menu-suspend").to_string(), EntryId::Suspend),
                    EntryKind::Command(fl!("top-menu-restart-app").to_string(), EntryId::Restart),
                    EntryKind::Command(fl!("top-menu-reboot-device").to_string(), EntryId::Reboot),
                    EntryKind::Command(fl!("top-menu-power-off").to_string(), EntryId::PowerOff),
                    EntryKind::Command(fl!("top-menu-quit").to_string(), EntryId::Quit),
                ]);
                exit_entries
            }),
        ]);

        let main_menu = Menu::new(rect, ViewId::MainMenu, MenuKind::DropDown, entries, context);
        rq.add(RenderData::new(
            main_menu.id(),
            *main_menu.rect(),
            UpdateMode::Gui,
        ));
        view.children_mut()
            .push(Box::new(main_menu) as Box<dyn View>);
    }
}

pub fn toggle_battery_menu(
    view: &mut dyn View,
    rect: Rectangle,
    enable: Option<bool>,
    rq: &mut RenderQueue,
    context: &mut AppContext,
) {
    if let Some(index) = locate_by_id(view, ViewId::BatteryMenu) {
        if let Some(true) = enable {
            return;
        }
        rq.add(RenderData::expose(
            *view.child(index).rect(),
            UpdateMode::Gui,
        ));
        view.children_mut().remove(index);
    } else {
        if let Some(false) = enable {
            return;
        }

        let mut entries = Vec::new();

        match context
            .device
            .battery()
            .status()
            .ok()
            .zip(context.device.battery().capacity().ok())
        {
            Some((status, capacity)) => {
                for (i, (s, c)) in status.iter().zip(capacity.iter()).enumerate() {
                    entries.push(EntryKind::Message(
                        format!("{:?} {}%", s, c),
                        if i > 0 {
                            Some("cover".to_string())
                        } else {
                            None
                        },
                    ));
                }
            }
            _ => {
                entries.push(EntryKind::Message(
                    "Information Unavailable".to_string(),
                    None,
                ));
            }
        }

        let battery_menu = Menu::new(
            rect,
            ViewId::BatteryMenu,
            MenuKind::DropDown,
            entries,
            context,
        );
        rq.add(RenderData::new(
            battery_menu.id(),
            *battery_menu.rect(),
            UpdateMode::Gui,
        ));
        view.children_mut()
            .push(Box::new(battery_menu) as Box<dyn View>);
    }
}

pub fn toggle_clock_menu(
    view: &mut dyn View,
    rect: Rectangle,
    enable: Option<bool>,
    rq: &mut RenderQueue,
    context: &mut AppContext,
) {
    if let Some(index) = locate_by_id(view, ViewId::ClockMenu) {
        if let Some(true) = enable {
            return;
        }
        rq.add(RenderData::expose(
            *view.child(index).rect(),
            UpdateMode::Gui,
        ));
        view.children_mut().remove(index);
    } else {
        if let Some(false) = enable {
            return;
        }
        let text = Local::now()
            .format(&context.settings.date_format)
            .to_string();
        let entries = vec![
            EntryKind::Message(text, None),
            EntryKind::Separator,
            EntryKind::Command(fl!("top-menu-sync-time"), EntryId::SyncTime),
        ];

        let clock_menu = Menu::new(
            rect,
            ViewId::ClockMenu,
            MenuKind::DropDown,
            entries,
            context,
        );
        rq.add(RenderData::new(
            clock_menu.id(),
            *clock_menu.rect(),
            UpdateMode::Gui,
        ));
        view.children_mut()
            .push(Box::new(clock_menu) as Box<dyn View>);
    }
}

pub fn toggle_input_history_menu(
    view: &mut dyn View,
    id: ViewId,
    rect: Rectangle,
    enable: Option<bool>,
    rq: &mut RenderQueue,
    context: &mut AppContext,
) {
    if let Some(index) = locate_by_id(view, ViewId::InputHistoryMenu) {
        if let Some(true) = enable {
            return;
        }
        rq.add(RenderData::expose(
            *view.child(index).rect(),
            UpdateMode::Gui,
        ));
        view.children_mut().remove(index);
    } else {
        if let Some(false) = enable {
            return;
        }
        let entries = context.input_history.get(&id).map(|h| {
            h.iter()
                .map(|s| {
                    EntryKind::Command(s.to_string(), EntryId::SetInputText(id, s.to_string()))
                })
                .collect::<Vec<EntryKind>>()
        });
        if let Some(entries) = entries {
            let menu_kind = match id {
                ViewId::HomeSearchInput
                | ViewId::ReaderSearchInput
                | ViewId::DictionarySearchInput
                | ViewId::CalculatorInput => MenuKind::DropDown,
                _ => MenuKind::Contextual,
            };
            let input_history_menu =
                Menu::new(rect, ViewId::InputHistoryMenu, menu_kind, entries, context);
            rq.add(RenderData::new(
                input_history_menu.id(),
                *input_history_menu.rect(),
                UpdateMode::Gui,
            ));
            view.children_mut()
                .push(Box::new(input_history_menu) as Box<dyn View>);
        }
    }
}

pub fn toggle_keyboard_layout_menu(
    view: &mut dyn View,
    rect: Rectangle,
    enable: Option<bool>,
    rq: &mut RenderQueue,
    context: &mut AppContext,
) {
    if let Some(index) = locate_by_id(view, ViewId::KeyboardLayoutMenu) {
        if let Some(true) = enable {
            return;
        }
        rq.add(RenderData::expose(
            *view.child(index).rect(),
            UpdateMode::Gui,
        ));
        view.children_mut().remove(index);
    } else {
        if let Some(false) = enable {
            return;
        }
        let entries = context
            .keyboard_layouts
            .keys()
            .map(|s| EntryKind::Command(s.to_string(), EntryId::SetKeyboardLayout(s.to_string())))
            .collect::<Vec<EntryKind>>();
        let keyboard_layout_menu = Menu::new(
            rect,
            ViewId::KeyboardLayoutMenu,
            MenuKind::Contextual,
            entries,
            context,
        );
        rq.add(RenderData::new(
            keyboard_layout_menu.id(),
            *keyboard_layout_menu.rect(),
            UpdateMode::Gui,
        ));
        view.children_mut()
            .push(Box::new(keyboard_layout_menu) as Box<dyn View>);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::WHITE;
    use crate::context::test_helpers::create_test_context;
    use crate::device::soft_suspend::SoftSuspendBackend as _;
    use crate::view::filler::Filler;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn toggle_main_menu_holds_sleep_lease_on_cadmus_test() {
        let mut context = create_test_context().await;
        context.release_startup_lease();
        let mut parent = Filler::new(rect![0, 0, 600, 800], WHITE);
        let mut rq = RenderQueue::new();
        let target = rect![10, 10, 100, 40];
        toggle_main_menu(&mut parent, target, Some(true), &mut rq, &mut context);
        if cfg!(feature = "test") {
            assert!(context.inhibitor.sleep_active());
            assert!(
                context
                    .inhibitor
                    .holders()
                    .iter()
                    .any(|name| name.as_str() == "sleep-inhibit")
            );
        } else {
            assert!(!context.inhibitor.sleep_active());
        }
        toggle_main_menu(&mut parent, target, Some(false), &mut rq, &mut context);
        assert!(!context.inhibitor.sleep_active());
    }

    fn rotation_modes(entries: &[EntryKind]) -> Vec<(EntryId, bool)> {
        entries
            .iter()
            .filter_map(|entry| match entry {
                EntryKind::RadioButton(_, id, checked) => Some((id.clone(), *checked)),
                _ => None,
            })
            .collect()
    }

    fn rotation_labels(entries: &[EntryKind]) -> Vec<(String, bool)> {
        entries
            .iter()
            .filter_map(|entry| match entry {
                EntryKind::RadioButton(label, _, checked) => Some((label.clone(), *checked)),
                _ => None,
            })
            .collect()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rotation_submenu_unifies_gyro_and_manual_entries() {
        let context = create_test_context().await;

        assert!(!context.device.has_gyroscope());
        assert!(!context.device.has_page_turn_buttons());
        let entries = rotation_entries(&context);
        let modes = rotation_modes(&entries);
        assert!(
            modes
                .iter()
                .all(|(id, _)| matches!(id, EntryId::SetRotation(RotationMode::Fixed(_)))),
            "gyro-less devices list only fixed angles: {modes:?}"
        );
        assert_eq!(modes.iter().filter(|(_, checked)| *checked).count(), 1);
        assert!(
            entries
                .iter()
                .all(|entry| !matches!(entry, EntryKind::Separator))
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rotation_submenu_marks_fixed_angle_current() {
        let mut context = create_test_context().await;
        context.settings.rotation_lock = Some(RotationLock::Current);

        let labels = rotation_labels(&rotation_entries(&context));
        let current = context.device.to_canonical(context.display.rotation);
        let degrees: i16 = (current as i16) * 90;
        let expected = fl!("settings-rotation-angle", degrees = degrees);
        assert!(labels.contains(&(expected, true)));
    }

    #[test]
    fn button_scheme_group_marks_active_scheme() {
        for scheme in [ButtonScheme::Natural, ButtonScheme::Inverted] {
            let entries = button_scheme_entries(scheme);
            assert!(
                entries.iter().all(|entry| matches!(
                    entry,
                    EntryKind::RadioButton(_, EntryId::SetButtonScheme(_), _)
                )),
                "button entries form their own radio group: {entries:?}"
            );
            assert_eq!(
                entries.iter().filter(|e| is_checked(e)).count(),
                1,
                "exactly one scheme is checked for {scheme:?}: {entries:?}"
            );
        }
    }

    fn is_checked(entry: &EntryKind) -> bool {
        matches!(entry, EntryKind::RadioButton(_, _, true))
    }
}
