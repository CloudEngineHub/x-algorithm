use super::{PostBatch, PostEvaluator};
use crate::hydration::fallback_cache::FallbackCaches;
use crate::hydration::{Cause, HydratedTweet, Hydrators, Lookup, Unresolved};
use crate::models::{Evaluation, EvaluationKey};
use std::collections::HashMap;
use tracing::warn;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct UserEvaluation {
    pub(crate) evaluation: Evaluation,
    pub(crate) failed: Hydrators,
}

impl PostEvaluator {
    pub(crate) async fn evaluate_authors(
        &self,
        mut request: PostBatch,
    ) -> HashMap<EvaluationKey, UserEvaluation> {
        let level = request.safety_level;
        let hydration = self
            .rule_engine
            .plan(level)
            .hydrate(
                &*self.sources,
                request.hydration_request(false, &FallbackCaches::default()),
            )
            .await;
        let admitted = self
            .rule_engine
            .admit_users(level, &hydration)
            .inspect_err(|error| {
                warn!(
                    ?error,
                    ?level,
                    "hydration not admitted by its level's policy; failing its authors"
                );
            })
            .ok();
        request
            .candidates
            .iter()
            .filter_map(|placed| {
                let evaluation = match (hydration.tweet(placed.tweet_id())?, &admitted) {
                    (
                        HydratedTweet::Unresolved {
                            reason:
                                Unresolved {
                                    lookup,
                                    cause: Cause::Failed,
                                },
                            ..
                        },
                        _,
                    ) => UserEvaluation {
                        evaluation: Evaluation::Failed(*lookup),
                        failed: Hydrators::empty(),
                    },
                    (HydratedTweet::Unresolved { .. }, _) => return None,
                    (HydratedTweet::Resolved { .. }, None) => UserEvaluation {
                        evaluation: Evaluation::Failed(Lookup::Tweet),
                        failed: Hydrators::empty(),
                    },
                    (HydratedTweet::Resolved { .. }, Some(admitted)) => {
                        let author = admitted.post(placed.tweet_id(), placed.placement)?;
                        UserEvaluation {
                            evaluation: author.evaluate(),
                            failed: author.user_candidate().failed,
                        }
                    }
                };
                Some((placed.key(), evaluation))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clients::socialgraph_client::{EdgeFrom, EdgeQuery, Graph};
    use crate::hydration::inventory::Source;
    use crate::hydration::sources::InMemorySources;
    use crate::hydration::Hydrator;
    use crate::metrics::Rpc;
    use crate::models::{ClientCapability, PlacedCandidate, Placement, TweetId};
    use crate::rules::fixtures::{allow, dropped};
    use crate::rules::{RuleEngine, SafetyLevel};
    use crate::scenario::raw;
    use std::sync::Arc;
    use xai_core_entities::entities::{
        GizmoduckUser, GizmoduckUserResult, Safety, UserResponseState,
    };
    use xai_visibility_filtering::models::FilteredReason;

    #[tokio::test]
    async fn the_quote_pass_fetches_a_missing_outer_and_reads_its_pair_edges_logged_out() {
        const QUOTING_AUTHOR: u64 = 30;
        let protected = || GizmoduckUserResult {
            user: Some(GizmoduckUser {
                safety: Safety {
                    is_protected: true,
                    ..Default::default()
                },
                ..Default::default()
            }),
            response_state: Some(UserResponseState::Found),
        };
        let sources = Arc::new(
            InMemorySources::default()
                .tweet(1, 20)
                .tweet(2, 21)
                .tweet(3, 22)
                .tweet(4, QUOTING_AUTHOR)
                .tweet(9, QUOTING_AUTHOR)
                .authors(&[20, QUOTING_AUTHOR])
                .user(21, protected())
                .user(22, protected())
                .user(QUOTING_AUTHOR, protected())
                .edge(Graph::Blocks, 20, QUOTING_AUTHOR)
                .edge(Graph::Follows, QUOTING_AUTHOR, 22),
        );
        let post_evaluator = PostEvaluator::new(
            Arc::clone(&sources) as _,
            RuleEngine::for_tests(),
            Arc::default(),
        );
        let quoted = |tweet_id| PlacedCandidate {
            candidate: raw(tweet_id, None),
            placement: Placement::InnerQuoted { outer: TweetId(9) },
        };
        let authors = post_evaluator
            .evaluate_authors(PostBatch {
                viewer_id: None,
                country_code: None,
                client_capability: ClientCapability::default(),
                safety_level: SafetyLevel::QuotedTweetRules,
                candidates: [1, 2, 3, 4].map(quoted).into(),
                rpc: Rpc::EvaluateTweets,
            })
            .await
            .into_iter()
            .map(|(key, author)| (key, author.evaluation))
            .collect::<HashMap<_, _>>();
        assert_eq!(
            authors,
            HashMap::from([
                (
                    quoted(1).key(),
                    Evaluation::complete(dropped(
                        FilteredReason::AuthorBlockViewer,
                        "quoted_author_blocks_quoting_author/drop/author_block_viewer"
                    ))
                ),
                (
                    quoted(2).key(),
                    Evaluation::complete(dropped(
                        FilteredReason::AuthorIsProtected,
                        "protected_quoted_author/drop"
                    ))
                ),
                (quoted(3).key(), Evaluation::complete(allow())),
                (quoted(4).key(), Evaluation::complete(allow())),
            ])
        );
        assert_eq!(
            sources.keys(Source::TesPureCoreData),
            [vec![1, 2, 3, 4], vec![9]]
        );
        let sourced = |graph, source_id, destination_ids| EdgeQuery {
            from: EdgeFrom::User(source_id),
            ..EdgeQuery::forward(graph, destination_ids)
        };
        let mut selects = sources.selects();
        selects.sort_by_key(|queries| queries.first().map(|query| <&str>::from(query.graph)));
        assert_eq!(
            selects,
            [
                vec![
                    sourced(Graph::Blocks, 20, vec![QUOTING_AUTHOR]),
                    sourced(Graph::Blocks, 21, vec![QUOTING_AUTHOR]),
                    sourced(Graph::Blocks, 22, vec![QUOTING_AUTHOR]),
                ],
                vec![sourced(Graph::Follows, QUOTING_AUTHOR, vec![21, 22])],
            ]
        );
    }

    #[tokio::test]
    async fn a_failed_outer_fails_only_its_quotes_pair_nodes() {
        let sources = InMemorySources::default()
            .tweet(1, 20)
            .tweet(2, 20)
            .tweet(8, 30)
            .authors(&[20, 30])
            .fail_key(Source::TesPureCoreData, 9);
        let post_evaluator =
            PostEvaluator::new(Arc::new(sources), RuleEngine::for_tests(), Arc::default());
        let quoted = |tweet_id, outer| PlacedCandidate {
            candidate: raw(tweet_id, None),
            placement: Placement::InnerQuoted {
                outer: TweetId(outer),
            },
        };
        let authors = post_evaluator
            .evaluate_authors(PostBatch {
                viewer_id: Some(50),
                country_code: None,
                client_capability: ClientCapability::default(),
                safety_level: SafetyLevel::QuotedTweetRules,
                candidates: vec![quoted(1, 8), quoted(2, 9)],
                rpc: Rpc::EvaluateTweets,
            })
            .await;
        assert_eq!(
            authors[&quoted(1, 8).key()].evaluation,
            Evaluation::complete(allow())
        );
        let pair_nodes = Hydrators::of(Hydrator::AuthorBlocksOuterAuthor)
            .with(Hydrator::OuterAuthorFollowsAuthor);
        assert_eq!(
            authors[&quoted(2, 9).key()],
            UserEvaluation {
                evaluation: Evaluation::Partial {
                    verdict: allow(),
                    fail_open_defaults: Hydrators::of(Hydrator::AuthorBlocksOuterAuthor),
                },
                failed: pair_nodes,
            }
        );
    }
}
