use crate::hydration::sources::Sources;
use crate::hydration::{HydrationOutput, HydrationRequest};
use crate::models::{RawCandidate, TweetId, Verdict};
use crate::rules::metrics::{self as ft_metrics, Rpc};
use crate::rules::{RuleEngine, SafetyLevel};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use xai_visibility_filtering_proto as vf_pb;

pub struct FilterRequest {
    pub viewer_id: Option<u64>,
    pub country_code: Option<String>,
    pub safety_level: SafetyLevel,
    pub candidates: Vec<RawCandidate>,
    pub rpc: Rpc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvaluationStatus {
    Evaluated,
    UnresolvedAuthor,
    Failed,
}

pub struct FilterOutcome {
    pub tweet_id: TweetId,
    pub source_tweet_id: Option<TweetId>,
    pub verdict: Verdict,
    pub status: EvaluationStatus,
    pub safety_labels: Option<vf_pb::SafetyLabelMap>,
}

pub struct FilterResponse {
    pub outcomes: Vec<FilterOutcome>,
}

pub struct FilterTweets {
    sources: Arc<dyn Sources>,
    rule_engine: RuleEngine,
}

impl FilterTweets {
    pub(crate) fn new(sources: Arc<dyn Sources>, rule_engine: RuleEngine) -> Self {
        Self {
            sources,
            rule_engine,
        }
    }

    pub async fn run(&self, request: FilterRequest) -> FilterResponse {
        let started = Instant::now();
        let hydration = self
            .rule_engine
            .plan(request.safety_level)
            .hydrate(
                &*self.sources,
                HydrationRequest::new(request.viewer_id, request.country_code, &request.candidates),
            )
            .await;
        let hydrated_at = Instant::now();
        ft_metrics::record_phase(request.rpc, "hydration", hydrated_at - started);
        let HydrationOutput {
            viewer_features,
            candidates: hydrated_candidates,
            safety_labels,
            failed_ids,
            pure_cores,
        } = hydration;
        let evaluated: HashMap<TweetId, Verdict> = hydrated_candidates
            .iter()
            .map(|candidate| {
                (
                    TweetId(candidate.tweet_id),
                    self.rule_engine
                        .evaluate(request.safety_level, &viewer_features, candidate),
                )
            })
            .collect();

        let outcomes: Vec<FilterOutcome> = request
            .candidates
            .iter()
            .map(|candidate| {
                let (verdict, status) = match evaluated.get(&candidate.tweet_id) {
                    None => (
                        Verdict::unresolved_author(),
                        EvaluationStatus::UnresolvedAuthor,
                    ),
                    Some(verdict) if failed_ids.contains(&candidate.tweet_id) => {
                        (verdict.clone(), EvaluationStatus::Failed)
                    }
                    Some(verdict) => (verdict.clone(), EvaluationStatus::Evaluated),
                };
                FilterOutcome {
                    tweet_id: candidate.tweet_id,
                    source_tweet_id: pure_cores
                        .get(&candidate.tweet_id)
                        .and_then(|core| core.source_tweet_id),
                    verdict,
                    status,
                    safety_labels: safety_labels
                        .get(&candidate.tweet_id)
                        .map(|labels| vf_pb::SafetyLabelMap::clone(labels)),
                }
            })
            .collect();

        ft_metrics::record_phase(request.rpc, "post_hydration", hydrated_at.elapsed());

        FilterResponse { outcomes }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use crate::clients::about_this_account_client::NoCountryRows;
    use crate::clients::socialgraph_client::NoEdges;
    use crate::hydration::sources::ProdSources;
    use crate::hydration::tes_composite::{MockTweetForVisibilitySource, TweetForVisibility};
    use crate::safety_label_source::lookup::{ManhattanLookup, RemoteSource, TwemcacheLookup};
    use crate::safety_label_source::types::{ManhattanOutcome, TwemcacheOutcome};
    use crate::safety_label_source::SafetyLabelSource;
    use tonic::async_trait;
    use xai_core_entities::gizmoduck_client::{GizmoduckClient, MockGizmoduckClient};
    use xai_core_entities::tweet_entity_service_client::{MockTESClient, TESClient};

    fn full_label_map() -> vf_pb::SafetyLabelMap {
        vf_pb::SafetyLabelMap {
            labels: HashMap::from([(999_999, vf_pb::SafetyLabel::default())]),
        }
    }

    struct FakeTwemcache;

    #[async_trait]
    impl TwemcacheLookup for FakeTwemcache {
        async fn get(&self, ids: &[u64]) -> HashMap<u64, TwemcacheOutcome> {
            ids.iter()
                .copied()
                .map(|id| {
                    let outcome = if id == 2 {
                        TwemcacheOutcome::Hit(full_label_map())
                    } else {
                        TwemcacheOutcome::Miss
                    };
                    (id, outcome)
                })
                .collect()
        }
    }

    struct FakeManhattan;

    #[async_trait]
    impl ManhattanLookup for FakeManhattan {
        async fn get(&self, ids: &[u64]) -> HashMap<u64, ManhattanOutcome> {
            ids.iter()
                .copied()
                .map(|id| (id, ManhattanOutcome::Resolved(full_label_map())))
                .collect()
        }
    }

    pub(crate) fn filter_tweets() -> FilterTweets {
        filter_tweets_with_gizmoduck(Arc::new(MockGizmoduckClient::default()))
    }

    pub(crate) fn filter_tweets_with_gizmoduck(
        gizmoduck: Arc<dyn GizmoduckClient + Send + Sync>,
    ) -> FilterTweets {
        filter_tweets_with_clients(Arc::new(MockTESClient::default()), gizmoduck)
    }

    pub(crate) fn safety_labels() -> Arc<SafetyLabelSource> {
        safety_labels_with_manhattan(Arc::new(FakeManhattan))
    }

    pub(crate) fn safety_labels_with_manhattan<M: ManhattanLookup + 'static>(
        manhattan: Arc<M>,
    ) -> Arc<SafetyLabelSource> {
        Arc::new(SafetyLabelSource::new(Arc::new(RemoteSource::new(
            Arc::new(FakeTwemcache),
            manhattan,
        ))))
    }

