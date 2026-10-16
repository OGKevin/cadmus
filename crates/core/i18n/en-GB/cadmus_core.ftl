# Keep the message ID's sorted please.

-app-name = Cadmus
-minutes = minutes
-wifi-idle-timeout = WiFi Idle Timeout

build-attributes =
    Built { $timestamp }
    By { $user }@{ $host }
build-features = Features: { $features }
build-kind-main = Main
build-kind-test = Test

# Keyboard

keyboard-key-alternate = ALT
keyboard-key-arrow-down = ▼
keyboard-key-arrow-left = ◀
keyboard-key-arrow-right = ▶
keyboard-key-arrow-up = ▲
keyboard-key-combine = CMB
keyboard-key-control = CTRL
keyboard-key-delete = delete
keyboard-key-delete-forward = fwd
keyboard-key-delete-short = del
keyboard-key-escape = ESC
keyboard-key-move-back = <
keyboard-key-move-forward = >
keyboard-key-return = RETURN
keyboard-key-return-short = RET
keyboard-key-shift = SHIFT
keyboard-key-shift-short = SFT
keyboard-key-space = space
keyboard-key-tab = TAB

# Notifications

notification-battery-low = The battery capacity is getting low.
notification-dictionary-install-failed = Failed to install dictionary for "{ $lang }"
notification-downloading-dictionary = Downloading dictionary for "{ $lang }"
notification-downloading-dictionary-completed = Downloading dictionary for "{ $lang }" completed
notification-downloading-dictionary-progress = Downloading { $lang } ({ $downloaded }/{ $total })
notification-dictionary-indexing = Indexing "{ $name }"
notification-library-name-empty = Library name cannot be empty
notification-library-path-missing = Path does not exist
notification-network-up = Network is up ({ $ip }, { $essid }).
notification-not-online = WiFi must be connected for this action.
notification-refresh-rate-invalid = Refresh rate must be a number between 0 and 255.
notification-time-sync-failed = Time sync failed
notification-time-unsynchronised = The clock is unsynchronised.
notification-timezone-detection-failed = Timezone detection failed

# Common
cancel = Cancel
delete = Delete
ok = OK

# GitHub device authorization
device-auth-connecting = Connecting to GitHub…
device-auth-go-to-uri = Go to: { $uri }
device-auth-enter-code = Enter code: { $code }
device-auth-cancelled-on-github = Authorization cancelled

# Top Menu Strings
top-menu-exit = Exit
top-menu-reboot-device = Reboot Device
top-menu-restart-app = Restart {-app-name}
top-menu-suspend = Suspend
top-menu-quit = Quit
top-menu-power-off = Power Off
top-menu-switch-to = Switch to { $build }
top-menu-sync-time = Sync Time
top-menu-wifi = WiFi

# OTA

ota-already-latest = You already have the latest version
ota-auth-error = GitHub authorization failed
ota-auth-timed-out = GitHub authorization timed out. Please try again.
ota-battery-too-low = Battery too low to start OTA update
ota-cannot-compare-versions = Cannot compare versions
ota-check-updates-failed = Failed to check for updates
ota-client-build-failed = Failed to create client
ota-confirm-pr = Install build from PR #{ $pr_number }: { $title }?
ota-deployment-failed = Deployment failed
ota-download-canceling = Canceling…
ota-download-failed = Download failed
ota-downloading-default-branch = Downloading main branch build… { $percent }%
ota-downloading-pr = Downloading PR #{ $pr_number } build… { $percent }%
ota-downloading-stable-release = Downloading stable release… { $percent }%
ota-install = Install
ota-installing-and-rebooting = Installing and rebooting…
ota-pr-checking = #{ $number }
ota-pr-checking-draft = #{ $number } · { $draft }
ota-pr-list-failed = Could not load pull requests
ota-pr-row = #{ $number } · { $status }
ota-pr-row-draft = #{ $number } · { $draft } · { $status }
ota-pr-status-failed = Could not check which builds can be installed
ota-source-main = Main Branch
ota-source-pr = PR Build
ota-source-stable = Stable Release
ota-source-title = Where should the update come from?
ota-status-building = Building
ota-status-draft = Draft
ota-status-failed = Failed
ota-status-no-build = No build
ota-status-ready = Ready
ota-status-unknown = Could not check
ota-version-comparison-error = Version comparison failed
ota-version-newer = Your version is newer than the latest release

