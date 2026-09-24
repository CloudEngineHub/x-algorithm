use crate::filter::{EvaluationStatus, FilterOutcome, FilterRequest, FilterTweets};
use crate::filter_tweets::normalize_viewer_id;
use crate::models::{RawCandidate, TweetId, Verdict};
use crate::rules::metrics::{self as ft_metrics, RequestMetricsGuard, Rpc};
use crate::rules::SafetyLevel;
use crate::treatment;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tonic::{Request, Response, Status};
use vf_pb::tweet_evaluation::Outcome;
use xai_visibility_filtering_proto as vf_pb;
use xai_x_thrift::safety_level::SafetyLevel as ThriftLevel;

const REQUESTS: &str = "evaluate_tweets_requests";
const LATENCY_MS: &str = "evaluate_tweets_latency_ms";
const BATCH_SIZE: &str = "evaluate_tweets_batch_size";
const RETWEET_SOURCES: &str = "evaluate_tweets_retweet_sources";

pub struct EvaluateTweetsEndpoint {
    filter_tweets: Arc<FilterTweets>,
}

impl EvaluateTweetsEndpoint {
    pub(crate) fn new(filter_tweets: Arc<FilterTweets>) -> Self {
        Self { filter_tweets }
    }

    pub async fn handle(
        &self,
        request: Request<vf_pb::EvaluateTweetsRequest>,
    ) -> Result<Response<vf_pb::EvaluateTweetsResponse>, Status> {
        let entered = tokio::time::Instant::now();
        let request_metrics = RequestMetricsGuard::named(REQUESTS, LATENCY_MS);
        let context = crate::hydration::request_context(
            entered,
            crate::filter_tweets::parse_grpc_timeout(request.metadata()),
        );
        match context.scope(self.handle_inner(request.into_inner())).await {
            Ok(response) => {
                request_metrics.mark_success();
                Ok(Response::new(response))
            }
            Err(status) => {
                request_metrics.mark_failure();
                Err(status)
            }
        }
    }

