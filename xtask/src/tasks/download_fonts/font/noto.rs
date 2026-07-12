use std::path::Path;

use anyhow::Result;

use super::super::util;

/// Submodule path; revision is pinned via the parent repository gitlink and
/// tracked by Renovate's `git-submodules` manager.
pub const SUBMODULE: &str = "thirdparty/noto-fonts";

const FILES: &[(&str, &str)] = &[
    (
        "NotoSans-Regular.ttf",
        "hinted/ttf/NotoSans/NotoSans-Regular.ttf",
    ),
    (
        "NotoSans-Italic.ttf",
        "hinted/ttf/NotoSans/NotoSans-Italic.ttf",
    ),
    ("NotoSans-Bold.ttf", "hinted/ttf/NotoSans/NotoSans-Bold.ttf"),
    (
        "NotoSans-BoldItalic.ttf",
        "hinted/ttf/NotoSans/NotoSans-BoldItalic.ttf",
    ),
    (
        "NotoSerif-Regular.ttf",
        "hinted/ttf/NotoSerif/NotoSerif-Regular.ttf",
    ),
    (
        "NotoSerif-Italic.ttf",
        "hinted/ttf/NotoSerif/NotoSerif-Italic.ttf",
    ),
    (
        "NotoSerif-Bold.ttf",
        "hinted/ttf/NotoSerif/NotoSerif-Bold.ttf",
    ),
    (
        "NotoSerif-BoldItalic.ttf",
        "hinted/ttf/NotoSerif/NotoSerif-BoldItalic.ttf",
    ),
];

pub fn is_complete(fonts_dir: &Path) -> bool {
    FILES.iter().all(|(dest, _)| fonts_dir.join(dest).exists())
}

pub fn install(root: &Path, fonts_dir: &Path) -> Result<()> {
    util::install_from_submodule(root, SUBMODULE, fonts_dir, FILES)
}
