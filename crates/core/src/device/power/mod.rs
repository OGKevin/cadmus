//! Power management submodule.

mod error;
mod manager;

pub use error::PowerError;
pub use manager::PowerManager;

#[cfg(unix)]
pub(crate) fn discover_cores(
    cpu_dir: &std::path::Path,
) -> Result<Vec<(std::path::PathBuf, String)>, std::io::Error> {
    let mut discovered = Vec::new();
    if !cpu_dir.is_dir() {
        return Ok(discovered);
    }

    for entry in std::fs::read_dir(cpu_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let file_name = match path.file_name().and_then(|s| s.to_str()) {
            Some(name) => name,
            None => continue,
        };

        if !file_name.starts_with("cpu") {
            continue;
        }
        let core_id_str = &file_name[3..];
        if !core_id_str.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }

        let online_path = path.join("online");
        if !online_path.is_file() {
            continue;
        }

        if let Ok(state_str) = std::fs::read_to_string(&online_path) {
            let trimmed = state_str.trim().to_string();
            discovered.push((online_path, trimmed));
        }
    }

    Ok(discovered)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn test_discover_cores() {
        let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
        let cpu_dir = temp_dir.path();

        let cpu0_dir = cpu_dir.join("cpu0");
        std::fs::create_dir(&cpu0_dir).expect("failed to create cpu0 dir");

        let cpu1_dir = cpu_dir.join("cpu1");
        std::fs::create_dir(&cpu1_dir).expect("failed to create cpu1 dir");
        let cpu1_online = cpu1_dir.join("online");
        std::fs::write(&cpu1_online, "0\n").expect("failed to write cpu1 online");

        let cpu2_dir = cpu_dir.join("cpu2");
        std::fs::create_dir(&cpu2_dir).expect("failed to create cpu2 dir");
        let cpu2_online = cpu2_dir.join("online");
        std::fs::write(&cpu2_online, "1").expect("failed to write cpu2 online");

        let not_cpu_dir = cpu_dir.join("not_cpu");
        std::fs::create_dir(&not_cpu_dir).expect("failed to create not_cpu dir");
        let not_cpu_online = not_cpu_dir.join("online");
        std::fs::write(&not_cpu_online, "1").expect("failed to write not_cpu online");

        let mut cores = discover_cores(cpu_dir).expect("failed to discover cores");
        cores.sort_by(|a, b| a.0.cmp(&b.0));

        assert_eq!(cores.len(), 2);
        assert_eq!(cores[0].0, cpu1_online);
        assert_eq!(cores[0].1, "0");
        assert_eq!(cores[1].0, cpu2_online);
        assert_eq!(cores[1].1, "1");
    }
}
