//! How an open pull request gets an install answer.
//!
//! A pull is installable only when a live (not expired) Kobo artifact for that
//! PR number is still on GitHub. Draft does not change that answer. The
//! mapping is pure so it can be tested without the network. The screen draws
//! [`CheckedPull`] directly.

/// Result of the install check for one pull request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrBuild {
    /// The list is on screen and the install check has not finished.
    Pending,
    /// A Cargo run for this commit is still going, and no live artifact exists.
    Building,
    /// A non-expired Kobo artifact for this PR is available.
    Ready,
    /// A Cargo run finished unsuccessfully, and no live artifact exists.
    Failed,
    /// Nothing installable: no candidate run, or the artifact has expired.
    NoBuild,
    /// The install check failed. The row stays visible and cannot be installed.
    Unknown,
}

/// The artifact a ready row will download.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrArtifact {
    /// GitHub artifact id.
    pub id: u64,
    /// Uncompressed size GitHub reported, in bytes.
    pub size_in_bytes: u64,
}

/// One open pull request and the install check's answer for it.
///
/// The screen copies this into the row it draws. [`Self::build`] does not
/// depend on [`Self::draft`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedPull {
    /// Pull request number.
    pub number: u64,
    /// Pull request title. Empty when GitHub omitted it.
    pub title: String,
    /// GitHub marked the pull request as a draft.
    pub draft: bool,
    /// Install check. Independent of [`Self::draft`].
    pub build: PrBuild,
    /// Present only when [`Self::build`] is [`PrBuild::Ready`].
    pub artifact: Option<PrArtifact>,
}

impl CheckedPull {
    /// A tap installs this pull request.
    pub fn selectable(&self) -> bool {
        self.build == PrBuild::Ready && self.artifact.is_some()
    }
}

/// One open pull request before its install check.
///
/// GitHub returns these newest-activity first. The install check fills in
/// [`CheckedPull::build`] afterwards and keeps this order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenPull {
    /// Pull request number.
    pub number: u64,
    /// Title, or empty when GitHub omitted it.
    pub title: String,
    /// GitHub marked the pull request as a draft.
    pub draft: bool,
    /// Head commit SHA. Joins this pull request to its Cargo runs.
    pub head_sha: String,
}

/// A workflow run reduced to the fields the picker reads.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RunSnapshot<'a> {
    /// Commit the run built.
    pub head_sha: &'a str,
    /// Workflow name. Only `"Cargo"` counts.
    pub name: &'a str,
    /// GitHub status, such as `completed` or `in_progress`.
    pub status: &'a str,
    /// GitHub conclusion, when the run has one.
    pub conclusion: Option<&'a str>,
}

/// A repository artifact reduced to the fields the picker reads.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ArtifactSnapshot<'a> {
    /// Artifact file name.
    pub name: &'a str,
    /// GitHub artifact id.
    pub id: u64,
    /// Size in bytes.
    pub size_in_bytes: u64,
    /// GitHub has expired the artifact.
    pub expired: bool,
}

/// Name stem of the Kobo package built for a pull request.
///
/// Test builds use a different stem so they do not install over a release
/// package of the same number.
pub(crate) fn pr_artifact_stem() -> &'static str {
    cfg_select! {
        feature = "test" => "cadmus-kobo-test-pr",
        _ => "cadmus-kobo-pr",
    }
}

/// Name prefix of the Kobo package built for `pr_number`.
pub(crate) fn pr_artifact_prefix(pr_number: u64) -> String {
    format!("{}{pr_number}", pr_artifact_stem())
}

/// True when `name` might be the Kobo package of some pull request.
pub(crate) fn artifact_name_matches_any_pr(name: &str) -> bool {
    name.starts_with(pr_artifact_stem())
}

/// True when `name` is the Kobo package for `pr_number`.
///
/// `cadmus-kobo-pr42` matches `cadmus-kobo-pr42` and `cadmus-kobo-pr42-abcdef`.
/// It does not match `cadmus-kobo-pr420`.
pub(crate) fn artifact_name_matches(name: &str, pr_number: u64) -> bool {
    let prefix = pr_artifact_prefix(pr_number);
    let Some(rest) = name.strip_prefix(&prefix) else {
        return false;
    };
    rest.is_empty() || !rest.as_bytes().first().is_some_and(u8::is_ascii_digit)
}

