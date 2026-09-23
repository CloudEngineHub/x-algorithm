use crate::hydration::batch::TweetHydrationBatch;
use crate::hydration::fallback_cache::FallbackCache;
use crate::hydration::metrics::{record_batch_size, timed_results};
use crate::hydration::tes_composite::{TweetForVisibility, TweetForVisibilitySource};
use crate::models::{
    AuthorId, MediaFeature, NsfwFeature, TweetCandidateInput, TweetFeatures, TweetId,
};
use crate::rules::SafetyLevel;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use xai_core_entities::entities::MediaEntities;
use xai_core_entities::tweet_entity_service_client::TESClient;

const CLIENT_TIMEOUT: Duration = crate::hydration::HYDRATION_TIMEOUT;
const CLIENT: &str = "tes";

pub(crate) type AuthorIdFallbackCache = FallbackCache<TweetId, AuthorId>;

pub struct TesHydrator {
    tes_client: Arc<dyn TESClient + Send + Sync>,
    tweet_source: Arc<dyn TweetForVisibilitySource>,
    author_id_cache: Option<AuthorIdFallbackCache>,
}

impl TesHydrator {
    pub(crate) fn new(
        tes_client: Arc<dyn TESClient + Send + Sync>,
        tweet_source: Arc<dyn TweetForVisibilitySource>,
        author_id_cache: Option<AuthorIdFallbackCache>,
    ) -> Self {
        Self {
            tes_client,
            tweet_source,
            author_id_cache,
        }
    }

    pub(crate) fn author_id_fallback_cache(capacity: usize) -> AuthorIdFallbackCache {
        FallbackCache::new("author_id", capacity)
    }

    pub(crate) async fn fetch_author_ids(
        &self,
        tweet_ids: &[TweetId],
        safety_level: SafetyLevel,
    ) -> TweetHydrationBatch<AuthorId> {
        if tweet_ids.is_empty() {
            return TweetHydrationBatch::empty();
        }
        let cache_request = self
            .author_id_cache
            .as_ref()
            .map(|cache| (cache, cache.begin_request()));
        let candidate_count_by_key = candidates_per_tweet(tweet_ids);
        let raw_ids: Vec<u64> = candidate_count_by_key.keys().copied().collect();
        record_batch_size(CLIENT, candidate_count_by_key.len());
        let fetched = timed_results(
            CLIENT,
            "get_tweet_core_datas",
            safety_level,
            &candidate_count_by_key,
            CLIENT_TIMEOUT,
            self.tes_client.get_tweet_core_datas(raw_ids),
        )
        .await
        .map_keys(TweetId)
        .map(|core| AuthorId(core.author_id));
        match cache_request {
            Some((cache, generation)) => cache.resolve_hydration_batch(generation, fetched),
            None => fetched,
        }
    }

    pub(crate) async fn hydrate_tweets(
        &self,
        tweet_ids: &[TweetId],
        safety_level: SafetyLevel,
    ) -> TweetHydrationBatch<TweetForVisibility> {
        let candidate_count_by_key = candidates_per_tweet(tweet_ids);
        let raw_ids: Vec<u64> = candidate_count_by_key.keys().copied().collect();
        timed_results(
            CLIENT,
            "get_tweets_for_visibility",
            safety_level,
            &candidate_count_by_key,
            CLIENT_TIMEOUT,
            self.tweet_source.get_tweets_for_visibility(&raw_ids),
        )
        .await
        .map_keys(TweetId)
    }

    pub(crate) fn assemble_tweet_features(
        &self,
        candidates: &[TweetCandidateInput],
        tweet_keyed: &TweetHydrationBatch<TweetForVisibility>,
    ) -> HashMap<TweetId, TweetFeatures> {
        candidates
            .iter()
            .map(|c| {
                (
                    c.tweet_id,
                    build_tweet_features(tweet_keyed.get(&c.tweet_id)),
                )
            })
            .collect()
    }
}

pub(super) fn candidates_per_tweet(tweet_ids: &[TweetId]) -> HashMap<u64, usize> {
    let mut candidate_count_by_key = HashMap::with_capacity(tweet_ids.len());
    for tweet_id in tweet_ids {
        *candidate_count_by_key.entry(tweet_id.0).or_default() += 1;
    }
    candidate_count_by_key
}

fn build_tweet_features(tweet: Option<&TweetForVisibility>) -> TweetFeatures {
    match tweet {
        Some(tweet) => TweetFeatures {
            source_tweet_id: tweet.source_tweet_id,
            media: tweet.media.clone(),
            takedown_reasons: tweet.takedown_reasons.clone(),
            nsfw: NsfwFeature {
                user: tweet.nsfw_user,
                admin: tweet.nsfw_admin,
            },
            is_nullcast: tweet.is_nullcast,
            is_community_tweet: tweet.is_community_tweet,
            edit_control: tweet.edit_control.clone(),
        },
        None => TweetFeatures::default(),
    }
}

pub(crate) fn media_feature(entities: MediaEntities) -> MediaFeature {
    let mut feature = MediaFeature {
        has_media: !entities.is_empty(),
        ..Default::default()
    };

    for restrictions in entities
        .iter()
        .filter(|e| e.media_key.is_some())
        .filter_map(|e| e.additional_metadata.as_ref())
        .filter_map(|metadata| metadata.restrictions.as_ref())
    {
        feature.has_dmca_media |= restrictions.is_dmca == Some(true);
        if let Some(geo) = &restrictions.geo_restrictions {
            feature
                .geo_allow_list
                .extend(geo.whitelisted_country_codes.iter().flatten().cloned());
            feature
                .geo_deny_list
                .extend(geo.blacklisted_country_codes.iter().flatten().cloned());
        }
    }

    feature
}
