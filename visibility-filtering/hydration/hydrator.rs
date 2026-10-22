use crate::clients::socialgraph_client::{EdgeDirection, Graph};
use crate::hydration::inventory::Source;
use crate::hydration::relation::Relation;
use crate::hydration::sources::Scope;
use crate::hydration::store::KeyOrigin;
use strum::VariantArray;
use xai_core_entities::gizmoduck_client::QueryFields;

#[derive(Clone, Copy, Debug, PartialEq, Eq, strum::IntoStaticStr, strum::VariantArray)]
#[strum(serialize_all = "snake_case")]
#[repr(u8)]
pub enum Hydrator {
    PureCore,
    Tweet,
    ConversationControl,
    TweetSafetyLabels,
    ViewerProfile,
    ViewerLabels,
    AuthorSafety,
    AuthorLabels,
    Follows,
    Blocks,
    Mutes,
    MuteRetweets,
    BlockedByAuthor,
    BlockedByReplyRoot,
    SuperFollowsExclusive,
    RootFollowsViewer,
    RootFollowsViewerSecondDegree,
    SuperFollowsRoot,
    BlockedByConversationRoot,
    ConversationRootPost,
    BlockedByConversationRootPost,
    ViewerCountry,
    CommunityModeration,
    CommunityModerator,
    CommunityViewerRemoved,
    ArticleLifecycle,
    ArticleMediaEntities,
    ArticleCoverMedia,
    TrustedFriends,
    OutsideNarrowcastPlace,
    AuthorBlocksOuterAuthor,
    OuterAuthorFollowsAuthor,
    QuotedTweet,
    ReactedToPostSafetyLabels,
    ReactedToAuthorSafety,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum RequestRows {
    #[default]
    Posts,
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "remove once a served level takes user rows")
    )]
    Users,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Hydrators(u64);

impl Hydrators {
    pub const fn empty() -> Self {
        Self(0)
    }

    #[cfg(test)]
    pub fn all() -> Self {
        Self(u64::MAX >> (u64::BITS as usize - <Hydrator as strum::VariantArray>::VARIANTS.len()))
    }

    pub const fn of(hydrator: Hydrator) -> Self {
        Self(1 << hydrator as u8)
    }

    pub const fn with(self, hydrator: Hydrator) -> Self {
        self.union(Self::of(hydrator))
    }

