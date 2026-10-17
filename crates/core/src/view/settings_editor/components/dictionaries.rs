//! Rust side of `DictionaryPage`.
//!
//! The row model and its callback live on [`DictionaryPageIo`]. A tap passes
//! the model index, starting at 0. Downloads keep their job here.

use slint::ComponentHandle;

use super::setting;
use super::{Component, Env, Step, Waiting};
use crate::device::AppContext;
use crate::device::DevicePaths as _;
use crate::dictionary::{MonolingualDictionaryService, dictionary_label};
use crate::fl;
use crate::view::settings_editor::actions::start_dictionary_download;
use crate::view::settings_editor::kinds::dictionary::{self, DictionaryInfo};
use crate::view::settings_editor::kinds::{SettingKind, SettingsFetchData, WidgetKind};
use crate::view::slint::generated::{DictionaryPageIo, SettingLine, SettingsWindow};
use crate::view::slint::{CallbackQueue, changed_model, connect};
use crate::view::{EntryId, Event, NotificationEvent};

/// One callback from the dictionary list.
pub(super) enum Action {
    /// `index` is the index into the dictionary model, starting at 0.
    Press(i32),
}

pub(super) struct Dictionaries {
    rows: Vec<DictionaryInfo>,
    service: Option<MonolingualDictionaryService>,
    job: Option<crate::runtime::Job>,
    queue: CallbackQueue<Action>,
}

impl Dictionaries {
    pub(super) fn new(service: Option<MonolingualDictionaryService>) -> Self {
        Self {
            rows: Vec::new(),
            service,
            job: None,
            queue: CallbackQueue::new(),
        }
    }

    pub(super) fn stop_jobs(&self) {
        if let Some(job) = &self.job {
            job.cancel();
        }
    }

    pub(super) fn take_job(&mut self) -> Option<crate::runtime::Job> {
        self.job.take()
    }

    #[cfg(test)]
    pub(super) fn job(&self) -> Option<&crate::runtime::Job> {
        self.job.as_ref()
    }

    #[cfg(test)]
    pub(super) fn set_job(&mut self, job: Option<crate::runtime::Job>) {
        self.job = job;
    }

    pub(super) async fn reload(&mut self, context: &AppContext) {
        self.rows = match &self.service {
            Some(service) => dictionary::load_infos(context, service).await,
            None => Vec::new(),
        };
    }

    /// `row` is the index into the dictionary model, starting at 0.
    async fn press(&mut self, row: i32, env: &mut Env<'_>) -> Step {
        let Some(info) = self.rows.get(row as usize).cloned() else {
            return Step::Idle;
        };
        let install_dir = env.context.device.install_dir();
        let data = info.fetch(SettingsFetchData {
            settings: &env.context.settings,
            install_dir: Some(install_dir.as_path()),
        });
        match data.widget {
            WidgetKind::SubMenu(entries) => {
                Step::choice(Waiting::Dictionary, info.label.clone(), entries)
            }
            WidgetKind::ActionLabel(Event::Select(EntryId::RequestDictionaryDownload(lang))) => {
                self.prepare_download(&lang, env).await
            }
            _ => Step::Idle,
        }
    }

    /// Routes the entry `id` chosen from the dictionary choice sheet.
    pub(super) async fn apply_choice(&mut self, id: &EntryId, env: &mut Env<'_>) -> Step {
        match id {
            EntryId::RequestDictionaryDownload(lang) => self.prepare_download(lang, env).await,
            EntryId::DeleteDictionary(lang) => {
                self.delete(lang, env).await;
                Step::Redraw
            }
            _ => Step::Idle,
        }
    }

    async fn prepare_download(&mut self, lang: &str, env: &mut Env<'_>) -> Step {
        if !env.context.online && !env.context.settings.wifi.allows_on_demand() {
            env.hub
                .send(
                    (Event::Notification(NotificationEvent::Show(fl!("notification-not-online"))))
                        .into(),
                )
                .ok();
            return Step::Idle;
        }
        let entry = match self.service.as_ref() {
            Some(service) => service.get_entry_for_lang(lang).await.unwrap_or(None),
            None => None,
        };
        let label = dictionary_label(lang, entry.as_ref());
        Step::confirm(
            Waiting::Download(lang.to_string()),
            fl!("settings-dictionaries-confirm-download", lang = label),
            fl!("settings-dictionaries-confirm-download-confirm"),
        )
    }

