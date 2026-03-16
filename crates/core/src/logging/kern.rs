//! Kernel log capture.
//!
//! This module provides functionality to capture kernel logs and forward them
//! to the tracing system.
//!
//! By default, if no platform-specific implementation is available, the module
//! logs a debug message indicating that kernel log capture is not implemented
//! for the current platform.
//!
//! ## Example Log Retreival
//!
//! ```sh
//! [root@monza cadmus-tst]# cat logs/cadmus-019cf7e3-ef3a-7752-846f-83b92ac90634.json | jq '. | select(.target == "cadmus_core::logging::kern")| .fields.body' -r | tail
//! Mar 16 18:31:49 kernel: [ 3854.620803] -(0)[0:swapper/0][wlan] In HIF ISR.
//! Mar 16 18:31:50 kernel: [ 3855.188306] -(0)[2715:main_thread][wlan][2715]wlanDumpBssStatistics:(SW4 INFO) LLS BSS[0] BE: T[053937] R[000000] T_D[000000] T_F[000000]
//! Mar 16 18:31:50 kernel: [ 3855.188331] -(0)[2715:main_thread][wlan][2715]wlanDumpBssStatistics:(SW4 INFO) LLS BSS[0] BK: T[001878] R[000000] T_D[000000] T_F[000000]
//! Mar 16 18:31:50 kernel: [ 3855.188342] -(0)[2715:main_thread][wlan][2715]wlanDumpBssStatistics:(SW4 INFO) LLS BSS[0] VI: T[000000] R[000000] T_D[000000] T_F[000000]
//! Mar 16 18:31:50 kernel: [ 3855.188353] -(0)[2715:main_thread][wlan][2715]wlanDumpBssStatistics:(SW4 INFO) LLS BSS[0] VO: T[000004] R[000000] T_D[000000] T_F[000000]
//! Mar 16 18:31:50 kernel: [ 3855.188485] .(0)[2716:hif_thread]padding for alignment
//! Mar 16 18:31:50 kernel: [ 3855.641193] .(0)[2715:main_thread][wlan][2715]nicEventLayer0ExtMagic:(NIC INFO) Amsdu update event ucWlanIdx[1] ucLen[0] ucMaxMpduCount[0]
//! Mar 16 18:31:50 kernel: [ 3855.842447] .(0)[2715:main_thread][wlan][2715]nicEventLayer0ExtMagic:(NIC INFO) Amsdu update event ucWlanIdx[1] ucLen[0] ucMaxMpduCount[0]
//! Mar 16 18:31:51 kernel: [ 3856.636823] -(0)[0:swapper/0]mtk_axi_interrupt: 252 callbacks suppressed
//! Mar 16 18:31:51 kernel: [ 3856.636842] -(0)[0:swapper/0][wlan] In HIF ISR.
//! ```

/// Parsed kernel log entry with extracted fields.
#[derive(Debug, PartialEq)]
#[cfg(all(feature = "kobo", feature = "test"))]
struct ParsedKernelLog {
    /// The log timestamp (e.g., "Mar 16 17:30:46")
    pub timestamp: String,
    /// System uptime in seconds (e.g., "1293.879480")
    pub uptime: String,
    /// Process ID (e.g., "0")
    pub pid: String,
    /// Thread ID (e.g., "1697")
    pub thread_id: String,
    /// Thread name (e.g., "hif_thread", "main_thread", "kworker/0:3")
    pub thread: String,
    /// Kernel subsystem (e.g., "wlan") - may be empty
    pub subsystem: String,
    /// The actual log message
    pub message: String,
}

