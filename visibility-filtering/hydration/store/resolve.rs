use super::key_origin::{KeyOrigin, Names};
use super::{RequestRow, Store};
use crate::hydration::fetcher::AnyFetcher;
use crate::hydration::inventory::MissPolicy;
use crate::hydration::relation::Relation;
use crate::hydration::{
    Cause, HydratedTweet, HydratedUser, Hydrator, Hydrators, Lookup, Unresolved,
};
use crate::models::{
    AuthorId, ConversationControlFeatures, PostCandidate, PostEntity, SafetyLabelMap,
    TweetFeatures, TweetId, UserEntity, ViewerRelationship,
};
use rustc_hash::FxHashMap;
use xai_core_entities::entities::ConversationControlArm;

impl Store {
    pub(super) fn hydrated_tweet(
        &self,
        row: &RequestRow,
        position: usize,
        id: TweetId,
        tweet_misses: &FxHashMap<TweetId, Option<Cause>>,
        incomplete: Hydrators,
        unclaimed: &mut Vec<(Hydrator, u64)>,
    ) -> HydratedTweet {
        let safety_labels = self.labels.get(id.0).cloned();
        match self.resolve(row, id, tweet_misses) {
            Ok(author_id) => {
                let candidate = self.candidate(row, position, id, author_id, incomplete, unclaimed);
                HydratedTweet::Resolved {
                    candidate,
                    source_tweet_id: self.source_tweet_id(id),
                    safety_labels,
                }
            }
            Err(reason) => HydratedTweet::Unresolved {
                reason,
                safety_labels,
            },
        }
    }

    pub(super) fn hydrated_user(
        &self,
        row: &RequestRow,
        position: usize,
        user: AuthorId,
        incomplete: Hydrators,
        unclaimed: &mut Vec<(Hydrator, u64)>,
    ) -> HydratedUser {
        match self.author_miss(user) {
            Some(cause) => HydratedUser::Unresolved(cause),
            None => HydratedUser::Resolved {
                user: self.author(user),
                viewer_relationship: self.edges(row, position).0,
                failed: self.failed(row, position, incomplete, unclaimed),
            },
        }
    }

    fn resolve(
        &self,
        row: &RequestRow,
        id: TweetId,
        tweet_misses: &FxHashMap<TweetId, Option<Cause>>,
    ) -> Result<AuthorId, Unresolved> {
        let tweet = |cause| Unresolved {
            lookup: Lookup::Tweet,
            cause,
        };
        if let Some(&Some(cause)) = tweet_misses.get(&id) {
            match cause {
                Cause::NotFound if self.quotes.quoted_author(id).is_some() => {}
                Cause::NotFound if self.quotes.is_quoted_author_failed(id) => {
                    return Err(tweet(Cause::Failed));
                }
                Cause::NotFound | Cause::Failed => return Err(tweet(cause)),
            }
        }
        let author_id = row.author.ok_or_else(|| tweet(Cause::Failed))?;
        let (shared_tweet, shared_author) =
            self.shared_misses(id, tweet_misses).unwrap_or_default();
        [
            (Lookup::SharedTweet, shared_tweet),
            (Lookup::Author, self.author_miss(author_id)),
            (Lookup::SharedAuthor, shared_author),
        ]
        .into_iter()
        .filter_map(|(lookup, miss)| {
            Some(Unresolved {
                lookup,
                cause: miss?,
            })
        })
        .min_by_key(|unresolved| unresolved.cause)
        .map_or(Ok(author_id), Err)
    }

    fn shared_misses(
        &self,
        tweet_id: TweetId,
        tweet_misses: &FxHashMap<TweetId, Option<Cause>>,
    ) -> Option<(Option<Cause>, Option<Cause>)> {
        let core = self
            .pure_cores
            .get(tweet_id.0)
            .filter(|_| self.is_on(Relation::RetweetSource))?;
        let source_id = core.source_tweet_id?;
        let shared_tweet = *tweet_misses.get(&source_id)?;
        let shared_author = core.source_author_id.or_else(|| {
            self.rows
                .iter()
                .find(|row| row.tweet_id() == Some(source_id))?
                .author
        });
        Some((
            shared_tweet,
            shared_author.and_then(|author_id| self.author_miss(author_id)),
        ))
    }

