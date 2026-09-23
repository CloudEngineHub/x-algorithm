use crate::clients::socialgraph_client::SocialgraphClient;
use crate::hydration::batch::Completeness;
use crate::hydration::metrics::{record_batch_size, timed_rpc, HydratorOutcome};
use crate::models::{ExclusiveContentFeatures, TweetId};
use crate::rules::SafetyLevel;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

const CLIENT_TIMEOUT: Duration = crate::hydration::HYDRATION_TIMEOUT;
const CLIENT: &str = "exclusive_content";

pub struct ExclusiveContentHydrator {
    pub sg_client: Arc<dyn SocialgraphClient + Send + Sync>,
}

impl ExclusiveContentHydrator {
    pub async fn hydrate(
        &self,
        conversation_authors: HashMap<TweetId, u64>,
        candidate_count: usize,
        viewer_id: Option<u64>,
        safety_level: SafetyLevel,
    ) -> HashMap<TweetId, Completeness<ExclusiveContentFeatures>> {
        record_batch_size(CLIENT, candidate_count);
        let root_author_ids: Vec<u64> = conversation_authors
            .values()
            .copied()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let super_follows = match viewer_id {
            Some(vid) if !root_author_ids.is_empty() => {
                timed_rpc(
                    CLIENT,
                    "batch_check_super_follows",
                    safety_level,
                    candidate_count,
                    CLIENT_TIMEOUT,
                    |follows: &Option<_>| match follows {
                        Some(_) => HydratorOutcome::Success,
                        None => HydratorOutcome::Error,
                    },
                    self.sg_client
                        .batch_check_super_follows(vid, &root_author_ids),
                )
                .await
            }
            _ => Some(HashMap::new()),
        };

        conversation_authors
            .into_iter()
            .map(|(tweet_id, author_id)| {
                let features = ExclusiveContentFeatures {
                    conversation_author_id: author_id,
                    viewer_super_follows_author: super_follows
                        .as_ref()
                        .and_then(|follows| follows.get(&author_id))
                        .copied()
                        .unwrap_or(false),
                };
                (
                    tweet_id,
                    Completeness::new(super_follows.is_some(), features),
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ViewerAuthorRelationship;
    use tonic::async_trait;

    enum SuperFollowLookup {
        Fails,
        Hangs,
        Succeeds,
    }

    #[async_trait]
    impl SocialgraphClient for SuperFollowLookup {
        async fn batch_check_relationships(
            &self,
            _: u64,
            _: &[u64],
        ) -> HashMap<u64, ViewerAuthorRelationship> {
            unreachable!("exclusive hydration only checks super follows")
        }

        async fn batch_check_super_follows(&self, _: u64, _: &[u64]) -> Option<HashMap<u64, bool>> {
            match self {
                Self::Fails => None,
                Self::Hangs => std::future::pending().await,
                Self::Succeeds => Some(HashMap::from([(10, true)])),
            }
        }

        async fn batch_check_followed_by(&self, _: u64, _: &[u64]) -> Option<HashMap<u64, bool>> {
            unreachable!("exclusive hydration only checks super follows")
        }
    }

    async fn hydrate_with(
        sg_client: Arc<dyn SocialgraphClient + Send + Sync>,
    ) -> HashMap<TweetId, Completeness<ExclusiveContentFeatures>> {
        tokio::time::timeout(
            CLIENT_TIMEOUT * 2,
            ExclusiveContentHydrator { sg_client }.hydrate(
                HashMap::from([(TweetId(1), 10)]),
                2,
                Some(50),
                SafetyLevel::TimelineHome,
            ),
        )
        .await
        .unwrap()
    }

    #[tokio::test(start_paused = true)]
    async fn super_follow_lookup_completeness_tracks_success() {
        for (lookup, complete, super_follows) in [
            (SuperFollowLookup::Fails, false, false),
            (SuperFollowLookup::Hangs, false, false),
            (SuperFollowLookup::Succeeds, true, true),
        ] {
            let out = hydrate_with(Arc::new(lookup)).await;
            assert_eq!(out[&TweetId(1)].is_complete(), complete);
            assert_eq!(
                out[&TweetId(1)].value(),
                &ExclusiveContentFeatures {
                    conversation_author_id: 10,
                    viewer_super_follows_author: super_follows,
                }
            );
        }
    }
}