    pub(super) async fn start_download(&mut self, lang: &str, env: &mut Env<'_>) -> Step {
        if !env.context.online && !env.context.settings.wifi.allows_on_demand() {
            env.hub
                .send(
                    (Event::Notification(NotificationEvent::Show(fl!("notification-not-online"))))
                        .into(),
                )
                .ok();
            return Step::Idle;
        }
        if let Some(service) = self.service.clone() {
            start_dictionary_download(&service, &mut self.job, lang, env.hub, env.context).await;
        }
        self.reload(env.context).await;
        Step::Redraw
    }

    async fn delete(&mut self, lang: &str, env: &mut Env<'_>) {
        if let Some(service) = &self.service {
            service.remove_installed(lang).await;
        } else {
            tracing::warn!(
                lang,
                "No MonolingualDictionaryService available to delete dictionary"
            );
        }
        self.reload(env.context).await;
        env.hub.send((Event::ReindexDictionaries).into()).ok();
    }
}

#[async_trait::async_trait(?Send)]
impl Component for Dictionaries {
    type Window = SettingsWindow;
    type Action = Action;
    type Effect = Step;

    fn wire(&self, window: &SettingsWindow) {
        let io = window.global::<DictionaryPageIo>();
        connect!(io, self.queue, on_row_pressed, (index) => Action::Press(index));
    }

    fn take(&self) -> Vec<Action> {
        self.queue.drain()
    }

    fn sync(&mut self, window: &SettingsWindow, context: &AppContext) {
        let io = window.global::<DictionaryPageIo>();
        if let Some(rows) = changed_model(&io.get_rows(), dictionary_lines(&self.rows, context)) {
            io.set_rows(rows);
        }
    }

    async fn apply(&mut self, action: Action, env: &mut Env<'_>) -> Step {
        match action {
            Action::Press(index) => self.press(index, env).await,
        }
    }
}

