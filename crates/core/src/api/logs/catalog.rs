//! The logs the device can show, and the IDs that name them.
//!
//! A client never names a path. A Run log's ID is its run ID, and a Launcher
//! log's ID is its fixed name, so an ID resolves to exactly one file inside the
//! directory it belongs to, whatever text the client sends.

use buffa_types::google::protobuf::Timestamp;
use cadmus_api_proto::cadmus::api::public::v1::{LogFile, LogKind};
use std::path::PathBuf;
use std::time::UNIX_EPOCH;
use uuid::Uuid;

const RUN_PREFIX: &str = "cadmus-";
const RUN_SUFFIX: &str = ".json";
const LAUNCHER_LOGS: [(&str, &str); 2] = [("info", "info.log"), ("archive", "archive.log")];

/// Where the logs live for this Companion session.
pub(in crate::api) struct LogSources {
    run_dir: PathBuf,
    launcher_dir: Option<PathBuf>,
    current_run_id: String,
}

/// A listed log resolved to its file.
pub(super) struct Resolved {
    pub(super) path: PathBuf,
    pub(super) current: bool,
}

impl LogSources {
    /// `launcher_dir` holds `info.log` and `archive.log`, on devices whose
    /// launcher script writes them.
    pub(in crate::api) fn new(
        run_dir: PathBuf,
        launcher_dir: Option<PathBuf>,
        current_run_id: String,
    ) -> Self {
        Self {
            run_dir,
            launcher_dir,
            current_run_id,
        }
    }

