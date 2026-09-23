#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ViewerAuthorRelationship {
    pub viewer_blocks_author: bool,
    pub viewer_mutes_author: bool,
    pub viewer_follows_author: bool,
    pub viewer_mutes_retweets_from_author: bool,
}
