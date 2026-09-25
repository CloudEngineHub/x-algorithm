use crate::hydration::fallback_cache::FallbackCache;
use crate::hydration::tes_composite::TweetForVisibility;
use crate::models::{AuthorId, MediaFeature, NsfwFeature, PureCore, TweetFeatures, TweetId};
use std::collections::HashMap;
use xai_core_entities::entities::{MediaEntities, PureCoreData};

pub(crate) type PureCoreFallbackCache = FallbackCache<TweetId, PureCore>;

pub(crate) fn pure_core_fallback_cache(capacity: usize) -> PureCoreFallbackCache {
    FallbackCache::new("author_id", capacity)
}

pub(super) fn pure_core(core: &PureCoreData) -> PureCore {
    PureCore {
        author_id: AuthorId(core.author_id),
        source_tweet_id: core.source_tweet_id.map(TweetId),
        direct_reply_root_author_id: direct_reply_root_author(core),
    }
}

fn direct_reply_root_author(core: &PureCoreData) -> Option<AuthorId> {
    core.in_reply_to_tweet_id
        .filter(|&replied_to| core.conversation_id == Some(replied_to))
        .and(core.in_reply_to_user_id)
        .map(AuthorId)
}

pub(super) fn candidates_per_tweet(tweet_ids: &[TweetId]) -> HashMap<u64, usize> {
    let mut candidate_count_by_key = HashMap::with_capacity(tweet_ids.len());
    for tweet_id in tweet_ids {
        *candidate_count_by_key.entry(tweet_id.0).or_default() += 1;
    }
    candidate_count_by_key
}

pub(super) fn build_tweet_features(tweet: Option<&TweetForVisibility>) -> TweetFeatures {
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
            exclusive_conversation_author_id: tweet.exclusive_conversation_author_id,
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
