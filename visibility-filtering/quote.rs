use crate::hydration::request_context;
use crate::metrics::{self as ft_metrics, Rpc};
use crate::models::{Decided, Evaluation, EvaluationKey, PlacedCandidate, Placement, Verdict};
use crate::post_evaluator::{PostBatch, PostEvaluation, PostEvaluator, UserEvaluation};
use crate::retweet;
use crate::rules::{QuoteEntries, RuleId, SafetyLevel};
use crate::treatment;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tokio::time::Instant;
use xai_x_thrift::safety_result::FilteredReason as ThriftFilteredReason;

pub(crate) const QUOTE_PASS: &str = "vf_quote_pass";
const SHED: &str = "shed";
pub(crate) const MAX_SHADOW_PASSES_IN_FLIGHT: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum QuotePassMode {
    Off,
    Shadow,
}

#[derive(Clone, Debug, PartialEq, strum::IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum PassOutcome {
    OuterWithheld,
    OuterAbsent,
    InnerUnresolved,
    FailedOpen,
    NotRerendered,
    Dropped(Rerendered),
    Passed,
}

impl PassOutcome {
    pub(crate) fn rendering(&self) -> QuoteRendering<'_> {
        match self {
            Self::OuterWithheld => QuoteRendering::Omitted,
            Self::Dropped(rerendered) => QuoteRendering::Rerendered(rerendered),
            Self::OuterAbsent
            | Self::InnerUnresolved
            | Self::FailedOpen
            | Self::NotRerendered
            | Self::Passed => QuoteRendering::Served,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Rerendered {
    pub(crate) by: RuleId,
    pub(crate) reason: &'static ThriftFilteredReason,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum QuoteRendering<'a> {
    Served,
    Rerendered(&'a Rerendered),
    Omitted,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct QuotePass {
    pub(crate) key: EvaluationKey,
    pub(crate) outcome: PassOutcome,
    pub(crate) author: Option<Evaluation>,
}

impl QuotePass {
    pub(crate) fn decided_by(&self, served_rule: Option<&'static str>) -> Option<&'static str> {
        match self.outcome.rendering() {
            QuoteRendering::Rerendered(_) => self
                .author
                .as_ref()
                .and_then(|author| treatment::decided_by(author.verdict())),
            QuoteRendering::Served | QuoteRendering::Omitted => served_rule,
        }
    }
}

pub(crate) fn quoted_authors(request: &PostBatch) -> Option<PostBatch> {
    let quotes: Vec<PlacedCandidate> = request
        .candidates
        .iter()
        .filter(|placed| matches!(placed.placement, Placement::InnerQuoted { .. }))
        .copied()
        .collect();
    (!quotes.is_empty()).then(|| PostBatch {
        viewer_id: request.viewer_id,
        country_code: request.country_code.clone(),
        client_capability: request.client_capability,
        safety_level: SafetyLevel::QuotedTweetRules,
        candidates: quotes,
        rpc: request.rpc,
    })
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Passes {
    Skipped,
    Shed,
    Ran(Vec<QuotePass>),
}

impl Passes {
    pub(crate) fn ran(&self) -> &[QuotePass] {
        match self {
            Self::Ran(passes) => passes,
            Self::Skipped | Self::Shed => &[],
        }
    }
}

pub(crate) struct QuotePassRunner {
    mode: QuotePassMode,
    permits: Arc<Semaphore>,
}

pub(crate) struct PendingPass {
    level: SafetyLevel,
    authors: Option<PostBatch>,
}

impl QuotePassRunner {
    pub(crate) fn new(mode: QuotePassMode) -> Self {
        Self {
            mode,
            permits: Arc::new(Semaphore::new(MAX_SHADOW_PASSES_IN_FLIGHT)),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_permits(self, permits: usize) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(permits)),
            ..self
        }
    }

    pub(crate) async fn serve(
        &self,
        post_evaluator: &PostEvaluator,
        request: PostBatch,
    ) -> (Vec<PostEvaluation>, PendingPass) {
        let pending = PendingPass {
            level: request.safety_level,
            authors: match self.mode {
                QuotePassMode::Off => None,
                QuotePassMode::Shadow => quoted_authors(&request),
            },
        };
        let outcomes = retweet::evaluate_merging_sources(post_evaluator, request).await;
        (outcomes, pending)
    }

    pub(crate) fn finish(
        &self,
        post_evaluator: &Arc<PostEvaluator>,
        pending: PendingPass,
        outcomes: Vec<PostEvaluation>,
        then: impl FnOnce(&[PostEvaluation], &Passes) + Send + 'static,
    ) {
        let Some(authors) = &pending.authors else {
            return then(&outcomes, &Passes::Skipped);
        };
        let Ok(permit) = Arc::clone(&self.permits).try_acquire_owned() else {
            ft_metrics::incr_nonzero(
                QUOTE_PASS,
                &[("outcome", SHED), ("safety_level", pending.level.into())],
                u64::try_from(authors.candidates.len()).unwrap_or(u64::MAX),
            );
            return then(&outcomes, &Passes::Shed);
        };
        let post_evaluator = Arc::clone(post_evaluator);
        tokio::spawn(async move {
            let _permit = permit;
            let passes = request_context(Instant::now(), None)
                .scope(pending.run(&post_evaluator, &outcomes))
                .await;
            then(&outcomes, &passes);
        });
    }
}

impl PendingPass {
    pub(crate) async fn run(
        self,
        post_evaluator: &PostEvaluator,
        outcomes: &[PostEvaluation],
    ) -> Passes {
        match self.authors {
            None => Passes::Skipped,
            Some(authors) => {
                Passes::Ran(quote_pass(post_evaluator, self.level, authors, outcomes).await)
            }
        }
    }
}

async fn quote_pass(
    post_evaluator: &PostEvaluator,
    level: SafetyLevel,
    authors: PostBatch,
    outcomes: &[PostEvaluation],
) -> Vec<QuotePass> {
    let authors = post_evaluator.evaluate_authors(authors).await;
    let passes = passes(level, outcomes, &authors);
    record(level, &passes);
    passes
}

fn passes(
    level: SafetyLevel,
    outcomes: &[PostEvaluation],
    authors: &HashMap<EvaluationKey, UserEvaluation>,
) -> Vec<QuotePass> {
    let outers: HashMap<_, _> = outcomes
        .iter()
        .filter(|outcome| outcome.placement == Placement::Standalone)
        .map(|outcome| (outcome.tweet_id, &outcome.evaluation))
        .collect();
    outcomes
        .iter()
        .filter_map(|outcome| {
            let Placement::InnerQuoted { outer } = outcome.placement else {
                return None;
            };
            let author = authors.get(&outcome.key());
            Some(QuotePass {
                key: outcome.key(),
                outcome: pass(
                    level,
                    outers.get(&outer).copied(),
                    &outcome.evaluation,
                    author,
                ),
                author: author.map(|author| author.evaluation.clone()),
            })
        })
        .collect()
}

fn pass(
    level: SafetyLevel,
    outer: Option<&Evaluation>,
    inner: &Evaluation,
    author: Option<&UserEvaluation>,
) -> PassOutcome {
    match outer {
        None => return PassOutcome::OuterAbsent,
        Some(Evaluation::Complete {
            verdict: Verdict::Shown { .. },
            ..
        }) => {}
        Some(
            Evaluation::Complete {
                verdict: Verdict::Withheld(_),
                ..
            }
            | Evaluation::Partial { .. }
            | Evaluation::NotFound(_)
            | Evaluation::Failed(_),
        ) => return PassOutcome::OuterWithheld,
    }
    match (inner, author, judge(level, author)) {
        (
            Evaluation::NotFound(_),
            Some(_),
            judged @ (PassOutcome::Dropped(_) | PassOutcome::FailedOpen),
        ) => judged,
        (Evaluation::NotFound(_) | Evaluation::Failed(_), _, _) => PassOutcome::InnerUnresolved,
        (Evaluation::Complete { .. } | Evaluation::Partial { .. }, _, judged) => judged,
    }
}

fn judge(level: SafetyLevel, author: Option<&UserEvaluation>) -> PassOutcome {
    let Some(author) = author.filter(|author| author.failed.is_empty()) else {
        return PassOutcome::FailedOpen;
    };
    match &author.evaluation {
        Evaluation::Complete {
            verdict: Verdict::Withheld(Decided { by, .. }),
            ..
        } => rerender(level, by).map_or(PassOutcome::NotRerendered, PassOutcome::Dropped),
        Evaluation::Complete {
            verdict: Verdict::Shown { .. },
            ..
        } => PassOutcome::Passed,
        Evaluation::Partial { .. } | Evaluation::NotFound(_) | Evaluation::Failed(_) => {
            PassOutcome::FailedOpen
        }
    }
}

fn rerender(level: SafetyLevel, rule: &str) -> Option<Rerendered> {
    let QuoteEntries::Judged { rerenders } = level.spec().quote_entries else {
        return None;
    };
    let by = RuleId::of_rule_name(rule)?;
    let (_, reason) = rerenders.iter().find(|(id, _)| *id == by)?;
    Some(Rerendered { by, reason })
}

fn record(level: SafetyLevel, passes: &[QuotePass]) {
    let mut counts: HashMap<&'static str, u64> = HashMap::new();
    for pass in passes {
        *counts.entry((&pass.outcome).into()).or_default() += 1;
    }
    for (outcome, count) in counts {
        ft_metrics::incr_nonzero(
            QUOTE_PASS,
            &[("outcome", outcome), ("safety_level", level.into())],
            count,
        );
    }
    ft_metrics::record_verdicts(
        Rpc::EvaluateTweets,
        SafetyLevel::QuotedTweetRules,
        passes.iter().filter_map(|pass| match pass.outcome {
            PassOutcome::Dropped(_) | PassOutcome::Passed => {
                pass.author.as_ref().map(Evaluation::verdict)
            }
            PassOutcome::OuterWithheld
            | PassOutcome::OuterAbsent
            | PassOutcome::InnerUnresolved
            | PassOutcome::FailedOpen
            | PassOutcome::NotRerendered => None,
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clients::socialgraph_client::Graph;
    use crate::evaluate_tweets::{EntryComparator, EvaluateTweetsEndpoint};
    use crate::hydration::fallback_cache::FallbackCaches;
    use crate::hydration::inventory::Source;
    use crate::hydration::sources::{suspended, Fault, InMemorySources};
    use crate::hydration::tweet_fallback_cache;
    use crate::metrics::thread_recorded;
    use crate::models::TweetId;
    use crate::params::limited_actions_copy::LimitedActionsCopy;
    use crate::params::ClientSwitches;
    use crate::rules::RuleEngine;
    use std::time::Duration;
    use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
    use tokio::time::timeout;
    use tonic::Request;
    use vf_pb::tweet_evaluation::Outcome;
    use xai_core_entities::entities::{
        GizmoduckUser, GizmoduckUserResult, Safety, UserResponseState,
    };
    use xai_twittercontext_proto::TwitterContextViewer;
    use xai_visibility_filtering_proto as vf_pb;
    use xai_x_thrift::safety_level::SafetyLevel as ThriftLevel;

    const VIEWER: u64 = 50;
    const QUOTED_AUTHOR: u64 = 20;
    const QUOTING_AUTHOR: u64 = 30;
    const INNER: u64 = 1;
    const OUTER: u64 = 9;

    struct Captured(UnboundedSender<Passes>);

    impl EntryComparator for Captured {
        fn compare(
            &self,
            _: Option<u64>,
            _: Option<&TwitterContextViewer>,
            _: Option<&str>,
            _: SafetyLevel,
            _: &[PostEvaluation],
            quote_passes: &Passes,
        ) {
            self.0.send(quote_passes.clone()).unwrap();
        }
    }

    struct Served {
        endpoint: EvaluateTweetsEndpoint,
        compared: UnboundedReceiver<Passes>,
    }

    impl Served {
        fn new(world: Arc<InMemorySources>, caches: FallbackCaches, mode: QuotePassMode) -> Self {
            let (captured, compared) = unbounded_channel();
            let endpoint = EvaluateTweetsEndpoint::new(
                Arc::new(PostEvaluator::new(
                    world,
                    RuleEngine::for_tests(),
                    Arc::new(caches),
                )),
                ClientSwitches::for_tests(),
                LimitedActionsCopy::from_json("[]"),
                Some(Box::new(Captured(captured))),
            )
            .with_quote_pass(mode);
            Self { endpoint, compared }
        }

        async fn outcomes(
            &self,
            viewer_id: Option<u64>,
            tweets: Vec<vf_pb::TweetData>,
        ) -> Vec<Outcome> {
            self.endpoint
                .handle(Request::new(vf_pb::EvaluateTweetsRequest {
                    safety_level: ThriftLevel::TIMELINE_HOME_HYDRATION.0,
                    viewer_id,
                    tweets,
                }))
                .await
                .unwrap()
                .into_inner()
                .results
                .into_iter()
                .map(|result| result.outcome.unwrap())
                .collect()
        }

        async fn compared(&mut self) -> Passes {
            timeout(Duration::from_secs(5), self.compared.recv())
                .await
                .expect("the comparator hears of every request")
                .unwrap()
        }

        async fn serve(
            &mut self,
            viewer_id: Option<u64>,
            tweets: Vec<vf_pb::TweetData>,
        ) -> (Vec<Outcome>, Vec<QuotePass>) {
            let outcomes = self.outcomes(viewer_id, tweets).await;
            (outcomes, self.compared().await.ran().to_vec())
        }
    }

    fn author(safety: Safety) -> GizmoduckUserResult {
        GizmoduckUserResult {
            user: Some(GizmoduckUser {
                safety,
                ..Default::default()
            }),
            response_state: Some(UserResponseState::Found),
        }
    }

    fn protected() -> GizmoduckUserResult {
        author(Safety {
            is_protected: true,
            ..Default::default()
        })
    }

    fn world() -> InMemorySources {
        InMemorySources::default()
            .tweet(INNER, QUOTED_AUTHOR)
            .tweet(OUTER, QUOTING_AUTHOR)
            .authors(&[QUOTED_AUTHOR, QUOTING_AUTHOR])
    }

    fn deleted_inner() -> InMemorySources {
        InMemorySources::default()
            .tweet(OUTER, QUOTING_AUTHOR)
            .quote(OUTER, INNER, QUOTED_AUTHOR)
            .authors(&[QUOTED_AUTHOR, QUOTING_AUTHOR])
    }

    fn live_inner_of_a_not_found_author() -> InMemorySources {
        InMemorySources::default()
            .tweet(INNER, QUOTED_AUTHOR)
            .tweet(OUTER, QUOTING_AUTHOR)
            .authors(&[QUOTING_AUTHOR])
    }

    fn entry(tweet_id: u64, outer_tweet_id: Option<u64>) -> vf_pb::TweetData {
        vf_pb::TweetData {
            tweet_id,
            quote_context: outer_tweet_id
                .map(|outer_tweet_id| vf_pb::QuoteContext { outer_tweet_id }),
        }
    }

    fn quote_with_outer() -> Vec<vf_pb::TweetData> {
        vec![entry(OUTER, None), entry(INNER, Some(OUTER))]
    }

    async fn serve(
        world: Arc<InMemorySources>,
        mode: QuotePassMode,
        viewer_id: Option<u64>,
        tweets: Vec<vf_pb::TweetData>,
    ) -> (Vec<Outcome>, Vec<QuotePass>) {
        Served::new(world, FallbackCaches::default(), mode)
            .serve(viewer_id, tweets)
            .await
    }

    fn dropped(rule: &str, reason: ThriftFilteredReason) -> PassOutcome {
        PassOutcome::Dropped(Rerendered {
            by: RuleId::of_rule_name(rule).unwrap(),
            reason: Box::leak(Box::new(reason)),
        })
    }

    fn erased_drop() -> PassOutcome {
        dropped(
            "erased_author/drop/inactive",
            ThriftFilteredReason::AuthorIsDeactivated(true),
        )
    }

    #[tokio::test]
    async fn the_shadow_pass_judges_the_outer_first_then_the_quoted_author_and_serves_as_off() {
        let quote = || EvaluationKey {
            tweet_id: TweetId(INNER),
            placement: Placement::InnerQuoted {
                outer: TweetId(OUTER),
            },
        };
        let deactivated = Safety {
            deactivated: true,
            ..Default::default()
        };
        let erased = Safety {
            erased: true,
            ..Default::default()
        };
        let offboarded = Safety {
            offboarded: true,
            ..Default::default()
        };
        let cases = [
            (
                "passed",
                Some(VIEWER),
                world(),
                quote_with_outer(),
                PassOutcome::Passed,
            ),
            (
                "deactivated",
                Some(VIEWER),
                world().user(QUOTED_AUTHOR, author(deactivated.clone())),
                quote_with_outer(),
                dropped(
                    "deactivated_author/drop",
                    ThriftFilteredReason::AuthorIsDeactivated(true),
                ),
            ),
            (
                "erased",
                Some(VIEWER),
                world().user(QUOTED_AUTHOR, author(erased)),
                quote_with_outer(),
                dropped(
                    "erased_author/drop/inactive",
                    ThriftFilteredReason::AuthorIsDeactivated(true),
                ),
            ),
            (
                "offboarded",
                Some(VIEWER),
                world().user(QUOTED_AUTHOR, author(offboarded)),
                quote_with_outer(),
                dropped(
                    "offboarded_author/drop/inactive",
                    ThriftFilteredReason::AuthorAccountIsInactive(true),
                ),
            ),
            (
                "suspended",
                Some(VIEWER),
                world().user(QUOTED_AUTHOR, suspended()),
                quote_with_outer(),
                dropped(
                    "suspended_author/drop",
                    ThriftFilteredReason::AuthorIsSuspended(true),
                ),
            ),
            (
                "quoted author blocks quoting author",
                Some(VIEWER),
                world().edge(Graph::Blocks, QUOTED_AUTHOR, QUOTING_AUTHOR),
                quote_with_outer(),
                dropped(
                    "quoted_author_blocks_quoting_author/drop/author_block_viewer",
                    ThriftFilteredReason::AuthorBlockViewer(true),
                ),
            ),
            (
                "viewer blocks author",
                Some(VIEWER),
                world().edge(Graph::Blocks, VIEWER, QUOTED_AUTHOR),
                quote_with_outer(),
                dropped(
                    "viewer_blocks_author/drop",
                    ThriftFilteredReason::UnspecifiedReason(true),
                ),
            ),
            (
                "viewer mutes author",
                Some(VIEWER),
                world().edge(Graph::Mutes, VIEWER, QUOTED_AUTHOR),
                quote_with_outer(),
                dropped(
                    "quoted_viewer_mutes_author_not_following/drop",
                    ThriftFilteredReason::UnspecifiedReason(true),
                ),
            ),
            (
                "protected",
                Some(VIEWER),
                world().user(QUOTED_AUTHOR, protected()),
                quote_with_outer(),
                dropped(
                    "protected_quoted_author/drop",
                    ThriftFilteredReason::AuthorIsProtected(true),
                ),
            ),
            (
                "outer withheld",
                Some(VIEWER),
                world().user(QUOTING_AUTHOR, suspended()),
                quote_with_outer(),
                PassOutcome::OuterWithheld,
            ),
            (
                "outer not found",
                Some(VIEWER),
                InMemorySources::default()
                    .tweet(INNER, QUOTED_AUTHOR)
                    .authors(&[QUOTED_AUTHOR]),
                quote_with_outer(),
                PassOutcome::OuterWithheld,
            ),
            (
                "outer failed",
                Some(VIEWER),
                world().fail_key(Source::TesPureCoreData, OUTER),
                quote_with_outer(),
                PassOutcome::OuterWithheld,
            ),
            (
                "outer absent",
                Some(VIEWER),
                world().user(QUOTED_AUTHOR, suspended()),
                vec![entry(INNER, Some(OUTER))],
                PassOutcome::OuterAbsent,
            ),
            (
                "inner deleted",
                Some(VIEWER),
                InMemorySources::default()
                    .tweet(OUTER, QUOTING_AUTHOR)
                    .authors(&[QUOTING_AUTHOR]),
                quote_with_outer(),
                PassOutcome::InnerUnresolved,
            ),
            (
                "deleted inner whose author blocks the quoting author",
                Some(VIEWER),
                deleted_inner().edge(Graph::Blocks, QUOTED_AUTHOR, QUOTING_AUTHOR),
                quote_with_outer(),
                dropped(
                    "quoted_author_blocks_quoting_author/drop/author_block_viewer",
                    ThriftFilteredReason::AuthorBlockViewer(true),
                ),
            ),
            (
                "deleted inner of a deactivated author",
                Some(VIEWER),
                deleted_inner().user(QUOTED_AUTHOR, author(deactivated)),
                quote_with_outer(),
                dropped(
                    "deactivated_author/drop",
                    ThriftFilteredReason::AuthorIsDeactivated(true),
                ),
            ),
            (
                "deleted inner of a protected author",
                Some(VIEWER),
                deleted_inner().user(QUOTED_AUTHOR, protected()),
                quote_with_outer(),
                dropped(
                    "protected_quoted_author/drop",
                    ThriftFilteredReason::AuthorIsProtected(true),
                ),
            ),
            (
                "deleted inner whose author level 65 does not drop",
                Some(VIEWER),
                deleted_inner(),
                quote_with_outer(),
                PassOutcome::InnerUnresolved,
            ),
            (
                "deleted inner of a not-found author",
                Some(VIEWER),
                InMemorySources::default()
                    .tweet(OUTER, QUOTING_AUTHOR)
                    .quote(OUTER, INNER, QUOTED_AUTHOR)
                    .authors(&[QUOTING_AUTHOR]),
                quote_with_outer(),
                erased_drop(),
            ),
            (
                "live inner of a not-found author",
                Some(VIEWER),
                live_inner_of_a_not_found_author(),
                quote_with_outer(),
                erased_drop(),
            ),
            (
                "live inner of a soft user",
                Some(VIEWER),
                world().user(
                    QUOTED_AUTHOR,
                    GizmoduckUserResult {
                        user: None,
                        response_state: Some(UserResponseState::SoftUser),
                    },
                ),
                quote_with_outer(),
                erased_drop(),
            ),
            (
                "deleted inner whose author lookup failed",
                Some(VIEWER),
                deleted_inner().fail_key(Source::GizmoduckAuthor, QUOTED_AUTHOR),
                quote_with_outer(),
                PassOutcome::FailedOpen,
            ),
            (
                "live inner whose author lookup failed",
                Some(VIEWER),
                world().fail_key(Source::GizmoduckAuthor, QUOTED_AUTHOR),
                quote_with_outer(),
                PassOutcome::InnerUnresolved,
            ),
            (
                "deleted inner whose outer's quoted tweet lookup failed",
                Some(VIEWER),
                deleted_inner()
                    .edge(Graph::Blocks, QUOTED_AUTHOR, QUOTING_AUTHOR)
                    .fault(Source::TesQuotedTweet, Fault::Fails),
                quote_with_outer(),
                PassOutcome::FailedOpen,
            ),
            (
                "block pair edge failed",
                Some(VIEWER),
                world()
                    .user(QUOTED_AUTHOR, protected())
                    .edge(Graph::Follows, QUOTING_AUTHOR, QUOTED_AUTHOR)
                    .fault(Source::FlockUserPair, Fault::Fails),
                quote_with_outer(),
                PassOutcome::FailedOpen,
            ),
            (
                "follow pair edge failed for a protected author the quoting author follows",
                None,
                world()
                    .user(QUOTED_AUTHOR, protected())
                    .edge(Graph::Follows, QUOTING_AUTHOR, QUOTED_AUTHOR)
                    .fail_graph(Graph::Follows),
                quote_with_outer(),
                PassOutcome::FailedOpen,
            ),
        ];
        for (name, viewer_id, world, tweets, outcome) in cases {
            let world = Arc::new(world);
            let (off, no_passes) = serve(
                Arc::clone(&world),
                QuotePassMode::Off,
                viewer_id,
                tweets.clone(),
            )
            .await;
            assert!(no_passes.is_empty(), "{name}");
            assert!(!world.calls().contains(&Source::FlockUserPair), "{name}");
            assert!(!world.calls().contains(&Source::TesQuotedTweet), "{name}");
            let (shadow, passes) = serve(world, QuotePassMode::Shadow, viewer_id, tweets).await;
            assert_eq!(
                shadow, off,
                "{name}: shadow serves the inner-quoted verdict"
            );
            assert_eq!(
                passes
                    .iter()
                    .map(|pass| (pass.key, &pass.outcome))
                    .collect::<Vec<_>>(),
                [(quote(), &outcome)],
                "{name}"
            );
        }
    }

    #[tokio::test]
    async fn a_not_found_quoted_author_still_fails_the_served_inner() {
        let (served, _) = serve(
            Arc::new(live_inner_of_a_not_found_author()),
            QuotePassMode::Off,
            Some(VIEWER),
            quote_with_outer(),
        )
        .await;
        assert!(
            matches!(served[0], Outcome::ResultStateThriftCompact(_)),
            "{served:?}"
        );
        assert_eq!(served[1], Outcome::Failed(vf_pb::Failed {}));
    }

    #[tokio::test(start_paused = true)]
    async fn a_deleted_inner_drops_for_a_protected_author_a_live_quote_already_hydrated() {
        const LIVE_INNER: u64 = 2;
        const LIVE_OUTER: u64 = 8;
        const LIVE_QUOTING_AUTHOR: u64 = 31;
        let world = deleted_inner()
            .tweet(LIVE_INNER, QUOTED_AUTHOR)
            .tweet(LIVE_OUTER, LIVE_QUOTING_AUTHOR)
            .authors(&[LIVE_QUOTING_AUTHOR])
            .user(QUOTED_AUTHOR, protected())
            .latency(Source::TesQuotedTweet, Duration::from_millis(10));
        let mut tweets = quote_with_outer();
        tweets.extend([entry(LIVE_OUTER, None), entry(LIVE_INNER, Some(LIVE_OUTER))]);
        let (_, passes) = serve(Arc::new(world), QuotePassMode::Shadow, Some(VIEWER), tweets).await;
        let protected_drop = dropped(
            "protected_quoted_author/drop",
            ThriftFilteredReason::AuthorIsProtected(true),
        );
        assert_eq!(
            passes
                .iter()
                .map(|pass| (pass.key.tweet_id.0, &pass.outcome))
                .collect::<Vec<_>>(),
            [(INNER, &protected_drop), (LIVE_INNER, &protected_drop)]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_hung_quote_pass_source_does_not_delay_the_served_response() {
        let world = Arc::new(world().fault(Source::FlockUserPair, Fault::Hangs));
        let (off, _) = serve(
            Arc::clone(&world),
            QuotePassMode::Off,
            Some(VIEWER),
            quote_with_outer(),
        )
        .await;
        let mut shadow = Served::new(world, FallbackCaches::default(), QuotePassMode::Shadow);
        let started = Instant::now();
        let served = shadow.outcomes(Some(VIEWER), quote_with_outer()).await;
        assert_eq!(started.elapsed(), Duration::ZERO);
        assert_eq!(served, off);
        let passes = shadow.compared().await.ran().to_vec();
        assert!(
            started.elapsed() > Duration::ZERO,
            "the pass waited out the hung source"
        );
        assert_eq!(
            passes.iter().map(|pass| &pass.outcome).collect::<Vec<_>>(),
            [&PassOutcome::FailedOpen]
        );
    }

    #[tokio::test]
    async fn a_finished_pass_frees_its_permit_for_the_next_request() {
        let mut served = Served::new(
            Arc::new(world()),
            FallbackCaches::default(),
            QuotePassMode::Shadow,
        );
        served.endpoint = served.endpoint.with_quote_pass_permits(1);
        for request in ["first", "second"] {
            let (_, passes) = served.serve(Some(VIEWER), quote_with_outer()).await;
            assert_eq!(
                passes.iter().map(|pass| &pass.outcome).collect::<Vec<_>>(),
                [&PassOutcome::Passed],
                "{request}"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_running_pass_holds_its_permit_until_it_ends() {
        let world = Arc::new(world().fault(Source::FlockUserPair, Fault::Hangs));
        let mut served = Served::new(
            Arc::clone(&world),
            FallbackCaches::default(),
            QuotePassMode::Shadow,
        );
        served.endpoint = served.endpoint.with_quote_pass_permits(1);
        thread_recorded::start();
        served.outcomes(Some(VIEWER), quote_with_outer()).await;
        served.outcomes(Some(VIEWER), quote_with_outer()).await;
        assert_eq!(
            served.compared().await,
            Passes::Shed,
            "the second pass is shed"
        );
        assert_eq!(
            served
                .compared()
                .await
                .ran()
                .iter()
                .map(|pass| &pass.outcome)
                .collect::<Vec<_>>(),
            [&PassOutcome::FailedOpen]
        );
        assert_eq!(
            world.keys(Source::FlockUserPair).len(),
            1,
            "the shed pass fetches nothing"
        );
        let shed = thread_recorded::take(QUOTE_PASS)
            .iter()
            .filter(|sample| sample.label("outcome") == Some(SHED))
            .map(|sample| sample.value)
            .sum::<f64>();
        assert_eq!(shed, 1.0);
    }

    #[tokio::test]
    async fn the_quote_pass_fills_no_fallback_cache_the_served_hydration_reads() {
        let world = Arc::new(world());
        let caches = FallbackCaches {
            tweets: Some(tweet_fallback_cache(8)),
            authors: None,
        };
        let mut served = Served::new(Arc::clone(&world), caches, QuotePassMode::Shadow);
        let (_, passes) = served
            .serve(Some(VIEWER), vec![entry(INNER, Some(OUTER))])
            .await;
        assert_eq!(passes[0].outcome, PassOutcome::OuterAbsent);
        assert!(
            world
                .keys(Source::TesPureCoreData)
                .concat()
                .contains(&OUTER),
            "the pass fetched the outer's pure core"
        );
        world.break_source(Source::TesPureCoreData, Fault::Fails);
        let (outer, _) = served.serve(Some(VIEWER), vec![entry(OUTER, None)]).await;
        assert_eq!(outer, [Outcome::Failed(vf_pb::Failed {})]);
    }

    #[tokio::test]
    async fn a_shadow_request_records_one_hydration_phase_sample() {
        let mut served = Served::new(
            Arc::new(world()),
            FallbackCaches::default(),
            QuotePassMode::Shadow,
        );
        thread_recorded::start();
        let (_, passes) = served.serve(Some(VIEWER), quote_with_outer()).await;
        let hydrations = thread_recorded::take("filter_tweets_phase_ms")
            .iter()
            .filter(|sample| sample.label("stage") == Some("hydration"))
            .count();
        assert_eq!(passes.len(), 1, "the pass ran");
        assert_eq!(hydrations, 1);
    }
}