    #[cfg(test)]
    pub const fn without(self, hydrator: Hydrator) -> Self {
        Self(self.0 & !Self::of(hydrator).0)
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn contains(self, hydrator: Hydrator) -> bool {
        self.0 & Self::of(hydrator).0 != 0
    }

    pub const fn difference(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Part {
    Column,
    Fields(&'static [QueryFields]),
    Edge(Edge),
    PairEdge(PairEdge),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, VariantArray)]
#[repr(u8)]
pub(super) enum Edge {
    Follows,
    Blocks,
    Mutes,
    MuteRetweets,
    BlockedBy,
    SuperFollows,
    FollowedBy,
    SecondDegree,
    TrustedFriends,
    OutsidePlace,
}

impl Edge {
    pub(super) const fn flock(self) -> Option<(Graph, EdgeDirection)> {
        use EdgeDirection::{Forward, Reverse};
        match self {
            Edge::Follows => Some((Graph::Follows, Forward)),
            Edge::Blocks => Some((Graph::Blocks, Forward)),
            Edge::Mutes => Some((Graph::Mutes, Forward)),
            Edge::MuteRetweets => Some((Graph::MuteRetweets, Forward)),
            Edge::BlockedBy => Some((Graph::Blocks, Reverse)),
            Edge::SuperFollows => Some((Graph::SuperFollows, Forward)),
            Edge::FollowedBy => Some((Graph::Follows, Reverse)),
            Edge::SecondDegree | Edge::TrustedFriends | Edge::OutsidePlace => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, VariantArray)]
#[repr(u8)]
pub(super) enum PairEdge {
    AuthorBlocksOuterAuthor,
    OuterAuthorFollowsAuthor,
}

impl PairEdge {
    pub(super) const fn graph(self) -> Graph {
        match self {
            PairEdge::AuthorBlocksOuterAuthor => Graph::Blocks,
            PairEdge::OuterAuthorFollowsAuthor => Graph::Follows,
        }
    }

    pub(super) fn of_graph(graph: Graph) -> Option<Self> {
        Self::VARIANTS
            .iter()
            .copied()
            .find(|edge| edge.graph() == graph)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct UserPair {
    pub(crate) source: u64,
    pub(crate) destination: u64,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct NodeSpec {
    pub(super) source: Source,
    pub(super) part: Part,
    pub(super) key: KeyOrigin,
    pub(super) label: (&'static str, &'static str),
    pub(super) relation: Option<Relation>,
}

impl Hydrator {
    pub(super) const fn spec(self) -> NodeSpec {
        use Hydrator as H;
        use KeyOrigin as K;
        use Source as S;
        const fn node(
            source: Source,
            part: Part,
            key: KeyOrigin,
            label: (&'static str, &'static str),
        ) -> NodeSpec {
            NodeSpec {
                source,
                part,
                key,
                label,
                relation: None,
            }
        }
        const fn reads(relation: Relation, node: NodeSpec) -> NodeSpec {
            NodeSpec {
                relation: Some(relation),
                ..node
            }
        }
        const RELATIONSHIPS: (&str, &str) = ("socialgraph", "batch_check_relationships");
        const BLOCKED_BY: (&str, &str) = ("blocked_by", "batch_check_blocked_by");
        const USER_PAIRS: (&str, &str) = ("socialgraph", "batch_check_user_pairs");
        match self {
            H::PureCore => node(
                S::TesPureCoreData,
                Part::Column,
                K::RequestTweets,
                ("tes", "get_tweet_core_datas"),
            ),
            H::Tweet => node(
                S::TesTweet,
                Part::Column,
                K::RequestTweets,
                ("tes", "get_tweets"),
            ),
            H::ConversationControl => node(
                S::TesConversationControl,
                Part::Column,
                K::RequestTweets,
                ("conversation_control", "get_conversation_controls"),
            ),
            H::TweetSafetyLabels => node(
                S::SafetyLabels,
                Part::Column,
                K::RequestTweets,
                ("safety_labels", "get"),
            ),
            H::ViewerProfile => node(
                S::GizmoduckViewer,
                Part::Fields(&[
                    QueryFields::ACCOUNT,
                    QueryFields::EXTENDED_PROFILE,
                    QueryFields::SAFETY,
                ]),
                K::Viewer,
                ("gizmoduck", "get_viewer_data"),
            ),
            H::ViewerLabels => node(
                S::GizmoduckViewer,
                Part::Fields(&[QueryFields::LABELS]),
                K::Viewer,
                ("gizmoduck", "get_viewer_data"),
            ),
            H::AuthorSafety => node(
                S::GizmoduckAuthor,
                Part::Fields(&[QueryFields::SAFETY]),
                K::PureCoreAuthor,
                ("gizmoduck", "get_users"),
            ),
            H::AuthorLabels => node(
                S::GizmoduckAuthor,
                Part::Fields(&[QueryFields::LABELS]),
                K::PureCoreAuthor,
                ("gizmoduck", "get_users"),
            ),
            H::Follows => node(
                S::Flock,
                Part::Edge(Edge::Follows),
                K::PureCoreAuthor,
                RELATIONSHIPS,
            ),
            H::Blocks => node(
                S::Flock,
                Part::Edge(Edge::Blocks),
                K::PureCoreAuthor,
                RELATIONSHIPS,
            ),
            H::Mutes => node(
                S::Flock,
                Part::Edge(Edge::Mutes),
                K::PureCoreAuthor,
                RELATIONSHIPS,
            ),
            H::MuteRetweets => node(
                S::Flock,
                Part::Edge(Edge::MuteRetweets),
                K::PureCoreRetweeter,
                RELATIONSHIPS,
            ),
            H::BlockedByAuthor => node(
                S::Flock,
                Part::Edge(Edge::BlockedBy),
                K::PureCoreAuthor,
                BLOCKED_BY,
            ),
            H::BlockedByReplyRoot => node(
                S::Flock,
                Part::Edge(Edge::BlockedBy),
                K::PureCoreReplyRoot,
                BLOCKED_BY,
            ),
            H::SuperFollowsExclusive => node(
                S::Flock,
                Part::Edge(Edge::SuperFollows),
                K::ExclusiveConversationAuthor,
                ("exclusive_content", "batch_check_super_follows"),
            ),
            H::RootFollowsViewer => node(
                S::Flock,
                Part::Edge(Edge::FollowedBy),
                K::CommunityOrMyNetworkRoot,
                ("conversation_control", "batch_check_followed_by"),
            ),
            H::RootFollowsViewerSecondDegree => node(
                S::Wingman,
                Part::Edge(Edge::SecondDegree),
                K::MyNetworkRootNotFollowingViewer,
                ("conversation_control", "exists_intersect"),
            ),
            H::SuperFollowsRoot => node(
                S::Flock,
                Part::Edge(Edge::SuperFollows),
                K::SubscribersRoot,
                ("conversation_control", "batch_check_super_follows"),
            ),
            H::BlockedByConversationRoot => node(
                S::Flock,
                Part::Edge(Edge::BlockedBy),
                K::ConversationControlBlockAuthor,
                ("conversation_control", "batch_check_blocked_by"),
            ),
            H::ConversationRootPost => reads(
                Relation::ConversationRoot,
                node(
                    S::TesPureCoreData,
                    Part::Column,
                    K::RelatedTweet,
                    ("conversation_root", "get_tweet_core_datas"),
                ),
            ),
            H::BlockedByConversationRootPost => reads(
                Relation::ConversationRoot,
                node(
                    S::Flock,
                    Part::Edge(Edge::BlockedBy),
                    K::ConversationRootPostAuthor,
                    ("conversation_root", "batch_check_blocked_by"),
                ),
            ),
            H::ViewerCountry => node(
                S::ViewerCountry,
                Part::Column,
                K::ViewerForCoAllowedList,
                ("conversation_control", "tfe_top_country"),
            ),
            H::CommunityModeration => node(
                S::CommunityModeration,
                Part::Column,
                K::CommunityPost,
                ("communities", "moderation_state"),
            ),
            H::CommunityModerator => node(
                S::CommunityModerator,
                Part::Column,
                K::ModeratedCommunity,
                ("communities", "visibility_features"),
            ),
            H::CommunityViewerRemoved => node(
                S::CommunityViewerRemoved,
                Part::Column,
                K::TweetCommunity,
                ("communities", "is_removed"),
            ),
            H::ArticleLifecycle => node(
                S::ArticleLifecycle,
                Part::Column,
                K::TweetArticle,
                ("article", "get_lifecycles"),
            ),
            H::ArticleMediaEntities => node(
                S::ArticleMediaEntities,
                Part::Column,
                K::TweetArticle,
                ("article", "get_media_entities"),
            ),
            H::ArticleCoverMedia => node(
                S::ArticleCoverMedia,
                Part::Column,
                K::TweetArticle,
                ("article", "get_cover_media"),
            ),
            H::TrustedFriends => node(
                S::TrustedFriends,
                Part::Edge(Edge::TrustedFriends),
                K::TrustedFriendsList,
                ("trusted_friends", "is_member_or_owner"),
            ),
            H::OutsideNarrowcastPlace => node(
                S::UserLocation,
                Part::Edge(Edge::OutsidePlace),
                K::NarrowcastPlace,
                ("geoduck", "user_location"),
            ),
            H::AuthorBlocksOuterAuthor => node(
                S::FlockUserPair,
                Part::PairEdge(PairEdge::AuthorBlocksOuterAuthor),
                K::QuotedAuthorToQuotingAuthor,
                USER_PAIRS,
            ),
            H::OuterAuthorFollowsAuthor => node(
                S::FlockUserPair,
                Part::PairEdge(PairEdge::OuterAuthorFollowsAuthor),
                K::QuotingAuthorToProtectedQuotedAuthor,
                USER_PAIRS,
            ),
            H::QuotedTweet => node(
                S::TesQuotedTweet,
                Part::Column,
                K::OuterOfMissingQuote,
                ("tes", "get_quoted_tweets"),
            ),
            H::ReactedToPostSafetyLabels => reads(
                Relation::ReactedTo,
                node(
                    S::SafetyLabels,
                    Part::Column,
                    K::RelatedTweet,
                    ("safety_labels", "get"),
                ),
            ),
            H::ReactedToAuthorSafety => reads(
                Relation::ReactedTo,
                node(
                    S::GizmoduckAuthor,
                    Part::Fields(&[QueryFields::SAFETY]),
                    K::RelatedAuthor,
                    ("gizmoduck", "get_users"),
                ),
            ),
        }
    }

    pub(crate) const fn is_best_effort(self) -> bool {
        use Hydrator as H;
        match self {
            H::ConversationControl
            | H::ConversationRootPost
            | H::Follows
            | H::BlockedByAuthor
            | H::BlockedByReplyRoot
            | H::BlockedByConversationRoot
            | H::BlockedByConversationRootPost
            | H::RootFollowsViewer
            | H::RootFollowsViewerSecondDegree
            | H::SuperFollowsRoot
            | H::ViewerCountry
            | H::CommunityViewerRemoved
            | H::OutsideNarrowcastPlace
            | H::OuterAuthorFollowsAuthor
            | H::QuotedTweet
            | H::ReactedToPostSafetyLabels
            | H::ReactedToAuthorSafety => true,
            H::PureCore
            | H::Tweet
            | H::TweetSafetyLabels
            | H::ViewerProfile
            | H::ViewerLabels
            | H::AuthorSafety
            | H::AuthorLabels
            | H::Blocks
            | H::Mutes
            | H::MuteRetweets
            | H::SuperFollowsExclusive
            | H::CommunityModeration
            | H::CommunityModerator
            | H::ArticleLifecycle
            | H::ArticleMediaEntities
            | H::ArticleCoverMedia
            | H::TrustedFriends
            | H::AuthorBlocksOuterAuthor => false,
        }
    }

    pub(super) const fn input(self) -> Option<Hydrator> {
        self.input_at(RequestRows::Posts)
    }

    pub(super) const fn key_at(self, rows: RequestRows) -> KeyOrigin {
        self.spec().key.at(rows)
    }

    pub(super) const fn input_at(self, rows: RequestRows) -> Option<Hydrator> {
        let input = self.key_at(rows).input();
        match (input, self.spec().relation) {
            (None, Some(relation)) => relation.spec().trigger.input(),
            _ => input,
        }
    }

    pub(super) const fn precedes_author(self, rows: RequestRows) -> bool {
        matches!(self.input_at(rows), None | Some(Hydrator::Tweet))
    }

    pub(super) const fn reads_at(self, rows: RequestRows) -> Hydrators {
        let reads = self.key_at(rows).reads();
        match self.spec().relation {
            Some(relation) => reads.union(relation.spec().trigger.reads()),
            None => reads,
        }
    }

    pub(super) const fn edge(self) -> Option<Edge> {
        match self.spec().part {
            Part::Edge(edge) => Some(edge),
            Part::Column | Part::Fields(_) | Part::PairEdge(_) => None,
        }
    }

    pub(super) const fn pair_edge(self) -> Option<PairEdge> {
        match self.spec().part {
            Part::PairEdge(edge) => Some(edge),
            Part::Column | Part::Fields(_) | Part::Edge(_) => None,
        }
    }

    pub(crate) const fn is_edge(self) -> bool {
        self.edge().is_some()
    }

    pub(crate) const fn needs_viewer(self) -> bool {
        let spec = self.spec();
        let joins_for_viewer = match spec.relation {
            Some(relation) => relation.spec().trigger.needs_viewer(),
            None => false,
        };
        self.is_edge() || spec.key.needs_viewer() || joins_for_viewer
    }
}

const MUST_HAVE: Hydrators = {
    let mut must_have = Hydrators::empty();
    let mut rest = Hydrator::VARIANTS;
    while let [node, tail @ ..] = rest {
        if !node.is_best_effort() {
            must_have = must_have.with(*node);
        }
        rest = tail;
    }
    must_have
};

impl Hydrators {
    pub(crate) const fn must_have(self) -> Self {
        self.intersection(MUST_HAVE)
    }

    pub(crate) const fn has_must_have(self) -> bool {
        !self.must_have().is_empty()
    }
}

const fn inputs_precede_nodes() -> bool {
    let mut rest = Hydrator::VARIANTS;
    while let [node, tail @ ..] = rest {
        if let Some(input) = node.input()
            && input as u8 >= *node as u8
        {
            return false;
        }
        rest = tail;
    }
    true
}

const fn best_effort_inputs_key_only_best_effort_nodes() -> bool {
    let mut rest = Hydrator::VARIANTS;
    while let [node, tail @ ..] = rest {
        if let Some(input) = node.input()
            && input.is_best_effort()
            && !node.is_best_effort()
        {
            return false;
        }
        rest = tail;
    }
    true
}

const fn viewer_scopes_fit_their_nodes() -> bool {
    let mut rest = Hydrator::VARIANTS;
    while let [node, tail @ ..] = rest {
        let is_viewer_scope = matches!(node.spec().source.scope(), Scope::Viewer);
        let fits = if matches!(node, Hydrator::ConversationRootPost) {
            node.needs_viewer() && !is_viewer_scope
        } else {
            node.needs_viewer() == is_viewer_scope
        };
        if !fits {
            return false;
        }
        rest = tail;
    }
    true
}

const _: () = assert!(inputs_precede_nodes());
const _: () = assert!(best_effort_inputs_key_only_best_effort_nodes());
const _: () = assert!(viewer_scopes_fit_their_nodes());
const _: () = assert!(Hydrator::VARIANTS.len() <= u64::BITS as usize);

impl Hydrators {
    pub(crate) const fn closed(self, rows: RequestRows) -> Self {
        let mut closed = match rows {
            RequestRows::Posts => self.with(Hydrator::PureCore),
            RequestRows::Users => self,
        };
        if closed.contains(Hydrator::AuthorBlocksOuterAuthor) {
            closed = closed.with(Hydrator::QuotedTweet);
        }
        let mut rest = Hydrator::VARIANTS;
        while let [head @ .., node] = rest {
            if let Some(input) = node.input_at(rows)
                && closed.contains(*node)
            {
                closed = closed.with(input);
            }
            if node.spec().relation.is_some() && closed.contains(*node) {
                closed = closed.union(node.reads_at(rows));
            }
            rest = head;
        }
        closed
    }

    pub(crate) fn iter(self) -> impl Iterator<Item = Hydrator> {
        Hydrator::VARIANTS
            .iter()
            .copied()
            .filter(move |&node| self.contains(node))
    }
}
