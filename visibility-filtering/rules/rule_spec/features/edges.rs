use crate::hydration::Hydrator;
use crate::models::{PairEdges, ViewerRelationship};
use crate::rules::rule_spec::engine::{Engine, UserEngine};
use crate::rules::rule_spec::rule::RuleCond;
use crate::rules::rule_spec::{Predicate, Truth};

#[derive(Clone, Copy, Debug, PartialEq, Eq, strum::IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub enum RelationshipLeaf {
    ViewerFollowsAuthor,
    ViewerBlocksAuthor,
    ViewerMutesAuthor,
    ViewerIsBlockedByAuthor,
}

impl RelationshipLeaf {
    #[inline]
    pub(in crate::rules) const fn node(self) -> Hydrator {
        match self {
            Self::ViewerFollowsAuthor => Hydrator::Follows,
            Self::ViewerBlocksAuthor => Hydrator::Blocks,
            Self::ViewerMutesAuthor => Hydrator::Mutes,
            Self::ViewerIsBlockedByAuthor => Hydrator::BlockedByAuthor,
        }
    }

    #[inline]
    pub(in crate::rules) const fn holds(self, relationship: ViewerRelationship) -> bool {
        match self {
            Self::ViewerFollowsAuthor => relationship.follows,
            Self::ViewerBlocksAuthor => relationship.blocks,
            Self::ViewerMutesAuthor => relationship.mutes,
            Self::ViewerIsBlockedByAuthor => relationship.blocked_by,
        }
    }

    pub(in crate::rules) fn sentence(self, truth: Truth) -> Option<String> {
        (self == Self::ViewerFollowsAuthor).then(|| {
            match truth {
                Truth::True => "The viewer follows the author.",
                Truth::False => "The viewer does not follow the author.",
                Truth::Unknown { .. } => "Whether the viewer follows the author.",
            }
            .to_owned()
        })
    }

    pub(in crate::rules) const fn leaf<E: Engine>(self) -> RuleCond<E> {
        RuleCond::Leaf(Predicate::Relationship(self))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, strum::IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub enum EdgeLeaf {
    ViewerMutesRetweetsFromAuthor,
    ViewerIsFollowedByConversationRootAuthor,
    ViewerSuperFollowsConversationRootAuthor,
    ViewerIsTrustedFriendsListMemberOrOwner,
    ViewerIsOutsideNarrowcastPlace,
}

impl EdgeLeaf {
    #[inline]
    pub(in crate::rules) const fn node(self) -> Hydrator {
        match self {
            Self::ViewerMutesRetweetsFromAuthor => Hydrator::MuteRetweets,
            Self::ViewerIsFollowedByConversationRootAuthor => Hydrator::RootFollowsViewer,
            Self::ViewerSuperFollowsConversationRootAuthor => Hydrator::SuperFollowsRoot,
            Self::ViewerIsTrustedFriendsListMemberOrOwner => Hydrator::TrustedFriends,
            Self::ViewerIsOutsideNarrowcastPlace => Hydrator::OutsideNarrowcastPlace,
        }
    }

    pub(in crate::rules) const fn leaf(self) -> RuleCond {
        RuleCond::Leaf(Predicate::Edge(self))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, strum::IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub enum PairEdgeLeaf {
    AuthorBlocksOuterAuthor,
    OuterAuthorFollowsAuthor,
}

impl PairEdgeLeaf {
    #[inline]
    pub(in crate::rules) const fn node(self) -> Hydrator {
        match self {
            Self::AuthorBlocksOuterAuthor => Hydrator::AuthorBlocksOuterAuthor,
            Self::OuterAuthorFollowsAuthor => Hydrator::OuterAuthorFollowsAuthor,
        }
    }

    #[inline]
    pub(in crate::rules) const fn holds(self, edges: PairEdges) -> bool {
        match self {
            Self::AuthorBlocksOuterAuthor => edges.author_blocks_outer_author,
            Self::OuterAuthorFollowsAuthor => edges.outer_author_follows_author,
        }
    }

    pub(in crate::rules) const fn leaf(self) -> RuleCond<UserEngine> {
        RuleCond::Leaf(Predicate::PairEdge(self))
    }
}
