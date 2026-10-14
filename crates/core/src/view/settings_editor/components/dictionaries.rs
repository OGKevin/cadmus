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
