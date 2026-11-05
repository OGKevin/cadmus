# Keep the message ID's sorted please.

-app-name = Cadmus
-minutes = minuti
-wifi-idle-timeout = WiFi Idle Timeout
build-attributes =
    Built { $timestamp }
    di { $user }@{ $host}
build-features = Funzionalità: { $features}
build-kind-main = Principale
build-kind-test = Test

# Keyboard

keyboard-key-alternate = ALT
keyboard-key-arrow-down = ▼
keyboard-key-arrow-left = ◀
keyboard-key-arrow-right = ▶
keyboard-key-arrow-up = ▲
keyboard-key-combine = CMB
keyboard-key-control = CTRL
keyboard-key-escape = ESC
keyboard-key-return = RETURN
keyboard-key-shift = SHIFT
keyboard-key-tab = TAB
startup-loading = Avvio di Cadmus…

# Notifications

notification-battery-low = La capacità della batteria sta diventando bassa.
notification-dictionary-install-failed = Impossibile installare il dizionario per "{ $lang}"
notification-downloading-dictionary = Scaricamento dizionario per "{ $lang}"
notification-downloading-dictionary-completed = Download del dizionario per "{ $lang }" completato
notification-downloading-dictionary-progress = Download { $lang } ({ $downloaded }/{ $total})
notification-dictionary-indexing = Indicizzazione "{ $name}"
notification-network-up = Rete attiva ({ $ip }, { $essid}).
notification-not-online = Il WiFi deve essere connesso per questa azione.
notification-refresh-rate-invalid = La frequenza di aggiornamento deve essere un numero compreso tra 0 e 255.
notification-time-sync-failed = Sincronizzazione orario fallita
notification-time-unsynchronised = L'orologio non è sincronizzato.
notification-timezone-detection-failed = Rilevamento fuso orario fallito
# Common
cancel = Annulla
delete = Elimina
# GitHub device authorization
device-auth-connecting = Connessione a GitHub…
device-auth-go-to-uri = Vai a: { $uri}
device-auth-enter-code = Inserisci il codice: { $code}
device-auth-cancelled-on-github = Autorizzazione annullata
# Top Menu Strings
top-menu-exit = Esci
top-menu-reboot-device = Riavvia il Dispositivo
top-menu-restart-app = Riavvia { -app-name }
top-menu-suspend = Sospendi
top-menu-quit = Esci
top-menu-power-off = Spegni
top-menu-switch-to = Passa a { $build}
top-menu-sync-time = Sincronizza Orario
top-menu-wifi = WiFi

# OTA