/// Parses a Kobo kernel log line into structured fields.
///
/// Supports multiple log formats:
/// - Kernel format: `<timestamp> kernel: [<uptime>] -(<pid>)[<thread_id>][<thread>][<subsystem>] <message>`
/// - Service format: `<timestamp> <service>[<pid>]: <message>`
///
/// Returns `Some(ParsedKernelLog)` if the line matches the expected format,
/// or `None` if the line doesn't match.
#[cfg(all(feature = "kobo", feature = "test"))]
fn parse_kern_log(line: &str) -> Option<ParsedKernelLog> {
    use regex::Regex;

    lazy_static::lazy_static! {
        // Kernel log format: Mar 16 17:30:46 kernel: [ 1293.879480] -(0)[1697:hif_thread][wlan] In HIF ISR.
        static ref KERNEL_RE: Regex = Regex::new(
            r"^(\w{3}\s+\d{1,2}\s+\d{2}:\d{2}:\d{2})\s+\w+:\s+\[\s*(\d+\.\d+)\]\s+[.-]\((\d+)\)\[(\d+):([^\]]+)\](?:\[(\w+)\])?(?:\[\d+\])?\s*(.+)$"
        ).unwrap();

        // Generic service log format: Mar 16 17:39:06 wpa_supplicant[2000]: wlan0: CTRL-EVENT...
        static ref SERVICE_RE: Regex = Regex::new(
            r"^(\w{3}\s+\d{1,2}\s+\d{2}:\d{2}:\d{2})\s+(\w+)\[(\d+)\]:\s+(.+)$"
        ).unwrap();
    }

    if let Some(caps) = KERNEL_RE.captures(line) {
        return Some(ParsedKernelLog {
            timestamp: caps.get(1)?.as_str().to_string(),
            uptime: caps.get(2)?.as_str().to_string(),
            pid: caps.get(3)?.as_str().to_string(),
            thread_id: caps.get(4)?.as_str().to_string(),
            thread: caps.get(5)?.as_str().to_string(),
            subsystem: caps
                .get(6)
                .map(|m| m.as_str().to_string())
                .unwrap_or_default(),
            message: caps.get(7)?.as_str().to_string(),
        });
    }

    if let Some(caps) = SERVICE_RE.captures(line) {
        return Some(ParsedKernelLog {
            timestamp: caps.get(1)?.as_str().to_string(),
            uptime: "".to_string(),
            pid: caps.get(3)?.as_str().to_string(),
            thread_id: "".to_string(),
            thread: caps.get(2)?.as_str().to_string(),
            subsystem: "".to_string(),
            message: caps.get(4)?.as_str().to_string(),
        });
    }

    None
}

/// Spawns a background thread that captures kernel logs.
///
/// # Platform-specific behavior
///
/// ## Kobo
/// When the `kobo` feature is enabled, this starts `klogd` to enable kernel
/// logging, then runs `logread -F` to read kernel log messages line by line.
///
/// The kernel log format used on Kobo devices:
/// `<timestamp> kernel: [<uptime>] -(<pid>)[<thread_id>][<thread>][<subsystem>] <message>`
///
/// Examples:
/// - `Mar 16 17:30:46 kernel: [ 1293.879480] -(0)[1697:hif_thread][wlan] In HIF ISR.`
/// - `Mar 16 17:30:46 kernel: [ 1293.131642] .(0)[1696:main_thread][wlan][1696]wlanPktTxDone:...`
///
/// Parsed fields:
/// - `timestamp`: The log timestamp (e.g., "Mar 16 17:30:46")
/// - `uptime`: System uptime in seconds (e.g., "1293.879480")
/// - `pid`: Process ID (e.g., "0")
/// - `thread_id`: Thread ID (e.g., "1697")
/// - `thread`: Thread name (e.g., "hif_thread", "main_thread", "kworker/0:3")
/// - `subsystem`: Kernel subsystem (e.g., "wlan") - may be empty
/// - `message`: The actual log message
#[cfg(all(feature = "kobo", feature = "test"))]
pub fn spawn_kern_log_thread() {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    use std::thread;

    fn is_process_running(name: &str) -> bool {
        Command::new("pgrep")
            .arg("-x")
            .arg(name)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .and_then(|mut child| child.wait().map(|status| status.success()))
            .unwrap_or(false)
    }

    let klogd_running = is_process_running("klogd");
    if klogd_running {
        tracing::info!("klogd already running, reusing existing process");
    }

    thread::spawn(move || {
        tracing::info!("Starting kernel log capture thread");

        let klogd = if klogd_running {
            None
        } else {
            match Command::new("klogd").spawn() {
                Ok(child) => Some(child),
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to start klogd");
                    None
                }
            }
        };

        let mut child = match Command::new("logread")
            .arg("-F")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(e) => {
                tracing::warn!(error = %e, "Failed to start logread command");
                return;
            }
        };

        let stdout = match child.stdout.take() {
            Some(stdout) => stdout,
            None => {
                tracing::warn!("Failed to capture logread stdout");
                return;
            }
        };

        let reader = BufReader::new(stdout);

        for line in reader.lines() {
            match line {
                Ok(line) => {
                    if let Some(parsed) = parse_kern_log(&line) {
                        tracing::debug!(
                            body = %line,
                            timestamp = %parsed.timestamp,
                            uptime = %parsed.uptime,
                            pid = %parsed.pid,
                            thread_id = %parsed.thread_id,
                            thread = %parsed.thread,
                            subsystem = %parsed.subsystem,
                            message = %parsed.message,
                        );
                    } else {
                        tracing::debug!("{}", line);
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Error reading from logread");
                    break;
                }
            }
        }

        tracing::info!("Kernel log capture thread ending");

        let _ = child.wait();
        if let Some(mut klogd) = klogd {
            let _ = klogd.kill();
            let _ = klogd.wait();
        }
    });
}

#[cfg(all(not(feature = "kobo"), feature = "test"))]
pub fn spawn_kern_log_thread() {
    tracing::debug!("Kernel log capture is a no-op on non-Kobo platforms");
}

