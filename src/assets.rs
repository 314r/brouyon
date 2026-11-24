use gpui_kit::{AssetSource, Result, SharedString, assets::icon_assets};
use std::borrow::Cow;

icon_assets!(ExtraIcons, [Trash]);

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        match ExtraIcons.load(path)? {
            Some(icon) => Ok(Some(icon)),
            None => gpui_kit::assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        paths.sort();
        paths.dedup();
        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_omarchy::IconName;

    #[test]
    fn every_icon_the_app_draws_is_in_the_bundle() {
        for name in [
            IconName::Check,
            IconName::ChevronUp,
            IconName::Close,
            IconName::Copy,
            IconName::ExternalLink,
            IconName::FileText,
            IconName::Folder,
            IconName::Info,
            IconName::Palette,
            IconName::Plus,
            IconName::Search,
            IconName::Settings,
            IconName::Trash,
        ] {
            let path = name.path();
            assert!(
                Assets.load(&path).is_ok_and(|icon| icon.is_some()),
                "{path}"
            );
        }
    }
}
