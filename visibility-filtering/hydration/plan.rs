use crate::clients::socialgraph_client::{EdgeDirection, Graph};
use crate::hydration::hydrator::{Edge, Part, RequestRows};
use crate::hydration::inventory::Source;
use crate::hydration::relation::{Relation, Role};
use crate::hydration::{Hydrator, Hydrators, BEST_EFFORT_HYDRATION_TIMEOUT, HYDRATION_TIMEOUT};
use crate::rules::SafetyLevel;
use std::fmt;
use std::time::Duration;
use strum::VariantArray;
use xai_core_entities::gizmoduck_client::QueryFields;

pub(crate) struct HydrationPlan {
    level: SafetyLevel,
    rows: RequestRows,
    groups: Vec<Group>,
    nodes: Hydrators,
    logged_out_nodes: Hydrators,
}

pub(super) struct Group {
    pub(super) position: usize,
    pub(super) source: Source,
    pub(super) input: Option<Hydrator>,
    pub(super) nodes: Hydrators,
    clients: Vec<&'static str>,
    methods: Vec<&'static str>,
    edges: Vec<(Edge, Graph, EdgeDirection, Hydrators)>,
    fields: Vec<QueryFields>,
    readers: Vec<usize>,
}

impl HydrationPlan {
    pub(crate) fn new(level: SafetyLevel, hydrators: Hydrators) -> Self {
        Self::with_rows(level, level.spec().rows, hydrators)
    }

    pub(crate) fn with_rows(level: SafetyLevel, rows: RequestRows, hydrators: Hydrators) -> Self {
        let nodes = hydrators.closed(rows);
        let mut groups: Vec<Group> = Vec::new();
        for node in nodes.iter() {
            let spec = node.spec();
            let (client, method) = spec.label;
            let input = node.input_at(rows);
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
                    position: groups.len(),
                    source: spec.source,
                    input,
                    nodes: Hydrators::of(node),
                    clients: vec![client],
                    methods: vec![method],
                    edges: Vec::new(),
                    fields: Vec::new(),
                    readers: Vec::new(),
                }),
            }
        }
        let reads: Vec<Hydrators> = groups
            .iter()
            .map(|group| {
                group.nodes.iter().fold(Hydrators::empty(), |reads, node| {
                    reads.union(node.reads_at(rows))
                })
            })
            .collect();
        let readers: Vec<Vec<usize>> = groups
            .iter()
            .map(|landed| {
                reads
                    .iter()
                    .enumerate()
                    .filter(|(_, reads)| !reads.intersection(landed.nodes).is_empty())
                    .map(|(position, _)| position)
                    .collect()
            })
            .collect();
        for (group, readers) in groups.iter_mut().zip(readers) {
            group.edges = edges(group.nodes);
            group.fields = fields(group.source, group.nodes);
            group.readers = readers;
        }
        Self {
            level,
            rows,
            groups,
            nodes,
            logged_out_nodes: nodes
                .iter()
                .filter(|node| !node.needs_viewer())
                .fold(Hydrators::empty(), Hydrators::with),
        }
    }

    pub(crate) fn level(&self) -> SafetyLevel {
        self.level
    }

    pub(crate) fn rows(&self) -> RequestRows {
        self.rows
    }

    pub(crate) fn keys_only_user_rows_or_viewer(&self) -> bool {
        self.nodes
            .iter()
            .all(|node| node.key_at(self.rows).keys_user_rows())
    }

    pub(super) fn reads_viewer_profile(&self) -> bool {
        self.nodes.contains(Hydrator::ViewerProfile)
    }

    pub(super) fn reads_viewer_labels(&self) -> bool {
        self.nodes.contains(Hydrator::ViewerLabels)
    }

    pub(super) fn callable(&self, viewer_id: Option<u64>) -> Hydrators {
        match viewer_id {
            Some(_) => self.nodes,
            None => self.logged_out_nodes,
        }
    }

    pub(super) fn groups(&self) -> impl Iterator<Item = &Group> {
        self.groups.iter()
    }

    fn relations(&self) -> impl Iterator<Item = Relation> + '_ {
        Relation::VARIANTS
            .iter()
            .copied()
            .filter(|&relation| match relation.spec().role {
                Role::Judged { rpc } => {
                    self.level.is_carried_by(rpc)
                        && !self
                            .nodes
                            .intersection(relation.spec().trigger.reads())
                            .is_empty()
                }
                Role::Read => self
                    .nodes
                    .iter()
                    .any(|node| node.spec().relation == Some(relation)),
            })
    }

    pub(super) fn readers<'a>(&'a self, landed: &'a Group) -> impl Iterator<Item = &'a Group> {
        landed
            .readers
            .iter()
            .filter_map(|&position| self.groups.get(position))
    }
}

impl Group {
    pub(super) fn ceiling(&self) -> Duration {
        if self.nodes.has_must_have() {
            HYDRATION_TIMEOUT
        } else {
            BEST_EFFORT_HYDRATION_TIMEOUT
        }
    }