ota-already-latest = Hai già l'ultima versione
ota-auth-error = Autorizzazione GitHub fallita
ota-auth-timed-out = Autorizzazione di GitHub scaduta. Per favore riprova.
ota-battery-too-low = Batteria troppo bassa per avviare l'aggiornamento OTA
ota-cannot-compare-versions = Impossibile confrontare le versioni
ota-check-updates-failed = Controllo aggiornamenti non riuscito
ota-client-build-failed = Creazione del client non riuscita
ota-deployment-failed = Distribuzione fallita
ota-download-cancel = { cancel }
ota-download-canceling = Annullamento…
ota-download-failed = Download fallito
ota-downloading-default-branch = Download versione principale branch… { $percent}%
ota-downloading-pr = Download PR #{ $pr_number } build… { $percent}%
ota-downloading-stable-release = Download versione stabile… { $percent}%
ota-installing-and-rebooting = Installazione e riavvio…
ota-invalid-pr-number = Numero PR non valido
ota-pr-input-title = Scarica Build da PR
ota-version-comparison-error = Confronto versione fallito
ota-version-newer = La tua versione è più recente dell'ultima versione
# Settings - Button Scheme
settings-button-scheme-natural = Naturale
settings-button-scheme-inverted = Invertito
# Settings - WiFi Mode
settings-wifi-mode-off = Spento
settings-wifi-mode-always-on = Sempre Acceso
settings-wifi-mode-auto = Auto
# Settings - Finished Actions
settings-finished-action-notify = Notifica
settings-finished-action-close = Chiuso
settings-finished-action-goto-next = Vai al prossimo
# Settings - Categories
settings-category-dictionaries = Dizionari
settings-category-general = Generale
settings-category-import = Importa
settings-category-intermissions = Schermate di pausa
settings-category-libraries = Librerie
settings-category-power = Alimentazione
settings-category-reader = Lettore
settings-category-telemetry = Telemetria
# Settings - General
settings-general-auto-frontlight = Illuminazione frontale automatica
settings-general-auto-frontlight-brightness = Luminosità Notturna Automatica
settings-general-auto-frontlight-brightness-input = Luminosità Notturna (% 0-100)
settings-general-auto-frontlight-manual-coordinates = Coordinate Manuale
settings-general-auto-frontlight-manual-coordinates-input = lat,lon | coordinates
settings-general-auto-time = Sincronizzazione Automatica Orario
settings-general-button-scheme = Schema Pulsanti
settings-general-db-backup-retention = Conservazione Backup DB
settings-general-db-backup-retention-input = Conservazione backup DB (numero di backup)
settings-general-enable-auto-share = Abilita Condivisione Automatica
settings-general-keyboard-layout = Layout di tastiera
settings-general-language = Lingua
settings-general-never = Mai
settings-general-not-set = Non impostato
settings-general-ntp-server = Server NTP
settings-general-ntp-server-input = Server NTP (hostname o IP)
settings-general-settings-retention = Conservazione Impostazioni
settings-general-startup-mode = Modalità di avvio
settings-general-toggle-off = spento
settings-general-toggle-on = attivo
settings-general-trigger = Attivazione
settings-general-unknown = Sconosciuto
settings-general-wifi-idle-immediate = Immediato
settings-general-wifi-idle-timeout = { -wifi-idle-timeout } ({ -minutes })
settings-general-wifi-idle-timeout-input = { -wifi-idle-timeout } ({ -minutes }, 0 = { settings-general-wifi-idle-immediate })
# Settings - Power
settings-power-auto-power-off = Spegnimento Automatico (giorni)
settings-power-auto-power-off-input = Spegnimento Automatico (giorni, 0 = mai)
settings-power-auto-suspend = Sospendi Automaticamente (minuti)
settings-power-auto-suspend-input = Sospensione Automatica (minuti, 0 = mai)
settings-power-autosleep-mode = Sospensione Soft
settings-power-autosleep-mode-freeze = Congela
settings-power-autosleep-mode-mem = Memoria
settings-power-autosleep-mode-off = Disattivo
settings-power-enable-sleep-cover = Abilita Sleep Cover
settings-power-indicate-autosleep-led = Usa LED per indicare Soft Suspend
settings-power-autosleep-grace = Sospendi Morbido Rilascio Grace (secondi)
settings-power-autosleep-grace-input = Sospensione Morbida Grace (secondi, 0 = immediata)
# Settings - Startup Mode
settings-startup-mode-home = Home
settings-startup-mode-last-file = Ultimo File
# Settings - Reader
settings-reader-end-of-book-action = Azione Fine Libro
settings-reader-dithered-kinds = Tipi Di File differenti
settings-reader-font-family = Famiglia di caratteri
settings-reader-refresh-rate = Frequenza di Aggiornamento
settings-reader-refresh-rate-by-kind-inverted-input = { $ext } inverted refresh rate (0 = mai)
settings-reader-refresh-rate-by-kind-regular-input = { $ext } frequenza di aggiornamento (0 = mai)
settings-reader-refresh-rate-inverted = Invertito
settings-reader-refresh-rate-inverted-input = Frequenza di aggiornamento invertito (0 = mai)
settings-reader-refresh-rate-regular = Normale
settings-reader-refresh-rate-regular-input = Frequenza di aggiornamento (0 = mai)
settings-reader-refresh-rate-summary = { $regular } / { $inverted }
# Settings - Intermission
settings-intermission-blank = Vuoto
settings-intermission-blank-inverted = Vuoto Invertito
settings-intermission-calendar = Calendario
settings-intermission-cover = Copertina
settings-intermission-custom = Personalizza
settings-intermission-custom-image = Immagine Personalizzata...
settings-intermission-fill-color = Colore di Riempimento
settings-intermission-fill-color-black = Nero
settings-intermission-fill-color-custom = Personalizza
settings-intermission-fill-color-white = Bianco
settings-intermission-logo = Logo
settings-intermission-power-off-screen = Spegni Schermo
settings-intermission-share-screen = Condividi Schermo
settings-intermission-suspend-screen = Sospendi Lo Schermo
# Settings - Library
settings-library-name = Nome
settings-library-path = Percorso
settings-library-end-of-book-action = Azione Fine Libro
settings-library-inherit = Eredita
# Settings - Import
settings-import-allowed-kinds = Tipi Di File Indicizzati
settings-import-force-full-import = Forza Importazione Completa
settings-import-force-full-import-cancel = { cancel }
settings-import-force-full-import-confirm =
    Reimportare tutti i file da tutte le librerie.
    Questo potrebbe richiedere tempo e consumare la batteria, è
    consigliabile mantenere il dispositivo collegato mentre questo funziona.
