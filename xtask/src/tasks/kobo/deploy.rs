//! `cadmuscli kobo deploy` — build the test feature set, assemble `dist/`,
//! and upload it to the device.
//!
//! With `--restart` the same authenticated connection then restarts Cadmus,
//! so a deploy and the relaunch it needs stay one command.

use anyhow::Result;

use crate::blocking;
use crate::tasks::build_kobo::{self, BuildKoboArgs};
use crate::tasks::dist::{self, DistArgs};
use crate::tasks::util::workspace;

use super::ssh;
use super::{DeployArgs, INSTALL_DIR, deploy_features, restart, upload};

/// Builds, assembles, uploads, and optionally restarts.
pub(crate) async fn run(args: DeployArgs) -> Result<()> {
    let settings = ssh::resolve(&args.ssh).await?;
    let features = deploy_features(args.features.as_deref())?;
    let root = blocking(workspace::root).await?;

    println!("Building {features} for ARM…");
    blocking(move || {
        build_kobo::run(BuildKoboArgs {
            features: Some(features),
        })
    })
    .await?;

    blocking(|| dist::run(DistArgs { test: false })).await?;

    println!("Uploading dist/ to {} ({INSTALL_DIR})…", settings.host());
    let mut handle = ssh::connect(&settings).await?;
    upload::run(
        &mut handle,
        settings.host(),
        &root.join("dist"),
        &upload::manifest_path(&root),
    )
    .await?;

    if args.restart {
        restart::restart(&mut handle, settings.host()).await
    } else {
        println!(
            "Deployed to {}. The running process still uses the old binary until restart.",
            settings.host()
        );
        Ok(())
    }
}
