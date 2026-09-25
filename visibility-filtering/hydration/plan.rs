use crate::clients::socialgraph_client::{EdgeDirection, Graph};
use crate::hydration::{Hydrator, Hydrators};
use crate::rules::SafetyLevel;
use std::fmt;
use strum::VariantArray;
use xai_core_entities::entities::ConversationControlArm;
use xai_core_entities::gizmoduck_client::QueryFields;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Source {
    TesPureCore,
    TesComposite,
    TesConversationControl,
    SafetyLabels,
    GizmoduckViewer,
    GizmoduckAuthor,
    Flock,
    ViewerCountry,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Part {
    Column,
    Fields(&'static [QueryFields]),
    Edge(Graph, EdgeDirection),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum KeyOrigin {
    RequestTweets,
    Viewer,
    PureCoreAuthor,
    PureCoreReplyRoot,
    ExclusiveConversationAuthor,
    ConversationRoot(&'static [ConversationControlArm]),
        ViewerForCoAllowedList,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct NodeSpec {
    pub(super) source: Source,
    pub(super) part: Part,
    pub(super) key: KeyOrigin,
        pub(super) label: (&'static str, &'static str),
}

impl KeyOrigin {
    const fn input(self) -> Option<Hydrator> {
        match self {
            KeyOrigin::RequestTweets | KeyOrigin::Viewer => None,
            KeyOrigin::PureCoreAuthor | KeyOrigin::PureCoreReplyRoot => Some(Hydrator::PureCore),
            KeyOrigin::ExclusiveConversationAuthor => Some(Hydrator::Tweet),
            KeyOrigin::ConversationRoot(_) | KeyOrigin::ViewerForCoAllowedList => {
                Some(Hydrator::ConversationControl)
            }
        }
    }
}

impl Hydrator {
    pub(super) const fn spec(self) -> NodeSpec {
        use EdgeDirection::{Forward, Reverse};
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
            }
        }
        const RELATIONSHIPS: (&str, &str) = ("socialgraph", "batch_check_relationships");
        const BLOCKED_BY: (&str, &str) = ("blocked_by", "batch_check_blocked_by");
        match self {
            H::PureCore => node(
                S::TesPureCore,
                Part::Column,
                K::RequestTweets,
                ("tes", "get_tweet_core_datas"),
            ),
            H::Tweet => node(
                S::TesComposite,
                Part::Column,
                K::RequestTweets,
                ("tes", "get_tweets_for_visibility"),
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
                Part::Edge(Graph::Follows, Forward),
                K::PureCoreAuthor,
                RELATIONSHIPS,
            ),
            H::Blocks => node(
                S::Flock,
                Part::Edge(Graph::Blocks, Forward),
                K::PureCoreAuthor,
                RELATIONSHIPS,
            ),
            H::Mutes => node(
                S::Flock,
                Part::Edge(Graph::Mutes, Forward),
                K::PureCoreAuthor,
                RELATIONSHIPS,
            ),
            H::MuteRetweets => node(
                S::Flock,
                Part::Edge(Graph::MuteRetweets, Forward),
                K::PureCoreAuthor,
                RELATIONSHIPS,
            ),
            H::BlockedByAuthor => node(
                S::Flock,
                Part::Edge(Graph::Blocks, Reverse),
                K::PureCoreAuthor,
                BLOCKED_BY,
            ),
            H::BlockedByReplyRoot => node(
                S::Flock,
                Part::Edge(Graph::Blocks, Reverse),
                K::PureCoreReplyRoot,
                BLOCKED_BY,
            ),
            H::SuperFollowsExclusive => node(
                S::Flock,
                Part::Edge(Graph::SuperFollows, Forward),
                K::ExclusiveConversationAuthor,
                ("exclusive_content", "batch_check_super_follows"),
            ),
            H::RootFollowsViewer => node(
                S::Flock,
                Part::Edge(Graph::Follows, Reverse),
                K::ConversationRoot(&[
                    ConversationControlArm::Community,
                    ConversationControlArm::MyNetwork,
                ]),
                ("conversation_control", "batch_check_followed_by"),
            ),
            H::SuperFollowsRoot => node(
                S::Flock,
                Part::Edge(Graph::SuperFollows, Forward),
                K::ConversationRoot(&[ConversationControlArm::Subscribers]),
                ("conversation_control", "batch_check_super_follows"),
            ),
            H::ViewerCountry => node(
                S::ViewerCountry,
                Part::Column,
                K::ViewerForCoAllowedList,
                ("conversation_control", "tfe_top_country"),
            ),
        }
    }

    pub(super) const fn input(self) -> Option<Hydrator> {
        self.spec().key.input()
    }

        pub(super) const fn needs_viewer(self) -> bool {
        let spec = self.spec();
        matches!(spec.source, Source::Flock)
            || matches!(
                spec.key,
                KeyOrigin::Viewer | KeyOrigin::ViewerForCoAllowedList
            )
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

const fn author_keys_come_from_pure_core() -> bool {
    let mut rest = Hydrator::VARIANTS;
    while let [node, tail @ ..] = rest {
        if matches!(node.spec().source, Source::GizmoduckAuthor)
            && !matches!(node.input(), Some(Hydrator::PureCore))
        {
            return false;
        }
        rest = tail;
    }
    true
}

const _: () = assert!(inputs_precede_nodes());
const _: () = assert!(author_keys_come_from_pure_core());
const _: () = assert!(Hydrator::VARIANTS.len() <= u32::BITS as usize);

impl Hydrators {
        pub const fn closed(self) -> Self {
        let mut closed = self.with(Hydrator::PureCore);
        let mut rest = Hydrator::VARIANTS;
        while let [head @ .., node] = rest {
            if let Some(input) = node.input()
                && closed.contains(*node)
            {
                closed = closed.with(input);
            }
            rest = head;
        }
        closed
    }

    pub(super) fn iter(self) -> impl Iterator<Item = Hydrator> {
        Hydrator::VARIANTS
            .iter()
            .copied()
            .filter(move |&node| self.contains(node))
    }
}

pub(crate) struct HydrationPlan {
    level: SafetyLevel,
    groups: Vec<Group>,
}

pub(super) struct Group {
    pub(super) source: Source,
    pub(super) input: Option<Hydrator>,
    pub(super) nodes: Hydrators,
    clients: Vec<&'static str>,
    methods: Vec<&'static str>,
}

impl HydrationPlan {
    pub(crate) fn new(level: SafetyLevel, hydrators: Hydrators) -> Self {
        let nodes = hydrators.closed();
        let mut groups: Vec<Group> = Vec::new();
        for node in nodes.iter() {
            let spec = node.spec();
            let (client, method) = spec.label;
            let input = node.input();
            match groups
                .iter_mut()
                .find(|group| group.source == spec.source && group.input == input)
            {
                Some(group) => {
                    group.nodes = group.nodes.with(node);
                    if !group.clients.contains(&client) {
                        group.clients.push(client);
                    }
                    if !group.methods.contains(&method) {
                        group.methods.push(method);
                    }
                }
                None => groups.push(Group {
                    source: spec.source,
                    input,
                    nodes: Hydrators::of(node),
                    clients: vec![client],
                    methods: vec![method],
                }),
            }
        }
        Self { level, groups }
    }

    pub(crate) fn level(&self) -> SafetyLevel {
        self.level
    }

    pub(super) fn groups(&self) -> impl Iterator<Item = &Group> {
        self.groups.iter()
    }
}

impl Group {
            pub(super) fn label(&self) -> (String, String) {
        (self.clients.join("+"), self.methods.join("+"))
    }

        pub(super) fn fields(&self) -> Vec<QueryFields> {
        let mut fields = Vec::new();
        for node in Hydrator::VARIANTS {
            if let (true, Part::Fields(node_fields)) =
                (node.spec().source == self.source, node.spec().part)
            {
                for field in node_fields {
                    if !fields.contains(field) {
                        fields.push(*field);
                    }
                }
            }
        }
        fields
    }

        pub(super) fn edges(&self) -> Vec<(Graph, EdgeDirection, Vec<KeyOrigin>)> {
        let mut edges: Vec<(Graph, EdgeDirection, Vec<KeyOrigin>)> = Vec::new();
        for node in self.nodes.iter() {
            let spec = node.spec();
            let Part::Edge(graph, direction) = spec.part else {
                continue;
            };
            match edges
                .iter_mut()
                .find(|(g, d, _)| (*g, *d) == (graph, direction))
            {
                Some((_, _, keys)) => keys.push(spec.key),
                None => edges.push((graph, direction, vec![spec.key])),
            }
        }
        edges
    }
}

impl fmt::Display for KeyOrigin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyOrigin::RequestTweets => f.write_str("tweets"),
            KeyOrigin::Viewer => f.write_str("viewer"),
            KeyOrigin::PureCoreAuthor => f.write_str("author"),
            KeyOrigin::PureCoreReplyRoot => f.write_str("reply_root"),
            KeyOrigin::ExclusiveConversationAuthor => f.write_str("exclusive_author"),
            KeyOrigin::ConversationRoot(arms) => {
                let arms: Vec<String> = arms.iter().map(|arm| format!("{arm:?}")).collect();
                write!(f, "root:{}", arms.join("|"))
            }
            KeyOrigin::ViewerForCoAllowedList => f.write_str("viewer:co_allowed_list"),
        }
    }
}

impl fmt::Display for HydrationPlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "{}: {} calls",
            <&str>::from(self.level),
            self.groups.len()
        )?;
        for group in &self.groups {
            let (client, method) = group.label();
            let after = group.input.map_or("-", <&str>::from);
            let nodes: Vec<&str> = group.nodes.iter().map(<&str>::from).collect();
            write!(
                f,
                "{client}/{method} after: {after} nodes: {}",
                nodes.join(",")
            )?;
            let fields = group.fields();
            if !fields.is_empty() {
                let fields: Vec<String> = fields.iter().map(|field| format!("{field:?}")).collect();
                write!(f, " fields: {}", fields.join("|"))?;
            }
            for (graph, direction, keys) in group.edges() {
                let direction = match direction {
                    EdgeDirection::Forward => "fwd",
                    EdgeDirection::Reverse => "rev",
                };
                let keys: Vec<String> = keys.iter().map(KeyOrigin::to_string).collect();
                write!(
                    f,
                    " {}-{direction}[{}]",
                    <&str>::from(graph),
                    keys.join(",")
                )?;
            }
            if group.nodes.iter().all(Hydrator::needs_viewer) {
                f.write_str(" (skipped logged out)")?;
            }
            writeln!(f)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::rules::{RuleEngine, SafetyLevel};
    use strum::VariantArray;

            const PLANS: &str = "\
filter_all: 1 calls
tes/get_tweet_core_datas after: - nodes: pure_core
timeline_home: 7 calls
tes/get_tweet_core_datas after: - nodes: pure_core
tes/get_tweets_for_visibility after: - nodes: tweet
safety_labels/get after: - nodes: tweet_safety_labels
gizmoduck/get_viewer_data after: - nodes: viewer_profile fields: ACCOUNT|EXTENDED_PROFILE|SAFETY (skipped logged out)
gizmoduck/get_users after: pure_core nodes: author_safety fields: SAFETY|LABELS
socialgraph/batch_check_relationships after: pure_core nodes: follows,blocks,mutes,mute_retweets follows-fwd[author] blocks-fwd[author] mutes-fwd[author] mute_retweets-fwd[author] (skipped logged out)
exclusive_content/batch_check_super_follows after: tweet nodes: super_follows_exclusive super_follows-fwd[exclusive_author] (skipped logged out)
timeline_home_recommendations: 7 calls
tes/get_tweet_core_datas after: - nodes: pure_core
tes/get_tweets_for_visibility after: - nodes: tweet
safety_labels/get after: - nodes: tweet_safety_labels
gizmoduck/get_viewer_data after: - nodes: viewer_profile fields: ACCOUNT|EXTENDED_PROFILE|SAFETY (skipped logged out)
gizmoduck/get_users after: pure_core nodes: author_safety,author_labels fields: SAFETY|LABELS
socialgraph/batch_check_relationships after: pure_core nodes: follows,blocks,mutes,mute_retweets follows-fwd[author] blocks-fwd[author] mutes-fwd[author] mute_retweets-fwd[author] (skipped logged out)
exclusive_content/batch_check_super_follows after: tweet nodes: super_follows_exclusive super_follows-fwd[exclusive_author] (skipped logged out)
timeline_home_hydration: 10 calls
tes/get_tweet_core_datas after: - nodes: pure_core
tes/get_tweets_for_visibility after: - nodes: tweet
conversation_control/get_conversation_controls after: - nodes: conversation_control
safety_labels/get after: - nodes: tweet_safety_labels
gizmoduck/get_viewer_data after: - nodes: viewer_profile fields: ACCOUNT|EXTENDED_PROFILE|SAFETY (skipped logged out)
gizmoduck/get_users after: pure_core nodes: author_safety fields: SAFETY|LABELS
blocked_by/batch_check_blocked_by after: pure_core nodes: blocked_by_author,blocked_by_reply_root blocks-rev[author,reply_root] (skipped logged out)
exclusive_content/batch_check_super_follows after: tweet nodes: super_follows_exclusive super_follows-fwd[exclusive_author] (skipped logged out)
conversation_control/batch_check_followed_by+batch_check_super_follows after: conversation_control nodes: root_follows_viewer,super_follows_root follows-rev[root:Community|MyNetwork] super_follows-fwd[root:Subscribers] (skipped logged out)
conversation_control/tfe_top_country after: conversation_control nodes: viewer_country (skipped logged out)
immersive_expanded_recommendations: 7 calls
tes/get_tweet_core_datas after: - nodes: pure_core
tes/get_tweets_for_visibility after: - nodes: tweet
safety_labels/get after: - nodes: tweet_safety_labels
gizmoduck/get_viewer_data after: - nodes: viewer_profile fields: ACCOUNT|EXTENDED_PROFILE|SAFETY (skipped logged out)
gizmoduck/get_users after: pure_core nodes: author_safety,author_labels fields: SAFETY|LABELS
socialgraph/batch_check_relationships after: pure_core nodes: follows,blocks,mutes,mute_retweets follows-fwd[author] blocks-fwd[author] mutes-fwd[author] mute_retweets-fwd[author] (skipped logged out)
exclusive_content/batch_check_super_follows after: tweet nodes: super_follows_exclusive super_follows-fwd[exclusive_author] (skipped logged out)
";

    #[test]
    fn each_level_plans_these_calls() {
        let engine = RuleEngine::for_tests();
        let plans: String = SafetyLevel::VARIANTS
            .iter()
            .map(|&level| engine.plan(level).to_string())
            .collect();
        assert_eq!(plans, PLANS);
    }
}