/// Joins one pull request's commit to the Cargo runs and repository artifacts
/// already fetched.
pub(crate) fn classify_pr(
    pr_number: u64,
    head_sha: &str,
    runs: &[RunSnapshot<'_>],
    artifacts: &[ArtifactSnapshot<'_>],
) -> (PrBuild, Option<PrArtifact>) {
    if let Some(artifact) = live_artifact(pr_number, artifacts) {
        return (PrBuild::Ready, Some(artifact));
    }

    let mut building = false;
    let mut failed = false;
    for run in runs.iter().filter(|run| is_candidate(run, head_sha)) {
        if run.status == "in_progress" {
            building = true;
        } else if run.conclusion != Some("success") {
            failed = true;
        }
    }

    if building {
        (PrBuild::Building, None)
    } else if failed {
        (PrBuild::Failed, None)
    } else {
        (PrBuild::NoBuild, None)
    }
}

/// Every pull request, before the install check has an answer.
#[cfg_attr(
    feature = "tracing",
    tracing::instrument(skip_all, fields(pulls = pulls.len()))
)]
pub(crate) fn pending_pulls(pulls: &[OpenPull]) -> Vec<CheckedPull> {
    pulls
        .iter()
        .map(|pull| CheckedPull {
            number: pull.number,
            title: pull.title.clone(),
            draft: pull.draft,
            build: PrBuild::Pending,
            artifact: None,
        })
        .collect()
}

/// Classifies `pulls` against the runs and artifacts fetched so far.
///
/// Until `settled` is set, a pull is ready only when a live package is already
/// in `artifacts`. Every other pull stays [`PrBuild::Pending`].
#[cfg_attr(
    feature = "tracing",
    tracing::instrument(skip_all, fields(pulls = pulls.len(), settled))
)]
pub(crate) fn project_pulls(
    pulls: &[OpenPull],
    runs: &[RunSnapshot<'_>],
    artifacts: &[ArtifactSnapshot<'_>],
    settled: bool,
) -> Vec<CheckedPull> {
    pulls
        .iter()
        .map(|pull| {
            let (mut build, mut artifact) =
                classify_pr(pull.number, &pull.head_sha, runs, artifacts);
            if !settled && build != PrBuild::Ready {
                build = PrBuild::Pending;
                artifact = None;
            }
            CheckedPull {
                number: pull.number,
                title: pull.title.clone(),
                draft: pull.draft,
                build,
                artifact,
            }
        })
        .collect()
}

/// Keeps ready pulls. Every other pull becomes [`PrBuild::Unknown`].
#[cfg_attr(
    feature = "tracing",
    tracing::instrument(skip_all, fields(pulls = pulls.len()))
)]
pub(crate) fn unknown_unless_ready(pulls: Vec<CheckedPull>) -> Vec<CheckedPull> {
    pulls
        .into_iter()
        .map(|mut pull| {
            if pull.build == PrBuild::Ready {
                pull
            } else {
                pull.build = PrBuild::Unknown;
                pull.artifact = None;
                pull
            }
        })
        .collect()
}