settings-import-force-full-import-confirm-button = Reimporta Tutto
settings-import-metadata-kinds = Tipi Di File Metadati
settings-import-sync-metadata = Sincronizza Metadati
# Importer
importer-import-interrupted = Importazione libreria interrotta.
importer-importing-library = Importa Libreria…
# Settings - Log Level
settings-log-level-trace = TRACCIA
settings-log-level-debug = DEBUG
settings-log-level-info = INFO
settings-log-level-warn = WARN
settings-log-level-error = ERROR
# Settings - Telemetry
settings-telemetry-enable-logging = Abilita Logging
settings-telemetry-log-level = Livello di Log
settings-telemetry-otlp-endpoint = OTLP Endpoint
settings-telemetry-pyroscope-endpoint = Pyroscope Endpoint
settings-telemetry-enable-kernel-log = Abilita Log Kernel
settings-telemetry-enable-dbus-log = Abilita Log D-Bus
# Settings - Dictionaries
settings-dictionaries-confirm-download = Scaricamento dizionario per "{ $lang}"?
settings-dictionaries-confirm-download-cancel = Annulla
settings-dictionaries-confirm-download-confirm = Scarica
settings-dictionaries-delete = { delete }
settings-dictionaries-download = Scarica
settings-dictionaries-downloading = Download in corso
settings-dictionaries-installed = Installato
settings-dictionaries-re-download = Scarica di nuovo
settings-dictionaries-update = Aggiorna
settings-dictionaries-update-available = Aggiornamento Disponibile
# Languages
catalan = Catalano
chinese = Cinese
czech = Ceco
danish = Danese
dutch = Olandese
english = Inglese
esperanto = Esperanto
finnish = Finlandese
french = Francese
german = Tedesco
greek = Greco
italian = Italiano
japanese = Giapponese
korean = Coreano
latin = Latino
lithuanian = Lituano
lojban = Lojban
malagasy = Malgascio
norwegian = Norvegese
polish = Polacco
portuguese = Portoghese
romanian = Rumeno
russian = Russo
spanish = Spagnolo
swedish = Svedese
thai = Thailandese
turkish = Turco
ukrainian = Ucraino

# Library metadata

untitled = Senza titolo
# Calendar intermission
calendar-date-line = { $day } { $month } { $year } - { $weekday }
calendar-poweroff = Spegnimento automatico in { $duration}
calendar-month-january = Gennaio
calendar-month-february = Febbraio
calendar-month-march = Marzo
calendar-month-april = Aprile
calendar-month-may = Maggio
calendar-month-june = Giugno
calendar-month-july = Luglio
calendar-month-august = Agosto
calendar-month-september = Settembre
calendar-month-october = Ottobre
calendar-month-november = Novembre
calendar-month-december = Dicembre
calendar-month-short-jan = GEN
calendar-month-short-feb = FEB
calendar-month-short-mar = MAR
calendar-month-short-apr = APR
calendar-month-short-may = MAG
calendar-month-short-jun = GIU
calendar-month-short-jul = LUG
calendar-month-short-aug = AGO
calendar-month-short-sep = SET
calendar-month-short-oct = OTT
calendar-month-short-nov = NOV
calendar-month-short-dec = DIC
calendar-weekday-mon = LUN
calendar-weekday-tue = MAR
calendar-weekday-wed = MER
calendar-weekday-thu = GIO
calendar-weekday-fri = VEN
calendar-weekday-sat = SAB
calendar-weekday-sun = DOM
