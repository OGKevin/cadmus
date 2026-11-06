//! Kernel wake-IRQ reads for deep-idle telemetry.
//!
//! Reads the two Linux nodes that name the IRQ behind a wake: the IRQ the kernel
//! recorded (`/sys/power/pm_wakeup_irq`) and that IRQ's owner in
//! `/proc/interrupts`. The RTC's own wake-alarm slot is read by
//! [`LinuxRtc`](super::LinuxRtc)'s [`Rtc::wake_alarm`](crate::device::rtc::Rtc::wake_alarm).
//!
//! `pm_wakeup_irq` fails the read with `-ENODATA` while the kernel's recorded
//! IRQ is zero, so an unavailable node means "not reported", not "no wake".

const PM_WAKEUP_IRQ_PATH: &str = "/sys/power/pm_wakeup_irq";
const PROC_INTERRUPTS_PATH: &str = "/proc/interrupts";

/// Reads the IRQ the kernel recorded as the primary wake source.
pub(crate) async fn read_wakeup_irq() -> Option<u32> {
    parse_wakeup_irq(&read_node(PM_WAKEUP_IRQ_PATH).await?)
}

/// Resolves the device name that owns `irq` from `/proc/interrupts`.
pub(crate) async fn read_irq_name(irq: u32) -> Option<String> {
    parse_irq_name(&read_node(PROC_INTERRUPTS_PATH).await?, irq)
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
fn parse_irq_name(interrupts: &str, irq: u32) -> Option<String> {
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