fn live_artifact(pr_number: u64, artifacts: &[ArtifactSnapshot<'_>]) -> Option<PrArtifact> {
    artifacts.iter().find_map(|artifact| {
        if artifact.expired || !artifact_name_matches(artifact.name, pr_number) {
            return None;
        }
        Some(PrArtifact {
            id: artifact.id,
            size_in_bytes: artifact.size_in_bytes,
        })
    })
}

fn is_candidate(run: &RunSnapshot<'_>, head_sha: &str) -> bool {
    run.head_sha == head_sha
        && run.name == "Cargo"
        && matches!(run.status, "completed" | "in_progress")
        && !matches!(run.conclusion, Some("cancelled") | Some("skipped"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run<'a>(head_sha: &'a str, status: &'a str, conclusion: Option<&'a str>) -> RunSnapshot<'a> {
        RunSnapshot {
            head_sha,
            name: "Cargo",
            status,
            conclusion,
        }
    }

    fn artifact<'a>(name: &'a str, expired: bool) -> ArtifactSnapshot<'a> {
        ArtifactSnapshot {
            name,
            id: 9,
            size_in_bytes: 100,
            expired,
        }
    }

    #[test]
    fn a_ready_artifact_is_selectable_and_a_pending_row_is_not() {
        let ready = CheckedPull {
            number: 1,
            title: "t".into(),
            draft: true,
            build: PrBuild::Ready,
            artifact: Some(PrArtifact {
                id: 2,
                size_in_bytes: 3,
            }),
        };
        assert!(ready.selectable());
        let pending = pending_pulls(&[OpenPull {
            number: 1,
            title: "t".into(),
            draft: false,
            head_sha: "h".into(),
        }]);
        assert!(!pending[0].selectable());
    }

    #[test]
    fn a_live_artifact_is_ready_even_when_the_pull_request_is_a_draft() {
        let name = format!("{}-abc", pr_artifact_prefix(42));
        let (build, artifact) = classify_pr(42, "sha", &[], &[artifact(&name, false)]);
        assert_eq!(build, PrBuild::Ready);
        assert_eq!(
            artifact,
            Some(PrArtifact {
                id: 9,
                size_in_bytes: 100
            })
        );
    }

    #[test]
    fn an_expired_artifact_is_not_ready() {
        let name = pr_artifact_prefix(42);
        let (build, artifact) = classify_pr(
            42,
            "sha",
            &[run("sha", "completed", Some("success"))],
            &[artifact(&name, true)],
        );
        assert_eq!(build, PrBuild::NoBuild);
        assert!(artifact.is_none());
    }

    #[test]
    fn a_shorter_pull_request_number_does_not_take_a_longer_ones_artifact() {
        let name = pr_artifact_prefix(420);
        assert!(!artifact_name_matches(&name, 42));
        assert!(artifact_name_matches(&name, 420));
    }

    #[test]
    fn an_in_progress_run_without_an_artifact_is_building() {
        let (build, _) = classify_pr(7, "sha", &[run("sha", "in_progress", None)], &[]);
        assert_eq!(build, PrBuild::Building);
    }

    #[test]
    fn a_failed_run_without_an_artifact_is_failed() {
        let (build, _) = classify_pr(7, "sha", &[run("sha", "completed", Some("failure"))], &[]);
        assert_eq!(build, PrBuild::Failed);
    }

    #[test]
    fn a_successful_run_whose_artifact_is_gone_is_no_build() {
        let (build, _) = classify_pr(7, "sha", &[run("sha", "completed", Some("success"))], &[]);
        assert_eq!(build, PrBuild::NoBuild);
    }

    #[test]
    fn a_cancelled_run_is_ignored() {
        let (build, _) = classify_pr(7, "sha", &[run("sha", "completed", Some("cancelled"))], &[]);
        assert_eq!(build, PrBuild::NoBuild);
    }

    #[test]
    fn a_run_for_another_commit_is_ignored() {
        let (build, _) = classify_pr(7, "sha", &[run("other", "in_progress", None)], &[]);
        assert_eq!(build, PrBuild::NoBuild);
    }

    #[test]
    fn building_wins_over_an_older_failure_when_nothing_is_live() {
        let (build, _) = classify_pr(
            7,
            "sha",
            &[
                run("sha", "completed", Some("failure")),
                run("sha", "in_progress", None),
            ],
            &[],
        );
        assert_eq!(build, PrBuild::Building);
    }

    #[test]
    fn a_live_artifact_stays_ready_while_a_newer_run_is_building() {
        let name = pr_artifact_prefix(7);
        let (build, artifact) = classify_pr(
            7,
            "sha",
            &[run("sha", "in_progress", None)],
            &[artifact(&name, false)],
        );
        assert_eq!(build, PrBuild::Ready);
        assert!(artifact.is_some());
    }

    fn pull(number: u64, head_sha: &str) -> OpenPull {
        OpenPull {
            number,
            title: format!("Change {number}"),
            draft: false,
            head_sha: head_sha.to_string(),
        }
    }

    #[test]
    fn an_unsettled_pull_without_an_artifact_stays_pending() {
        let rows = project_pulls(
            &[pull(7, "sha")],
            &[run("sha", "in_progress", None)],
            &[],
            false,
        );
        assert_eq!(rows[0].build, PrBuild::Pending);
        assert!(rows[0].artifact.is_none());
    }

    #[test]
    fn an_unsettled_pull_with_a_live_artifact_is_ready() {
        let name = pr_artifact_prefix(7);
        let rows = project_pulls(&[pull(7, "sha")], &[], &[artifact(&name, false)], false);
        assert_eq!(rows[0].build, PrBuild::Ready);
        assert!(rows[0].artifact.is_some());
    }

    #[test]
    fn a_settled_pull_without_an_artifact_is_classified() {
        let rows = project_pulls(
            &[pull(7, "sha")],
            &[run("sha", "in_progress", None)],
            &[],
            true,
        );
        assert_eq!(rows[0].build, PrBuild::Building);
    }

    #[test]
    fn a_failed_check_keeps_only_ready_rows() {
        let name = pr_artifact_prefix(1);
        let rows = unknown_unless_ready(project_pulls(
            &[pull(1, "sha"), pull(2, "other")],
            &[],
            &[artifact(&name, false)],
            false,
        ));
        assert_eq!(rows[0].build, PrBuild::Ready);
        assert_eq!(rows[1].build, PrBuild::Unknown);
        assert!(rows[1].artifact.is_none());
    }
}
