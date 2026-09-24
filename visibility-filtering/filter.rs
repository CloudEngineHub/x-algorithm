use crate::hydration::{HydrationOutput, HydrationPipeline, HydrationRequest};
use crate::models::{RawCandidate, TweetId, Verdict};
use crate::rules::metrics::{self as ft_metrics, Rpc};
use crate::rules::{RuleEngine, SafetyLevel};
use std::collections::HashMap;
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
    hydration_pipeline: HydrationPipeline,
    rule_engine: RuleEngine,
}

impl FilterTweets {
    pub(crate) fn new(hydration_pipeline: HydrationPipeline, rule_engine: RuleEngine) -> Self {
        Self {
            hydration_pipeline,
            rule_engine,
        }
    }

    pub async fn run(&self, request: FilterRequest) -> FilterResponse {
        let started = Instant::now();
        let hydration = self
            .hydration_pipeline
            .hydrate(HydrationRequest::new(
                request.viewer_id,
                request.country_code,
                &request.candidates,
                request.safety_level,
            ))
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
    use crate::clients::socialgraph_client::FakeSocialgraphClient;
    use crate::hydration::tes_composite::{MockTweetForVisibilitySource, TweetForVisibility};
    use crate::safety_label_source::lookup::{ManhattanLookup, RemoteSource, TwemcacheLookup};
    use crate::safety_label_source::types::{ManhattanOutcome, TwemcacheOutcome};
    use crate::safety_label_source::SafetyLabelSource;
    use std::sync::Arc;
    use tonic::async_trait;
    use xai_core_entities::entities::{
        ApiCounts, CashtagAttachments, ConversationControl, EditControl,
        EscherbirdEntityAnnotation, ExclusiveTweetControl, MediaEntities, PureCoreData,
        QuotedTweet, ReactionContext, TakedownReason, TrustedFriendsControl, UrlEntities,
    };
    use xai_core_entities::gizmoduck_client::{GizmoduckClient, MockGizmoduckClient};
    use xai_core_entities::tweet_entity_service_client::{MockTESClient, TESClient};
    use xai_x_thrift::entities::ApiMediaEntity;
    use xai_x_thrift::tweets::ApiPerspective;

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
        let socialgraph = Arc::new(FakeSocialgraphClient);
        FilterTweets::new(
            HydrationPipeline::new(
                tes,
                Arc::new(MockTweetForVisibilitySource::default()),
                gizmoduck,
                socialgraph,
                safety_labels(),
                None,
                None,
            ),
            RuleEngine::for_tests(),
        )
    }

    pub(crate) struct PendingTes;

    macro_rules! pending_tes {
        ($($method:ident -> $value:ty),* $(,)?) => {
            #[async_trait]
            impl TESClient for PendingTes {
                $(
                    async fn $method(
                        &self,
                        _: Vec<u64>,
                    ) -> HashMap<u64, anyhow::Result<Option<$value>>> {
                        std::future::pending().await
                    }
                )*

                async fn get_core_data_and_api_counts(
                    &self,
                    _: u64,
                ) -> (
                    anyhow::Result<Option<PureCoreData>>,
                    anyhow::Result<Option<ApiCounts>>,
                ) {
                    std::future::pending().await
                }

                async fn get_status_perspectives(
                    &self,
                    _: Vec<u64>,
                    _: Option<&tonic::metadata::MetadataMap>,
                ) -> HashMap<u64, anyhow::Result<Option<ApiPerspective>>> {
                    std::future::pending().await
                }

                async fn get_api_media_entities(
                    &self,
                    _: Vec<u64>,
                    _: Option<&tonic::metadata::MetadataMap>,
                ) -> HashMap<u64, anyhow::Result<Option<Vec<ApiMediaEntity>>>> {
                    std::future::pending().await
                }
            }
        };
    }

