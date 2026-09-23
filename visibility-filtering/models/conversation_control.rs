use xai_core_entities::entities::ConversationControl;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConversationControlFeatures {
    pub control: ConversationControl,
    pub root_author_follows_viewer: Option<bool>,
    pub viewer_super_follows_root_author: Option<bool>,
}
