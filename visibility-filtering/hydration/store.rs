use crate::clients::socialgraph_client::{EdgeFrom, EdgeQuery};
use crate::hydration::community_source::CommunityPost;
use crate::hydration::decode::author::{AuthorNotFound, DecodedAuthor};
use crate::hydration::decode::viewer::DecodedViewer;
use crate::hydration::execute::Reply;
use crate::hydration::fetcher::{AnyFetcher, Fetcher};
use crate::hydration::hydrator::{Edge, RequestRows};
use crate::hydration::inventory::{Batch, MissPolicy, Source};
use crate::hydration::metrics::record_unasked_keys;
use crate::hydration::plan::Group;
use crate::hydration::relation::{Related, Relation, Role, Trigger};
use crate::hydration::{
    Cause, HydratedTweet, HydratedUser, Hydration, HydrationPlan, HydrationRequest, Hydrator,
    Hydrators, QuoteEntry,
};
use crate::metrics::Rpc;
use crate::models::{
    ArticleLifecycle, AuthorId, ClientCapability, CommunityModeration, PureCore, RawCandidate,
    TweetFeatures, TweetId, Viewer, ViewerFeatures,
};
use conversation_root::ConversationRootAuthor;
pub(super) use key_origin::KeyOrigin;
use key_origin::Names;
use quotes::{Pairing, Quotes};
use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::Arc;
use std::time::Duration;
use strum::VariantArray;
use xai_core_entities::entities::{ConversationControl, ConversationControlArm};
use xai_visibility_filtering_proto as vf_pb;

mod conversation_root;
mod key_origin;
mod quotes;
mod resolve;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowSubject {
    Post(TweetId),
    User,
}

struct RequestRow {
    subject: RowSubject,
    request_author: Option<AuthorId>,
    author: Option<AuthorId>,
}

impl RequestRow {
    fn user(user: AuthorId) -> Self {
        Self {
            subject: RowSubject::User,
            request_author: Some(user),
            author: Some(user),
        }
    }

    fn tweet_id(&self) -> Option<TweetId> {
        match self.subject {
            RowSubject::Post(tweet_id) => Some(tweet_id),
            RowSubject::User => None,
        }
    }
}

pub(super) struct CallRequest<'p> {
    pub(super) group: &'p Group,
    pub(super) is_first: bool,
    pub(super) keys: Vec<u64>,
    pub(super) key_count: usize,
    pub(super) queries: Vec<EdgeQuery>,
    pub(super) community_posts: Vec<CommunityPost>,
    pub(super) viewer_id: Option<u64>,
    pub(super) batch_size: Option<usize>,
        pub(super) candidate_count_by_claimed_key: FxHashMap<u64, usize>,
}

