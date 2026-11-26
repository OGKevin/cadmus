//! Power management submodule.

mod error;
mod manager;

pub use error::PowerError;
pub use manager::PowerManager;

#[cfg(all(target_os = "linux", feature = "kobo"))]
pub(crate) async fn discover_cores(
    cpu_dir: &std::path::Path,
) -> Result<Vec<(std::path::PathBuf, String)>, std::io::Error> {
    let mut discovered = Vec::new();
    if !tokio::fs::metadata(cpu_dir)
        .await
        .is_ok_and(|metadata| metadata.is_dir())
    {
        return Ok(discovered);
    }

    let mut entries = tokio::fs::read_dir(cpu_dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if !tokio::fs::metadata(&path)
            .await
            .is_ok_and(|metadata| metadata.is_dir())
        {
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
        if !tokio::fs::metadata(&online_path)
            .await
            .is_ok_and(|metadata| metadata.is_file())
        {
            continue;
        }

        if let Ok(state_str) = tokio::fs::read_to_string(&online_path).await {
            discovered.push((online_path, state_str.trim().to_owned()));
        }
    }

    Ok(discovered)
}

#[cfg(all(test, target_os = "linux", feature = "kobo"))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_discover_cores() {
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

        let mut cores = discover_cores(cpu_dir)
            .await
            .expect("failed to discover cores");
        cores.sort_by(|a, b| a.0.cmp(&b.0));

        assert_eq!(cores.len(), 2);
        assert_eq!(cores[0].0, cpu1_online);
        assert_eq!(cores[0].1, "0");
        assert_eq!(cores[1].0, cpu2_online);
        assert_eq!(cores[1].1, "1");
    }
}