# Settings - Button Scheme
settings-button-scheme-natural = Natural
settings-button-scheme-inverted = Inverted

# Settings - WiFi Mode
settings-wifi-mode-off = Off
settings-wifi-mode-always-on = Always On
settings-wifi-mode-auto = Auto

# Settings - Finished Actions
settings-finished-action-notify = Notify
settings-finished-action-close = Close
settings-finished-action-goto-next = Go to Next

# Settings - General
settings-general-auto-frontlight = Automatic Frontlight
settings-general-auto-frontlight-brightness = Auto Night Brightness
settings-general-auto-frontlight-brightness-input = Night Brightness (% 0-100)
settings-general-auto-frontlight-manual-coordinates = Manual Coordinates
settings-general-auto-frontlight-manual-coordinates-input = lat,lon | coordinates
settings-general-auto-time = Automatic Time Sync
settings-general-button-scheme = Button Scheme
settings-general-db-backup-retention = DB Backup Retention
settings-general-db-backup-retention-input = DB Backup Retention (number of backups)
settings-general-enable-auto-share = Enable Auto Share
settings-general-keyboard-layout = Keyboard Layout
settings-general-language = Language
settings-general-never = Never
settings-general-not-set = Not set
settings-general-ntp-server = NTP Server
settings-general-ntp-server-input = NTP Server (hostname or IP)
settings-general-settings-retention = Settings Retention
settings-general-startup-mode = Startup Mode
settings-general-toggle-off = off
settings-general-toggle-on = on
settings-general-trigger = Trigger
settings-general-unknown = Unknown
settings-general-wifi-idle-immediate = Immediate
settings-general-wifi-idle-timeout = { -wifi-idle-timeout } ({ -minutes })
settings-general-wifi-idle-timeout-input = { -wifi-idle-timeout } ({ -minutes }, 0 = { settings-general-wifi-idle-immediate })

# Settings - Power
settings-power-auto-power-off = Auto Power Off (days)
settings-power-auto-power-off-input = Auto Power Off (days, 0 = never)
settings-power-auto-suspend = Auto Suspend (minutes)
settings-power-auto-suspend-input = Auto Suspend (minutes, 0 = never)
settings-power-autosleep-mode = Soft Suspend
settings-power-autosleep-mode-freeze = Freeze
settings-power-autosleep-mode-mem = Memory
settings-power-autosleep-mode-off = Off
settings-power-enable-sleep-cover = Enable Sleep Cover
settings-power-indicate-autosleep-led = Use LED to Indicate Soft Suspend
settings-power-autosleep-grace = Soft Suspend Release Grace (seconds)
settings-power-autosleep-grace-input = Soft Suspend Release Grace (seconds, 0 = immediate)

# Settings - Startup Mode
settings-startup-mode-home = Home
settings-startup-mode-last-file = Last File

# Settings - Reader
settings-reader-end-of-book-action = End of Book Action
settings-reader-dithered-kinds = Dithered File Types
settings-reader-font-family = Font Family
settings-reader-refresh-rate = Refresh Rate
settings-reader-refresh-rate-by-kind-inverted-input = { $ext } inverted refresh rate (0 = never)
settings-reader-refresh-rate-by-kind-regular-input = { $ext } regular refresh rate (0 = never)
settings-reader-refresh-rate-inverted = Inverted
settings-reader-refresh-rate-inverted-input = Inverted refresh rate (0 = never)
settings-reader-refresh-rate-regular = Regular
settings-reader-refresh-rate-regular-input = Regular refresh rate (0 = never)
settings-reader-refresh-rate-summary = { $regular } / { $inverted }

