use std::path::Path;

use anyhow::Result;

use super::super::util;

/// Submodule path; revision is pinned via the parent repository gitlink and
/// tracked by Renovate's `git-submodules` manager.
pub const SUBMODULE: &str = "thirdparty/google-fonts";

const FILES: &[(&str, &str)] = &[
    (
        "VarelaRound-Regular.ttf",
        "ofl/varelaround/VarelaRound-Regular.ttf",
    ),
    ("Cormorant-Regular.ttf", "ofl/cormorant/Cormorant[wght].ttf"),
    (
        "Parisienne-Regular.ttf",
        "ofl/parisienne/Parisienne-Regular.ttf",
    ),
    ("Delius-Regular.ttf", "ofl/delius/Delius-Regular.ttf"),
];

pub fn is_complete(fonts_dir: &Path) -> bool {
    FILES.iter().all(|(dest, _)| fonts_dir.join(dest).exists())
}

pub fn install(root: &Path, fonts_dir: &Path) -> Result<()> {
    util::install_from_submodule(root, SUBMODULE, fonts_dir, FILES)
}
