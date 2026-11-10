use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};

use super::{
    DEFAULT_SKIN_NAME, LinkedMeshType, MeshType, SpineColor, SpineJson, default_scale, zerof32,
};

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(untagged)]
pub(crate) enum Attachment {
    Mesh {
        #[serde(rename = "type")]
        r#type: MeshType,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        path: Option<String>,
        uvs: Vec<f32>,
        triangles: Vec<u32>,
        vertices: Vec<f32>,
        #[serde(default)]
        edges: Vec<usize>,
        #[serde(default)]
        hull: usize,
        #[serde(default)]
        width: f32,
        #[serde(default)]
        height: f32,
        #[serde(default)]
        color: SpineColor,
    },

    LinkedMesh {
        #[serde(rename = "type")]
        r#type: LinkedMeshType,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        path: Option<String>,
        #[serde(default)]
        skin: Option<String>,
        parent: String,
        #[serde(default)]
        width: f32,
        #[serde(default)]
        height: f32,
        #[serde(default)]
        color: SpineColor,
    },

    Rectangle {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        path: Option<String>,
        #[serde(default = "zerof32")]
        x: f32,
        #[serde(default = "zerof32")]
        y: f32,
        #[serde(default = "zerof32")]
        rotation: f32,
        #[serde(rename = "scaleX", default = "default_scale")]
        scale_x: f32,
        #[serde(rename = "scaleY", default = "default_scale")]
        scale_y: f32,
        /// Original size, not cropped one!
        width: f32,
        height: f32,
        #[serde(default)]
        color: SpineColor,
    },
}

impl Attachment {
    pub(crate) fn new_rectangle(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self::Rectangle {
            name: None,
            path: None,
            x,
            y,
            width,
            height,
            rotation: 0.0,
            scale_x: 1.0,
            scale_y: 1.0,
            color: SpineColor::WHITE,
        }
    }

    /// The attachment's own color (to be multiplied with the slot color).
    pub(crate) fn color(&self) -> SpineColor {
        match self {
            Attachment::Mesh { color, .. }
            | Attachment::LinkedMesh { color, .. }
            | Attachment::Rectangle { color, .. } => *color,
        }
    }

    pub(crate) fn region_name(&self, attachment_name: &str) -> String {
        match self {
            Attachment::Mesh { name, path, .. }
            | Attachment::LinkedMesh { name, path, .. }
            | Attachment::Rectangle { name, path, .. } => path
                .as_deref()
                .or(name.as_deref())
                .unwrap_or(attachment_name)
                .to_string(),
        }
    }

    /// Resolve the underlying attachment & region name for LinkedMeshes and update the linked_mesh_stack.
    /// For non-LinkedMesh attachments return a clone of self and the region name.
    pub(crate) fn resolve(
        &self,
        spine: &SpineJson,
        slot_name: &str,
        attachment_name: &str,
        linked_mesh_stack: &mut Vec<(String, String)>,
    ) -> Result<(Attachment, String)> {
        match self {
            Attachment::LinkedMesh {
                name,
                skin,
                parent,
                path,
                color: linked_color,
                ..
            } => {
                let parent_skin_name = skin.as_deref().unwrap_or(DEFAULT_SKIN_NAME);
                let stack_key = (parent_skin_name.to_string(), parent.clone());
                if linked_mesh_stack.contains(&stack_key) {
                    return Err(anyhow!(
                        "linkedmesh cycle while resolving slot {:?}",
                        slot_name,
                    ));
                }
                linked_mesh_stack.push(stack_key);

                let parent_skin = spine.find_skin(parent_skin_name).ok_or_else(|| {
                    anyhow!(
                        "linkedmesh parent skin {:?} not found for slot {:?}",
                        parent_skin_name,
                        slot_name,
                    )
                })?;
                let parent_attachment =
                    parent_skin.attachment(slot_name, parent).ok_or_else(|| {
                        anyhow!(
                            "linkedmesh parent attachment {:?} not found in skin {:?}, slot {:?}",
                            parent,
                            parent_skin_name,
                            slot_name
                        )
                    })?;

                let (mut resolved, _) = parent_attachment.resolve(
                    spine,
                    slot_name,
                    attachment_name,
                    linked_mesh_stack,
                )?;
                linked_mesh_stack.pop();

                // The linked mesh has its own color, not the one of its parent
                let Attachment::Mesh { color, .. } = &mut resolved else {
                    return Err(anyhow!(
                        "linkedmesh parent {:?} in skin {:?}, slot {:?} did not resolve to a mesh",
                        parent,
                        parent_skin_name,
                        slot_name
                    ));
                };
                *color = *linked_color;

                Ok((
                    resolved,
                    path.as_deref()
                        .or(name.as_deref())
                        .unwrap_or(attachment_name)
                        .to_string(),
                ))
            }
            _ => Ok((self.clone(), self.region_name(attachment_name))),
        }
    }
}