# Settings - Intermission
settings-intermission-blank = Blank
settings-intermission-blank-inverted = Blank Inverted
settings-intermission-calendar = Calendar
settings-intermission-cover = Cover
settings-intermission-custom = Custom
settings-intermission-custom-image = Custom Image...
settings-intermission-fill-color = Fill Color
settings-intermission-fill-color-black = Black
settings-intermission-fill-color-custom = Custom
settings-intermission-fill-color-white = White
settings-intermission-logo = Logo
settings-intermission-power-off-screen = Power Off Screen
settings-intermission-share-screen = Share Screen
settings-intermission-suspend-screen = Suspend Screen

# Settings - Library
settings-library-name = Name
settings-library-path = Path
settings-library-end-of-book-action = End of Book Action
settings-library-inherit = Inherit

# Settings - Import
settings-import-allowed-kinds = Indexed File Types
settings-import-force-full-import = Force Full Import
settings-import-force-full-import-confirm = Re-import all the files from all your libraries.
                                            This could take time and consume battery, it's
                                            advisable to keep your device plugged in while this runs.
settings-import-force-full-import-confirm-button = Re-import All
settings-import-metadata-kinds = Metadata File Types
settings-import-sync-metadata = Sync Metadata

# Importer
importer-import-interrupted = Library import was interrupted.
importer-importing-library = Importing Library…

# Settings - Log Level
settings-log-level-trace = TRACE
settings-log-level-debug = DEBUG
settings-log-level-info = INFO
settings-log-level-warn = WARN
settings-log-level-error = ERROR

# Settings - Telemetry
settings-telemetry-enable-logging = Enable Logging
settings-telemetry-log-level = Log Level
settings-telemetry-otlp-endpoint = OTLP Endpoint
settings-telemetry-pyroscope-endpoint = Pyroscope Endpoint
settings-telemetry-enable-kernel-log = Enable Kernel Log
settings-telemetry-enable-dbus-log = Enable D-Bus Log

# Settings - Dictionaries
settings-dictionaries-confirm-download = Download dictionary for "{ $lang }"?
settings-dictionaries-confirm-download-confirm = Download
settings-dictionaries-delete = {delete}
settings-dictionaries-download = Download
settings-dictionaries-downloading = Downloading
settings-dictionaries-installed = Installed
settings-dictionaries-re-download = Re-download
settings-dictionaries-update = Update
settings-dictionaries-update-available = Update Available

# Languages
catalan = Catalan
chinese = Chinese
czech = Czech
danish = Danish
dutch = Dutch
english = English
esperanto = Esperanto
finnish = Finnish
french = French
german = German
greek = Greek
italian = Italian
japanese = Japanese
korean = Korean
latin = Latin
lithuanian = Lithuanian
lojban = Lojban
malagasy = Malagasy
norwegian = Norwegian
polish = Polish
portuguese = Portuguese
romanian = Romanian
russian = Russian
spanish = Spanish
swedish = Swedish
thai = Thai
turkish = Turkish
ukrainian = Ukrainian

# Library metadata

untitled = Untitled

# Calendar intermission
calendar-date-line = { $day } { $month } { $year } - { $weekday }
calendar-poweroff = Auto power off in { $duration }

calendar-month-january = January
calendar-month-february = February
calendar-month-march = March
calendar-month-april = April
calendar-month-may = May
calendar-month-june = June
calendar-month-july = July
calendar-month-august = August
calendar-month-september = September
calendar-month-october = October
calendar-month-november = November
calendar-month-december = December

calendar-month-short-jan = JAN
calendar-month-short-feb = FEB
calendar-month-short-mar = MAR
calendar-month-short-apr = APR
calendar-month-short-may = MAY
calendar-month-short-jun = JUN
calendar-month-short-jul = JUL
calendar-month-short-aug = AUG
calendar-month-short-sep = SEP
calendar-month-short-oct = OCT
calendar-month-short-nov = NOV
calendar-month-short-dec = DEC

calendar-weekday-mon = MON
calendar-weekday-tue = TUE
calendar-weekday-wed = WED
calendar-weekday-thu = THU
calendar-weekday-fri = FRI
calendar-weekday-sat = SAT
calendar-weekday-sun = SUN
