use crate::clients::socialgraph_client::{EdgeFrom, EdgeQuery};
use crate::hydration::community_source::CommunityPost;
use crate::hydration::decode::author::{AuthorNotFound, DecodedAuthor};
use crate::hydration::decode::viewer::DecodedViewer;
use crate::hydration::execute::Reply;
use crate::hydration::fetcher::{AnyFetcher, Fetcher};
use crate::hydration::hydrator::{
    Edge, KeyOrigin, RequestRows, arm_starts_conversation_root_select,
};
use crate::hydration::inventory::{Batch, MissPolicy, Source};
use crate::hydration::metrics::record_unasked_keys;
use crate::hydration::plan::Group;
use crate::hydration::{
    Cause, HydratedTweet, HydratedUser, Hydration, HydrationPlan, HydrationRequest, Hydrator,
    Hydrators, QuoteEntry, candidate_count_by_key,
};
use crate::models::{
    ArticleLifecycle, AuthorId, ClientCapability, CommunityModeration, PureCore, RawCandidate,
    TweetFeatures, TweetId, Viewer, ViewerFeatures,
};
use conversation_root::ConversationRootAuthor;
use quotes::{Pairing, Quotes};
use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::num::NonZeroU64;
use std::sync::Arc;
use std::time::Duration;
use strum::VariantArray;
use xai_core_entities::entities::{ConversationControl, ConversationControlArm};
use xai_visibility_filtering_proto as vf_pb;