    pub(super) fn reads_relation(&self) -> bool {
        self.nodes.iter().any(|node| node.spec().relation.is_some())
    }

    pub(super) fn label(&self) -> (String, String) {
        (self.clients.join("+"), self.methods.join("+"))
    }

    pub(super) fn fields(&self) -> &[QueryFields] {
        &self.fields
    }

    pub(super) fn edges(&self) -> &[(Edge, Graph, EdgeDirection, Hydrators)] {
        &self.edges
    }
}

fn fields(source: Source, nodes: Hydrators) -> Vec<QueryFields> {
    let nodes = match source {
        Source::GizmoduckAuthor => Hydrator::VARIANTS
            .iter()
            .copied()
            .filter(|node| node.spec().source == source)
            .fold(Hydrators::empty(), Hydrators::with),
        Source::TesPureCoreData
        | Source::TesTweet
        | Source::TesConversationControl
        | Source::TesQuotedTweet
        | Source::SafetyLabels
        | Source::GizmoduckViewer
        | Source::Flock
        | Source::FlockUserPair
        | Source::ViewerCountry
        | Source::Wingman
        | Source::CommunityModeration
        | Source::CommunityModerator
        | Source::CommunityViewerRemoved
        | Source::ArticleLifecycle
        | Source::ArticleMediaEntities
        | Source::ArticleCoverMedia
        | Source::TrustedFriends
        | Source::UserLocation => nodes,
    };
    let mut fields = Vec::new();
    for node in nodes.iter() {
        if let Part::Fields(node_fields) = node.spec().part {
            for field in node_fields {
                if !fields.contains(field) {
                    fields.push(*field);
                }
            }
        }
    }
    fields
}

fn edges(nodes: Hydrators) -> Vec<(Edge, Graph, EdgeDirection, Hydrators)> {
    let mut edges: Vec<(Edge, Graph, EdgeDirection, Hydrators)> = Vec::new();
    for node in nodes.iter() {
        let Some(edge) = node.edge() else {
            continue;
        };
        let Some((graph, direction)) = edge.flock() else {
            continue;
        };
        match edges.iter_mut().find(|(queried, ..)| *queried == edge) {
            Some((.., nodes)) => *nodes = nodes.with(node),
            None => edges.push((edge, graph, direction, Hydrators::of(node))),
        }
    }
    edges
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
            for &(_, graph, direction, nodes) in group.edges() {
                let direction = match direction {
                    EdgeDirection::Forward => "fwd",
                    EdgeDirection::Reverse => "rev",
                };
                let keys: Vec<String> = nodes
                    .iter()
                    .map(|node| node.key_at(self.rows).to_string())
                    .collect();
                write!(
                    f,
                    " {}-{direction}[{}]",
                    <&str>::from(graph),
                    keys.join(",")
                )?;
            }
            for node in group.nodes.iter() {
                if let Some(edge) = node.pair_edge() {
                    let graph: &str = edge.graph().into();
                    write!(f, " {graph}-fwd[{}]", node.key_at(self.rows))?;
                }
            }
            if group.nodes.iter().all(Hydrator::needs_viewer) {
                f.write_str(" (skipped logged out)")?;
            }
            if group.ceiling() != HYDRATION_TIMEOUT {
                write!(f, " timeout: {:?}", group.ceiling())?;
            }
            writeln!(f)?;
        }
        for relation in self.relations() {
            writeln!(f, "{relation}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{HydrationPlan, RequestRows};
    use crate::hydration::{Hydrator, Hydrators};
    use crate::rules::{RuleEngine, SafetyLevel};
    use strum::VariantArray;

    #[test]
    fn each_level_plans_these_calls() {
        let engine = RuleEngine::for_tests();
        let plans: String = SafetyLevel::VARIANTS
            .iter()
            .map(|&level| engine.plan(level).to_string())
            .collect();
        crate::pinned::assert_pinned("tests/fixtures/plans.txt", &plans);
    }

    #[test]
    fn a_level_of_user_rows_plans_no_pure_core_and_keys_author_lookups_by_user() {
        let nodes = Hydrators::of(Hydrator::ViewerProfile)
            .with(Hydrator::AuthorSafety)
            .with(Hydrator::Follows)
            .with(Hydrator::Blocks);
        let plan =
            HydrationPlan::with_rows(SafetyLevel::QuotedTweetRules, RequestRows::Users, nodes);
        assert_eq!(
            plan.to_string(),
            "\
quoted_tweet_rules: 3 calls
gizmoduck/get_viewer_data after: - nodes: viewer_profile fields: ACCOUNT|EXTENDED_PROFILE|SAFETY (skipped logged out)
gizmoduck/get_users after: - nodes: author_safety fields: SAFETY|LABELS
socialgraph/batch_check_relationships after: - nodes: follows,blocks follows-fwd[users] blocks-fwd[users] (skipped logged out)
"
        );
        assert!(plan.keys_only_user_rows_or_viewer());
    }
}
