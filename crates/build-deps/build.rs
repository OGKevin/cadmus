use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const GENERIC_TIER: &str = "generic";

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let build_scripts = manifest_dir.join("../../build-scripts");
    let tiers = discover_patch_tier_names(&build_scripts);
    let template_path = manifest_dir.join("patch_tiers.rs.template");
    let template = fs::read_to_string(&template_path).expect("read patch_tiers.rs.template");
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let out_path = out_dir.join("patch_tiers.rs");
    fs::write(&out_path, render_patch_tiers(&template, &tiers)).expect("write patch_tiers.rs");
    println!("cargo:rerun-if-changed={}", build_scripts.display());
    println!("cargo:rerun-if-changed={}", template_path.display());
}

fn is_patch_tier_dir(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    match fs::read_dir(dir) {
        Ok(entries) => entries.filter_map(Result::ok).any(|entry| {
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                return false;
            }
            let path = entry.path();
            path.extension().is_some_and(|ext| ext == "patch") || entry.file_name() == ".gitkeep"
        }),
        Err(_) => false,
    }
}

fn discover_patch_tier_names(build_scripts: &Path) -> Vec<String> {
    let mut names = BTreeSet::new();
    if !build_scripts.is_dir() {
        return Vec::new();
    }
    for lib_entry in fs::read_dir(build_scripts).into_iter().flatten().flatten() {
        if !lib_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        for tier_entry in fs::read_dir(lib_entry.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            if !tier_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            if !is_patch_tier_dir(&tier_entry.path()) {
                continue;
            }
            if let Some(name) = tier_entry.file_name().to_str() {
                names.insert(name.to_owned());
            }
        }
    }
    names.into_iter().collect()
}

fn tier_variant_name(tier: &str) -> String {
    let mut chars = tier.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

fn render_patch_tiers(template: &str, tiers: &[String]) -> String {
    let has_generic = tiers.iter().any(|t| t == GENERIC_TIER);
    let generic_variant = tier_variant_name(GENERIC_TIER);

    let variants = tiers
        .iter()
        .map(|t| format!("    {},", tier_variant_name(t)))
        .collect::<Vec<_>>()
        .join("\n");

    let as_str_arms = tiers
        .iter()
        .map(|t| format!("            Self::{} => \"{}\",", tier_variant_name(t), t))
        .collect::<Vec<_>>()
        .join("\n");

    let all_list = tiers
        .iter()
        .map(|t| format!("Self::{}", tier_variant_name(t)))
        .collect::<Vec<_>>()
        .join(", ");

    let stack_arms = tiers
        .iter()
        .map(|t| {
            let variant = tier_variant_name(t);
            if t == GENERIC_TIER {
                format!("            Self::{variant} => &[Self::{variant}],")
            } else if has_generic {
                format!(
                    "            Self::{variant} => &[Self::{generic_variant}, Self::{variant}],"
                )
            } else {
                format!("            Self::{variant} => &[Self::{variant}],")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    template
        .replace("{{PATCH_TIER_VARIANTS}}", &variants)
        .replace("{{PATCH_TIER_AS_STR_ARMS}}", &as_str_arms)
        .replace("{{PATCH_TIER_ALL_LIST}}", &all_list)
        .replace("{{PATCH_TIER_STACK_ARMS}}", &stack_arms)
}
