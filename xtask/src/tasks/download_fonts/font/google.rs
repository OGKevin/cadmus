use std::path::Path;

use anyhow::Result;

use super::super::util;

const SUBMODULE: &str = "thirdparty/google-fonts";
const MARKER_NAME: &str = "google-fonts";

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
    // Regional Noto CJK subsets. Slint registers these as script fallbacks
    // so settings labels can draw Japanese, Korean, and Chinese names.
    ("NotoSansJP.ttf", "ofl/notosansjp/NotoSansJP[wght].ttf"),
    ("NotoSansKR.ttf", "ofl/notosanskr/NotoSansKR[wght].ttf"),
    ("NotoSansSC.ttf", "ofl/notosanssc/NotoSansSC[wght].ttf"),
    ("NotoSansTC.ttf", "ofl/notosanstc/NotoSansTC[wght].ttf"),
];

pub(crate) fn is_complete(root: &Path, fonts_dir: &Path) -> bool {
    util::is_submodule_install_current(root, fonts_dir, SUBMODULE, MARKER_NAME, FILES)
}

pub(crate) fn install(root: &Path, fonts_dir: &Path) -> Result<()> {
    util::install_from_submodule(root, SUBMODULE, fonts_dir, MARKER_NAME, FILES)
}