    pub(crate) fn exclusive_tweet() -> TweetForVisibility {
        TweetForVisibility {
            author_id: 900,
            source_tweet_id: None,
            is_nullcast: false,
            nsfw_user: false,
            nsfw_admin: false,
            has_takedown: false,
            takedown_reasons: vec![],
            media: Default::default(),
            is_community_tweet: false,
            edit_control: None,
            exclusive_conversation_author_id: Some(30),
        }
    }

    pub(crate) fn filter_tweets_with_clients(
        tes: Arc<dyn TESClient + Send + Sync>,
        gizmoduck: Arc<dyn GizmoduckClient + Send + Sync>,
    ) -> FilterTweets {
        FilterTweets::new(
            Arc::new(ProdSources::new(
                tes,
                Arc::new(MockTweetForVisibilitySource::default()),
                gizmoduck,
                Arc::new(NoEdges),
                Arc::new(NoCountryRows),
                safety_labels(),
                None,
                None,
            )),
            RuleEngine::for_tests(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clients::socialgraph_client::{EdgeQuery, Graph};
    use crate::filter::test_support::filter_tweets;
    use crate::hydration::plan::Source;
    use crate::hydration::sources::{Fault, InMemorySources};
    use crate::models::LimitedEngagementReason;
    use crate::rules::fixtures::{allow, limited};
    use xai_core_entities::entities::PureCoreData;

    fn candidate(tweet_id: u64, author_id: Option<u64>) -> RawCandidate {
        RawCandidate {
            tweet_id: TweetId(tweet_id),
            request_author_id: author_id,
        }
    }

    fn service(sources: &Arc<InMemorySources>) -> FilterTweets {
        FilterTweets::new(sources.clone(), RuleEngine::for_tests())
    }

    #[tokio::test(start_paused = true)]
    async fn pure_core_timeout_fails_every_candidate_at_hydration_timeout() {
        let sources = Arc::new(InMemorySources::default().fault(Source::TesPureCore, Fault::Hangs));
        let started = tokio::time::Instant::now();
        let response = tokio::time::timeout(
            crate::hydration::HYDRATION_TIMEOUT * 2,
            service(&sources).run(FilterRequest {
                viewer_id: Some(50),
                country_code: None,
                safety_level: SafetyLevel::TimelineHome,
                candidates: vec![candidate(1, None), candidate(2, Some(20))],
                rpc: Rpc::FilterTweets,
            }),
        )
        .await
        .unwrap();
        assert_eq!(started.elapsed(), crate::hydration::HYDRATION_TIMEOUT);
        assert_eq!(
            response
                .outcomes
                .iter()
                .map(|outcome| (outcome.verdict.clone(), outcome.status))
                .collect::<Vec<_>>(),
            vec![
                (
                    Verdict::unresolved_author(),
                    EvaluationStatus::UnresolvedAuthor
                ),
                (allow(), EvaluationStatus::Failed),
            ]
        );
    }

    #[tokio::test]
    async fn home_hydration_limits_posts_whose_author_or_direct_reply_root_blocks_the_viewer() {
        let reply = |author_id, in_reply_to_tweet_id, in_reply_to_user_id| PureCoreData {
            author_id,
            conversation_id: Some(100),
            in_reply_to_tweet_id: Some(in_reply_to_tweet_id),
            in_reply_to_user_id: Some(in_reply_to_user_id),
            ..Default::default()
        };
        let sources = Arc::new(
            InMemorySources::default()
                .pure_core(1, reply(10, 100, 30))
                .pure_core(2, reply(20, 101, 40))
                .tweet(3, 10)
                .edge(Graph::Blocks, 20, 50)
                .edge(Graph::Blocks, 30, 50),
        );
        let service = &service(&sources);
        let verdicts = |viewer_id| async move {
            service
                .run(FilterRequest {
                    viewer_id,
                    country_code: None,
                    safety_level: SafetyLevel::TimelineHomeHydration,
                    candidates: vec![candidate(1, None), candidate(2, None), candidate(3, None)],
                    rpc: Rpc::FilterTweets,
                })
                .await
                .outcomes
                .into_iter()
                .map(|outcome| (outcome.status, outcome.verdict))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            verdicts(Some(50)).await,
            vec![
                (
                    EvaluationStatus::Evaluated,
                    limited(
                        LimitedEngagementReason::RootAuthorBlockedViewer,
                        "RootAuthorBlocksViewerLimitedActionsRule",
                    ),
                ),
                (
                    EvaluationStatus::Evaluated,
                    limited(
                        LimitedEngagementReason::BlockedViewer,
                        "BlockedViewerLimitedActionsRule",
                    ),
                ),
                (EvaluationStatus::Evaluated, allow()),
            ]
        );
        assert_eq!(
            verdicts(None).await,
            vec![(EvaluationStatus::Evaluated, allow()); 3]
        );
        assert_eq!(
            sources.selects(),
            [vec![EdgeQuery::reverse(Graph::Blocks, vec![10, 20, 30])]]
        );
    }
    #[tokio::test]
    async fn run_preserves_order_duplicates_unresolved_authors_and_labels() {
        let response = filter_tweets()
            .run(FilterRequest {
                viewer_id: None,
                country_code: None,
                safety_level: SafetyLevel::TimelineHome,
                candidates: vec![
                    candidate(2, Some(20)),
                    candidate(1, None),
                    candidate(2, Some(20)),
                ],
                rpc: Rpc::FilterTweets,
            })
            .await;

        assert_eq!(
            response
                .outcomes
                .iter()
                .map(|outcome| outcome.tweet_id)
                .collect::<Vec<_>>(),
            vec![TweetId(2), TweetId(1), TweetId(2)]
        );
        assert_eq!(response.outcomes[0].verdict, allow());
        assert_eq!(response.outcomes[1].verdict, Verdict::unresolved_author());
        assert_eq!(
            response
                .outcomes
                .iter()
                .map(|outcome| outcome.status)
                .collect::<Vec<_>>(),
            vec![
                EvaluationStatus::Evaluated,
                EvaluationStatus::UnresolvedAuthor,
                EvaluationStatus::Evaluated
            ]
        );
        assert_eq!(response.outcomes[2].verdict, allow());
        assert!(response
            .outcomes
            .iter()
            .all(|outcome| outcome.safety_labels.is_some()));
        assert!(!response.outcomes[0]
            .safety_labels
            .as_ref()
            .unwrap()
            .labels
            .is_empty());
        assert_eq!(
            response.outcomes[0].safety_labels,
            response.outcomes[2].safety_labels
        );
    }
}
