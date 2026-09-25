#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ViewerAuthorRelationship {
    pub viewer_blocks_author: bool,
    pub viewer_mutes_author: bool,
    pub viewer_follows_author: bool,
    pub viewer_mutes_retweets_from_author: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ViewerBlockedBy {
    pub author: bool,
    pub root_author: bool,
}