    pending_tes! {
        get_tweet_core_datas -> PureCoreData,
        get_tweet_media_entities -> MediaEntities,
        get_subscription_author_ids -> u64,
        get_conversation_controls -> ConversationControl,
        get_quoted_tweets -> QuotedTweet,
        get_reaction_context -> ReactionContext,
        get_min_video_durations -> i64,
        get_media_count -> i64,
        get_nullcast -> bool,
        get_community -> i64,
        get_nsfw_user -> bool,
        get_nsfw_admin -> bool,
        get_has_takedown -> bool,
        get_takedown_country_codes -> Vec<String>,
        get_takedown_reasons -> Vec<TakedownReason>,
        get_language_code -> String,
        get_api_counts -> ApiCounts,
        get_cashtag_attachments -> CashtagAttachments,
        get_is_article -> bool,
        get_is_premium -> bool,
        get_urls -> UrlEntities,
        get_exclusive_controls -> ExclusiveTweetControl,
        get_trusted_friends_controls -> TrustedFriendsControl,
        get_grok_post_ids -> String,
        get_edit_control -> EditControl,
        get_escherbird_entity_annotations -> Vec<EscherbirdEntityAnnotation>,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clients::socialgraph_client::SocialgraphClient;
    use crate::filter::test_support::{exclusive_tweet, filter_tweets, PendingTes};
    use crate::hydration::tes_composite::{
        MockTweetForVisibilitySource, TweetForVisibility, TweetForVisibilitySource,
    };
    use crate::models::{
        ExclusiveContentFeatures, TweetFeatures, Viewer, ViewerAuthorRelationship, ViewerProfile,
    };
    use std::sync::{Arc, Mutex};
    use tonic::async_trait;
    use xai_core_entities::entities::{GizmoduckUser, GizmoduckUserResult, PureCoreData, Safety};
    use xai_core_entities::gizmoduck_client::MockGizmoduckClient;
    use xai_core_entities::tweet_entity_service_client::MockTESClient;

    #[derive(Default)]
    struct RecordingSocialgraph {
        relationships: Mutex<Vec<Vec<u64>>>,
        super_follows: Mutex<Vec<Vec<u64>>>,
    }

    #[async_trait]
    impl SocialgraphClient for RecordingSocialgraph {
        async fn batch_check_relationships(
            &self,
            _: u64,
            authors: &[u64],
        ) -> HashMap<u64, ViewerAuthorRelationship> {
            self.relationships.lock().unwrap().push(authors.to_vec());
            authors
                .iter()
                .map(|&id| {
                    (
                        id,
                        ViewerAuthorRelationship {
                            viewer_follows_author: true,
                            ..Default::default()
                        },
                    )
                })
                .collect()
        }

        async fn batch_check_super_follows(
            &self,
            _: u64,
            authors: &[u64],
        ) -> Option<HashMap<u64, bool>> {
            self.super_follows.lock().unwrap().push(authors.to_vec());
            Some(authors.iter().map(|&id| (id, true)).collect())
        }

        async fn batch_check_followed_by(&self, _: u64, _: &[u64]) -> Option<HashMap<u64, bool>> {
            unreachable!("filter hydration never checks followed-by edges")
        }
    }

    struct PendingComposite;

    #[async_trait]
    impl TweetForVisibilitySource for PendingComposite {
        async fn get_tweets_for_visibility(
            &self,
            _: &[u64],
        ) -> HashMap<u64, anyhow::Result<Option<TweetForVisibility>>> {
            std::future::pending().await
        }
    }

    fn unrestricted() -> Verdict {
        Verdict::Shown {
            media: None,
            engagement: None,
        }
    }

    fn core_client() -> Arc<MockTESClient> {
        Arc::new(MockTESClient {
            core_data: HashMap::from([
                (
                    1,
                    Some(PureCoreData {
                        author_id: 10,
                        ..Default::default()
                    }),
                ),
                (
                    2,
                    Some(PureCoreData {
                        author_id: 20,
                        ..Default::default()
                    }),
                ),
            ]),
            ..Default::default()
        })
    }

    fn candidate(tweet_id: u64, author_id: Option<u64>) -> RawCandidate {
        RawCandidate {
            tweet_id: TweetId(tweet_id),
            request_author_id: author_id,
        }
    }

    #[tokio::test]
    async fn run_uses_only_core_and_composite_tes_calls() {
        let tes = core_client();
        let composite = Arc::new(MockTweetForVisibilitySource {
            tweets: HashMap::from([(1, Some(exclusive_tweet()))]),
            ..Default::default()
        });
        let sg = Arc::new(RecordingSocialgraph::default());
        let service = FilterTweets::new(
            HydrationPipeline::new(
                tes.clone(),
                composite.clone(),
                Arc::new(MockGizmoduckClient::default()),
                sg.clone(),
                test_support::safety_labels(),
                None,
                None,
            ),
            RuleEngine::for_tests(),
        );
        let result = service
            .run(FilterRequest {
                viewer_id: Some(50),
                country_code: None,
                safety_level: SafetyLevel::TimelineHome,
                candidates: vec![candidate(1, None), candidate(1, None)],
                rpc: Rpc::FilterTweets,
            })
            .await;
        assert_eq!(tes.call_count(), 1);
        assert_eq!(*composite.requests.lock().unwrap(), vec![vec![1]]);
        assert_eq!(*sg.relationships.lock().unwrap(), vec![vec![10]]);
        assert_eq!(*sg.super_follows.lock().unwrap(), vec![vec![30]]);
        assert_eq!(result.outcomes.len(), 2);
        assert!(result
            .outcomes
            .iter()
            .all(|outcome| outcome.verdict == unrestricted()));
    }

    #[tokio::test(start_paused = true)]
    async fn composite_timeout_preserves_pure_core_author_and_relationship_features() {
        let tes = core_client();
        let gizmoduck = Arc::new(MockGizmoduckClient {
            users: HashMap::from([(
                10,
                Some(GizmoduckUserResult {
                    user: Some(GizmoduckUser {
                        safety: Safety {
                            suspended: true,
                            ..Default::default()
                        },
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            )]),
            ..Default::default()
        });
        let sg = Arc::new(RecordingSocialgraph::default());
        let pipeline = HydrationPipeline::new(
            tes.clone(),
            Arc::new(PendingComposite),
            gizmoduck,
            sg.clone(),
            test_support::safety_labels(),
            None,
            None,
        );
        let raw = [candidate(1, None)];
        let started = tokio::time::Instant::now();
        let hydration = tokio::time::timeout(
            crate::hydration::HYDRATION_TIMEOUT * 2,
            pipeline.hydrate(HydrationRequest::new(
                Some(50),
                None,
                &raw,
                SafetyLevel::TimelineHome,
            )),
        );
        tokio::pin!(hydration);
        assert!(futures::poll!(&mut hydration).is_pending());
        assert_eq!(*sg.relationships.lock().unwrap(), vec![vec![10]]);
        let result = hydration.await.unwrap();
        assert_eq!(started.elapsed(), crate::hydration::HYDRATION_TIMEOUT);
        let tweet = &result.candidates[0];
        assert_eq!(tweet.author_id, 10);
        assert_eq!(tweet.tweet_features, TweetFeatures::default());
        assert!(tweet.author_features.is_suspended);
        assert!(tweet.relationship.viewer_follows_author);
        assert_eq!(tweet.exclusive_content, None);
        assert_eq!(tes.call_count(), 1);
        assert!(sg.super_follows.lock().unwrap().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn pure_core_timeout_fails_every_candidate_at_hydration_timeout() {
        let service = test_support::filter_tweets_with_clients(
            Arc::new(PendingTes),
            Arc::new(MockGizmoduckClient::default()),
        );
        let started = tokio::time::Instant::now();
        let response = tokio::time::timeout(
            crate::hydration::HYDRATION_TIMEOUT * 2,
            service.run(FilterRequest {
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
                (unrestricted(), EvaluationStatus::Failed),
            ]
        );
    }

    #[tokio::test]
    async fn filter_all_hydrates_pure_core_only_and_keeps_the_request_side_viewer() {
        let sg = Arc::new(RecordingSocialgraph::default());
        let gizmoduck = Arc::new(MockGizmoduckClient::default());
        let pipeline = HydrationPipeline::new(
            core_client(),
            Arc::new(MockTweetForVisibilitySource {
                tweets: HashMap::from([(1, Some(exclusive_tweet()))]),
                ..Default::default()
            }),
            gizmoduck.clone(),
            sg.clone(),
            test_support::safety_labels(),
            None,
            None,
        );
        let raw = [candidate(1, None)];
        let result = pipeline
            .hydrate(HydrationRequest::new(
                Some(50),
                Some("US".into()),
                &raw,
                SafetyLevel::FilterAll,
            ))
            .await;
        assert_eq!(gizmoduck.call_count(), 0);
        assert!(sg.relationships.lock().unwrap().is_empty());
        assert!(sg.super_follows.lock().unwrap().is_empty());
        assert_eq!(
            result.viewer_features.viewer,
            Viewer::LoggedIn {
                id: 50,
                profile: ViewerProfile::default(),
            }
        );
        assert_eq!(result.viewer_features.country_code.as_deref(), Some("us"));
        let tweet = &result.candidates[0];
        assert_eq!(tweet.author_id, 10);
        assert_eq!(tweet.relationship, ViewerAuthorRelationship::default());
        assert_eq!(tweet.exclusive_content, None);
        assert!(result.safety_labels.is_empty());
        assert!(result.failed_ids.is_empty());
    }

    #[tokio::test]
    async fn exclusive_content_deduplicates_tweets_and_conversation_authors() {
        let composite = Arc::new(MockTweetForVisibilitySource {
            tweets: [1, 2]
                .into_iter()
                .map(|id| (id, Some(exclusive_tweet())))
                .collect(),
            ..Default::default()
        });
        for viewer_id in [Some(50), None] {
            let sg = Arc::new(RecordingSocialgraph::default());
            let pipeline = HydrationPipeline::new(
                core_client(),
                composite.clone(),
                Arc::new(MockGizmoduckClient::default()),
                sg.clone(),
                test_support::safety_labels(),
                None,
                None,
            );
            let raw = [
                candidate(1, None),
                candidate(2, None),
                candidate(1, None),
                candidate(3, Some(40)),
            ];
            let result = pipeline
                .hydrate(HydrationRequest::new(
                    viewer_id,
                    None,
                    &raw,
                    SafetyLevel::TimelineHome,
                ))
                .await;
            let expected = Some(ExclusiveContentFeatures {
                conversation_author_id: 30,
                viewer_super_follows_author: viewer_id.is_some(),
            });
            assert_eq!(
                result
                    .candidates
                    .iter()
                    .map(|c| c.exclusive_content.clone())
                    .collect::<Vec<_>>(),
                vec![expected.clone(), expected.clone(), expected, None]
            );
            let expected_calls = if viewer_id.is_some() {
                vec![vec![30]]
            } else {
                vec![]
            };
            assert_eq!(*sg.super_follows.lock().unwrap(), expected_calls);
        }
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
        assert_eq!(response.outcomes[0].verdict, unrestricted());
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
        assert_eq!(response.outcomes[2].verdict, unrestricted());
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