fn claimed<'a>(keys: &'a [u64], queries: &'a [EdgeQuery]) -> impl Iterator<Item = u64> + 'a {
    let destinations = queries.iter().flat_map(|query| &query.destination_ids);
    keys.iter().chain(destinations).copied()
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Landing {
    Answered,
    KeysJoined,
}

#[derive(Default)]
pub(super) struct Store {
    viewer_id: Option<u64>,
    client_capability: ClientCapability,
    rows: Vec<RequestRow>,
    related: FxHashMap<(Relation, usize), TweetId>,
    row_tweet_ids: FxHashSet<TweetId>,
    request_rows: RequestRows,
    requested: usize,
    quotes: Quotes,
    posts_judged_for: Option<Rpc>,
    has_joined_retweet_sources: bool,
    callable: Hydrators,
    pure_cores: Fetcher<PureCore>,
    tweets: Fetcher<TweetFeatures>,
    controls: Fetcher<ConversationControl>,
    labels: Fetcher<Arc<vf_pb::SafetyLabelMap>>,
    viewer: Fetcher<DecodedViewer>,
    authors: Fetcher<DecodedAuthor>,
    author_not_found: AuthorNotFound,
    edges: [Fetcher<bool>; Edge::VARIANTS.len()],
    has_called: Vec<bool>,
    viewer_country: Fetcher<Arc<str>>,
    community_moderations: Fetcher<CommunityModeration>,
    community_moderators: Fetcher<bool>,
    community_viewer_removals: Fetcher<bool>,
    article_lifecycles: Fetcher<ArticleLifecycle>,
    article_media_entities: Fetcher<bool>,
    article_cover_media: Fetcher<bool>,
    pub(super) core_elapsed: Duration,
    pub(super) tweets_elapsed: Option<Duration>,
}

impl Store {
    pub(super) fn new(
        plan: &HydrationPlan,
        viewer_id: Option<u64>,
        client_capability: ClientCapability,
        raw: &[RawCandidate],
        posts_judged_for: Option<Rpc>,
    ) -> Self {
        Self {
            viewer_id,
            client_capability,
            requested: raw.len(),
            posts_judged_for,
            rows: raw
                .iter()
                .map(|candidate| RequestRow {
                    subject: RowSubject::Post(candidate.tweet_id),
                    request_author: candidate.request_author_id.map(AuthorId),
                    author: None,
                })
                .collect(),
            row_tweet_ids: raw.iter().map(|candidate| candidate.tweet_id).collect(),
            request_rows: plan.rows(),
            callable: plan.callable(viewer_id),
            author_not_found: plan.level().spec().author_not_found,
            has_called: vec![false; plan.groups().count()],
            ..Self::default()
        }
    }

    pub(super) fn with_quotes(mut self, quotes: &[QuoteEntry]) -> Self {
        self.quotes = Quotes::new(quotes);
        self
    }

    pub(super) fn with_users(mut self, users: &[AuthorId]) -> Self {
        self.rows
            .extend(users.iter().copied().map(RequestRow::user));
        self.requested = self.rows.len();
        self
    }

    fn origin(&self, node: Hydrator) -> KeyOrigin {
        node.key_at(self.request_rows)
    }

    fn candidate_count(&self) -> usize {
        self.rows
            .iter()
            .take(self.requested)
            .filter(|row| row.author.is_some())
            .count()
    }

    fn is_on(&self, relation: Relation) -> bool {
        relation.is_on(self.posts_judged_for, self.callable)
    }

    fn named(&self, relation: Relation, parent: &RequestRow) -> Option<Related> {
        match relation.spec().trigger {
            Trigger::PureCore(named) => named(self.pure_cores.get(parent.tweet_id()?.0)?),
            Trigger::Tweet(named) => {
                let tweet_id = parent.tweet_id()?.0;
                if !self.pure_cores.has_landed(tweet_id) {
                    return None;
                }
                named(self.tweets.get(tweet_id)?)
            }
            Trigger::Key(origin) => Some(Related {
                tweet_id: TweetId(self.key(origin, parent)?),
                author: None,
            }),
        }
    }

    fn related_post(&self, relation: Relation, parent: usize) -> Option<Related> {
        let tweet_id = *self.related.get(&(relation, parent))?;
        let named = self.named(relation, self.rows.get(parent)?)?;
        (named.tweet_id == tweet_id).then_some(named)
    }

    fn related_posts(&self, relation: Relation) -> impl Iterator<Item = Related> + '_ {
        self.related
            .keys()
            .filter(move |&&(joined, _)| joined == relation)
            .filter_map(|&(relation, parent)| self.related_post(relation, parent))
    }

    fn read_key(&self, node: Hydrator, parent: usize) -> Option<u64> {
        let related = self.related_post(node.spec().relation?, parent)?;
        self.related_key(self.origin(node), related)
    }

    fn parent_key(&self, node: Hydrator, row: &RequestRow, position: usize) -> Option<u64> {
        match node.spec().relation {
            None => self.key(self.origin(node), row),
            Some(_) => self.read_key(node, position),
        }
    }

    fn pairing(&self) -> Pairing<'_> {
        Pairing::new(&self.rows, &self.pure_cores, &self.authors)
    }

    fn controls(&self) -> impl Iterator<Item = &ConversationControl> {
        self.rows.iter().filter_map(|row| self.control(row))
    }

    fn control(&self, row: &RequestRow) -> Option<&ConversationControl> {
        self.controls.get(row.tweet_id()?.0)
    }

    fn related_authors_wait_for_pending_authors(&self, group: &Group) -> bool {
        group
            .nodes
            .iter()
            .any(|node| self.origin(node) == KeyOrigin::RelatedAuthor)
            && self.rows.iter().any(|row| {
                row.author.is_none()
                    && row
                        .tweet_id()
                        .is_some_and(|tweet_id| !self.pure_cores.has_landed(tweet_id.0))
            })
    }

    fn source_tweet_id(&self, tweet_id: TweetId) -> Option<TweetId> {
        self.pure_cores.get(tweet_id.0)?.source_tweet_id
    }

        fn community_post(&self, row: &RequestRow) -> Option<CommunityPost> {
        let tweet_id = row.tweet_id()?.0;
        let author_id = row.author?.get();
        if self.viewer_id == Some(author_id) {
            return None;
        }
        Some(CommunityPost {
            tweet_id,
            author_id,
            community_id: self.tweets.get(tweet_id)?.community_id?.get(),
        })
    }

    fn community_posts(&self, tweet_ids: &[u64]) -> Vec<CommunityPost> {
        let mut posts: Vec<CommunityPost> = self
            .rows
            .iter()
            .filter_map(|row| self.community_post(row))
            .filter(|post| tweet_ids.binary_search(&post.tweet_id).is_ok())
            .collect();
        posts.sort_unstable_by_key(|post| post.tweet_id);
        posts.dedup_by_key(|post| post.tweet_id);
        posts
    }

    fn keys(&self, nodes: Hydrators) -> Vec<u64> {
        let mut keys = Vec::new();
        for (position, node) in nodes.iter().enumerate() {
            let origin = self.origin(node);
            if nodes
                .iter()
                .take(position)
                .any(|earlier| self.origin(earlier) == origin)
            {
                continue;
            }
            keys.extend(self.node_keys(node));
        }
        if nodes.contains(Hydrator::PureCore) {
            keys.extend(self.quotes.outers_to_fetch().iter().map(|outer| outer.0));
        }
        keys.sort_unstable();
        keys.dedup();
        keys
    }

    fn claim(&mut self, nodes: Hydrators) -> Vec<u64> {
        let keys = self.keys(nodes);
        match nodes.iter().next().and_then(|node| self.fetcher_mut(node)) {
            Some(fetcher) => fetcher.claim(keys),
            None => Vec::new(),
        }
    }

    fn edge_fetcher(&self, edge: Edge) -> Option<&Fetcher<bool>> {
        self.edges.get(usize::from(edge as u8))
    }

    fn edge_fetcher_mut(&mut self, edge: Edge) -> Option<&mut Fetcher<bool>> {
        self.edges.get_mut(usize::from(edge as u8))
    }

    fn fetcher(&self, node: Hydrator) -> Option<&dyn AnyFetcher> {
        let fetcher: &dyn AnyFetcher = match node.spec().source {
            Source::TesPureCoreData => &self.pure_cores,
            Source::TesTweet => &self.tweets,
            Source::TesConversationControl => &self.controls,
            Source::TesQuotedTweet => &self.quotes.quoted_tweets,
            Source::SafetyLabels => &self.labels,
            Source::GizmoduckViewer => &self.viewer,
            Source::GizmoduckAuthor => &self.authors,
            Source::ViewerCountry => &self.viewer_country,
            Source::CommunityModeration => &self.community_moderations,
            Source::CommunityModerator => &self.community_moderators,
            Source::CommunityViewerRemoved => &self.community_viewer_removals,
            Source::ArticleLifecycle => &self.article_lifecycles,
            Source::ArticleMediaEntities => &self.article_media_entities,
            Source::ArticleCoverMedia => &self.article_cover_media,
            Source::Flock | Source::Wingman | Source::TrustedFriends | Source::UserLocation => {
                self.edge_fetcher(node.edge()?)?
            }
            Source::FlockUserPair => return None,
        };
        Some(fetcher)
    }

    fn fetcher_mut(&mut self, node: Hydrator) -> Option<&mut dyn AnyFetcher> {
        let fetcher: &mut dyn AnyFetcher = match node.spec().source {
            Source::TesPureCoreData => &mut self.pure_cores,
            Source::TesTweet => &mut self.tweets,
            Source::TesConversationControl => &mut self.controls,
            Source::TesQuotedTweet => &mut self.quotes.quoted_tweets,
            Source::SafetyLabels => &mut self.labels,
            Source::GizmoduckViewer => &mut self.viewer,
            Source::GizmoduckAuthor => &mut self.authors,
            Source::ViewerCountry => &mut self.viewer_country,
            Source::CommunityModeration => &mut self.community_moderations,
            Source::CommunityModerator => &mut self.community_moderators,
            Source::CommunityViewerRemoved => &mut self.community_viewer_removals,
            Source::ArticleLifecycle => &mut self.article_lifecycles,
            Source::ArticleMediaEntities => &mut self.article_media_entities,
            Source::ArticleCoverMedia => &mut self.article_cover_media,
            Source::Flock | Source::Wingman | Source::TrustedFriends | Source::UserLocation => {
                self.edge_fetcher_mut(node.edge()?)?
            }
            Source::FlockUserPair => return None,
        };
        Some(fetcher)
    }

    fn candidate_count_by_key(&self, nodes: Hydrators) -> FxHashMap<u64, usize> {
        let mut counts = self
            .quotes
            .candidate_count_by_quoted_author(nodes, &self.pairing());
        let (viewer_nodes, row_nodes): (Vec<Hydrator>, Vec<Hydrator>) = nodes
            .iter()
            .partition(|&node| self.origin(node).names() == Names::Viewer);
        for node in viewer_nodes {
            counts.extend(self.node_keys(node).into_iter().map(|key| (key, 1)));
        }
        for (parent, row) in self.rows.iter().enumerate() {
            for (position, &node) in row_nodes.iter().enumerate() {
                let Some(key) = self.parent_key(node, row, parent) else {
                    continue;
                };
                let is_candidate = row.author.is_some() || node.precedes_author(self.request_rows);
                let counted = row_nodes
                    .iter()
                    .take(position)
                    .any(|&earlier| self.parent_key(earlier, row, parent) == Some(key));
                if is_candidate && !counted {
                    *counts.entry(key).or_default() += 1;
                }
            }
        }
        counts
    }

    pub(super) fn offer<'p>(&mut self, group: &'p Group) -> Option<CallRequest<'p>> {
        if self.viewer_id.is_none() && group.nodes.iter().all(Hydrator::needs_viewer) {
            return None;
        }
        if self.related_authors_wait_for_pending_authors(group) {
            return None;
        }
        let is_first = self.has_called.get(group.position) != Some(&true);
        let mut queries: Vec<EdgeQuery> = group
            .edges()
            .iter()
            .map(|&(_, graph, direction, nodes)| EdgeQuery {
                graph,
                direction,
                from: EdgeFrom::Viewer,
                destination_ids: self.claim(nodes),
            })
            .collect();
        let pairing = Pairing::new(&self.rows, &self.pure_cores, &self.authors);
        queries.extend(self.quotes.claim_pairs(group.nodes, &pairing));
        let keys = if queries.is_empty() {
            self.claim(group.nodes)
        } else {
            Vec::new()
        };
        let key_count = claimed(&keys, &queries).count();
        if key_count == 0 {
            return None;
        }
        let community_posts = match group.source {
            Source::CommunityModeration => self.community_posts(&keys),
            Source::TesPureCoreData
            | Source::TesTweet
            | Source::TesConversationControl
            | Source::TesQuotedTweet
            | Source::SafetyLabels
            | Source::GizmoduckViewer
            | Source::GizmoduckAuthor
            | Source::Flock
            | Source::FlockUserPair
            | Source::ViewerCountry
            | Source::Wingman
            | Source::CommunityModerator
            | Source::CommunityViewerRemoved
            | Source::ArticleLifecycle
            | Source::ArticleMediaEntities
            | Source::ArticleCoverMedia
            | Source::TrustedFriends
            | Source::UserLocation => Vec::new(),
        };
        if let Some(has_called) = self.has_called.get_mut(group.position) {
            *has_called = true;
        }
        let candidate_count_by_key = self.candidate_count_by_key(group.nodes);
        let candidate_count_by_claimed_key = claimed(&keys, &queries)
            .map(|key| (key, candidate_count_by_key.get(&key).copied().unwrap_or(0)))
            .collect();
        Some(CallRequest {
            group,
            is_first,
            batch_size: self.batch_size(group, is_first, key_count),
            candidate_count_by_claimed_key,
            viewer_id: self.viewer_id,
            keys,
            key_count,
            queries,
            community_posts,
        })
    }

    fn batch_size(&self, group: &Group, is_first: bool, key_count: usize) -> Option<usize> {
        let size = match group.source.row().batch {
            Batch::Unrecorded => return None,
            Batch::Keys => key_count,
            Batch::Requested
                if !is_first
                    || group
                        .nodes
                        .iter()
                        .any(|node| self.origin(node) == KeyOrigin::RelatedTweet) =>
            {
                key_count
            }
            Batch::Requested if group.input == Some(Hydrator::PureCore) => self.candidate_count(),
            Batch::Requested => self.requested,
        };
        Some(size)
    }

    pub(super) fn is_country_lookup_skipped(&self) -> bool {
        self.callable.contains(Hydrator::ViewerCountry)
            && !self.viewer_country.has_claimed()
            && self
                .controls()
                .any(|control| control.arm == ConversationControlArm::Co)
    }

    pub(super) fn land(
        &mut self,
        call: &CallRequest<'_>,
        reply: Reply,
        elapsed: Duration,
    ) -> Landing {
        let keys = &call.keys;
        let is_first_request_core = call.is_first && call.group.nodes.contains(Hydrator::PureCore);
        match reply {
            Reply::PureCores(pure_cores) => {
                if is_first_request_core {
                    self.core_elapsed = elapsed;
                }
                self.pure_cores.land(keys, pure_cores);
                self.land_authors();
            }
            Reply::Tweets(tweets) => {
                if call.is_first {
                    self.tweets_elapsed = Some(elapsed);
                }
                self.tweets.land(keys, tweets);
            }
            Reply::Controls(controls) => self.controls.land(keys, controls),
            Reply::QuotedTweets(quoted_tweets) => {
                self.quotes.quoted_tweets.land(keys, quoted_tweets);
                self.land_authors();
            }
            Reply::Labels(labels) => self.labels.land(keys, labels),
            Reply::Viewer(viewer) => self.viewer.land(keys, viewer),
            Reply::Authors(authors) => self.authors.land(keys, self.author_not_found.read(authors)),
            Reply::Select(answers) => {
                let queries = call.group.edges().iter().zip(&call.queries);
                for ((&(edge, ..), query), answer) in queries.zip(answers) {
                    if let Some(fetcher) = self.edge_fetcher_mut(edge) {
                        fetcher.land(&query.destination_ids, answer);
                    }
                }
            }
            Reply::Edge(edge, answers) => {
                if let Some(fetcher) = self.edge_fetcher_mut(edge) {
                    fetcher.land(keys, answers);
                }
            }
            Reply::PairEdges(answers) => self.quotes.land_pairs(&call.queries, answers),
            Reply::ViewerCountry(country) => self.viewer_country.land(keys, country),
            Reply::CommunityModerations(moderations) => {
                self.community_moderations.land(keys, moderations);
            }
            Reply::CommunityModerators(moderators) => {
                self.community_moderators.land(keys, moderators);
            }
            Reply::CommunityViewerRemovals(removals) => {
                self.community_viewer_removals.land(keys, removals);
            }
            Reply::ArticleLifecycles(lifecycles) => self.article_lifecycles.land(keys, lifecycles),
            Reply::ArticleMediaEntities(rows) => self.article_media_entities.land(keys, rows),
            Reply::ArticleCoverMedia(rows) => self.article_cover_media.land(keys, rows),
        }
        let has_joined = self.join(call.group.nodes);
        if is_first_request_core && self.callable.contains(Hydrator::AuthorBlocksOuterAuthor) {
            self.quotes.join_outers(&self.rows);
            if !self.quotes.outers_to_fetch().is_empty() {
                return Landing::KeysJoined;
            }
        }
        if has_joined {
            Landing::KeysJoined
        } else {
            Landing::Answered
        }
    }

    fn land_authors(&mut self) {
        let (pure_cores, quotes) = (&self.pure_cores, &self.quotes);
        for row in &mut self.rows {
            row.author = row
                .request_author
                .or_else(|| Some(pure_cores.get(row.tweet_id()?.0)?.author_id))
                .or_else(|| quotes.quoted_author(row.tweet_id()?));
        }
    }

    fn join(&mut self, landed: Hydrators) -> bool {
        let mut has_joined = false;
        for &relation in Relation::VARIANTS {
            let spec = relation.spec();
            if spec.trigger.reads().intersection(landed).is_empty() || !self.is_on(relation) {
                continue;
            }
            let named: Vec<(usize, Related)> = self
                .rows
                .iter()
                .enumerate()
                .filter_map(|(parent, row)| Some((parent, self.named(relation, row)?)))
                .collect();
            for (parent, related) in named {
                match spec.role {
                    Role::Judged { .. } => {
                        if !self.row_tweet_ids.insert(related.tweet_id) {
                            continue;
                        }
                        if relation == Relation::RetweetSource {
                            self.has_joined_retweet_sources = true;
                        }
                        self.rows.push(RequestRow {
                            subject: RowSubject::Post(related.tweet_id),
                            request_author: None,
                            author: related.author,
                        });
                    }
                    Role::Read => match self.related.entry((relation, parent)) {
                        Entry::Vacant(entry) => {
                            entry.insert(related.tweet_id);
                        }
                        Entry::Occupied(_) => continue,
                    },
                }
                has_joined = true;
            }
        }
        has_joined
    }

    pub(super) fn assemble(mut self, request: HydrationRequest<'_>) -> Hydration {
        let incomplete = self
            .callable
            .iter()
            .filter(|&node| {
                self.fetcher(node)
                    .is_some_and(|fetcher| fetcher.has_incomplete())
            })
            .fold(self.quotes.incomplete(self.callable), Hydrators::with);
        let mut unclaimed = Vec::new();
        let mut tweet_misses: FxHashMap<TweetId, Option<Cause>> =
            FxHashMap::with_capacity_and_hasher(self.rows.len(), Default::default());
        for tweet_id in self.rows.iter().filter_map(RequestRow::tweet_id) {
            if let Entry::Vacant(entry) = tweet_misses.entry(tweet_id) {
                entry.insert(self.tweet_miss(tweet_id, &mut unclaimed));
            }
        }
        let mut tweets: FxHashMap<TweetId, HydratedTweet> =
            FxHashMap::with_capacity_and_hasher(self.rows.len(), Default::default());
        let mut users: FxHashMap<AuthorId, HydratedUser> = FxHashMap::default();
        for (position, row) in self.rows.iter().enumerate() {
            match row.subject {
                RowSubject::Post(tweet_id) => {
                    if let Entry::Vacant(entry) = tweets.entry(tweet_id) {
                        entry.insert(self.hydrated_tweet(
                            row,
                            position,
                            tweet_id,
                            &tweet_misses,
                            incomplete,
                            &mut unclaimed,
                        ));
                    }
                }
                RowSubject::User => {
                    if let Some(user) = row.author
                        && let Entry::Vacant(entry) = users.entry(user)
                    {
                        entry.insert(self.hydrated_user(
                            row,
                            position,
                            user,
                            incomplete,
                            &mut unclaimed,
                        ));
                    }
                }
            }
        }
        for ((client, method), keys) in unclaimed_keys_by_label(unclaimed) {
            record_unasked_keys(client, method, keys);
        }
        for (id, tweet) in &mut tweets {
            match tweet {
                HydratedTweet::Resolved { candidate, .. } => {
                    candidate.post.tweet_features = self.tweets.take(id.0).unwrap_or_default();
                }
                HydratedTweet::Unresolved { .. } => {}
            }
        }
        let (viewer, viewer_state) = match request.viewer_id {
            None => (Viewer::LoggedOut, None),
            Some(id) => {
                let DecodedViewer {
                    profile,
                    has_age_verified_18_label,
                    state,
                } = self.viewer.take(id).unwrap_or_default();
                let viewer = Viewer::LoggedIn {
                    id,
                    profile,
                    has_age_verified_18_label,
                };
                (viewer, state)
            }
        };
        let quotes = self.quotes.quoted(self.callable, &self.pairing());
        Hydration {
            viewer: ViewerFeatures::from_request(
                viewer,
                request.country_code,
                request.client_capability,
            ),
            viewer_state,
            planned: self.callable,
            incomplete,
            tweets,
            users,
            quotes,
            has_fetched_retweet_sources: self.has_joined_retweet_sources,
            posts_judged_for: self.posts_judged_for,
        }
    }

    fn failed(
        &self,
        row: &RequestRow,
        position: usize,
        incomplete: Hydrators,
        unclaimed: &mut Vec<(Hydrator, u64)>,
    ) -> Hydrators {
        let has_joined_retweet_sources = self.has_joined_retweet_sources;
        let checked = if has_joined_retweet_sources {
            self.callable
        } else {
            incomplete
        };
        let mut answered_incompletely = Hydrators::empty();
        for node in checked.iter() {
            let Some(key) = self.parent_key(node, row, position) else {
                continue;
            };
            let Some(fetcher) = self.fetcher(node) else {
                continue;
            };
            if has_joined_retweet_sources && !fetcher.is_claimed(key) {
                unclaimed.push((node, key));
                answered_incompletely = answered_incompletely.with(node);
            } else if fetcher.is_incomplete(key) {
                match node.spec().source.miss_policy() {
                    MissPolicy::Unresolves | MissPolicy::FailsNode => {
                        answered_incompletely = answered_incompletely.with(node);
                    }
                    MissPolicy::Reads => {}
                }
            }
        }
        if self.callable.contains(Hydrator::BlockedByReplyRoot)
            && self.conversation_root_author(row)
                == Some(ConversationRootAuthor::RootPureCoreFailed)
        {
            answered_incompletely = answered_incompletely.with(Hydrator::BlockedByReplyRoot);
        }
        if answered_incompletely.is_empty() {
            return answered_incompletely;
        }
        self.callable
            .iter()
            .fold(answered_incompletely, |failed, node| {
                match node.input_at(self.request_rows) {
                    Some(input_node)
                        if failed.contains(input_node)
                            && self.parent_key(node, row, position).is_none() =>
                    {
                        failed.with(node)
                    }
                    _ => failed,
                }
            })
    }
}

