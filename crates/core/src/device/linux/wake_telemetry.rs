//! Kernel wake-IRQ reads for deep-idle telemetry.
//!
//! Reads the two Linux nodes that name the IRQ behind a wake: the IRQ the kernel
//! recorded (`/sys/power/pm_wakeup_irq`) and that IRQ's owner in
//! `/proc/interrupts`. The RTC's own wake-alarm slot is read by
//! [`LinuxRtc`](super::LinuxRtc)'s [`Rtc::wake_alarm`](crate::device::rtc::Rtc::wake_alarm).
//!
//! [`InterruptSnapshot`] parses numeric `/proc/interrupts` rows so a deep-idle
//! wait can compare per-IRQ counters taken before sleep and after wake.
//!
//! `pm_wakeup_irq` fails the read with `-ENODATA` while the kernel's recorded
//! IRQ is zero. Other read errors and malformed content also yield `None`, so
//! an unavailable value means "not reported", not "no wake".

use std::collections::{BTreeMap, BTreeSet, HashSet, btree_map::Entry};

const PM_WAKEUP_IRQ_PATH: &str = "/sys/power/pm_wakeup_irq";
#[cfg(not(test))]
const PROC_INTERRUPTS_PATH: &str = "/proc/interrupts";
type IrqRows = BTreeMap<String, Result<InterruptRow, ()>>;