    /// The Current run first, then the other Run logs newest first, then the
    /// Launcher logs that exist.
    ///
    /// A missing logs directory lists as empty: logging may be off. Any other
    /// read failure is logged and still lists the Launcher logs.
    pub(super) async fn list(&self) -> std::io::Result<Vec<LogFile>> {
        let mut runs = Vec::new();
        match tokio::fs::read_dir(&self.run_dir).await {
            Ok(mut entries) => loop {
                match entries.next_entry().await {
                    Ok(Some(entry)) => {
                        let name = entry.file_name().to_string_lossy().into_owned();
                        let Some(run_id) = run_id_of(&name) else {
                            continue;
                        };
                        let Ok(metadata) = entry.metadata().await else {
                            continue;
                        };
                        if metadata.is_file() {
                            runs.push(self.run_log(name, run_id, &metadata));
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        tracing::warn!(error = %error, "run log listing stopped");
                        break;
                    }
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!(error = %error, "run logs unreadable"),
        }
        runs.sort_by(|a, b| {
            b.current
                .cmp(&a.current)
                .then_with(|| start_rank(b).cmp(&start_rank(a)))
                .then_with(|| b.name.cmp(&a.name))
        });
        if let Some(dir) = &self.launcher_dir {
            for (id, name) in LAUNCHER_LOGS {
                if let Ok(metadata) = tokio::fs::metadata(dir.join(name)).await
                    && metadata.is_file()
                {
                    runs.push(LogFile {
                        id: id.to_owned(),
                        kind: LogKind::LOG_KIND_LAUNCHER.into(),
                        name: name.to_owned(),
                        size: metadata.len(),
                        modified: modified_timestamp(&metadata).into(),
                        ..Default::default()
                    });
                }
            }
        }
        Ok(runs)
    }

    fn run_log(&self, name: String, run_id: Uuid, metadata: &std::fs::Metadata) -> LogFile {
        let mut buffer = Uuid::encode_buffer();
        let id = run_id.hyphenated().encode_lower(&mut buffer).to_owned();
        LogFile {
            current: id == self.current_run_id,
            kind: LogKind::LOG_KIND_RUN.into(),
            size: metadata.len(),
            modified: modified_timestamp(metadata).into(),
            started: started_timestamp(&run_id).into(),
            run_id: id.clone(),
            id,
            name,
            ..Default::default()
        }
    }

    /// The file a listed ID names, or `None` for any other text.
    ///
    /// The file may have been deleted since the listing; opening it reports
    /// that.
    pub(super) fn resolve(&self, id: &str) -> Option<Resolved> {
        if let Some((_, name)) = LAUNCHER_LOGS.iter().find(|(known, _)| *known == id) {
            let dir = self.launcher_dir.as_deref()?;
            return Some(Resolved {
                path: dir.join(name),
                current: false,
            });
        }
        let run_id = canonical_run_id(id)?;
        let mut buffer = Uuid::encode_buffer();
        let canonical: &str = run_id.hyphenated().encode_lower(&mut buffer);
        Some(Resolved {
            path: self
                .run_dir
                .join(format!("{RUN_PREFIX}{canonical}{RUN_SUFFIX}")),
            current: canonical == self.current_run_id,
        })
    }
}

/// A run ID spelled the one way the device issues it: lowercase, hyphenated.
fn canonical_run_id(id: &str) -> Option<Uuid> {
    let uuid = Uuid::parse_str(id).ok()?;
    let mut buffer = Uuid::encode_buffer();
    (uuid.hyphenated().encode_lower(&mut buffer) == id).then_some(uuid)
}

fn run_id_of(file_name: &str) -> Option<Uuid> {
    canonical_run_id(
        file_name
            .strip_prefix(RUN_PREFIX)?
            .strip_suffix(RUN_SUFFIX)?,
    )
}

/// Sort key of a run's start: later starts first.
fn start_rank(log: &LogFile) -> (i64, i32) {
    (log.started.seconds, log.started.nanos)
}

/// Last modification of a file, when the platform reports one.
fn modified_timestamp(metadata: &std::fs::Metadata) -> Option<Timestamp> {
    let elapsed = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    Some(Timestamp {
        seconds: elapsed.as_secs() as i64,
        nanos: elapsed.subsec_nanos() as i32,
        ..Default::default()
    })
}

/// Start of a run, from the time in its v7 run ID.
fn started_timestamp(run_id: &Uuid) -> Option<Timestamp> {
    let (seconds, nanos) = run_id.get_timestamp()?.to_unix();
    Some(Timestamp {
        seconds: seconds as i64,
        nanos: nanos as i32,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const OLD: &str = "01a1f426-9a23-7673-b3c4-b840e4ad2e1a";
    const NEW: &str = "01a20edc-8083-75d7-b900-35f14f330c68";

    fn sources(dir: &Path, launcher: Option<&Path>) -> LogSources {
        LogSources::new(
            dir.to_path_buf(),
            launcher.map(Path::to_path_buf),
            NEW.to_owned(),
        )
    }

    fn touch(dir: &Path, name: &str, bytes: &[u8]) {
        std::fs::write(dir.join(name), bytes).unwrap();
    }

    #[tokio::test]
    async fn run_logs_list_newest_first_with_the_current_run_marked() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), &format!("cadmus-{OLD}.json"), b"{}\n");
        touch(dir.path(), &format!("cadmus-{NEW}.json"), b"{}\n{}\n");
        let logs = sources(dir.path(), None).list().await.unwrap();
        let names: Vec<&str> = logs.iter().map(|log| log.name.as_str()).collect();
        assert_eq!(
            names,
            [format!("cadmus-{NEW}.json"), format!("cadmus-{OLD}.json")]
        );
        assert!(logs[0].current);
        assert!(!logs[1].current);
        assert_eq!((logs[0].size, logs[1].size), (6, 3));
        assert_eq!(logs[0].run_id, NEW);
        assert!(logs[0].modified.is_set());
        assert!(logs[0].started.seconds > logs[1].started.seconds);
    }

    #[tokio::test]
    async fn the_current_run_lists_first_even_when_it_started_earlier() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), &format!("cadmus-{OLD}.json"), b"{}\n");
        touch(dir.path(), &format!("cadmus-{NEW}.json"), b"{}\n");
        let logs = LogSources::new(dir.path().to_path_buf(), None, OLD.to_owned())
            .list()
            .await
            .unwrap();
        let names: Vec<&str> = logs.iter().map(|log| log.name.as_str()).collect();
        assert_eq!(
            names,
            [format!("cadmus-{OLD}.json"), format!("cadmus-{NEW}.json")]
        );
        assert!(logs[0].current);
        assert!(!logs[1].current);
        assert!(start_rank(&logs[0]) < start_rank(&logs[1]));
    }

    #[tokio::test]
    async fn launcher_logs_follow_the_run_logs_when_they_exist() {
        let run = tempfile::tempdir().unwrap();
        let install = tempfile::tempdir().unwrap();
        touch(run.path(), &format!("cadmus-{NEW}.json"), b"{}\n");
        touch(install.path(), "info.log", b"x\n");
        let logs = sources(run.path(), Some(install.path()))
            .list()
            .await
            .unwrap();
        let ids: Vec<&str> = logs.iter().map(|log| log.id.as_str()).collect();
        assert_eq!(ids, [NEW, "info"]);
        assert_eq!(logs[1].kind.as_known(), Some(LogKind::LOG_KIND_LAUNCHER));
        assert!(logs[1].started.is_unset());
    }

    #[tokio::test]
    async fn files_that_are_not_run_logs_are_not_listed() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "cadmus-not-a-uuid.json", b"{}");
        touch(dir.path(), &format!("cadmus-{NEW}.json.gz"), b"{}");
        touch(
            dir.path(),
            &format!("cadmus-{}.json", NEW.to_uppercase()),
            b"{}",
        );
        touch(dir.path(), "notes.txt", b"{}");
        std::fs::create_dir(dir.path().join(format!("cadmus-{OLD}.json"))).unwrap();
        assert!(sources(dir.path(), None).list().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_unreadable_logs_directory_still_lists_launcher_logs() {
        let dir = tempfile::tempdir().unwrap();
        let not_a_dir = dir.path().join("logs");
        std::fs::write(&not_a_dir, b"file, not a directory").unwrap();
        let install = tempfile::tempdir().unwrap();
        touch(install.path(), "info.log", b"x\n");
        let logs = sources(&not_a_dir, Some(install.path()))
            .list()
            .await
            .unwrap();
        let ids: Vec<&str> = logs.iter().map(|log| log.id.as_str()).collect();
        assert_eq!(ids, ["info"]);
    }

    #[tokio::test]
    async fn a_missing_logs_directory_lists_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("never-created");
        assert!(sources(&missing, None).list().await.unwrap().is_empty());
    }

    #[test]
    fn only_listed_kinds_of_id_resolve() {
        let dir = tempfile::tempdir().unwrap();
        let install = tempfile::tempdir().unwrap();
        let sources = sources(dir.path(), Some(install.path()));
        let run = sources.resolve(NEW).unwrap();
        assert_eq!(run.path, dir.path().join(format!("cadmus-{NEW}.json")));
        assert!(run.current);
        assert_eq!(
            sources.resolve("info").unwrap().path,
            install.path().join("info.log")
        );
        for bad in [
            "",
            "../etc/passwd",
            "/etc/passwd",
            "info.log",
            "cadmus-01a20edc-8083-75d7-b900-35f14f330c68.json",
            "01a20edc-8083-75d7-b900-35f14f330c68/../x",
        ] {
            assert!(sources.resolve(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn launcher_ids_do_not_resolve_without_a_launcher_directory() {
        let dir = tempfile::tempdir().unwrap();
        assert!(sources(dir.path(), None).resolve("info").is_none());
    }

    #[test]
    fn only_the_canonical_spelling_of_a_run_id_resolves() {
        let dir = tempfile::tempdir().unwrap();
        let sources = sources(dir.path(), None);
        let canonical = sources.resolve(NEW).unwrap().path;
        assert_eq!(canonical, dir.path().join(format!("cadmus-{NEW}.json")));
        for spelling in [
            NEW.replace('-', ""),
            NEW.to_uppercase(),
            format!("{{{NEW}}}"),
            format!("urn:uuid:{NEW}"),
        ] {
            assert!(sources.resolve(&spelling).is_none(), "{spelling}");
        }
    }
}
