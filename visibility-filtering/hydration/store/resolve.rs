use super::{RequestTweet, Store};
use crate::hydration::fetcher::AnyFetcher;
use crate::hydration::inventory::{MissPolicy, Subject};
use crate::hydration::{Cause, HydratedTweet, Hydrator, Hydrators, Lookup, Unresolved};
use crate::models::{AuthorId, TweetId};
use rustc_hash::FxHashMap;

impl Store {
    pub(super) fn hydrated_tweet(
        &self,
        request_tweet: &RequestTweet,
        tweet_misses: &FxHashMap<TweetId, Option<Cause>>,
        incomplete: Hydrators,
        unclaimed: &mut Vec<(Hydrator, u64)>,
    ) -> HydratedTweet {
        let id = request_tweet.tweet_id;
        let safety_labels = self.labels.get(id.0).cloned();
        match self.resolve(request_tweet, tweet_misses) {
            Ok(author_id) => {
                let candidate = self.candidate(request_tweet, author_id, incomplete, unclaimed);
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

    fn resolve(
        &self,
        request_tweet: &RequestTweet,
        tweet_misses: &FxHashMap<TweetId, Option<Cause>>,
    ) -> Result<AuthorId, Unresolved> {
        let tweet = |cause| Unresolved {
            lookup: Lookup::Tweet,
            cause,
        };
        let id = request_tweet.tweet_id;
        if let Some(&Some(cause)) = tweet_misses.get(&id) {
            match cause {
                Cause::NotFound if self.quotes.quoted_author(id).is_some() => {}
                Cause::NotFound if self.quotes.is_quoted_author_failed(id) => {
                    return Err(tweet(Cause::Failed));
                }
                Cause::NotFound | Cause::Failed => return Err(tweet(cause)),
            }
        }
        let author_id = request_tweet.author.ok_or_else(|| tweet(Cause::Failed))?;
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
            .filter(|_| self.is_expanding_retweet_sources)?;
        let source_id = core.source_tweet_id?;
        let shared_tweet = *tweet_misses.get(&source_id)?;
        let shared_author = core.source_author_id.or_else(|| {
            self.request_tweets
                .iter()
                .find(|request_tweet| request_tweet.tweet_id == source_id)?
                .author
        });
        Some((
            shared_tweet,
            shared_author.and_then(|author_id| self.author_miss(author_id)),
        ))
    }

    fn unresolving(
        &self,
        subject: Subject,
    ) -> impl Iterator<Item = (Hydrator, &dyn AnyFetcher)> + '_ {
        self.callable
            .iter()
            .filter(move |node| node.spec().source.miss_policy() == MissPolicy::Unresolves(subject))
            .filter_map(|node| Some((node, self.fetcher(node)?)))
    }

    pub(super) fn tweet_miss(
        &self,
        tweet_id: TweetId,
        unclaimed: &mut Vec<(Hydrator, u64)>,
    ) -> Option<Cause> {
        let key = tweet_id.0;
        let has_joined_sources = self.has_joined_sources();
        self.unresolving(Subject::Tweet)
            .filter_map(|(node, fetcher)| {
                if has_joined_sources && !fetcher.is_claimed(key) {
                    unclaimed.push((node, key));
                }
                fetcher.miss(key)
            })
            .min()
    }

    fn author_miss(&self, author_id: AuthorId) -> Option<Cause> {
        self.unresolving(Subject::Author)
            .filter_map(|(_, fetcher)| fetcher.miss(author_id.get()))
            .min()
    }
}
