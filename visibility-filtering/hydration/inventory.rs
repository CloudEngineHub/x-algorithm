use crate::hydration::plan::{MissPolicy, Subject};
use crate::hydration::sources::{exchange, Exchange, Scope, Visit};
use strum::VariantArray;

pub(super) struct Row {
    pub(super) miss: MissPolicy,
    pub(super) batch: Batch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Batch {
    Unrecorded,
    Keys,
    Requested,
}

macro_rules! sources {
    ($($source:ident { $($field:ident: $value:expr),* $(,)? })*) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, VariantArray)]
        pub(crate) enum Source {
            $($source,)*
        }

        impl Source {
            pub(super) const fn row(self) -> Row {
                match self {
                    $(Source::$source => Row { $($field: $value),* },)*
                }
            }

            pub(super) const fn scope(self) -> Scope {
                match self {
                    $(Source::$source => <exchange::$source as Exchange>::SCOPE,)*
                }
            }

            pub(crate) fn visit(self, visitor: &mut impl Visit) {
                match self {
                    $(Source::$source => visitor.exchange::<exchange::$source>(),)*
                }
            }
        }
    };
}

sources! {
    TesPureCoreData {
        miss: MissPolicy::Unresolves(Subject::Tweet),
        batch: Batch::Keys,
    }
    TesConversationRoot {
        miss: MissPolicy::FailsNode,
        batch: Batch::Keys,
    }
    TesTweet {
        miss: MissPolicy::Unresolves(Subject::Tweet),
        batch: Batch::Unrecorded,
    }
    TesConversationControl {
        miss: MissPolicy::FailsNode,
        batch: Batch::Keys,
    }
    SafetyLabels {
        miss: MissPolicy::FailsNode,
        batch: Batch::Requested,
    }
    GizmoduckViewer {
        miss: MissPolicy::FailsNode,
        batch: Batch::Unrecorded,
    }
    GizmoduckAuthor {
        miss: MissPolicy::Unresolves(Subject::Author),
        batch: Batch::Keys,
    }
    Flock {
        miss: MissPolicy::FailsNode,
        batch: Batch::Requested,
    }
    ViewerCountry {
        miss: MissPolicy::FailsNode,
        batch: Batch::Unrecorded,
    }
    Wingman {
        miss: MissPolicy::ReadsNoEdge,
        batch: Batch::Requested,
    }
    CommunityModeration {
        miss: MissPolicy::FailsNode,
        batch: Batch::Keys,
    }
    CommunityModerator {
        miss: MissPolicy::FailsNode,
        batch: Batch::Keys,
    }
    CommunityViewerRemoved {
        miss: MissPolicy::FailsNode,
        batch: Batch::Keys,
    }
    ArticleLifecycle {
        miss: MissPolicy::FailsNode,
        batch: Batch::Keys,
    }
    ArticleMediaEntities {
        miss: MissPolicy::ReadsNotFound,
        batch: Batch::Keys,
    }
    ArticleCoverMedia {
        miss: MissPolicy::ReadsNotFound,
        batch: Batch::Keys,
    }
    TrustedFriends {
        miss: MissPolicy::ReadsNoEdge,
        batch: Batch::Keys,
    }
    UserLocation {
        miss: MissPolicy::FailsNode,
        batch: Batch::Unrecorded,
    }
}
