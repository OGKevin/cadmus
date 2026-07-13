use std::path::Path;

use anyhow::Result;

use super::super::util;

const SUBMODULE: &str = "thirdparty/noto-fonts";
const MARKER_NAME: &str = "noto-fonts";

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

pub(crate) fn is_complete(root: &Path, fonts_dir: &Path) -> bool {
    util::is_submodule_install_current(root, fonts_dir, SUBMODULE, MARKER_NAME, FILES)
}

pub(crate) fn install(root: &Path, fonts_dir: &Path) -> Result<()> {
    util::install_from_submodule(root, SUBMODULE, fonts_dir, MARKER_NAME, FILES)
}
