# Keep the message ID's sorted please.

-app-name = Cadmus
build-attributes =
    Built { $timestamp }
    By { $user }@{ $host }
english = Anglais
startup-loading = Cadmus démarre…

# Notifications

notification-dictionary-install-failed = Échec dans l'installation du dictionnaire "{ $lang }"
notification-downloading-dictionary = Téléchargement du dictionnaire "{ $lang }"
notification-downloading-dictionary-completed = Téléchargement du dictionnaire "{ $lang }" terminé
notification-downloading-dictionary-progress = Téléchargement { $lang } en cours ({ $downloaded }/{ $total })
notification-dictionary-indexing = Indexation de "{ $name }" en cours
notification-not-online = Le WiFi doit être connecté pour cette action.
notification-refresh-rate-invalid = Le taux de rafraîchissement doit être compris entre 0 et 255.
notification-time-sync-failed = Échec de synchronisation de l'horloge
notification-timezone-detection-failed = Échec de la détection du fuseau horaire
# Common
cancel = Annuler
delete = Supprimer
# Top Menu Strings
top-menu-exit = Sortir
top-menu-reboot-device = Redémarrer l'appareil
top-menu-restart-app = Redémarrer { -app-name }
top-menu-suspend = Mise en veille
top-menu-quit = Quitter
top-menu-power-off = Éteindre
top-menu-sync-time = Synchroniser l'horloge
# Settings - Button Scheme
settings-button-scheme-natural = Naturel
settings-button-scheme-inverted = Inversé
# Settings - Finished Actions
settings-finished-action-notify = Notifier
settings-finished-action-close = Fermer
settings-finished-action-goto-next = Aller au Prochain
# Settings - General
settings-general-auto-frontlight = Automatic Frontlight
settings-general-auto-frontlight-brightness = Auto Night Brightness
settings-general-auto-frontlight-brightness-input = Night Brightness (% 0-100)
settings-general-auto-frontlight-manual-coordinates = Manual Coordinates
settings-general-auto-frontlight-manual-coordinates-input = lat,lon | coordinates
settings-general-auto-power-off = Extinction Auto (jours)
settings-general-auto-power-off-input = Extinction Auto (jours, 0 = jamais)
settings-general-auto-suspend = Mise en veille Auto (minutes)
settings-general-auto-suspend-input = Mise en veille Auto (minutes, 0 = jamais)
settings-general-auto-time = Automatic Time Sync
settings-general-button-scheme = Schéma des boutons
settings-general-enable-auto-share = Activer Partage Auto
settings-general-enable-sleep-cover = Activer Sleep Cover
settings-general-keyboard-layout = Disposition du clavier
settings-general-language = Langue
settings-general-never = Jamais
settings-general-not-set = Non Défini
settings-general-settings-retention = Settings Retention
settings-general-startup-mode = Startup Mode
settings-general-toggle-off = off
settings-general-toggle-on = on
settings-general-trigger = Déclencher
settings-general-unknown = Inconnu
# Settings - Startup Mode
settings-startup-mode-home = Page d'accueil
settings-startup-mode-last-file = Fichier précédent
# Settings - Reader
settings-reader-end-of-book-action = Action Fin de Livre
settings-reader-dithered-kinds = Dithered File Types
settings-reader-refresh-rate = Taux de rafraîchissement
settings-reader-refresh-rate-by-kind-inverted-input = { $ext } taux de rafraîchissement inversé (0 = jamais)
settings-reader-refresh-rate-by-kind-regular-input = { $ext } taux de rafraîchissement inversé (0 = jamais)
settings-reader-refresh-rate-inverted = Inversé
settings-reader-refresh-rate-inverted-input = Taux de rafraîchissement Inversé (0 = jamais)
settings-reader-refresh-rate-regular = Normal
settings-reader-refresh-rate-regular-input = Taux de rafraîchissement Normal (0 = jamais)
settings-reader-refresh-rate-summary = { $regular } / { $inverted }
# Settings - Intermission
settings-intermission-blank = Écran Blanc
settings-intermission-blank-inverted = Écran Noir
settings-intermission-calendar = Calendrier
settings-intermission-cover = Couverture
settings-intermission-custom = Personnalisé
settings-intermission-custom-image = Image Personnalisée...
settings-intermission-logo = Logo
settings-intermission-power-off-screen = Écran d'extinction
settings-intermission-share-screen = Écran de partage des données
settings-intermission-suspend-screen = Écran de veille
# Settings - Library
settings-library-name = Nom
settings-library-path = Chemin
settings-library-end-of-book-action = Action Fin de Livre
settings-library-inherit = Hériter
# Settings - Import
settings-import-allowed-kinds = Indexed File Types
settings-import-force-full-import = Importation totale forcée
settings-import-force-full-import-cancel = { cancel }
settings-import-force-full-import-confirm = Réimporte tous les fichiers de toutes vos bibliothèques. Cela peut prendre du temps et vider la batterie, il est conseillé de laisser votre appareil branché le temps de l'opération.
settings-import-force-full-import-confirm-button = Tout réimporter
settings-import-sync-metadata = Synchroniser Metadata
# Importer
importer-importing-library = Importation de la bibliothèque en cours…
# Settings - Log Level
settings-log-level-trace = TRACE
settings-log-level-debug = DEBUG
settings-log-level-info = INFO
settings-log-level-warn = WARN
settings-log-level-error = ERROR
# Settings - Telemetry
settings-telemetry-enable-logging = Activer le Logging
settings-telemetry-log-level = Niveau de Log
settings-telemetry-otlp-endpoint = OTLP Endpoint
settings-telemetry-pyroscope-endpoint = Pyroscope Endpoint
settings-telemetry-enable-kernel-log = Activer les Logs Kernel
settings-telemetry-enable-dbus-log = Activer les Logs D-Bus
# Settings - Dictionaries
settings-dictionaries-confirm-download = Télécharger le dictionnaire "{ $lang}" ?
settings-dictionaries-confirm-download-cancel = Annuler
settings-dictionaries-confirm-download-confirm = Télécharger
settings-dictionaries-delete = { delete }
settings-dictionaries-download = Télécharger
settings-dictionaries-downloading = Téléchargement
settings-dictionaries-installed = Installé
settings-dictionaries-re-download = Re-Télécharger
settings-dictionaries-update = Mettre à jour
settings-dictionaries-update-available = Mise à jour disponible
# Calendar intermission
calendar-date-line = { $day } { $month } { $year } - { $weekday }
calendar-poweroff = Extinction automatique dans { $duration }
calendar-month-january = Janvier
calendar-month-february = Février
calendar-month-march = Mars
calendar-month-april = Avril
calendar-month-may = Mai
calendar-month-june = Juin
calendar-month-july = Juillet
calendar-month-august = Août
calendar-month-september = Septembre
calendar-month-october = Octobre
calendar-month-november = Novembre
calendar-month-december = Décembre
calendar-month-short-jan = JANV
calendar-month-short-feb = FÉV
calendar-month-short-mar = MAR
calendar-month-short-apr = AVR
calendar-month-short-may = MAI
calendar-month-short-jun = JUN
calendar-month-short-jul = JUL
calendar-month-short-aug = AOÛ
calendar-month-short-sep = SEP
calendar-month-short-oct = OCT
calendar-month-short-nov = NOV
calendar-month-short-dec = DÉC
calendar-weekday-mon = LUN
calendar-weekday-tue = MAR
calendar-weekday-wed = MER
calendar-weekday-thu = JEU
calendar-weekday-fri = VEN
calendar-weekday-sat = SAM
calendar-weekday-sun = DIM