fn unclaimed_keys_by_label(
    mut unclaimed: Vec<(Hydrator, u64)>,
) -> HashMap<(&'static str, &'static str), usize> {
    unclaimed.sort_unstable_by_key(|&(node, key)| (node as u8, key));
    unclaimed.dedup();
    let mut by_label = HashMap::new();
    for (node, _) in &unclaimed {
        *by_label.entry(node.spec().label).or_default() += 1;
    }
    by_label
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hydration::batch::HydrationBatch;
    use crate::rules::SafetyLevel;

    #[test]
    fn authors_prefer_the_request_author_and_leave_unresolved_tweets_without_one() {
        let core = |author| PureCore {
            author_id: AuthorId(author),
            source_tweet_id: None,
            source_author_id: None,
            reply_to: None,
        };
        let raw =
            [(1, Some(10)), (2, None), (3, None), (4, Some(41))].map(|(id, author)| RawCandidate {
                tweet_id: TweetId(id),
                request_author_id: author,
            });
        let plan = HydrationPlan::new(SafetyLevel::FilterAll, Hydrators::empty());
        let mut store = Store::new(&plan, None, ClientCapability::default(), &raw, None);
        let group = plan.groups().next().unwrap();
        let call = store
            .offer(group)
            .expect("pure core claims the request tweets");
        let pure_cores = HydrationBatch::from_results(
            [1, 2, 3, 4],
            HashMap::from([(2, Ok::<_, &str>(Some(core(20)))), (4, Ok(Some(core(40))))]),
        );
        store.land(&call, Reply::PureCores(pure_cores), Duration::ZERO);
        let resolved: Vec<(TweetId, u64)> = store
            .rows
            .iter()
            .filter_map(|row| Some((row.tweet_id()?, row.author?.get())))
            .collect();
        assert_eq!(
            resolved,
            vec![(TweetId(1), 10), (TweetId(2), 20), (TweetId(4), 41)]
        );
    }

    #[test]
    fn a_key_no_call_claimed_fails_its_candidate_once_sources_joined() {
        let raw = [RawCandidate {
            tweet_id: TweetId(1),
            request_author_id: None,
        }];
        let plan = HydrationPlan::new(SafetyLevel::FilterAll, Hydrators::empty());
        let mut store = Store::new(
            &plan,
            None,
            ClientCapability::default(),
            &raw,
            Some(Rpc::EvaluateTweets),
        );
        let call = store.offer(plan.groups().next().unwrap()).unwrap();
        let retweet = PureCore {
            author_id: AuthorId(10),
            source_tweet_id: Some(TweetId(5)),
            source_author_id: Some(AuthorId(20)),
            reply_to: None,
        };
        let cores =
            HydrationBatch::from_results([1], HashMap::from([(1, Ok::<_, ()>(Some(retweet)))]));
        assert_eq!(
            store.land(&call, Reply::PureCores(cores), Duration::ZERO),
            Landing::KeysJoined
        );
        let mut unclaimed = Vec::new();
        let source = &store.rows[1];
        for _ in 0..2 {
            let candidate = store.candidate(
                source,
                1,
                TweetId(5),
                AuthorId(20),
                Hydrators::empty(),
                &mut unclaimed,
            );
            assert_eq!(candidate.failed, Hydrators::of(Hydrator::PureCore));
        }
        assert_eq!(
            unclaimed_keys_by_label(unclaimed),
            HashMap::from([(Hydrator::PureCore.spec().label, 1)])
        );
    }
}