fn dictionary_lines(rows: &[DictionaryInfo], context: &AppContext) -> Vec<SettingLine> {
    rows.iter()
        .map(|info| SettingLine {
            label: info.label.clone().into(),
            value: setting::display_of(info, context).into(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use slint::ComponentHandle;
    use slint::Model;

    use super::super::notification;
    use super::super::setting;
    use super::super::settings_env;
    use super::super::{Sheet, Waiting};
    use super::{Action, Component, Dictionaries, Step};
    use crate::dictionary::{MonolingualDictionaryService, reconcile_installed_dictionaries};
    use crate::fl;
    use crate::settings::WifiMode;
    use crate::view::slint::generated::{DictionaryPageIo, SettingLine};
    use crate::view::{EntryId, Event};

    fn line_at(rows: &slint::ModelRc<SettingLine>, index: usize) -> SettingLine {
        rows.row_data(index)
            .unwrap_or_else(|| panic!("missing dictionary {index}"))
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn without_a_service_a_press_does_nothing() {
        let (mut context, session) = super::super::shell::open_test_session().await;
        let window = session.component();
        let mut page = Dictionaries::new(None);
        page.wire(window);
        page.reload(&context).await;
        page.sync(window, &context);
        assert_eq!(
            window.global::<DictionaryPageIo>().get_rows().row_count(),
            0
        );

        window.global::<DictionaryPageIo>().invoke_row_pressed(0);
        assert!(matches!(page.take().as_slice(), [Action::Press(0)]));
        let (hub, _rx) = crate::view::hub_channel();
        let mut bus = crate::view::Bus::new();
        let step = {
            let mut env = settings_env(&hub, &mut bus, &mut context);
            page.apply(Action::Press(0), &mut env).await
        };
        assert!(matches!(step, Step::Idle));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_installed_dictionary_opens_a_choice_and_delete_removes_it() {
        let (mut context, session) = super::super::shell::open_test_session().await;
        let window = session.component();
        context.online = false;
        context.settings.wifi = WifiMode::Off;

        let dir = tempfile::tempdir().expect("dictionary dir");
        let lang_dir = dir.path().join("reader-dict").join("en");
        tokio::fs::create_dir_all(&lang_dir)
            .await
            .expect("lang dir");
        tokio::fs::write(lang_dir.join("dict.index"), [])
            .await
            .expect("index");
        tokio::fs::write(lang_dir.join("dict.dict"), [])
            .await
            .expect("dict");
        reconcile_installed_dictionaries(&context.database, dir.path())
            .await
            .expect("register dictionary");
        let service = MonolingualDictionaryService::new(&context.database, dir.path())
            .expect("dictionary service");

        let mut page = Dictionaries::new(Some(service));
        page.wire(window);
        page.reload(&context).await;
        page.sync(window, &context);

        let rows = window.global::<DictionaryPageIo>().get_rows();
        assert_eq!(rows.row_count(), 1);
        let line = line_at(&rows, 0);
        assert_eq!(line.label.as_str(), "en");
        assert_eq!(line.value.as_str(), fl!("settings-dictionaries-installed"));

        window.global::<DictionaryPageIo>().invoke_row_pressed(0);
        assert!(matches!(page.take().as_slice(), [Action::Press(0)]));

        let (hub, mut rx) = crate::view::hub_channel();
        let mut bus = crate::view::Bus::new();
        let pressed = {
            let mut env = settings_env(&hub, &mut bus, &mut context);
            page.apply(Action::Press(0), &mut env).await
        };
        let Step::Ask(Waiting::Dictionary, Sheet::Choice { title, entries }) = pressed else {
            panic!("the installed dictionary did not open a choice");
        };
        assert_eq!(title, "en");
        let ids: Vec<EntryId> = entries.iter().filter_map(setting::entry_id).collect();
        assert_eq!(
            ids,
            vec![
                EntryId::RequestDictionaryDownload("en".into()),
                EntryId::DeleteDictionary("en".into()),
            ]
        );

        let missing = {
            let mut env = settings_env(&hub, &mut bus, &mut context);
            page.apply(Action::Press(1), &mut env).await
        };
        assert!(matches!(missing, Step::Idle));

        let offline = {
            let mut env = settings_env(&hub, &mut bus, &mut context);
            page.apply_choice(&EntryId::RequestDictionaryDownload("en".into()), &mut env)
                .await
        };
        assert!(matches!(offline, Step::Idle));
        assert_eq!(notification(&mut rx), fl!("notification-not-online"));

        let deleted = {
            let mut env = settings_env(&hub, &mut bus, &mut context);
            page.apply_choice(&EntryId::DeleteDictionary("en".into()), &mut env)
                .await
        };
        assert!(matches!(deleted, Step::Redraw));
        page.sync(window, &context);
        assert_eq!(
            window.global::<DictionaryPageIo>().get_rows().row_count(),
            0
        );
        assert!(
            !tokio::fs::try_exists(&lang_dir)
                .await
                .expect("stat dictionary dir")
        );
        let reindex = rx.try_recv().expect("missing reindex");
        assert!(matches!(reindex.event, Event::ReindexDictionaries));

        context.online = true;
        let download = {
            let mut env = settings_env(&hub, &mut bus, &mut context);
            page.apply_choice(&EntryId::RequestDictionaryDownload("en".into()), &mut env)
                .await
        };
        let Step::Ask(Waiting::Download(lang), Sheet::Confirm { message, accept }) = download
        else {
            panic!("an online download did not ask for confirmation");
        };
        assert_eq!(lang, "en");
        assert_eq!(
            message,
            fl!("settings-dictionaries-confirm-download", lang = "en")
        );
        assert_eq!(
            accept,
            fl!("settings-dictionaries-confirm-download-confirm")
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn start_download_stays_idle_offline_and_does_not_start_without_metadata() {
        let (mut context, _session) = super::super::shell::open_test_session().await;
        context.online = false;
        context.settings.wifi = WifiMode::Off;
        let dir = tempfile::tempdir().expect("dictionary dir");
        let service = MonolingualDictionaryService::new(&context.database, dir.path())
            .expect("dictionary service");
        let mut page = Dictionaries::new(Some(service));
        let (hub, mut rx) = crate::view::hub_channel();
        let mut bus = crate::view::Bus::new();

        let offline = {
            let mut env = settings_env(&hub, &mut bus, &mut context);
            page.start_download("en", &mut env).await
        };
        assert!(matches!(offline, Step::Idle));
        assert_eq!(notification(&mut rx), fl!("notification-not-online"));
        assert!(page.job().is_none());

        context.online = true;
        let online = {
            let mut env = settings_env(&hub, &mut bus, &mut context);
            page.start_download("en", &mut env).await
        };
        assert!(matches!(online, Step::Redraw));
        assert!(page.job().is_none());
    }
}
