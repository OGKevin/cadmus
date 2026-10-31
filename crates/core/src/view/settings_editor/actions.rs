//! Side effects the settings screen starts without drawing them.
//!
//! Dictionary downloads report progress through notifications and finish by
//! sending [`Event::DictionaryInstallComplete`].

use tracing::Instrument;

use crate::device::AppContext;
use crate::dictionary::MonolingualDictionaryService;
use crate::fl;
use crate::view::{Event, Hub, ID_FEEDER, NotificationEvent, ViewId};

/// Starts a dictionary install for `lang` when one is not already running.
///
/// Prefers the smaller no-etymology file. A pinned notification tracks
/// progress. The previous download, if any, is cancelled.
#[cfg_attr(
    feature = "tracing",
    tracing::instrument(skip(service, job, hub, context))
)]
pub(super) async fn start_dictionary_download(
    service: &MonolingualDictionaryService,
    job: &mut Option<crate::runtime::Job>,
    lang: &str,
    hub: &Hub,
    context: &AppContext,
) {
    let notify_failure = || {
        hub.send(
            (Event::Notification(NotificationEvent::Show(fl!(
                "notification-dictionary-install-failed",
                lang = lang
            ))))
            .into(),
        )
        .ok();
    };
    let entry = match service.get_entry_for_lang(lang).await {
        Ok(Some(entry)) => entry,
        Ok(None) => {
            tracing::warn!(lang, "No metadata entry found for language; cannot install");
            notify_failure();
            return;
        }
        Err(error) => {
            tracing::warn!(lang, error = %error, "Failed to look up dictionary entry");
            notify_failure();
            return;
        }
    };

    if !service.try_begin_install(lang) {
        return;
    }

    let service = service.clone();
    let lang_owned = lang.to_string();
    let hub = hub.clone();
    let parent_span = tracing::Span::current();
    let wifi_session = context.wifi_session.clone();
    let download_id = ViewId::MessageNotif(ID_FEEDER.next());
    hub.send(
        (Event::Notification(NotificationEvent::ShowPinned(
            download_id,
            fl!("notification-downloading-dictionary", lang = lang),
        )))
        .into(),
    )
    .ok();

    if let Some(previous) = job.as_ref() {
        previous.cancel();
    }

    let download_cancel = crate::http::CancelFlag::new();
    let span = tracing::info_span!(parent: &parent_span, "dictionary_install_async");
    *job = Some(crate::runtime::Job::with_token(
        download_cancel.cancellation_token(),
        move |cancel| {
            async move {
                if cancel.is_cancelled() {
                    service.finish_install(&lang_owned);
                    hub.send((Event::Close(download_id)).into()).ok();
                    return;
                }
                let _wifi = match wifi_session.acquire("dictionary-download").await {
                    Ok(lease) => lease,
                    Err(error) => {
                        tracing::error!(
                            error = %error,
                            "Failed to acquire WiFi lease for dictionary download"
                        );
                        service.finish_install(&lang_owned);
                        hub.send((Event::Close(download_id)).into()).ok();
                        hub.send(
                            (Event::DictionaryInstallComplete {
                                lang: lang_owned,
                                result: Err(error.to_string()),
                            })
                            .into(),
                        )
                        .ok();
                        return;
                    }
                };

                let result = service
                    .install_reserved_dictionary(
                        &lang_owned,
                        &entry,
                        false,
                        &mut |downloaded, total| {
                            use humanize_bytes::humanize_bytes_decimal;
                            let downloaded_str = humanize_bytes_decimal!(downloaded).to_string();
                            let total_str = humanize_bytes_decimal!(total).to_string();
                            let percent = (downloaded as f64 / total as f64 * 100.0) as u8;
                            hub.send(
                                (Event::Notification(NotificationEvent::UpdateProgress(
                                    download_id,
                                    percent,
                                )))
                                .into(),
                            )
                            .ok();
                            hub.send(
                                (Event::Notification(NotificationEvent::UpdateText(
                                    download_id,
                                    fl!(
                                        "notification-downloading-dictionary-progress",
                                        lang = lang_owned.as_str(),
                                        downloaded = downloaded_str.as_str(),
                                        total = total_str.as_str()
                                    ),
                                )))
                                .into(),
                            )
                            .ok();
                        },
                        Some(&download_cancel),
                    )
                    .await
                    .map_err(|error| error.to_string());

                hub.send((Event::Close(download_id)).into()).ok();
                hub.send(
                    (Event::DictionaryInstallComplete {
                        lang: lang_owned,
                        result,
                    })
                    .into(),
                )
                .ok();
            }
            .instrument(span)
        },
    ));
}
