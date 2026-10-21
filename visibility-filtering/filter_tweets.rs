use crate::caller_identity::Endpoint;
use crate::metrics::{self as ft_metrics, Rpc};
use crate::models::{ClientCapability, PlacedCandidate, RawCandidate, TweetId};
use crate::params::ClientSwitches;
use crate::post_evaluator::{normalize_viewer_id, PostBatch, PostEvaluation, PostEvaluator};
use crate::rules::SafetyLevel;
use crate::treatment;
use std::sync::Arc;
use std::time::Duration;
use tonic::{Request, Response, Status};
use xai_visibility_filtering_proto as vf_pb;

fn impresses_soft_nsfw_new_user(level: SafetyLevel) -> bool {
    match level {
        SafetyLevel::TimelineHomeRecommendations => true,
        SafetyLevel::FilterAll
        | SafetyLevel::TimelineHome
        | SafetyLevel::TimelineHomeHydration
        | SafetyLevel::ImmersiveExpandedRecommendations
        | SafetyLevel::QuotedTweetRules => false,
    }
}

pub(crate) fn filter_tweets_request(
    client_switches: &ClientSwitches,
    viewer_id: Option<u64>,
    country_code: Option<String>,
    safety_level: SafetyLevel,
    candidates: Vec<PlacedCandidate>,
) -> PostBatch {
    let country_code = country_code.filter(|code| !code.is_empty());
    let client_capability = ClientCapability {
        soft_nsfw_new_user_drop: impresses_soft_nsfw_new_user(safety_level)
            && client_switches.soft_nsfw_new_user_drop(viewer_id, country_code.as_deref()),
        ..ClientCapability::default()
    };
    PostBatch {
        viewer_id,
        country_code,
        client_capability,
        safety_level,
        candidates,
        rpc: Rpc::FilterTweets,
    }
}

pub(crate) type FinishComparison = Box<dyn FnOnce(&[PostEvaluation]) + Send>;

pub(crate) trait Comparator: Send + Sync {
    fn begin(
        &self,
        viewer_id: Option<u64>,
        country_code: Option<String>,
        safety_level: SafetyLevel,
        tweet_ids: Vec<u64>,
    ) -> Option<FinishComparison>;
}

pub struct FilterTweetsEndpoint {
    post_evaluator: Arc<PostEvaluator>,
    client_switches: ClientSwitches,
    comparator: Option<Box<dyn Comparator>>,
}

impl FilterTweetsEndpoint {
    pub(crate) fn new(
        post_evaluator: Arc<PostEvaluator>,
        client_switches: ClientSwitches,
        comparator: Option<Box<dyn Comparator>>,
    ) -> Self {
        Self {
            post_evaluator,
            client_switches,
            comparator,
        }
    }

    pub async fn handle(
        &self,
        request: Request<vf_pb::VisibilityFilterRequest>,
    ) -> Result<Response<vf_pb::VisibilityFilterResponse>, Status> {
        let entered = tokio::time::Instant::now();
        ft_metrics::observe_request(Endpoint::FilterTweets, request, |request, grpc_timeout| {
            self.handle_inner(request, entered, grpc_timeout)
        })
        .await
    }

    async fn handle_inner(
        &self,
        request: Request<vf_pb::VisibilityFilterRequest>,
        entered: tokio::time::Instant,
        grpc_timeout: Option<Duration>,
    ) -> Result<Response<vf_pb::VisibilityFilterResponse>, Status> {
        let context = crate::hydration::request_context(entered, grpc_timeout);
        let req = request.into_inner();
        ft_metrics::record_batch_size(ft_metrics::BATCH_SIZE, req.tweets.len());
        let viewer_id = normalize_viewer_id(req.viewer_id);
        ft_metrics::record_viewer_state(req.viewer_id, viewer_id);

        let Some(safety_level) = SafetyLevel::from_proto(req.safety_level()) else {
            return Err(Status::unimplemented("safety level has no Rust policy"));
        };
        let candidates: Vec<PlacedCandidate> = req
            .tweets
            .iter()
            .map(|t| {
                PlacedCandidate::standalone(RawCandidate {
                    tweet_id: TweetId(t.tweet_id),
                    request_author_id: t.author_id,
                })
            })
            .collect();

        let finish_comparison = self.comparator.as_ref().and_then(|comparator| {
            comparator.begin(
                viewer_id,
                req.country_code.clone(),
                safety_level,
                req.tweets.iter().map(|t| t.tweet_id).collect(),
            )
        });

        let request = filter_tweets_request(
            &self.client_switches,
            viewer_id,
            req.country_code,
            safety_level,
            candidates,
        );
        let outcomes = context.scope(self.post_evaluator.run(request)).await;
        ft_metrics::record_verdicts(
            Rpc::FilterTweets,
            safety_level,
            outcomes.iter().map(|outcome| outcome.evaluation.verdict()),
        );
        ft_metrics::record_unresolved(
            Rpc::FilterTweets,
            safety_level,
            outcomes.iter().map(|outcome| &outcome.evaluation),
        );
        ft_metrics::record_fail_open_defaults(
            Rpc::FilterTweets,
            safety_level,
            outcomes
                .iter()
                .map(|outcome| outcome.evaluation.fail_open_defaults()),
        );

        if let Some(finish_comparison) = finish_comparison {
            finish_comparison(&outcomes);
        }

        let results = outcomes.into_iter().map(to_visibility_result).collect();

        Ok(Response::new(vf_pb::VisibilityFilterResponse { results }))
    }
}