mod conversation_root;
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
    request_rows: RequestRows,
    requested: usize,
    quotes: Quotes,
    is_expanding_retweet_sources: bool,
    callable: Hydrators,
    pure_cores: Fetcher<PureCore>,
    root_authors: Fetcher<AuthorId>,
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
        is_expanding_retweet_sources: bool,
    ) -> Self {
        Self {
            viewer_id,
            client_capability,
            requested: raw.len(),
            is_expanding_retweet_sources,
            rows: raw
                .iter()
                .map(|candidate| RequestRow {
                    subject: RowSubject::Post(candidate.tweet_id),
                    request_author: candidate.request_author_id.map(AuthorId),
                    author: None,
                })
                .collect(),
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

    fn has_joined_sources(&self) -> bool {
        self.rows.len() > self.requested
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

    fn source_tweet_id(&self, tweet_id: TweetId) -> Option<TweetId> {
        self.pure_cores.get(tweet_id.0)?.source_tweet_id
    }

    fn key(&self, origin: KeyOrigin, row: &RequestRow) -> Option<u64> {
        match origin {
            KeyOrigin::RequestTweets => row.tweet_id().map(|tweet_id| tweet_id.0),
            KeyOrigin::RequestUsers => match row.subject {
                RowSubject::User => row.author.map(AuthorId::get),
                RowSubject::Post(_) => None,
            },
            KeyOrigin::Viewer => self.viewer_id,
            KeyOrigin::ViewerForCoAllowedList => self
                .viewer_id
                .filter(|_| self.control(row).is_some_and(lists_countries)),
            KeyOrigin::PureCoreAuthor => row.tweet_id().and(row.author).map(AuthorId::get),
            KeyOrigin::PureCoreRetweeter => self
                .source_tweet_id(row.tweet_id()?)
                .and(row.author)
                .map(AuthorId::get),
            KeyOrigin::PureCoreReplyRoot => match self.conversation_root_author(row)? {
                ConversationRootAuthor::PureCore(author) => Some(author.get()),
                ConversationRootAuthor::ConversationControl(_)
                | ConversationRootAuthor::RootPost(_)
                | ConversationRootAuthor::RootPureCoreFailed
                | ConversationRootAuthor::Unresolved => None,
            },
            KeyOrigin::ExclusiveConversationAuthor => {
                self.tweets
                    .get(row.tweet_id()?.0)?
                    .exclusive_conversation_author_id
            }
            KeyOrigin::TweetArticle => self
                .tweets
                .get(row.tweet_id()?.0)?
                .article_id
                .map(NonZeroU64::get),
            KeyOrigin::ConversationRoot(arms) => self
                .control(row)
                .filter(|control| arms.contains(&control.arm))
                .map(|control| control.conversation_tweet_author_id),
            KeyOrigin::ConversationControlBlockAuthor => {
                let control = self.control(row)?;
                if arm_starts_conversation_root_select(control.arm) {
                    return Some(control.conversation_tweet_author_id);
                }
                match self.conversation_root_author(row)? {
                    ConversationRootAuthor::ConversationControl(author) => Some(author.get()),
                    ConversationRootAuthor::PureCore(_)
                    | ConversationRootAuthor::RootPost(_)
                    | ConversationRootAuthor::RootPureCoreFailed
                    | ConversationRootAuthor::Unresolved => None,
                }
            }
            KeyOrigin::ConversationRootToFetch => match self.conversation_root_author(row)? {
                ConversationRootAuthor::RootPost(root) => Some(root.0),
                ConversationRootAuthor::PureCore(_)
                | ConversationRootAuthor::ConversationControl(_)
                | ConversationRootAuthor::RootPureCoreFailed
                | ConversationRootAuthor::Unresolved => None,
            },
            KeyOrigin::ConversationRootPostAuthor => match self.conversation_root_author(row)? {
                ConversationRootAuthor::RootPost(root) => {
                    self.root_authors.get(root.0).map(|author| author.get())
                }
                ConversationRootAuthor::PureCore(_)
                | ConversationRootAuthor::ConversationControl(_)
                | ConversationRootAuthor::RootPureCoreFailed
                | ConversationRootAuthor::Unresolved => None,
            },
            KeyOrigin::MyNetworkRootNotFollowingViewer => self
                .control(row)
                .and_then(|control| self.root_not_following_viewer(control)),
            KeyOrigin::CommunityPost => self.community_post(row).map(|post| post.tweet_id),
            KeyOrigin::ModeratedCommunity => self
                .viewer_id
                .and_then(|_| self.community_post(row))
                .filter(|post| {
                    self.community_moderations
                        .get(post.tweet_id)
                        .is_some_and(|moderation| moderation.is_moderated())
                })
                .map(|post| post.community_id),
            KeyOrigin::TweetCommunity => self
                .viewer_id
                .filter(|_| self.client_capability.community_viewer_removed_limits)
                .and(
                    self.tweets
                        .get(row.tweet_id()?.0)?
                        .community_id
                        .map(NonZeroU64::get),
                ),
            KeyOrigin::TrustedFriendsList => {
                self.tweets.get(row.tweet_id()?.0)?.trusted_friends_list_id
            }
            KeyOrigin::NarrowcastPlace => self.tweets.get(row.tweet_id()?.0)?.narrowcast_place_id,
            KeyOrigin::OuterOfMissingQuote => {
                let tweet_id = row.tweet_id()?;
                self.quotes
                    .outer_of(tweet_id)
                    .filter(|_| self.pure_cores.miss(tweet_id.0) == Some(Cause::NotFound))
                    .map(|outer| outer.0)
            }
            KeyOrigin::QuotedAuthorToQuotingAuthor
            | KeyOrigin::QuotingAuthorToProtectedQuotedAuthor => None,
        }
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
            match origin {
                KeyOrigin::Viewer => keys.extend(self.viewer_id),
                KeyOrigin::QuotedAuthorToQuotingAuthor
                | KeyOrigin::QuotingAuthorToProtectedQuotedAuthor => {}
                KeyOrigin::RequestTweets
                | KeyOrigin::RequestUsers
                | KeyOrigin::PureCoreAuthor
                | KeyOrigin::PureCoreRetweeter
                | KeyOrigin::PureCoreReplyRoot
                | KeyOrigin::ExclusiveConversationAuthor
                | KeyOrigin::TweetArticle
                | KeyOrigin::ConversationRoot(_)
                | KeyOrigin::ConversationControlBlockAuthor
                | KeyOrigin::ConversationRootToFetch
                | KeyOrigin::ConversationRootPostAuthor
                | KeyOrigin::ViewerForCoAllowedList
                | KeyOrigin::MyNetworkRootNotFollowingViewer
                | KeyOrigin::CommunityPost
                | KeyOrigin::ModeratedCommunity
                | KeyOrigin::TweetCommunity
                | KeyOrigin::TrustedFriendsList
                | KeyOrigin::NarrowcastPlace
                | KeyOrigin::OuterOfMissingQuote => {
                    keys.extend(self.rows.iter().filter_map(|row| self.key(origin, row)));
                }
            }
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
            Source::TesConversationRoot => &self.root_authors,
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
            Source::TesConversationRoot => &mut self.root_authors,
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
        match nodes.iter().next().map(|node| self.origin(node)) {
            Some(KeyOrigin::RequestTweets) => {
                return candidate_count_by_key(
                    self.rows.iter().filter_map(|row| Some(row.tweet_id()?.0)),
                );
            }
            Some(KeyOrigin::Viewer | KeyOrigin::ViewerForCoAllowedList) => {
                return self
                    .keys(nodes)
                    .into_iter()
                    .map(|viewer| (viewer, 1))
                    .collect();
            }
            Some(
                KeyOrigin::QuotedAuthorToQuotingAuthor
                | KeyOrigin::QuotingAuthorToProtectedQuotedAuthor,
            ) => {
                return self
                    .quotes
                    .candidate_count_by_quoted_author(nodes, &self.pairing());
            }
            Some(
                KeyOrigin::RequestUsers
                | KeyOrigin::PureCoreAuthor
                | KeyOrigin::PureCoreRetweeter
                | KeyOrigin::PureCoreReplyRoot
                | KeyOrigin::ExclusiveConversationAuthor
                | KeyOrigin::TweetArticle
                | KeyOrigin::ConversationRoot(_)
                | KeyOrigin::ConversationControlBlockAuthor
                | KeyOrigin::ConversationRootToFetch
                | KeyOrigin::ConversationRootPostAuthor
                | KeyOrigin::MyNetworkRootNotFollowingViewer
                | KeyOrigin::CommunityPost
                | KeyOrigin::ModeratedCommunity
                | KeyOrigin::TweetCommunity
                | KeyOrigin::TrustedFriendsList
                | KeyOrigin::NarrowcastPlace
                | KeyOrigin::OuterOfMissingQuote,
            )
            | None => {}
        }
        let is_tweet_keyed = nodes
            .iter()
            .next()
            .is_some_and(|node| node.input_at(self.request_rows) == Some(Hydrator::Tweet));
        let mut counts = FxHashMap::default();
        for row in self
            .rows
            .iter()
            .filter(|row| is_tweet_keyed || row.author.is_some())
        {
            for (position, node) in nodes.iter().enumerate() {
                let Some(key) = self.key(self.origin(node), row) else {
                    continue;
                };
                let counted = nodes
                    .iter()
                    .take(position)
                    .any(|earlier| self.key(self.origin(earlier), row) == Some(key));
                if !counted {
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
            | Source::TesConversationRoot
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
            Batch::Requested if !is_first => key_count,
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
        match reply {
            Reply::PureCores(pure_cores) => {
                if call.is_first {
                    self.core_elapsed = elapsed;
                }
                self.pure_cores.land(keys, pure_cores);
                self.land_authors();
                if call.is_first {
                    if self.is_expanding_retweet_sources {
                        self.join_retweet_sources();
                    }
                    if self.callable.contains(Hydrator::AuthorBlocksOuterAuthor) {
                        self.quotes.join_outers(&self.rows);
                    }
                    if self.has_joined_sources() || !self.quotes.outers_to_fetch().is_empty() {
                        return Landing::KeysJoined;
                    }
                }
            }
            Reply::Tweets(tweets) => {
                if call.is_first {
                    self.tweets_elapsed = Some(elapsed);
                }
                self.tweets.land(keys, tweets);
            }
            Reply::RootAuthors(authors) => self.root_authors.land(keys, authors),
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
        Landing::Answered
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

    fn join_retweet_sources(&mut self) {
        let mut known: FxHashSet<TweetId> =
            self.rows.iter().filter_map(RequestRow::tweet_id).collect();
        let sources: Vec<RequestRow> = self
            .rows
            .iter()
            .filter_map(|row| {
                let core = self.pure_cores.get(row.tweet_id()?.0)?;
                let source = core
                    .source_tweet_id
                    .filter(|&source| known.insert(source))?;
                Some(RequestRow {
                    subject: RowSubject::Post(source),
                    request_author: None,
                    author: core.source_author_id,
                })
            })
            .collect();
        self.rows.extend(sources);
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
        for row in &self.rows {
            match row.subject {
                RowSubject::Post(tweet_id) => {
                    if let Entry::Vacant(entry) = tweets.entry(tweet_id) {
                        entry.insert(self.hydrated_tweet(
                            row,
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
                        entry.insert(self.hydrated_user(row, user, incomplete, &mut unclaimed));
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
            has_fetched_sources: self.has_joined_sources(),
        }
    }

    fn failed(
        &self,
        row: &RequestRow,
        incomplete: Hydrators,
        unclaimed: &mut Vec<(Hydrator, u64)>,
    ) -> Hydrators {
        let has_joined_sources = self.has_joined_sources();
        let checked = if has_joined_sources {
            self.callable
        } else {
            incomplete
        };
        let mut answered_incompletely = Hydrators::empty();
        for node in checked.iter() {
            let Some(key) = self.key(self.origin(node), row) else {
                continue;
            };
            let Some(fetcher) = self.fetcher(node) else {
                continue;
            };
            if has_joined_sources && !fetcher.is_claimed(key) {
                unclaimed.push((node, key));
                answered_incompletely = answered_incompletely.with(node);
            } else if fetcher.is_incomplete(key) {
                match node.spec().source.miss_policy() {
                    MissPolicy::Unresolves(_) | MissPolicy::FailsNode => {
                        answered_incompletely = answered_incompletely.with(node);
                    }
                    MissPolicy::ReadsNoEdge | MissPolicy::ReadsNotFound => {}
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
                            && self.key(self.origin(node), row).is_none() =>
                    {
                        failed.with(node)
                    }
                    _ => failed,
                }
            })
    }
}

fn lists_countries(control: &ConversationControl) -> bool {
    control.arm == ConversationControlArm::Co && !control.allowed_country_codes.is_empty()
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
        let mut store = Store::new(&plan, None, ClientCapability::default(), &raw, false);
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
        let mut store = Store::new(&plan, None, ClientCapability::default(), &raw, true);
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
