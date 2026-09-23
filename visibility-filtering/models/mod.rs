pub mod author;
pub mod conversation_control;
pub mod exclusive_content;
pub mod relationship;
pub mod safety_labels;
pub mod tweet;
pub mod verdict;
pub mod viewer;

pub use author::{AuthorFeatures, AuthorLabel, AuthorLabelSet};
pub use conversation_control::ConversationControlFeatures;
pub use exclusive_content::ExclusiveContentFeatures;
pub use relationship::ViewerAuthorRelationship;
pub use safety_labels::{SafetyLabelMap, SafetyLabelType};
pub use tweet::{MediaFeature, NsfwFeature, TweetFeatures};
pub use verdict::{
    Decided, LimitedEngagement, LimitedEngagementReason, MediaInterstitial, TombstoneReason,
    Verdict, Withholding,
};
pub use viewer::{Viewer, ViewerAge, ViewerFeatures, ViewerProfile};

use crate::hydration::batch::TweetHydrationBatch;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TweetId(pub u64);

pub fn tweet_timestamp_ms(tweet_id: u64) -> u64 {
    const SNOWFLAKE_EPOCH_MS: u64 = 1288834974657;
    const SNOWFLAKE_TIMESTAMP_SHIFT: u32 = 22;
    (tweet_id >> SNOWFLAKE_TIMESTAMP_SHIFT) + SNOWFLAKE_EPOCH_MS
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AuthorId(pub u64);

impl AuthorId {
    pub fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RawCandidate {
    pub tweet_id: TweetId,
    pub request_author_id: Option<u64>,
}

#[derive(Clone, Copy, Debug)]
pub struct TweetCandidateInput {
    pub tweet_id: TweetId,
    pub author_id: AuthorId,
}

pub(crate) fn resolve_candidates(
    raw: &[RawCandidate],
    authors: &TweetHydrationBatch<AuthorId>,
) -> Vec<TweetCandidateInput> {
    raw.iter()
        .filter_map(|c| {
            let author_id = match c.request_author_id {
                Some(author_id) => AuthorId(author_id),
                None => *authors.get(&c.tweet_id)?,
            };
            Some(TweetCandidateInput {
                tweet_id: c.tweet_id,
                author_id,
            })
        })
        .collect()
}

#[derive(Clone, Debug, Default)]
pub struct HydratedTweetCandidate {
    pub tweet_id: u64,
    pub author_id: u64,
    pub tweet_features: TweetFeatures,
    pub author_features: AuthorFeatures,
    pub safety_labels: SafetyLabelMap,
    pub relationship: ViewerAuthorRelationship,
    pub exclusive_content: Option<ExclusiveContentFeatures>,
    pub conversation_control: Option<ConversationControlFeatures>,
}

pub fn assemble(
    candidate: &TweetCandidateInput,
    tweet_features: TweetFeatures,
    author_features: AuthorFeatures,
    safety_labels: SafetyLabelMap,
    relationship: ViewerAuthorRelationship,
    exclusive_content: Option<ExclusiveContentFeatures>,
    conversation_control: Option<ConversationControlFeatures>,
) -> HydratedTweetCandidate {
    HydratedTweetCandidate {
        tweet_id: candidate.tweet_id.0,
        author_id: candidate.author_id.get(),
        tweet_features,
        author_features,
        safety_labels,
        relationship,
        exclusive_content,
        conversation_control,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn resolve_candidates_prefers_the_request_author_and_drops_unresolved_tweets() {
        let authors = TweetHydrationBatch::from_values(
            [TweetId(2), TweetId(3), TweetId(4)],
            HashMap::from([(TweetId(2), AuthorId(20)), (TweetId(4), AuthorId(40))]),
        );
        let raw = vec![
            RawCandidate {
                tweet_id: TweetId(1),
                request_author_id: Some(10),
            },
            RawCandidate {
                tweet_id: TweetId(2),
                request_author_id: None,
            },
            RawCandidate {
                tweet_id: TweetId(3),
                request_author_id: None,
            },
            RawCandidate {
                tweet_id: TweetId(4),
                request_author_id: Some(41),
            },
        ];
        let resolved: Vec<(TweetId, u64)> = resolve_candidates(&raw, &authors)
            .into_iter()
            .map(|c| (c.tweet_id, c.author_id.get()))
            .collect();
        assert_eq!(
            resolved,
            vec![(TweetId(1), 10), (TweetId(2), 20), (TweetId(4), 41)]
        );
    }
}
