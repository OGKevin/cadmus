use crate::api::proto;
use crate::document::file_extension::FileExtension;
use crate::settings::{RefreshRatePair, Settings};
use connectrpc::ConnectError;
use std::collections::HashMap;

/// Global pair plus per-file-type pairs. `available` lists unused extensions.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
pub(super) fn refresh_rate_value(settings: &Settings) -> proto::SettingValue {
    let rate = &settings.reader.refresh_rate;
    let mut pairs: Vec<_> = rate.by_kind.iter().collect();
    pairs.sort_by(|(left, _), (right, _)| left.as_str().cmp(right.as_str()));
    proto::SettingValue {
        kind: Some(proto::setting_value::Kind::RefreshRate(Box::new(
            proto::RefreshRateValue {
                regular: u32::from(rate.global.regular),
                inverted: u32::from(rate.global.inverted),
                by_kind: pairs
                    .into_iter()
                    .map(|(extension, pair)| proto::RefreshRateByKind {
                        extension: extension.as_str().to_string(),
                        regular: u32::from(pair.regular),
                        inverted: u32::from(pair.inverted),
                        ..Default::default()
                    })
                    .collect(),
                available: FileExtension::all()
                    .iter()
                    .filter(|extension| !rate.by_kind.contains_key(*extension))
                    .map(|extension| extension.as_str().to_string())
                    .collect(),
                ..Default::default()
            },
        ))),
        ..Default::default()
    }
}

/// Write the RefreshRateValue onto Settings. Unknown extensions are invalid_argument.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
pub(super) fn apply_refresh_rate(
    settings: &mut Settings,
    value: &proto::SettingValue,
) -> Result<(), ConnectError> {
    let proto::setting_value::Kind::RefreshRate(rate) = value
        .kind
        .as_ref()
        .ok_or_else(|| ConnectError::invalid_argument("missing value"))?
    else {
        return Err(ConnectError::invalid_argument(
            "setting is not a refresh rate",
        ));
    };
    let regular = refresh_rate_u8(rate.regular)?;
    let inverted = refresh_rate_u8(rate.inverted)?;
    let mut by_kind = HashMap::new();
    for entry in &rate.by_kind {
        let extension = entry
            .extension
            .parse::<FileExtension>()
            .map_err(|_| ConnectError::invalid_argument("unknown file type"))?;
        by_kind.insert(
            extension,
            RefreshRatePair {
                regular: refresh_rate_u8(entry.regular)?,
                inverted: refresh_rate_u8(entry.inverted)?,
            },
        );
    }
    settings.reader.refresh_rate.global.regular = regular;
    settings.reader.refresh_rate.global.inverted = inverted;
    settings.reader.refresh_rate.by_kind = by_kind;
    Ok(())
}

/// Proto u32 into the on-device u8 range.
#[cfg_attr(feature = "tracing", tracing::instrument(skip_all))]
fn refresh_rate_u8(value: u32) -> Result<u8, ConnectError> {
    u8::try_from(value)
        .map_err(|_| ConnectError::invalid_argument("refresh rate must be between 0 and 255"))
}