    async fn handle_inner(
        &self,
        req: vf_pb::EvaluateTweetsRequest,
    ) -> Result<vf_pb::EvaluateTweetsResponse, Status> {
        let level = ThriftLevel(req.safety_level);
        if !ThriftLevel::ENUM_VALUES.contains(&level) {
            return Err(Status::invalid_argument("unknown safety level"));
        }
        let safety_level = match level {
            ThriftLevel::FILTER_ALL => SafetyLevel::FilterAll,
            ThriftLevel::TIMELINE_HOME => SafetyLevel::TimelineHome,
            ThriftLevel::TIMELINE_HOME_RECOMMENDATIONS => SafetyLevel::TimelineHomeRecommendations,
            ThriftLevel::TIMELINE_HOME_HYDRATION => SafetyLevel::TimelineHomeHydration,
            _ => return Err(Status::unimplemented("safety level has no Rust policy")),
        };
        ft_metrics::record_batch_size(BATCH_SIZE, req.tweets.len());
        let candidates = req
            .tweets
            .iter()
            .filter(|o| o.quote_context.is_none())
            .map(|o| RawCandidate {
                tweet_id: TweetId(o.tweet_id),
                request_author_id: None,
            })
            .collect();
        let viewer_id = normalize_viewer_id(req.viewer_id);
        let outcomes = self
            .filter_tweets
            .run(FilterRequest {
                viewer_id,
                country_code: req.country_code.clone(),
                safety_level,
                candidates,
                rpc: Rpc::EvaluateTweets,
            })
            .await
            .outcomes;
        let is_evaluated_retweet = |outcome: &FilterOutcome| {
            outcome.status == EvaluationStatus::Evaluated && outcome.source_tweet_id.is_some()
        };
        let outcomes = if !outcomes.iter().any(is_evaluated_retweet) {
            outcomes
        } else {
            let requested: HashSet<TweetId> =
                outcomes.iter().map(|outcome| outcome.tweet_id).collect();
            let (in_batch, fetched): (HashSet<TweetId>, HashSet<TweetId>) = outcomes
                .iter()
                .filter(|outcome| outcome.status == EvaluationStatus::Evaluated)
                .filter_map(|outcome| outcome.source_tweet_id)
                .partition(|source_id| requested.contains(source_id));
            ft_metrics::incr_nonzero(
                RETWEET_SOURCES,
                &[("outcome", "in_batch")],
                in_batch.len() as u64,
            );
            ft_metrics::incr_nonzero(
                RETWEET_SOURCES,
                &[("outcome", "fetched")],
                fetched.len() as u64,
            );
            let fetched_outcomes = if fetched.is_empty() {
                Vec::new()
            } else {
                self.filter_tweets
                    .run(FilterRequest {
                        viewer_id,
                        country_code: req.country_code,
                        safety_level,
                        candidates: fetched
                            .into_iter()
                            .map(|tweet_id| RawCandidate {
                                tweet_id,
                                request_author_id: None,
                            })
                            .collect(),
                        rpc: Rpc::EvaluateTweets,
                    })
                    .await
                    .outcomes
            };
            let sources: HashMap<TweetId, (EvaluationStatus, Verdict)> = outcomes
                .iter()
                .filter(|outcome| in_batch.contains(&outcome.tweet_id))
                .map(|outcome| (outcome.tweet_id, (outcome.status, outcome.verdict.clone())))
                .chain(
                    fetched_outcomes
                        .into_iter()
                        .map(|outcome| (outcome.tweet_id, (outcome.status, outcome.verdict))),
                )
                .collect();
            outcomes
                .into_iter()
                .map(|mut outcome| {
                    if is_evaluated_retweet(&outcome) {
                        match outcome.source_tweet_id.and_then(|id| sources.get(&id)) {
                            Some((EvaluationStatus::Evaluated, source)) => {
                                outcome.verdict =
                                    Verdict::merge_retweet_verdict(outcome.verdict, source);
                            }
                            _ => outcome.status = EvaluationStatus::Failed,
                        }
                    }
                    outcome
                })
                .collect()
        };
        ft_metrics::record_verdicts(
            Rpc::EvaluateTweets,
            safety_level,
            outcomes.iter().map(|outcome| &outcome.verdict),
        );
        let outcomes: HashMap<TweetId, FilterOutcome> = outcomes
            .into_iter()
            .map(|outcome| (outcome.tweet_id, outcome))
            .collect();
        let results = req
            .tweets
            .into_iter()
            .map(|tweet| {
                let outcome = if tweet.quote_context.is_some() {
                    Outcome::NotEvaluated(vf_pb::NotEvaluated {})
                } else {
                    match outcomes.get(&TweetId(tweet.tweet_id)) {
                        Some(FilterOutcome {
                            status: EvaluationStatus::Evaluated,
                            verdict,
                            ..
                        }) => match treatment::thrift_action(verdict, safety_level) {
                            Some(action) => match xai_x_thrift::serialize_compact(&action) {
                                Ok(bytes) => Outcome::ActionThriftCompact(bytes.into()),
                                Err(_) => Outcome::Failed(vf_pb::Failed {}),
                            },
                            None => Outcome::NotEvaluated(vf_pb::NotEvaluated {}),
                        },
                        _ => Outcome::Failed(vf_pb::Failed {}),
                    }
                };
                vf_pb::TweetEvaluation {
                    tweet: Some(tweet),
                    outcome: Some(outcome),
                }
            })
            .collect();
        Ok(vf_pb::EvaluateTweetsResponse { results })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xai_core_entities::entities::{
        GizmoduckUser, GizmoduckUserResult, PureCoreData, Safety, UserResponseState,
    };
    use xai_core_entities::gizmoduck_client::MockGizmoduckClient;
    use xai_core_entities::tweet_entity_service_client::MockTESClient;
    use xai_x_thrift::action::{self, Action, DropReason};

    #[tokio::test]
    async fn evaluate_tweets_gates_levels_maps_outcomes_and_merges_retweet_sources() {
        let core = |author_id, source_tweet_id| {
            Some(PureCoreData {
                author_id,
                source_tweet_id,
                ..Default::default()
            })
        };
        let tes = Arc::new(MockTESClient {
            core_data: [
                (3, core(30, None)),
                (4, core(40, Some(6))),
                (5, core(50, Some(6))),
                (6, core(60, None)),
                (7, core(70, Some(8))),
            ]
            .into(),
            ..Default::default()
        });
        let gizmoduck = MockGizmoduckClient {
            users: [(
                60,
                Some(GizmoduckUserResult {
                    user: Some(GizmoduckUser {
                        safety: Safety {
                            suspended: true,
                            ..Default::default()
                        },
                        ..Default::default()
                    }),
                    response_state: Some(UserResponseState::Found),
                }),
            )]
            .into(),
            ..Default::default()
        };
        let endpoint = EvaluateTweetsEndpoint::new(Arc::new(
            crate::filter::test_support::filter_tweets_with_clients(
                tes.clone(),
                Arc::new(gizmoduck),
            ),
        ));
        for (level, code) in [
            (0, tonic::Code::Unimplemented),
            (4, tonic::Code::Unimplemented),
            (9999, tonic::Code::InvalidArgument),
        ] {
            let error = endpoint
                .handle(Request::new(vf_pb::EvaluateTweetsRequest {
                    safety_level: level,
                    ..Default::default()
                }))
                .await
                .unwrap_err();
            assert_eq!(error.code(), code);
        }
        let tweet = |tweet_id, outer_tweet_id: Option<u64>| vf_pb::TweetData {
            tweet_id,
            quote_context: outer_tweet_id.map(|outer_tweet_id| vf_pb::QuoteContext {
                outer_tweet_id,
                outer_author_id: None,
            }),
        };
        let tweets = vec![
            tweet(1, None),
            tweet(1, Some(2)),
            tweet(3, None),
            tweet(3, None),
        ];
        for (level, action) in [
            (
                16,
                Action::Drop(action::Drop::new(Some(DropReason::Unspecified(true)), None)),
            ),
            (82, Action::Allow(action::Allow::new())),
        ] {
            let response = endpoint
                .handle(Request::new(vf_pb::EvaluateTweetsRequest {
                    safety_level: level,
                    tweets: tweets.clone(),
                    ..Default::default()
                }))
                .await
                .unwrap()
                .into_inner();
            assert_eq!(
                response
                    .results
                    .iter()
                    .map(|r| r.tweet.unwrap())
                    .collect::<Vec<_>>(),
                tweets
            );
            let bytes = xai_x_thrift::serialize_compact(&action).unwrap();
            assert_eq!(
                response
                    .results
                    .into_iter()
                    .map(|r| r.outcome.unwrap())
                    .collect::<Vec<_>>(),
                vec![
                    Outcome::Failed(vf_pb::Failed {}),
                    Outcome::NotEvaluated(vf_pb::NotEvaluated {}),
                    Outcome::ActionThriftCompact(bytes.clone().into()),
                    Outcome::ActionThriftCompact(bytes.into()),
                ]
            );
        }
        let suspended = Outcome::ActionThriftCompact(
            xai_x_thrift::serialize_compact(&Action::Drop(action::Drop::new(
                Some(DropReason::SuspendedAuthor(true)),
                None,
            )))
            .unwrap()
            .into(),
        );
        for (tweet_ids, core_data_calls, outcomes) in [
            (vec![4, 6], 1, vec![suspended.clone(), suspended.clone()]),
            (
                vec![5, 7],
                2,
                vec![suspended, Outcome::Failed(vf_pb::Failed {})],
            ),
        ] {
            let calls_before = tes.call_count();
            let response = endpoint
                .handle(Request::new(vf_pb::EvaluateTweetsRequest {
                    safety_level: 8,
                    tweets: tweet_ids.into_iter().map(|id| tweet(id, None)).collect(),
                    ..Default::default()
                }))
                .await
                .unwrap()
                .into_inner();
            assert_eq!(tes.call_count() - calls_before, core_data_calls);
            assert_eq!(
                response
                    .results
                    .into_iter()
                    .map(|r| r.outcome.unwrap())
                    .collect::<Vec<_>>(),
                outcomes
            );
        }
    }
}
