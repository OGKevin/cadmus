use anyhow::Error;
use chrono_tz::Tz;
use std::ffi::CString;
use std::path::Path;

const ZONEINFO_DIR: &str = "/etc/zoneinfo";
const LOCALTIME: &str = "/etc/localtime";
const TIMEZONE_FILE: &str = "/etc/timezone";

unsafe extern "C" {
    fn tzset();
}

/// Sets the system timezone on Kobo devices.
///
/// # Kobo
///
/// This function performs three operations to ensure the timezone change is persistent and
/// immediately reflected in the running process:
///
/// 1. Creates `/etc/localtime` as a symlink to the appropriate zoneinfo file (recognized by
///    system utilities and the C library).
/// 2. Writes the timezone name to `/etc/timezone` for persistence across reboots.
/// 3. Updates the current process's timezone state by setting the `TZ` environment variable
///    and calling `libc::tzset()`. This ensures that subsequent calls to libc time functions
///    in this process reflect the new timezone immediately, rather than the system-wide
///    timezone being adopted only after the process restarts.
///
/// # Errors
///
/// Returns an error if any filesystem operations fail.
pub fn set_system_timezone(tz: Tz) -> Result<(), Error> {
    cfg_select! {
        feature = "kobo" => {
            let tz_name = tz.to_string();
            let tz_path = Path::new(ZONEINFO_DIR).join(&tz_name);

            std::fs::remove_file(LOCALTIME).ok();
            std::os::unix::fs::symlink(&tz_path, LOCALTIME)?;
            std::fs::write(TIMEZONE_FILE, &tz_name)?;

            unsafe {
                let tz_cstr = CString::new(tz_name.as_str())?;
                libc::setenv(c"TZ".as_ptr(), tz_cstr.as_ptr(), 1);
                tzset();
            }

            tracing::info!(tz = %tz_name, "system timezone updated");
            Ok(())
        }
        _ => unimplemented!("set_system_timezone is only available on Kobo devices")
    }
}
