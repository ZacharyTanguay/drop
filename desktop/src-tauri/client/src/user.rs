use bitcode::{Decode, Encode};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, Encode, Decode)]
#[serde(rename_all = "camelCase")]
pub struct User {
    id: String,
    username: String,
    admin: bool,
    display_name: String,
    profile_picture_object_id: String,
}

// ZOUGCLOUD(ZC-011): the access rules key on the Drop user, but every field
// here is private and upstream exposes no accessor. Two read-only getters is
// the smallest possible change; nothing else about the type moves.
impl User {
    /// Stable opaque UUID. Permissions are keyed on this, never on the
    /// username, which is guessable and can be changed.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Login name. Used only to recognise the ZougCloud admin for UX, and for
    /// display in admin lists.
    pub fn username(&self) -> &str {
        &self.username
    }
}