    fn unresolving(&self, names: Names) -> impl Iterator<Item = (Hydrator, &dyn AnyFetcher)> + '_ {
        self.callable
            .iter()
            .filter(move |&node| {
                node.spec().source.miss_policy() == MissPolicy::Unresolves
                    && self.origin(node).names() == names
            })
            .filter_map(|node| Some((node, self.fetcher(node)?)))
    }

    pub(super) fn tweet_miss(
        &self,
        tweet_id: TweetId,
        unclaimed: &mut Vec<(Hydrator, u64)>,
    ) -> Option<Cause> {
        let key = tweet_id.0;
        let has_joined_retweet_sources = self.has_joined_retweet_sources;
        self.unresolving(Names::Post)
            .filter_map(|(node, fetcher)| {
                if has_joined_retweet_sources && !fetcher.is_claimed(key) {
                    unclaimed.push((node, key));
                }
                fetcher.miss(key)
            })
            .min()
    }

    fn author_miss(&self, author_id: AuthorId) -> Option<Cause> {
        self.unresolving(Names::Author)
            .filter_map(|(_, fetcher)| fetcher.miss(author_id.get()))
            .min()
    }

    fn author(&self, author_id: AuthorId) -> UserEntity {
        let (author_features, author_labels) = self
            .authors
            .get(author_id.get())
            .copied()
            .unwrap_or_default();
        UserEntity::new(author_id.get(), author_features, author_labels)
    }

    fn edges(&self, row: &RequestRow, position: usize) -> (ViewerRelationship, Hydrators) {
        let (viewer_relationship, post_edges) = self
            .callable
            .iter()
            .filter(|&node| {
                node.edge()
                    .and_then(|edge| {
                        let key = self.parent_key(node, row, position)?;
                        self.edge_fetcher(edge)?.get(key).copied()
                    })
                    .unwrap_or(false)
            })
            .fold(
                (ViewerRelationship::default(), Hydrators::empty()),
                |(mut relationship, mut post), node| {
                    if let Some(slot) = relationship.slot(node) {
                        *slot = true;
                    } else {
                        post = post.with(node);
                    }
                    (relationship, post)
                },
            );
        debug_assert!(post_edges
            .iter()
            .all(|node| viewer_relationship.get(node).is_none()));
        (viewer_relationship, post_edges)
    }

    pub(super) fn candidate(
        &self,
        row: &RequestRow,
        position: usize,
        tweet_id: TweetId,
        author_id: AuthorId,
        incomplete: Hydrators,
        unclaimed: &mut Vec<(Hydrator, u64)>,
    ) -> PostCandidate {
        let id = tweet_id.0;
        let (viewer_relationship, post_edges) = self.edges(row, position);
        PostCandidate {
            author: self.author(author_id),
            viewer_relationship,
            post: PostEntity {
                tweet_id: id,
                source_tweet_id: self.source_tweet_id(tweet_id).map(|source| source.0),
                is_reply: self
                    .pure_cores
                    .get(id)
                    .is_some_and(|core| core.reply_to.is_some()),
                tweet_features: TweetFeatures::default(),
                safety_labels: self
                    .labels
                    .get(id)
                    .map(|labels| SafetyLabelMap::from_proto_label_types(labels))
                    .unwrap_or_default(),
                edges: post_edges,
                conversation_control: self.control(row).cloned().map(|control| {
                    ConversationControlFeatures {
                        viewer_country: self
                            .viewer_id
                            .filter(|_| control.arm == ConversationControlArm::Co)
                            .and_then(|viewer_id| self.viewer_country.get(viewer_id))
                            .cloned(),
                        control,
                    }
                }),
                community_moderation: self
                    .community_moderations
                    .get(id)
                    .copied()
                    .unwrap_or_default(),
                viewer_is_community_moderator: self
                    .key(KeyOrigin::ModeratedCommunity, row)
                    .and_then(|community_id| self.community_moderators.get(community_id))
                    .copied(),
                viewer_is_removed_from_community: self
                    .key(KeyOrigin::TweetCommunity, row)
                    .and_then(|community_id| self.community_viewer_removals.get(community_id))
                    .copied()
                    .unwrap_or_default(),
                article_has_media_entities: self
                    .key(KeyOrigin::TweetArticle, row)
                    .and_then(|article_id| self.article_media_entities.get(article_id))
                    .copied()
                    .unwrap_or_default(),
                article_has_cover_media: self
                    .key(KeyOrigin::TweetArticle, row)
                    .and_then(|article_id| self.article_cover_media.get(article_id))
                    .copied()
                    .unwrap_or_default(),
                article_lifecycle: self
                    .key(KeyOrigin::TweetArticle, row)
                    .and_then(|article_id| self.article_lifecycles.get(article_id))
                    .copied(),
                reacted_to_post_safety_labels: self
                    .read_key(Hydrator::ReactedToPostSafetyLabels, position)
                    .and_then(|post_id| self.labels.get(post_id))
                    .map(|labels| SafetyLabelMap::from_proto_label_types(labels))
                    .unwrap_or_default(),
                reacted_to_author_is_protected: self
                    .read_key(Hydrator::ReactedToAuthorSafety, position)
                    .and_then(|user_id| self.authors.get(user_id))
                    .is_some_and(|(author, _)| author.is_protected),
            },
            failed: self.failed(row, position, incomplete, unclaimed),
        }
    }
}
