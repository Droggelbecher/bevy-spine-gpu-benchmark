use std::collections::BTreeMap;

use anyhow::{Result, anyhow, bail};

use super::{Attachment, DEFAULT_SKIN_NAME, Skin, SpineJson};

impl SpineJson {
    pub(crate) fn find_skin<'a>(&'a self, skin_name: &str) -> Option<&'a Skin> {
        self.skins.iter().find(|skin| skin.name == skin_name)
    }

    /// The names of the skins, comma separated, for error messages.
    fn skin_names(&self) -> String {
        let names: Vec<&str> = self.skins.iter().map(|skin| skin.name.as_str()).collect();
        names.join(", ")
    }

    pub(crate) fn select_skin<'a>(
        &'a self,
        requested_skin: Option<&str>,
        path: &str,
    ) -> Result<&'a Skin> {
        if let Some(skin_name) = requested_skin {
            return self.find_skin(skin_name).ok_or_else(|| {
                anyhow!(
                    "Skin {:?} not found in {:?}. Available skins: {}",
                    skin_name,
                    path,
                    self.skin_names()
                )
            });
        }

        if self.skins.len() == 1 {
            return Ok(&self.skins[0]);
        }

        if let Some(default_skin) = self.find_skin(DEFAULT_SKIN_NAME) {
            return Ok(default_skin);
        }

        bail!(
            "Multiple skins found in {:?}, but no skin was selected. Pass Some(\"skin name\") to LoadSpine::SpineDirectory. Available skins: {}",
            path,
            self.skin_names()
        )
    }
}

impl Skin {
    pub(crate) fn attachment(&self, slot_name: &str, attachment_name: &str) -> Option<&Attachment> {
        self.attachments
            .get(slot_name)
            .and_then(|slot| slot.get(attachment_name))
    }

    pub(crate) fn attachment_with_fallback<'a>(
        &'a self,
        default_skin: Option<&'a Skin>,
        slot_name: &str,
        attachment_name: &str,
    ) -> Option<(&'a Skin, &'a Attachment)> {
        self.attachment(slot_name, attachment_name)
            .map(|attachment| (self, attachment))
            .or_else(|| {
                default_skin.and_then(|default_skin| {
                    default_skin
                        .attachment(slot_name, attachment_name)
                        .map(|attachment| (default_skin, attachment))
                })
            })
    }

    pub(crate) fn attachments_with_fallback<'a>(
        &'a self,
        default_skin: Option<&'a Skin>,
    ) -> Vec<(String, String, &'a Skin, &'a Attachment)> {
        let mut attachments = BTreeMap::new();

        fn insert_skin_attachments<'a>(
            attachments: &mut BTreeMap<(String, String), (&'a Skin, &'a Attachment)>,
            skin: &'a Skin,
        ) {
            for (slot_name, slot) in &skin.attachments {
                for (attachment_name, attachment) in slot {
                    attachments.insert(
                        (slot_name.clone(), attachment_name.clone()),
                        (skin, attachment),
                    );
                }
            }
        }

        if let Some(default_skin) = default_skin {
            insert_skin_attachments(&mut attachments, default_skin);
        }

        if default_skin.is_none_or(|default_skin| !std::ptr::eq(default_skin, self)) {
            insert_skin_attachments(&mut attachments, self);
        }

        attachments
            .into_iter()
            .map(|((slot_name, attachment_name), (skin, attachment))| {
                (slot_name, attachment_name, skin, attachment)
            })
            .collect()
    }
}

/// The skin selected for a skeleton together with the default skin it falls back to.
#[derive(Clone, Copy)]
pub(crate) struct EffectiveSkin<'a> {
    pub(crate) skin: &'a Skin,
    pub(crate) default_skin: Option<&'a Skin>,
}

impl<'a> EffectiveSkin<'a> {
    pub(crate) fn attachment(
        &self,
        slot_name: &str,
        attachment_name: &str,
    ) -> Option<(&'a Skin, &'a Attachment)> {
        self.skin
            .attachment_with_fallback(self.default_skin, slot_name, attachment_name)
    }

    pub(crate) fn attachments(&self) -> Vec<(String, String, &'a Skin, &'a Attachment)> {
        self.skin.attachments_with_fallback(self.default_skin)
    }
}
