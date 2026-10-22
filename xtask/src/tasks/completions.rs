//! `cadmuscli completions` — emit shell completion scripts.

use std::io;
use std::path::Path;

use anyhow::{Context, Result};
use clap::{Args, CommandFactory};
use clap_complete::{Shell, generate, generate_to};

use crate::Cli;

/// Arguments for `cadmuscli completions`.
#[derive(Debug, Args)]
pub struct CompletionsArgs {
    /// Shell to generate.
    shell: Shell,

    /// Write the script to this directory instead of stdout.
    #[arg(long)]
    output_dir: Option<std::path::PathBuf>,
}

/// Writes a completion script for the given shell.
pub fn run(args: CompletionsArgs) -> Result<()> {
    let mut cmd = Cli::command();
    match &args.output_dir {
        Some(dir) => write_to_directory(args.shell, &mut cmd, dir),
        None => {
            generate(args.shell, &mut cmd, "cadmuscli", &mut io::stdout());
            Ok(())
        }
    }
}

fn write_to_directory(shell: Shell, cmd: &mut clap::Command, dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("create {dir}", dir = dir.display()))?;
    generate_to(shell, cmd, "cadmuscli", dir)
        .with_context(|| format!("write completions to {dir}", dir = dir.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn generate_zsh_and_bash_scripts() {
        let dir = tempdir().unwrap();
        let path = dir.path().to_path_buf();

        run(CompletionsArgs {
            shell: Shell::Zsh,
            output_dir: Some(path.clone()),
        })
        .unwrap();
        let zsh = path.join("_cadmuscli");
        assert!(zsh.is_file());
        assert!(zsh.metadata().unwrap().len() > 0);

        run(CompletionsArgs {
            shell: Shell::Bash,
            output_dir: Some(path),
        })
        .unwrap();
        let bash = dir.path().join("cadmuscli.bash");
        assert!(bash.is_file());
        assert!(bash.metadata().unwrap().len() > 0);
    }
}