fn to_visibility_result(outcome: PostEvaluation) -> vf_pb::TweetVisibilityResult {
    let (action, filtered_reason) = treatment::proto_action(outcome.evaluation.into_verdict());
    vf_pb::TweetVisibilityResult {
        tweet_id: outcome.tweet_id.0,
        action: Some(action),
        filtered_reason,
        safety_labels: outcome.safety_labels.map(Arc::unwrap_or_clone),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hydration::inventory::Source;
    use crate::hydration::sources::InMemorySources;
    use crate::rules::RuleEngine;
    use std::sync::Mutex;
    use strum::VariantArray;

    async fn gizmoduck_calls(viewer_id: Option<u64>) -> usize {
        let sources = Arc::new(InMemorySources::default());
        let endpoint = FilterTweetsEndpoint::new(
            Arc::new(PostEvaluator::new(
                Arc::<InMemorySources>::clone(&sources),
                RuleEngine::for_tests(),
                Arc::default(),
            )),
            ClientSwitches::for_tests(),
            None,
        );
        let response = endpoint
            .handle(Request::new(vf_pb::VisibilityFilterRequest {
                safety_level: vf_pb::SafetyLevel::TimelineHome.into(),
                tweets: vec![vf_pb::TweetInput {
                    tweet_id: 2,
                    author_id: Some(20),
                }],
                viewer_id,
                country_code: None,
            }))
            .await
            .unwrap();
        assert_eq!(response.into_inner().results.len(), 1);
        sources
            .calls()
            .into_iter()
            .filter(|source| matches!(source, Source::GizmoduckViewer | Source::GizmoduckAuthor))
            .count()
    }

    #[test]
    fn only_levels_that_wire_soft_nsfw_new_user_resolve_its_switch() {
        let rule_engine = RuleEngine::for_tests();
        for &level in SafetyLevel::VARIANTS {
            let wired = rule_engine
                .wired_rule_names(level)
                .iter()
                .any(|name| name.starts_with("soft_nsfw_new_user/"));
            assert_eq!(impresses_soft_nsfw_new_user(level), wired, "{level:?}");
        }
    }

    #[tokio::test]
    async fn endpoint_treats_zero_viewer_id_as_logged_out() {
        let logged_out = gizmoduck_calls(None).await;
        assert_eq!(gizmoduck_calls(Some(0)).await, logged_out);
        assert_eq!(gizmoduck_calls(Some(u64::MAX)).await, logged_out);
        assert_eq!(gizmoduck_calls(Some(42)).await, logged_out + 1);
    }

    #[derive(Default)]
    struct ViewerIds(Arc<Mutex<Vec<Option<u64>>>>);

    impl Comparator for ViewerIds {
        fn begin(
            &self,
            viewer_id: Option<u64>,
            _country_code: Option<String>,
            _safety_level: SafetyLevel,
            _tweet_ids: Vec<u64>,
        ) -> Option<FinishComparison> {
            self.0.lock().unwrap().push(viewer_id);
            None
        }
    }

    #[tokio::test]
    async fn comparator_sees_the_normalized_viewer_id() {
        let viewer_ids = ViewerIds::default();
        let seen = Arc::clone(&viewer_ids.0);
        let endpoint = FilterTweetsEndpoint::new(
            Arc::new(PostEvaluator::new(
                Arc::new(InMemorySources::default()),
                RuleEngine::for_tests(),
                Arc::default(),
            )),
            ClientSwitches::for_tests(),
            Some(Box::new(viewer_ids)),
        );
        for viewer_id in [Some(0), Some(42)] {
            endpoint
                .handle(Request::new(vf_pb::VisibilityFilterRequest {
                    safety_level: vf_pb::SafetyLevel::TimelineHome.into(),
                    tweets: vec![vf_pb::TweetInput {
                        tweet_id: 2,
                        author_id: Some(20),
                    }],
                    viewer_id,
                    country_code: None,
                }))
                .await
                .unwrap();
        }
        assert_eq!(*seen.lock().unwrap(), vec![None, Some(42)]);
    }

    #[derive(Default)]
    struct FinishedResults(Arc<Mutex<Vec<Vec<vf_pb::TweetVisibilityResult>>>>);

    impl Comparator for FinishedResults {
        fn begin(
            &self,
            _viewer_id: Option<u64>,
            _country_code: Option<String>,
            _safety_level: SafetyLevel,
            _tweet_ids: Vec<u64>,
        ) -> Option<FinishComparison> {
            let finished = Arc::clone(&self.0);
            Some(Box::new(move |outcomes: &[PostEvaluation]| {
                finished
                    .lock()
                    .unwrap()
                    .push(outcomes.iter().cloned().map(to_visibility_result).collect());
            }))
        }
    }

    #[tokio::test]
    async fn comparator_finishes_with_the_served_outcomes() {
        let comparator = FinishedResults::default();
        let finished = Arc::clone(&comparator.0);
        let endpoint = FilterTweetsEndpoint::new(
            Arc::new(PostEvaluator::new(
                Arc::new(InMemorySources::default()),
                RuleEngine::for_tests(),
                Arc::default(),
            )),
            ClientSwitches::for_tests(),
            Some(Box::new(comparator)),
        );
        let served = endpoint
            .handle(Request::new(vf_pb::VisibilityFilterRequest {
                safety_level: vf_pb::SafetyLevel::TimelineHome.into(),
                tweets: [(2, 20), (3, 30)]
                    .map(|(tweet_id, author_id)| vf_pb::TweetInput {
                        tweet_id,
                        author_id: Some(author_id),
                    })
                    .into(),
                viewer_id: Some(42),
                country_code: None,
            }))
            .await
            .unwrap()
            .into_inner()
            .results;
        assert_eq!(*finished.lock().unwrap(), vec![served]);
    }
}