#[cfg(test)]
#[cfg(all(feature = "kobo", feature = "test"))]
mod tests {
    use super::*;

    #[test]
    fn test_parse_kern_log_basic() {
        let line = "Mar 16 17:30:46 kernel: [ 1293.879480] -(0)[1697:hif_thread][wlan] In HIF ISR.";
        let parsed = parse_kern_log(line).unwrap();

        assert_eq!(parsed.timestamp, "Mar 16 17:30:46");
        assert_eq!(parsed.uptime, "1293.879480");
        assert_eq!(parsed.pid, "0");
        assert_eq!(parsed.thread_id, "1697");
        assert_eq!(parsed.thread, "hif_thread");
        assert_eq!(parsed.subsystem, "wlan");
        assert_eq!(parsed.message, "In HIF ISR.");
    }

    #[test]
    fn test_parse_kern_log_with_dot_prefix() {
        let line = "Mar 16 17:30:46 kernel: [ 1293.131642] .(0)[1696:main_thread][wlan][1696]wlanPktTxDone:(TX INFO) TX DONE, Type[ARP] Tag[0xea769a80] WIDX:PID[1:15] Status[0], SeqNo: 15<1576 -> 1862> ";
        let parsed = parse_kern_log(line).unwrap();

        assert_eq!(parsed.timestamp, "Mar 16 17:30:46");
        assert_eq!(parsed.uptime, "1293.131642");
        assert_eq!(parsed.pid, "0");
        assert_eq!(parsed.thread_id, "1696");
        assert_eq!(parsed.thread, "main_thread");
        assert_eq!(parsed.subsystem, "wlan");
        assert!(parsed.message.contains("wlanPktTxDone"));
    }

    #[test]
    fn test_parse_kern_log_without_subsystem() {
        let line = "Mar 16 17:30:46 kernel: [ 1293.879468] -(0)[1697:hif_thread]mtk_axi_interrupt: 191 callbacks suppressed";
        let parsed = parse_kern_log(line).unwrap();

        assert_eq!(parsed.timestamp, "Mar 16 17:30:46");
        assert_eq!(parsed.uptime, "1293.879468");
        assert_eq!(parsed.pid, "0");
        assert_eq!(parsed.thread_id, "1697");
        assert_eq!(parsed.thread, "hif_thread");
        assert_eq!(parsed.subsystem, "");
        assert!(parsed.message.contains("mtk_axi_interrupt"));
    }

    #[test]
    fn test_parse_kern_log_kworker() {
        let line = "Mar 16 17:30:42 kernel: [ 1289.462571] .(0)[1409:kworker/0:3]bd71827-power bd_work_callback()";
        let parsed = parse_kern_log(line).unwrap();

        assert_eq!(parsed.timestamp, "Mar 16 17:30:42");
        assert_eq!(parsed.uptime, "1289.462571");
        assert_eq!(parsed.pid, "0");
        assert_eq!(parsed.thread_id, "1409");
        assert_eq!(parsed.thread, "kworker/0:3");
        assert_eq!(parsed.subsystem, "");
        assert!(parsed.message.contains("bd71827-power"));
    }

    #[test]
    fn test_parse_kern_log_unparseable() {
        let line = "Some random log line without expected format";
        let parsed = parse_kern_log(line);
        assert!(parsed.is_none());
    }

    #[test]
    fn test_parse_kern_log_empty_line() {
        let line = "";
        let parsed = parse_kern_log(line);
        assert!(parsed.is_none());
    }

    #[test]
    fn test_parse_kern_log_wpa_supplicant() {
        let line = "Mar 16 17:39:06 wpa_supplicant[2000]: wlan0: CTRL-EVENT-REGDOM-CHANGE init=COUNTRY_IE type=COUNTRY alpha2=NL";
        let parsed = parse_kern_log(line).unwrap();

        assert_eq!(parsed.timestamp, "Mar 16 17:39:06");
        assert_eq!(parsed.uptime, "");
        assert_eq!(parsed.pid, "2000");
        assert_eq!(parsed.thread_id, "");
        assert_eq!(parsed.thread, "wpa_supplicant");
        assert_eq!(parsed.subsystem, "");
        assert!(parsed.message.contains("CTRL-EVENT-REGDOM-CHANGE"));
    }

    #[test]
    fn test_parse_generic_service_log() {
        let line = "Mar 16 17:39:06 GenericService[1500]: <info> connectivity check";
        let parsed = parse_kern_log(line).unwrap();

        assert_eq!(parsed.timestamp, "Mar 16 17:39:06");
        assert_eq!(parsed.uptime, "");
        assert_eq!(parsed.pid, "1500");
        assert_eq!(parsed.thread_id, "");
        assert_eq!(parsed.thread, "GenericService");
        assert_eq!(parsed.subsystem, "");
        assert!(parsed.message.contains("connectivity check"));
    }
}
