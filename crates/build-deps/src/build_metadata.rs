//! Compile-time build provenance derived from the repository.
//!
//! Cargo passes these values to rustc as `cargo:rustc-env`, so they are part
//! of the compiler invocation that build caches hash. Every value here is
//! derived from committed state instead of the clock, which keeps rebuilds of
//! the same commit cacheable.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::git::open_repo;

const UNKNOWN_COMMIT: &str = "unknown";
const SOURCE_DATE_EPOCH_ENV: &str = "SOURCE_DATE_EPOCH";

/// The commit a build was produced from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildProvenance {
    /// Committer time of the commit, in seconds since the Unix epoch.
    pub commit_unix_secs: u64,
    /// Full commit id, or `"unknown"` outside a repository.
    pub commit_id: String,
}

/// Reads the provenance of `HEAD` in the repository containing `root`.
///
/// Falls back to [`SOURCE_DATE_EPOCH_ENV`] and then to epoch zero for the
/// timestamp, and `"unknown"` for the commit id, so archive builds stay
/// deterministic.
pub fn commit_provenance(root: &Path) -> BuildProvenance {
    let repo = open_repo(root);
    let commit = repo.as_ref().and_then(|repo| repo.head_commit().ok());

    BuildProvenance {
        commit_unix_secs: commit
            .as_ref()
            .and_then(|commit| commit.time().ok())
            .and_then(|time| u64::try_from(time.seconds).ok())
            .or_else(source_date_epoch)
            .unwrap_or(0),
        commit_id: commit.map_or_else(
            || UNKNOWN_COMMIT.to_string(),
            |commit| commit.id().detach().to_string(),
        ),
    }
}

/// Builds the settings-ordering uuid for this build.
///
/// The value is a UUIDv7 whose timestamp is `timestamp_unix_secs` — the same
/// value embedded as `BUILD_TIMESTAMP` — so string order matches build order.
/// Its payload derives from the commit id and enabled features, so repeated
/// builds of the same inputs agree.
pub fn build_uuid(
    provenance: &BuildProvenance,
    features: &[String],
    timestamp_unix_secs: u64,
) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(provenance.commit_id.as_bytes());
    for feature in features {
        hasher.update(feature.as_bytes());
        hasher.update(&[0]);
    }

    let digest = hasher.finalize();
    let payload: [u8; 10] = digest.as_bytes()[..10]
        .try_into()
        .expect("blake3 digest is longer than the uuid payload");

    uuid::Builder::from_unix_timestamp_millis(timestamp_unix_secs.saturating_mul(1000), &payload)
        .into_uuid()
        .to_string()
}

/// Returns the build timestamp embedded in the binary, in Unix epoch seconds.
///
/// Release builds keep the real build time for provenance. Every other profile
/// uses the commit time so the crate stays cacheable.
pub fn build_timestamp(provenance: &BuildProvenance, release: bool) -> u64 {
    if !release {
        return provenance.commit_unix_secs;
    }

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn source_date_epoch() -> Option<u64> {
    std::env::var(SOURCE_DATE_EPOCH_ENV)
        .ok()?
        .trim()
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gix::bstr::ByteSlice;

    fn init_repo(dir: &Path) -> gix::Repository {
        std::fs::create_dir_all(dir).unwrap();
        gix::init(dir).expect("init repo")
    }

    fn commit_at(repo: &gix::Repository, secs: u32) -> gix::ObjectId {
        let time = format!("{secs} +0000");
        let signature = gix::actor::SignatureRef {
            name: b"test".as_bstr(),
            email: b"test@example.com".as_bstr(),
            time: time.as_str(),
        };
        repo.commit_as(
            signature,
            signature,
            "HEAD",
            "commit",
            repo.empty_tree().id().detach(),
            None::<gix::ObjectId>,
        )
        .expect("commit")
        .detach()
    }

    fn provenance_at(dir: &Path, secs: u32) -> BuildProvenance {
        let repo = init_repo(dir);
        commit_at(&repo, secs);
        commit_provenance(dir)
    }

    #[test]
    fn commit_provenance_reads_head_committer_time_and_id() {
        let tmp = tempfile::tempdir().unwrap();
        let provenance = provenance_at(tmp.path(), 1_700_000_000);

        assert_eq!(provenance.commit_unix_secs, 1_700_000_000);

        let repo = open_repo(tmp.path()).expect("open repo");
        let expected = repo.head_commit().expect("head commit").id().detach();
        assert_eq!(provenance.commit_id, expected.to_string());
        assert_eq!(expected.to_string().len(), 40);
    }

    #[test]
    fn commit_provenance_reports_unknown_outside_a_repository() {
        let tmp = tempfile::tempdir().unwrap();
        let first = commit_provenance(tmp.path());
        let second = commit_provenance(tmp.path());

        assert_eq!(first.commit_id, UNKNOWN_COMMIT);
        assert_eq!(first, second);
        if std::env::var(SOURCE_DATE_EPOCH_ENV).is_err() {
            assert_eq!(first.commit_unix_secs, 0);
        }
    }

    fn provenance() -> BuildProvenance {
        BuildProvenance {
            commit_unix_secs: 1_700_000_000,
            commit_id: "a".repeat(40),
        }
    }

    #[test]
    fn build_uuid_is_deterministic_for_the_same_inputs() {
        let provenance = provenance();
        let features = vec!["emulator".to_string(), "tracing".to_string()];
        let timestamp = build_timestamp(&provenance, false);

        assert_eq!(
            build_uuid(&provenance, &features, timestamp),
            build_uuid(&provenance, &features, timestamp)
        );
    }

    #[test]
    fn build_uuid_sorts_by_build_timestamp() {
        let older = BuildProvenance {
            commit_unix_secs: 1_700_000_000,
            commit_id: "a".repeat(40),
        };
        let newer = BuildProvenance {
            commit_unix_secs: 1_700_000_001,
            commit_id: "a".repeat(40),
        };

        assert!(
            build_uuid(&older, &[], 1_700_000_000) < build_uuid(&newer, &[], 1_700_000_001),
            "uuid order must follow the build timestamp, not the commit id"
        );
    }

    #[test]
    fn build_uuid_uses_the_supplied_timestamp() {
        let provenance = provenance();
        let parsed = uuid::Uuid::parse_str(&build_uuid(&provenance, &[], 1_800_000_000))
            .expect("valid uuid");
        let (secs, _) = parsed
            .get_timestamp()
            .expect("v7 uuid carries a timestamp")
            .to_unix();

        assert_eq!(secs, 1_800_000_000);
    }

    #[test]
    fn build_uuid_differs_for_different_feature_sets() {
        let provenance = provenance();

        assert_ne!(
            build_uuid(&provenance, &["emulator".to_string()], 1_700_000_000),
            build_uuid(
                &provenance,
                &["emulator".to_string(), "tracing".to_string()],
                1_700_000_000
            )
        );
    }

    #[test]
    fn build_uuid_is_a_version_7_uuid() {
        let parsed = uuid::Uuid::parse_str(&build_uuid(&provenance(), &[], 1_700_000_000))
            .expect("valid uuid");
        assert_eq!(parsed.get_version_num(), 7);
    }

    #[test]
    fn build_timestamp_uses_commit_time_for_non_release_builds() {
        assert_eq!(build_timestamp(&provenance(), false), 1_700_000_000);
    }

    #[test]
    fn build_timestamp_uses_wall_clock_for_release_builds() {
        let provenance = provenance();
        assert!(build_timestamp(&provenance, true) >= provenance.commit_unix_secs);
    }
}