/// Reads the IRQ the kernel recorded as the primary wake source.
pub(crate) async fn read_wakeup_irq() -> Option<u32> {
    parse_wakeup_irq(&read_node(PM_WAKEUP_IRQ_PATH).await?)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InterruptSnapshot {
    cpus: Vec<String>,
    rows: BTreeMap<String, Result<InterruptRow, ()>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InterruptRow {
    counts: Vec<u64>,
    name: Option<String>,
}

#[derive(Debug, PartialEq, Eq, serde::Serialize)]
pub(crate) struct InterruptDelta {
    irq: String,
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    before_name: Option<String>,
    delta: u64,
}

#[derive(Debug, PartialEq, Eq, serde::Serialize)]
pub(crate) struct InterruptDeltaReason {
    irq: String,
    reason: &'static str,
}

impl InterruptDeltaReason {
    fn new(irq: &str, reason: &'static str) -> Self {
        Self {
            irq: irq.to_owned(),
            reason,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct InterruptDeltas {
    status: &'static str,
    deltas: Vec<InterruptDelta>,
    reasons: Vec<InterruptDeltaReason>,
}

impl InterruptSnapshot {
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        parse_interrupt_snapshot(raw)
    }

    pub(crate) fn compare(&self, later: Option<&Self>) -> InterruptDeltas {
        let Some(later) = later.filter(|later| self.cpus == later.cpus) else {
            return InterruptDeltas {
                status: "unavailable",
                deltas: Vec::new(),
                reasons: Vec::new(),
            };
        };
        let mut reasons = Vec::new();
        let deltas = self
            .rows
            .keys()
            .chain(later.rows.keys())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .filter_map(|irq| {
                let (before, after) = match (self.rows.get(irq), later.rows.get(irq)) {
                    (Some(Err(())), _) => {
                        reasons.push(InterruptDeltaReason::new(irq, "unparseable_before"));
                        return None;
                    }
                    (_, Some(Err(()))) => {
                        reasons.push(InterruptDeltaReason::new(irq, "unparseable_after"));
                        return None;
                    }
                    (Some(Ok(before)), Some(Ok(after))) => (before, after),
                    (Some(Ok(_)), None) => {
                        reasons.push(InterruptDeltaReason::new(irq, "missing_after"));
                        return None;
                    }
                    (None, Some(Ok(_))) => {
                        reasons.push(InterruptDeltaReason::new(irq, "missing_before"));
                        return None;
                    }
                    (None, None) => return None,
                };
                match delta(before, after) {
                    Some(delta) if delta > 0 => Some(InterruptDelta {
                        irq: irq.clone(),
                        name: after.name.clone(),
                        before_name: if before.name != after.name {
                            before.name.clone()
                        } else {
                            None
                        },
                        delta,
                    }),
                    Some(_) => None,
                    None => {
                        reasons.push(InterruptDeltaReason::new(irq, "counter_not_monotonic"));
                        None
                    }
                }
            })
            .collect();
        InterruptDeltas {
            status: if reasons.is_empty() {
                "complete"
            } else {
                "partial"
            },
            deltas,
            reasons,
        }
    }
}

fn delta(before: &InterruptRow, after: &InterruptRow) -> Option<u64> {
    before
        .counts
        .iter()
        .zip(&after.counts)
        .try_fold(0_u64, |sum, (&before, &after)| {
            sum.checked_add(after.checked_sub(before)?)
        })
}

impl InterruptDeltas {
    pub(crate) fn unavailable() -> Self {
        Self {
            status: "unavailable",
            deltas: Vec::new(),
            reasons: Vec::new(),
        }
    }

    /// Serializes the delta and reason lists as JSON strings for the wake log.
    ///
    /// Stable `tracing` fields accept primitives, strings, `Display`, and
    /// `Debug`, but not arbitrary `serde::Serialize` values. Nested structured
    /// fields are available through `tracing`'s `valuable` integration, but that
    /// requires the unstable `--cfg tracing_unstable` flag. Encoding here keeps
    /// the logging stack stable and lets log consumers decode either list with
    /// `fromjson`; the status is returned separately so it can be filtered
    /// without decoding.
    pub(crate) fn json(self) -> (&'static str, String, String) {
        let serialized = serde_json::to_string(&self.deltas).and_then(|deltas| {
            serde_json::to_string(&self.reasons).map(|reasons| (deltas, reasons))
        });
        match serialized {
            Ok((deltas, reasons)) => (self.status, deltas, reasons),
            Err(error) => {
                tracing::debug!(%error, "interrupt delta serialization failed");
                ("unavailable", "[]".to_owned(), "[]".to_owned())
            }
        }
    }
}

/// Parses CPU-ordered numeric IRQ rows for a per-wait counter comparison.
///
/// Nonnumeric `/proc/interrupts` summary rows do not participate. Duplicate CPU
/// columns have no unambiguous mapping; absent or malformed numeric rows make
/// a comparison partial.
///
/// TODO: Investigate upstreaming `/proc/interrupts` parsing to `procfs`.
fn parse_interrupt_snapshot(raw: &str) -> Option<InterruptSnapshot> {
    let Some(header) = raw.lines().next() else {
        tracing::warn!(reason = "empty input", "interrupt snapshot unavailable");
        return None;
    };
    let cpus = header
        .split_whitespace()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if cpus.is_empty()
        || cpus.iter().any(|cpu| {
            cpu.strip_prefix("CPU").is_none_or(|number| {
                number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit())
            })
        })
    {
        tracing::warn!(
            header,
            reason = "invalid CPU header",
            "interrupt snapshot unavailable"
        );
        return None;
    }
    let unique_cpus = cpus.iter().collect::<HashSet<_>>();
    if unique_cpus.len() != cpus.len() {
        tracing::warn!(
            header,
            reason = "duplicate CPU columns",
            "interrupt snapshot unavailable"
        );
        return None;
    }
    let mut rows = IrqRows::new();
    for (irq, row) in raw
        .lines()
        .skip(1)
        .filter_map(|line| parse_interrupt_row(line, &cpus))
    {
        match rows.entry(irq) {
            Entry::Occupied(mut existing) => {
                tracing::warn!(
                    irq = %existing.key(),
                    reason = "duplicate IRQ label",
                    "unparseable interrupt row"
                );
                *existing.get_mut() = Err(());
            }
            Entry::Vacant(slot) => {
                slot.insert(row);
            }
        }
    }
    if !rows.values().any(Result::is_ok) {
        tracing::warn!(
            reason = "no valid numeric IRQ rows",
            "interrupt snapshot unavailable"
        );
        return None;
    }
    Some(InterruptSnapshot { cpus, rows })
}

/// Returns a row even when its counts cannot be compared, so missing data is
/// visibly partial rather than confused with a complete zero-delta sample.
fn parse_interrupt_row(line: &str, cpus: &[String]) -> Option<(String, Result<InterruptRow, ()>)> {
    let Some((label, remainder)) = line.split_once(':') else {
        if !line.trim().is_empty() {
            tracing::warn!(
                line,
                reason = "line without colon",
                "unparseable interrupt row"
            );
        }
        return None;
    };
    let label = label.trim();
    if label.is_empty() || !label.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let counts = remainder
        .split_whitespace()
        .take(cpus.len())
        .filter_map(|count| count.parse::<u64>().ok())
        .collect::<Vec<_>>();
    if counts.len() != cpus.len() {
        tracing::warn!(
            irq = label,
            expected = cpus.len(),
            found = counts.len(),
            reason = "missing or malformed counts",
            "unparseable interrupt row"
        );
        return Some((label.to_owned(), Err(())));
    }
    let name = remainder
        .split_whitespace()
        .last()
        .filter(|name| !names_no_action(name))
        .map(ToOwned::to_owned);
    Some((label.to_owned(), Ok(InterruptRow { counts, name })))
}

#[cfg(not(test))]
pub(crate) async fn read_interrupts() -> Option<String> {
    read_node(PROC_INTERRUPTS_PATH).await
}

async fn read_node(path: &str) -> Option<String> {
    match tokio::fs::read_to_string(path).await {
        Ok(raw) => Some(raw),
        Err(error) => {
            tracing::debug!(error = %error, path, "wake telemetry node unavailable");
            None
        }
    }
}

/// Parses `/sys/power/pm_wakeup_irq` content.
fn parse_wakeup_irq(raw: &str) -> Option<u32> {
    raw.trim().parse().ok()
}

/// Resolves the device name that owns `irq` from `/proc/interrupts` content.
pub(crate) fn parse_irq_name(interrupts: &str, irq: u32) -> Option<String> {
    let prefix = format!("{irq}:");
    interrupts.lines().find_map(|line| {
        let rest = line.trim_start().strip_prefix(&prefix)?;
        let name = rest.split_whitespace().next_back()?;
        (!names_no_action(name)).then(|| name.to_owned())
    })
}

/// Whether a row's last field is a column value rather than an action name.
///
/// `show_interrupts` ends an action-less row with the trigger type, and with a
/// per-CPU count when the row carries no chip or trigger column at all.
fn names_no_action(field: &str) -> bool {
    matches!(field, "Level" | "Edge") || field.bytes().all(|byte| byte.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    const BEFORE: &str = "           CPU0       CPU1
 90:        10          0      MTK   90 Level      bd71828_irq
195:         2          8      bd71828   195 Edge      bd71828_pwrkey
213:         4          4      bd71828   213 Edge      rtc-alarm
";

    const AFTER: &str = "           CPU0       CPU1
 90:        12          1      MTK   90 Level      bd71828_irq
195:         3         10      bd71828   195 Edge      bd71828_pwrkey
213:         4          4      bd71828   213 Edge      rtc-alarm
";

    #[test]
    fn interrupt_snapshot_ignores_nonnumeric_summary_rows() {
        let with_summaries = format!("{BEFORE}ERR: 1\nNMI: 2\n");
        assert_eq!(
            InterruptSnapshot::parse(&with_summaries),
            InterruptSnapshot::parse(BEFORE),
        );
    }

    #[test]
    fn interrupt_snapshot_ignores_empty_labels() {
        let with_empty_label = format!("{BEFORE}: 1 2 foo\n");
        assert_eq!(
            InterruptSnapshot::parse(&with_empty_label),
            InterruptSnapshot::parse(BEFORE),
        );
    }

    #[test]
    fn interrupt_deltas_sum_increases_across_cpus() {
        let before = InterruptSnapshot::parse(BEFORE).unwrap();
        let after = InterruptSnapshot::parse(AFTER).unwrap();
        let comparison = before.compare(Some(&after));

        assert_eq!(comparison.status, "complete");
        assert!(comparison.reasons.is_empty());
        assert_eq!(
            serde_json::to_value(&comparison.deltas).unwrap(),
            json!([
                {"irq": "195", "name": "bd71828_pwrkey", "delta": 3},
                {"irq": "90", "name": "bd71828_irq", "delta": 3},
            ]),
        );
    }

    #[test]
    fn interrupt_deltas_report_null_name_for_actionless_rows() {
        let before = InterruptSnapshot::parse("CPU0 CPU1\n300: 1 2\n").unwrap();
        let after = InterruptSnapshot::parse("CPU0 CPU1\n300: 3 2\n").unwrap();

        let comparison = before.compare(Some(&after));

        assert_eq!(comparison.status, "complete");
        assert_eq!(
            serde_json::to_value(&comparison.deltas).unwrap(),
            json!([{"irq": "300", "name": null, "delta": 2}]),
        );
    }

    #[test]
    fn interrupt_deltas_keep_counters_when_only_the_action_changes() {
        let before = InterruptSnapshot::parse(
            "CPU0
195: 1 old
196: 1 bd71828_pwrkey
 48: 1 elan_ktf
",
        )
        .unwrap();
        let after = InterruptSnapshot::parse(
            "CPU0
195: 3 new
196: 3 bd71828_pwrkey
 48: 3
",
        )
        .unwrap();

        let comparison = before.compare(Some(&after));

        assert_eq!(comparison.status, "complete");
        assert!(comparison.reasons.is_empty());
        assert_eq!(
            serde_json::to_value(&comparison.deltas).unwrap(),
            json!([
                {"irq": "195", "name": "new", "before_name": "old", "delta": 2},
                {"irq": "196", "name": "bd71828_pwrkey", "delta": 2},
                {"irq": "48", "name": null, "before_name": "elan_ktf", "delta": 2},
            ]),
        );
    }

    #[test]
    fn interrupt_deltas_ignore_action_change_without_a_count_increase() {
        let before = InterruptSnapshot::parse("CPU0\n195: 1 old\n").unwrap();
        let after = InterruptSnapshot::parse("CPU0\n195: 1 new\n").unwrap();

        let comparison = before.compare(Some(&after));

        assert_eq!(comparison.status, "complete");
        assert!(comparison.deltas.is_empty());
        assert!(comparison.reasons.is_empty());
    }

    #[test]
    fn interrupt_deltas_sum_single_cpu_increases() {
        let before = InterruptSnapshot::parse(
            "           CPU0
 90:        10      MTK   90 Level      bd71828_irq
195:         2      bd71828   195 Edge      bd71828_pwrkey
",
        )
        .unwrap();
        let after = InterruptSnapshot::parse(
            "           CPU0
 90:        12      MTK   90 Level      bd71828_irq
195:         3      bd71828   195 Edge      bd71828_pwrkey
",
        )
        .unwrap();

        let comparison = before.compare(Some(&after));

        assert_eq!(comparison.status, "complete");
        assert_eq!(
            serde_json::to_value(&comparison.deltas).unwrap(),
            json!([
                {"irq": "195", "name": "bd71828_pwrkey", "delta": 1},
                {"irq": "90", "name": "bd71828_irq", "delta": 2},
            ]),
        );
    }

    #[test]
    fn interrupt_deltas_reject_sum_overflow_across_cpus() {
        let before =
            InterruptSnapshot::parse("CPU0 CPU1\n195: 0 0 bd71828 bd71828_pwrkey\n").unwrap();
        let after = InterruptSnapshot::parse(
            "CPU0 CPU1\n195: 18446744073709551615 1 bd71828 bd71828_pwrkey\n",
        )
        .unwrap();

        let comparison = before.compare(Some(&after));

        assert_eq!(comparison.status, "partial");
        assert!(comparison.deltas.is_empty());
    }

    #[test]
    fn interrupt_deltas_report_unavailable_snapshots_and_headers() {
        let blank = " \n";
        let no_rows = "           CPU0       CPU1\nERR: 5\nNMI: invalid\n1: invalid other stuff\n";
        let duplicate_header = "           CPU0       CPU0\n1: 1 1\n";
        let duplicate_rows_only =
            "           CPU0       CPU1\n195: 2 8 bd71828_pwrkey\n195: 0 8 bd71828_pwrkey\n";
        let baseline = InterruptSnapshot::parse(BEFORE).unwrap();
        let changed_cpu = InterruptSnapshot::parse(&AFTER.replace("CPU1", "CPU2")).unwrap();
        let reordered_cpu = InterruptSnapshot::parse(&format!(
            "           CPU1       CPU0\n 90: {}  {} MTK 90 Level bd71828_irq\n",
            0, 12
        ))
        .unwrap();

        for raw in ["", blank, no_rows, duplicate_header, duplicate_rows_only] {
            assert_eq!(InterruptSnapshot::parse(raw), None, "{raw:?}");
        }
        let comparison = baseline.compare(None);
        assert_eq!(comparison.status, "unavailable");
        assert!(comparison.deltas.is_empty());
        assert!(comparison.reasons.is_empty());
        for snapshot in [
            baseline.compare(Some(&changed_cpu)),
            baseline.compare(Some(&reordered_cpu)),
        ] {
            assert_eq!(snapshot.status, "unavailable");
            assert!(snapshot.deltas.is_empty());
            assert!(snapshot.reasons.is_empty());
        }
    }

    #[test]
    fn interrupt_deltas_omit_changed_rows_and_report_partial() {
        let cases = [
            (
                BEFORE.replace("195:     ", "195x:     "),
                "non-numeric label",
            ),
            (
                BEFORE.replace("195:         2", "195:         x"),
                "malformed count",
            ),
            (
                BEFORE.replace("195:         2          8", "195:         2"),
                "missing count",
            ),
            (
                BEFORE.replace("195:         2", "195:         4"),
                "counter decrease",
            ),
            (
                BEFORE.replace("195:         2", "195: 18446744073709551615"),
                "counter decrease",
            ),
            (
                BEFORE.replace("195:         2", "195: 18446744073709551614"),
                "counter decrease from near-maximum",
            ),
            (
                BEFORE.replace(
                    "195:         2          8",
                    "195:         2 18446744073709551615",
                ),
                "counter decrease from maximum on one CPU",
            ),
            (
                BEFORE.replace(
                    "195:         2          8      bd71828   195 Edge      bd71828_pwrkey\n",
                    "",
                ),
                "missing row",
            ),
            (format!("{BEFORE}419: 0 1 PMIC irq-added\n"), "added row"),
            (
                format!(
                    "{BEFORE}195:         0          0      bd71828   195 Edge      bd71828_pwrkey\n"
                ),
                "duplicate row",
            ),
        ];

        for (before_text, reason) in cases {
            let before = InterruptSnapshot::parse(&before_text).expect(reason);
            let after = InterruptSnapshot::parse(AFTER).unwrap();
            let comparison = before.compare(Some(&after));
            assert_eq!(comparison.status, "partial", "{reason}");
            assert_eq!(
                comparison
                    .deltas
                    .iter()
                    .map(|delta| delta.irq.as_str())
                    .collect::<Vec<_>>(),
                if reason == "added row" {
                    vec!["195", "90"]
                } else {
                    vec!["90"]
                },
                "{reason}",
            );
        }
    }

    #[test]
    fn interrupt_deltas_report_partial_reasons() {
        let before = InterruptSnapshot::parse(
            "CPU0
 90: 1 a
195: 1 old
196: 1 b
197: 5 c
198: invalid d
200: 1 f
",
        )
        .unwrap();
        let after = InterruptSnapshot::parse(
            "CPU0
 90: 2 a
195: 1 new
197: 4 c
198: 1 d
199: 1 e
200: invalid f
",
        )
        .unwrap();

        let comparison = before.compare(Some(&after));

        assert_eq!(comparison.status, "partial");
        assert_eq!(
            serde_json::to_value(&comparison.deltas).unwrap(),
            json!([{"irq": "90", "name": "a", "delta": 1}]),
        );
        assert_eq!(
            serde_json::to_value(&comparison.reasons).unwrap(),
            json!([
                {"irq": "196", "reason": "missing_after"},
                {"irq": "197", "reason": "counter_not_monotonic"},
                {"irq": "198", "reason": "unparseable_before"},
                {"irq": "199", "reason": "missing_before"},
                {"irq": "200", "reason": "unparseable_after"},
            ]),
        );
    }

    #[test]
    fn interrupt_deltas_exclude_new_rows_without_a_baseline() {
        let before = InterruptSnapshot::parse(BEFORE).unwrap();
        let after = InterruptSnapshot::parse(&format!("{AFTER}419: 0 1 PMIC irq-added\n")).unwrap();
        let comparison = before.compare(Some(&after));
        assert_eq!(comparison.status, "partial");
        assert_eq!(
            comparison
                .deltas
                .iter()
                .map(|delta| delta.irq.as_str())
                .collect::<Vec<_>>(),
            vec!["195", "90"],
        );
    }

    #[test]
    fn interrupt_deltas_preserve_irrelevant_malformed_summary_rows() {
        let before = InterruptSnapshot::parse(&format!("{BEFORE}ERR: 1\nNMI: 2\n")).unwrap();
        let after = InterruptSnapshot::parse(AFTER).unwrap();
        assert_eq!(before.compare(Some(&after)).status, "complete");
    }

    #[test]
    fn interrupt_deltas_complete_when_nothing_changes() {
        let comparison = InterruptSnapshot::parse(BEFORE)
            .unwrap()
            .compare(Some(&InterruptSnapshot::parse(BEFORE).unwrap()));
        assert_eq!(comparison.status, "complete");
        assert!(comparison.deltas.is_empty());
    }

    #[test]
    fn interrupt_deltas_compare_reordered_rows_by_irq() {
        let before = InterruptSnapshot::parse(BEFORE).unwrap();
        let mut rows = AFTER.lines();
        let header = rows.next().unwrap();
        let reordered = std::iter::once(header)
            .chain(rows.rev())
            .collect::<Vec<_>>()
            .join("\n");
        let after = InterruptSnapshot::parse(&reordered).unwrap();
        let comparison = before.compare(Some(&after));
        assert_eq!(comparison.status, "complete");
        assert_eq!(
            comparison
                .deltas
                .iter()
                .map(|delta| (delta.irq.as_str(), delta.delta))
                .collect::<Vec<_>>(),
            vec![("195", 3), ("90", 3)],
        );
    }

    #[test]
    fn wakeup_irq_parses_reported_number() {
        assert_eq!(parse_wakeup_irq("90\n"), Some(90));
        assert_eq!(parse_wakeup_irq("  45  "), Some(45));
    }

    #[test]
    fn wakeup_irq_rejects_absent_and_malformed_values() {
        assert_eq!(parse_wakeup_irq(""), None);
        assert_eq!(parse_wakeup_irq(" \n"), None);
        assert_eq!(parse_wakeup_irq("No data available\n"), None);
        assert_eq!(parse_wakeup_irq("-1\n"), None);
    }

    #[test]
    fn irq_name_resolves_matching_row() {
        let table = "           CPU0       CPU1
 45:        874          0      MT_SYSIRQ  45 Level      kx122
 46:         12          0      MT_SYSIRQ  46 Level      bd70528-rtc
";
        assert_eq!(parse_irq_name(table, 45).as_deref(), Some("kx122"));
        assert_eq!(parse_irq_name(table, 46).as_deref(), Some("bd70528-rtc"));
    }

    #[test]
    fn irq_name_absent_for_unknown_or_prefix_matching_rows() {
        let table = "           CPU0
 45:        874      MT_SYSIRQ  45 Level      kx122
";
        assert_eq!(parse_irq_name(table, 4), None);
        assert_eq!(parse_irq_name(table, 999), None);
        assert_eq!(parse_irq_name("", 45), None);
    }

    #[test]
    fn irq_name_absent_when_row_has_no_action() {
        assert_eq!(
            parse_irq_name("  7:   3   0   MT_SYSIRQ   7 Level\n", 7),
            None
        );
        assert_eq!(parse_irq_name("  8:   3   0\n", 8), None);
    }
}
