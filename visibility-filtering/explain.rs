pub use crate::hydration::{Hydrator, Hydrators, Lookup};
pub use crate::models::viewer::Client;
pub use crate::models::{ClientCapability, VerifyBlurSupport, ViewerAge, ViewerProfile};
pub use crate::post_card::{Author, CardMedia, CardMediaKind, PostCard};
pub use crate::rules::metrics::Rpc;
pub use crate::rules::SafetyLevel;

use crate::evaluate_tweets::{evaluate_tweets_request, Sends};
use crate::filter::{FilterRequest, FilterTweets, HydratedRequest};
use crate::hydration::fallback_cache::FallbackCaches;
use crate::hydration::sources::{test_users_metadata, Sources};
use crate::hydration::{request_context, Hydration};
use crate::limited_actions_copy::LimitedActionsCopy;
use crate::models::{Evaluation, PostCandidate, RawCandidate, TweetId, Viewer};
use crate::params::{ClientSwitches, CountryLists};
use crate::result_label::{label, state_label, Class, NONE};
use crate::retweet;
use crate::rules::{ExplainedFact, RuleEngine, Truth};
use crate::server_deps::{self, CacheWarming};
use crate::treatment;
use futures::future::{join, join_all};
use std::num::NonZeroU64;
use std::sync::Arc;
use tokio::time::Instant;
use xai_twittercontext_proto::TwitterContextViewer;
use xai_x_thrift::tweet_service::TweetFieldsResultState;

pub struct Explainer {
    sources: Arc<dyn Sources>,
    filter_tweets: FilterTweets,
    client_switches: ClientSwitches,
    copy: LimitedActionsCopy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Route {
    rpc: Rpc,
    level: SafetyLevel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{rpc:?} cannot carry {level:?}")]
pub struct Unroutable {
    pub rpc: Rpc,
    pub level: SafetyLevel,
}

impl Route {
    pub fn new(rpc: Rpc, level: SafetyLevel) -> Result<Self, Unroutable> {
        if level.is_carried_by(rpc) {
            Ok(Self { rpc, level })
        } else {
            Err(Unroutable { rpc, level })
        }
    }

    pub fn rpc(self) -> Rpc {
        self.rpc
    }

    pub fn level(self) -> SafetyLevel {
        self.level
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ask<const N: usize> {
    pub viewer: Option<NonZeroU64>,
    pub post: u64,
    pub country: Option<String>,
    pub client: Client,
    pub routes: [Route; N],
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Explanation<const N: usize> {
    pub post: Option<PostCard>,
    pub viewer: ViewerFacts,
    pub in_nsfw_gating_country: bool,
    pub levels: [LevelOutcome; N],
}

impl<const N: usize> Explanation<N> {
    pub fn new(
        post: Option<PostCard>,
        viewer: ViewerFacts,
        in_nsfw_gating_country: bool,
        levels: [LevelOutcome; N],
    ) -> Self {
        Self {
            post,
            viewer,
            in_nsfw_gating_country,
            levels,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct LevelOutcome {
    pub route: Route,
    pub capability: ClientCapability,
    pub judgement: Judgement,
}

impl LevelOutcome {
    pub fn new(route: Route, capability: ClientCapability, judgement: Judgement) -> Self {
        Self {
            route,
            capability,
            judgement,
        }
    }

    pub fn evaluate_tweets_sends(&self) -> Option<Sent<'_>> {
        match self.route.rpc {
            Rpc::FilterTweets => None,
            Rpc::EvaluateTweets => Some(match &self.judgement {
                Judgement::Judged {
                    sent: Some(state), ..
                } => Sent::ResultState(state),
                Judgement::Judged { sent: None, .. }
                | Judgement::NotFound(_)
                | Judgement::Failed(_) => Sent::Failed,
            }),
        }
    }

    pub fn label(&self) -> (String, String) {
        match (self.route.rpc, &self.judgement) {
            (Rpc::FilterTweets, Judgement::Judged { state, .. }) => state_label(state),
            (Rpc::FilterTweets | Rpc::EvaluateTweets, judgement) => judgement.label(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[expect(
    clippy::large_enum_variant,
    reason = "an explanation holds one per level, and nearly every level judges"
)]
pub enum Judgement {
    #[non_exhaustive]
    Judged {
        state: TweetFieldsResultState,
        decided_by: Option<&'static str>,
        fail_open: Hydrators,
        failed: Hydrators,
        sent: Option<TweetFieldsResultState>,
    },
    NotFound(Lookup),
    Failed(Lookup),
}

impl Judgement {
    pub fn judged(state: TweetFieldsResultState, decided_by: Option<&'static str>) -> Self {
        Self::Judged {
            sent: Some(state.clone()),
            state,
            decided_by,
            fail_open: Hydrators::empty(),
            failed: Hydrators::empty(),
        }
    }

    pub fn judged_partially(
        state: TweetFieldsResultState,
        decided_by: Option<&'static str>,
        fail_open: Hydrators,
        failed: Hydrators,
    ) -> Self {
        Self::Judged {
            state,
            decided_by,
            fail_open,
            failed,
            sent: None,
        }
    }

    pub fn label(&self) -> (String, String) {
        match self {
            Self::Judged {
                sent: Some(state), ..
            } => state_label(state),
            Self::Judged { sent: None, .. } | Self::Failed(_) => label(Class::Failed, NONE),
            Self::NotFound(_) => label(Class::NotFound, NONE),
        }
    }

    pub fn hidden(&self) -> bool {
        match self {
            Self::Judged { state, .. } => match state {
                TweetFieldsResultState::Found(_) => false,
                TweetFieldsResultState::NotFound(_)
                | TweetFieldsResultState::Failed(_)
                | TweetFieldsResultState::Filtered(_) => true,
            },
            Self::NotFound(_) | Self::Failed(_) => true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sent<'a> {
    ResultState(&'a TweetFieldsResultState),
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViewerFacts {
    LoggedOut,
    NotRead {
        id: NonZeroU64,
        relationship: Relationship,
    },
    ReadFailed {
        id: NonZeroU64,
        relationship: Relationship,
    },
    NoUser {
        id: NonZeroU64,
        relationship: Relationship,
    },
    LoggedIn {
        id: NonZeroU64,
        profile: ViewerProfile,
        age_verified_18: Seen,
        relationship: Relationship,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Relationship {
    pub is_author: bool,
    pub follows: Seen,
    pub blocks: Seen,
    pub mutes: Seen,
    pub blocked_by: Seen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Seen {
    Yes,
    No,
    NotHydrated,
    Failed,
}

impl Seen {
    fn of(planned: Hydrators, fact: ExplainedFact, truth: Truth) -> Self {
        if planned.intersection(fact.reads()) != fact.reads() {
            return Self::NotHydrated;
        }
        match truth {
            Truth::Unknown { .. } => Self::Failed,
            Truth::True => Self::Yes,
            Truth::False => Self::No,
        }
    }

    fn read(reads: Hydrators, failed: Hydrators, hydrator: Hydrator, yes: bool) -> Self {
        if !reads.contains(hydrator) {
            Self::NotHydrated
        } else if failed.contains(hydrator) {
            Self::Failed
        } else if yes {
            Self::Yes
        } else {
            Self::No
        }
    }

    fn union(self, other: Self) -> Self {
        match (self, other) {
            (Self::Yes, _) | (_, Self::Yes) => Self::Yes,
            (Self::No, _) | (_, Self::No) => Self::No,
            (Self::Failed, _) | (_, Self::Failed) => Self::Failed,
            (Self::NotHydrated, Self::NotHydrated) => Self::NotHydrated,
        }
    }
}

impl Explainer {
    pub async fn connect(datacenter: &str) -> Self {
        let metadata = test_users_metadata();
        let serving = server_deps::serving(datacenter, Some(&metadata), CacheWarming::Off).await;
        Self::new(
            Arc::new(serving.sources),
            serving.fallback_caches,
            serving.country_lists,
            serving.client_switches,
            serving.limited_actions_copy,
        )
    }

    pub async fn explain<const N: usize>(&self, ask: &Ask<N>) -> Explanation<N> {
        let post = request_context(Instant::now(), None).scope(self.sources.post_card(ask.post));
        let ((viewer, in_nsfw_gating_country, levels), post) = join(self.judge(ask), post).await;
        Explanation::new(post, viewer, in_nsfw_gating_country, levels)
    }
}

struct RouteRun {
    route: Route,
    context: Option<TwitterContextViewer>,
    country_code: Option<String>,
    capability: ClientCapability,
    hydrated: HydratedRequest,
    evaluation: Evaluation,
}

impl Explainer {
    fn new(
        sources: Arc<dyn Sources>,
        fallback_caches: FallbackCaches,
        country_lists: Arc<CountryLists>,
        client_switches: ClientSwitches,
        copy: LimitedActionsCopy,
    ) -> Self {
        let rule_engine = RuleEngine::with_country_lists(country_lists);
        Self {
            sources: Arc::clone(&sources),
            filter_tweets: FilterTweets::new(sources, rule_engine, Arc::new(fallback_caches)),
            client_switches,
            copy,
        }
    }

    async fn judge<const N: usize>(&self, ask: &Ask<N>) -> (ViewerFacts, bool, [LevelOutcome; N]) {
        let evaluated = join_all(ask.routes.map(|route| self.evaluate(ask, route))).await;
        #[expect(clippy::unreachable, reason = "join_all answers each of the N routes")]
        let Ok(evaluated) = <[RouteRun; N]>::try_from(evaluated) else {
            unreachable!()
        };
        let viewer = self.viewer_facts(ask, &evaluated);
        let in_nsfw_gating_country = self.in_nsfw_gating_country(&evaluated);
        (
            viewer,
            in_nsfw_gating_country,
            evaluated.each_ref().map(|level| self.outcome(ask, level)),
        )
    }

    async fn evaluate<const N: usize>(&self, ask: &Ask<N>, route: Route) -> RouteRun {
        let viewer_id = ask.viewer.map(NonZeroU64::get);
        let candidates = vec![RawCandidate {
            tweet_id: TweetId(ask.post),
            request_author_id: None,
        }];
        let (context, request) = match route.rpc {
            Rpc::FilterTweets => (
                None,
                FilterRequest {
                    viewer_id,
                    country_code: ask.country.clone(),
                    client_capability: ClientCapability::default(),
                    safety_level: route.level,
                    candidates,
                    rpc: Rpc::FilterTweets,
                },
            ),
            Rpc::EvaluateTweets => {
                let context = ask.client.context(viewer_id, ask.country.as_deref());
                let request = evaluate_tweets_request(
                    &self.client_switches,
                    Some(&context),
                    viewer_id,
                    route.level,
                    candidates,
                );
                (Some(context), request)
            }
        };
        let capability = request.client_capability;
        let country_code = request.country_code.clone();
        request_context(Instant::now(), None)
            .scope(async {
                let (hydrated, outcomes) =
                    retweet::hydrate_and_evaluate(&self.filter_tweets, request).await;
                RouteRun {
                    route,
                    context,
                    country_code,
                    capability,
                    hydrated,
                    evaluation: outcomes
                        .into_iter()
                        .next()
                        .map_or(Evaluation::Failed(Lookup::Tweet), |outcome| {
                            outcome.evaluation
                        }),
                }
            })
            .await
    }

    fn outcome<const N: usize>(&self, ask: &Ask<N>, level: &RouteRun) -> LevelOutcome {
        let judgement = match &level.evaluation {
            Evaluation::Complete { verdict } | Evaluation::Partial { verdict, .. } => {
                let policies = self.client_switches.limited_actions_policies(
                    level.context.as_ref(),
                    ask.viewer.map(NonZeroU64::get),
                    level.country_code.as_deref(),
                    verdict.limited_engagement_reasons(),
                    &self.copy,
                    None,
                );
                let decided_by = treatment::decided_by(verdict);
                match Sends::new(&level.evaluation, level.route.level, &policies).result_state() {
                    Some(state) => Judgement::judged(state, decided_by),
                    None => Judgement::judged_partially(
                        treatment::thrift_result_state(verdict, level.route.level, &policies),
                        decided_by,
                        level.evaluation.fail_open_defaults(),
                        level.hydrated.hydration().failed_nodes(TweetId(ask.post)),
                    ),
                }
            }
            Evaluation::NotFound(lookup) => Judgement::NotFound(*lookup),
            Evaluation::Failed(lookup) => Judgement::Failed(*lookup),
        };
        LevelOutcome::new(level.route, level.capability, judgement)
    }

    fn viewer_facts<const N: usize>(&self, ask: &Ask<N>, evaluated: &[RouteRun]) -> ViewerFacts {
        let Some(id) = ask.viewer else {
            return ViewerFacts::LoggedOut;
        };
        let relationship = self.relationship(id, TweetId(ask.post), evaluated);
        let hydrations: Vec<&Hydration> = evaluated
            .iter()
            .map(|level| level.hydrated.hydration())
            .collect();
        let Some(hydration) = Self::profile_hydration(evaluated) else {
            let planned = hydrations
                .iter()
                .any(|hydration| hydration.planned().contains(Hydrator::ViewerProfile));
            return if planned {
                ViewerFacts::ReadFailed { id, relationship }
            } else {
                ViewerFacts::NotRead { id, relationship }
            };
        };
        let (Viewer::LoggedIn { profile, .. }, true) =
            (&hydration.viewer().viewer, hydration.viewer_found())
        else {
            return ViewerFacts::NoUser { id, relationship };
        };
        ViewerFacts::LoggedIn {
            id,
            profile: profile.clone(),
            age_verified_18: hydrations
                .iter()
                .map(|&hydration| {
                    let labeled = matches!(
                        hydration.viewer().viewer,
                        Viewer::LoggedIn {
                            has_age_verified_18_label: true,
                            ..
                        }
                    );
                    Seen::read(
                        hydration.planned(),
                        hydration.incomplete(),
                        Hydrator::ViewerLabels,
                        labeled,
                    )
                })
                .fold(Seen::NotHydrated, Seen::union),
            relationship,
        }
    }

    fn profile_hydration(evaluated: &[RouteRun]) -> Option<&Hydration> {
        evaluated
            .iter()
            .map(|level| level.hydrated.hydration())
            .find(|hydration| {
                hydration.planned().contains(Hydrator::ViewerProfile)
                    && !hydration.incomplete().contains(Hydrator::ViewerProfile)
            })
    }

    fn in_nsfw_gating_country(&self, evaluated: &[RouteRun]) -> bool {
        Self::profile_hydration(evaluated)
            .or_else(|| evaluated.first().map(|level| level.hydrated.hydration()))
            .is_some_and(|hydration| {
                self.filter_tweets
                    .rule_engine()
                    .in_nsfw_gating_country(hydration.viewer())
                    .resolves_true()
            })
    }

    fn relationship(
        &self,
        viewer: NonZeroU64,
        post: TweetId,
        evaluated: &[RouteRun],
    ) -> Relationship {
        let posts: Vec<(&Hydration, &PostCandidate)> = evaluated
            .iter()
            .filter_map(|level| {
                let hydration = level.hydrated.hydration();
                Some((hydration, hydration.tweet(post)?.candidate()?))
            })
            .collect();
        let rules = self.filter_tweets.rule_engine();
        let edge = |fact| {
            posts
                .iter()
                .map(|&(hydration, candidate)| {
                    let truth = rules.truth(fact, hydration.viewer(), candidate);
                    Seen::of(hydration.planned(), fact, truth)
                })
                .fold(Seen::NotHydrated, Seen::union)
        };
        Relationship {
            is_author: posts
                .iter()
                .any(|(_, candidate)| candidate.author.id == viewer.get()),
            follows: edge(ExplainedFact::Follows),
            blocks: edge(ExplainedFact::Blocks),
            mutes: edge(ExplainedFact::Mutes),
            blocked_by: edge(ExplainedFact::IsBlockedBy),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clients::socialgraph_client::Graph;
    use crate::evaluate_tweets::EvaluateTweetsEndpoint;
    use crate::hydration::plan::Source;
    use crate::hydration::sources::{control, Fault, InMemorySources};
    use crate::hydration::HydratedTweet;
    use crate::limited_actions_copy::tests::BUNDLE;
    use strum::VariantArray;
    use vf_pb::tweet_evaluation::Outcome;
    use xai_core_entities::entities::{
        ConversationControlArm, ExtendedProfile, GizmoduckAccount, GizmoduckUser,
        GizmoduckUserProfile, GizmoduckUserResult, Label, Labels, PureCoreData, Safety,
        UserResponseState,
    };
    use xai_visibility_filtering_proto as vf_pb;
    use xai_x_thrift::safety_level::SafetyLevel as ThriftLevel;
    use xai_x_thrift::safety_result::FilteredReason as ThriftFilteredReason;
    use xai_x_thrift::tweet_service::TweetFieldsResultFiltered;
    use xai_x_thrift::user_labels::LabelValue;

    const VIEWER: u64 = 50;

    fn home() -> [Route; 3] {
        [
            Route::new(Rpc::FilterTweets, SafetyLevel::TimelineHome).unwrap(),
            Route::new(Rpc::FilterTweets, SafetyLevel::TimelineHomeRecommendations).unwrap(),
            Route::new(Rpc::EvaluateTweets, SafetyLevel::TimelineHomeHydration).unwrap(),
        ]
    }

    fn ask(viewer: u64, post: u64, client: Client) -> Ask<3> {
        Ask {
            viewer: NonZeroU64::new(viewer),
            post,
            country: Some("us".to_owned()),
            client,
            routes: home(),
        }
    }

    fn explainer(sources: InMemorySources, copy: LimitedActionsCopy) -> Explainer {
        Explainer::new(
            Arc::new(sources),
            FallbackCaches::default(),
            Arc::new(CountryLists::starting_at_default()),
            ClientSwitches::for_tests(),
            copy,
        )
    }

    fn no_copy() -> LimitedActionsCopy {
        LimitedActionsCopy::from_json("[]")
    }

    fn state(outcome: &LevelOutcome) -> &TweetFieldsResultState {
        match &outcome.judgement {
            Judgement::Judged { state, .. } => state,
            Judgement::NotFound(_) | Judgement::Failed(_) => panic!("{outcome:?}"),
        }
    }

    #[test]
    fn a_route_pairs_a_level_only_with_an_rpc_that_carries_it() {
        use Rpc::{EvaluateTweets, FilterTweets};
        use SafetyLevel::*;
        let routable: Vec<_> = [FilterTweets, EvaluateTweets]
            .into_iter()
            .flat_map(|rpc| SafetyLevel::VARIANTS.iter().map(move |&level| (rpc, level)))
            .filter(|&(rpc, level)| Route::new(rpc, level).is_ok())
            .collect();
        assert_eq!(
            routable,
            [
                (FilterTweets, FilterAll),
                (FilterTweets, TimelineHome),
                (FilterTweets, TimelineHomeRecommendations),
                (FilterTweets, ImmersiveExpandedRecommendations),
                (EvaluateTweets, FilterAll),
                (EvaluateTweets, TimelineHome),
                (EvaluateTweets, TimelineHomeRecommendations),
                (EvaluateTweets, TimelineHomeHydration),
            ]
        );
        assert_eq!(
            Route::new(FilterTweets, TimelineHomeHydration),
            Err(Unroutable {
                rpc: FilterTweets,
                level: TimelineHomeHydration
            })
        );
    }

    #[tokio::test]
    async fn filter_tweets_routes_take_the_default_capability_and_evaluate_tweets_the_clients() {
        let explainer = explainer(
            InMemorySources::default().tweet(1, 10).authors(&[10]),
            no_copy(),
        );
        let outcomes = explainer
            .explain(&ask(VIEWER, 1, Client::IosOutdated))
            .await
            .levels;
        let resolved = ClientSwitches::for_tests().resolve(
            Some(&Client::IosOutdated.context(Some(VIEWER), Some("us"))),
            Some(VIEWER),
            Some("us"),
        );
        assert_eq!(
            resolved.verify_blur_support,
            Some(VerifyBlurSupport::IosNeedsUpdate)
        );
        assert_eq!(
            outcomes.map(|outcome| outcome.capability),
            [
                ClientCapability::default(),
                ClientCapability::default(),
                resolved
            ]
        );
        let mut hydrated_with = explainer
            .filter_tweets
            .client_capabilities
            .lock()
            .unwrap()
            .clone();
        hydrated_with.retain(|&capability| capability == resolved);
        assert_eq!(hydrated_with, [resolved]);
    }

    #[tokio::test]
    async fn level_82_merges_a_retweets_source_and_the_filter_tweets_levels_do_not() {
        let core = |author_id, source_tweet_id| PureCoreData {
            author_id,
            source_tweet_id,
            ..Default::default()
        };
        let suspended = GizmoduckUserResult {
            user: Some(GizmoduckUser {
                safety: Safety {
                    suspended: true,
                    ..Default::default()
                },
                ..Default::default()
            }),
            response_state: Some(UserResponseState::Found),
        };
        let explainer = explainer(
            InMemorySources::default()
                .pure_core(4, core(40, Some(6)))
                .pure_core(6, core(60, None))
                .authors(&[40])
                .user(60, suspended),
            no_copy(),
        );
        let outcomes = explainer.explain(&ask(VIEWER, 4, Client::Web)).await.levels;
        assert_eq!(
            outcomes
                .each_ref()
                .map(|outcome| outcome.judgement.hidden()),
            [false, false, true]
        );
    }

    #[tokio::test]
    async fn the_level_82_state_is_what_evaluate_tweets_sends_with_the_loaded_limited_actions_copy()
    {
        let world = || {
            InMemorySources::default()
                .tweet(1, 40)
                .authors(&[40])
                .control(1, control(ConversationControlArm::Subscribers, 40, &[]))
        };
        let ask = ask(VIEWER, 1, Client::Web);
        let with_copy = explainer(world(), LimitedActionsCopy::from_json(BUNDLE))
            .explain(&ask)
            .await
            .levels;
        let without_copy = explainer(world(), no_copy()).explain(&ask).await.levels;
        let [.., level_82] = &with_copy;
        assert_ne!(state(level_82), state(&without_copy[2]));
        assert_eq!(evaluate_tweets(world(), 1).await, [on_the_wire(level_82)]);
    }

    #[tokio::test]
    async fn level_82_sends_failed_for_a_retweet_whose_source_is_partial() {
        let world = || {
            InMemorySources::default()
                .pure_core(
                    4,
                    PureCoreData {
                        author_id: 40,
                        source_tweet_id: Some(6),
                        ..Default::default()
                    },
                )
                .pure_core(
                    6,
                    PureCoreData {
                        author_id: 60,
                        ..Default::default()
                    },
                )
                .authors(&[40, 60])
                .fail_edge(Graph::Blocks, 60)
        };
        let outcomes = explainer(world(), no_copy())
            .explain(&ask(VIEWER, 4, Client::Web))
            .await
            .levels;
        assert_eq!(outcomes[2].evaluate_tweets_sends(), Some(Sent::Failed));
        let Judgement::Judged { failed, .. } = &outcomes[2].judgement else {
            panic!("{:?}", outcomes[2]);
        };
        assert_eq!(
            *failed,
            Hydrators::of(Hydrator::Follows).with(Hydrator::BlockedByAuthor)
        );
        assert_eq!(
            evaluate_tweets(world(), 4).await,
            [on_the_wire(&outcomes[2])]
        );
    }

    fn on_the_wire(outcome: &LevelOutcome) -> Outcome {
        match outcome.evaluate_tweets_sends() {
            Some(Sent::ResultState(state)) => Outcome::ResultStateThriftCompact(
                xai_x_thrift::serialize_compact(state).unwrap().into(),
            ),
            Some(Sent::Failed) => Outcome::Failed(vf_pb::Failed {}),
            None => panic!("{outcome:?} is a FilterTweets route"),
        }
    }

    async fn evaluate_tweets(world: InMemorySources, tweet_id: u64) -> Vec<Outcome> {
        let endpoint = EvaluateTweetsEndpoint::new(
            Arc::new(FilterTweets::new(
                Arc::new(world),
                RuleEngine::for_tests(),
                Arc::default(),
            )),
            ClientSwitches::for_tests(),
            LimitedActionsCopy::from_json(BUNDLE),
        );
        let mut request = tonic::Request::new(vf_pb::EvaluateTweetsRequest {
            safety_level: ThriftLevel::TIMELINE_HOME_HYDRATION.0,
            viewer_id: Some(VIEWER),
            tweets: vec![vf_pb::TweetData {
                tweet_id,
                quote_context: None,
            }],
        });
        request.metadata_mut().insert(
            "twittercontext",
            xai_twittercontext::hydrate_twitter_context(
                &Client::Web.context(Some(VIEWER), Some("us")),
            )
            .unwrap(),
        );
        let response = endpoint.handle(request).await.unwrap().into_inner();
        response
            .results
            .into_iter()
            .filter_map(|result| result.outcome)
            .collect()
    }

    #[tokio::test]
    async fn a_retweet_names_its_sources_failed_nodes_when_its_own_failure_blocks_the_merge() {
        let world = InMemorySources::default()
            .retweet(4, 40, 6, Some(60))
            .tweet(6, 60)
            .authors(&[40, 60])
            .fail_key(Source::SafetyLabels, 4)
            .fail_key(Source::TesConversationControl, 6);
        let explainer = explainer(world, no_copy());
        let ask = ask(VIEWER, 4, Client::Web);
        let [.., route_82] = ask.routes;
        let run = explainer.evaluate(&ask, route_82).await;
        let post = run.hydrated.hydration().tweet(TweetId(4));
        assert_eq!(post.map(HydratedTweet::source_to_merge), Some(None));
        let [.., level_82] = explainer.explain(&ask).await.levels;
        let Judgement::Judged { failed, .. } = &level_82.judgement else {
            panic!("{level_82:?}");
        };
        assert!(
            failed.contains(Hydrator::TweetSafetyLabels)
                && failed.contains(Hydrator::ConversationControl),
            "{failed:?}"
        );
    }

    #[tokio::test]
    async fn viewer_facts_tell_logged_out_missing_and_logged_in_viewers_apart() {
        let world = || InMemorySources::default().tweet(1, 10).authors(&[10]);
        let facts = |sources, viewer| async move {
            explainer(sources, no_copy())
                .explain(&ask(viewer, 1, Client::Web))
                .await
                .viewer
        };
        assert_eq!(facts(world(), 0).await, ViewerFacts::LoggedOut);
        let id = NonZeroU64::new(VIEWER).unwrap();
        let stranger = Relationship {
            is_author: false,
            follows: Seen::No,
            blocks: Seen::No,
            mutes: Seen::No,
            blocked_by: Seen::No,
        };
        assert_eq!(
            facts(world(), VIEWER).await,
            ViewerFacts::NoUser {
                id,
                relationship: stranger
            }
        );
        let ViewerFacts::LoggedIn {
            profile,
            relationship,
            ..
        } = facts(
            world()
                .viewer(VIEWER, GizmoduckUser::default())
                .edge(Graph::Follows, VIEWER, 10),
            VIEWER,
        )
        .await
        else {
            panic!("a Gizmoduck user is logged in");
        };
        assert_eq!(profile.viewer_age, ViewerAge::NotStated);
        assert_eq!(
            relationship,
            Relationship {
                follows: Seen::Yes,
                ..stranger
            }
        );
        let ViewerFacts::LoggedIn { relationship, .. } =
            facts(world().viewer(10, GizmoduckUser::default()), 10).await
        else {
            panic!("the author is logged in");
        };
        assert!(relationship.is_author);
    }

    #[test]
    fn a_level_labels_a_judgement_as_its_route_sends_it() {
        let suspended = TweetFieldsResultState::Filtered(TweetFieldsResultFiltered::new(
            ThriftFilteredReason::AuthorIsSuspended(true),
        ));
        let partial = Judgement::judged_partially(
            suspended.clone(),
            None,
            Hydrators::of(Hydrator::Follows),
            Hydrators::of(Hydrator::Follows),
        );
        let label_at = |rpc, level, judgement: &Judgement| {
            LevelOutcome::new(
                Route::new(rpc, level).unwrap(),
                ClientCapability::default(),
                judgement.clone(),
            )
            .label()
        };
        assert_eq!(
            [
                label_at(Rpc::FilterTweets, SafetyLevel::TimelineHome, &partial),
                label_at(
                    Rpc::FilterTweets,
                    SafetyLevel::TimelineHomeRecommendations,
                    &partial
                ),
                label_at(
                    Rpc::EvaluateTweets,
                    SafetyLevel::TimelineHomeHydration,
                    &partial
                ),
                label_at(
                    Rpc::EvaluateTweets,
                    SafetyLevel::TimelineHomeHydration,
                    &Judgement::judged(suspended, None)
                ),
                label_at(
                    Rpc::FilterTweets,
                    SafetyLevel::TimelineHome,
                    &Judgement::NotFound(Lookup::Author)
                ),
                label_at(
                    Rpc::FilterTweets,
                    SafetyLevel::TimelineHome,
                    &Judgement::Failed(Lookup::Tweet)
                ),
            ],
            [
                ("drop", "author_is_suspended"),
                ("drop", "author_is_suspended"),
                ("failed", "none"),
                ("drop", "author_is_suspended"),
                ("not_found", "none"),
                ("failed", "none"),
            ]
            .map(|(class, reason)| (class.to_owned(), reason.to_owned()))
        );
    }

    #[tokio::test]
    async fn nsfw_gating_reads_the_account_country_then_the_request_country() {
        let gated = |account: Option<&str>, request: &str| {
            let viewer = GizmoduckUser {
                account: GizmoduckAccount {
                    country_code: account.map(str::to_owned),
                    ..Default::default()
                },
                ..Default::default()
            };
            let ask = Ask {
                country: Some(request.to_owned()),
                ..ask(VIEWER, 1, Client::Web)
            };
            async move {
                let sources = InMemorySources::default()
                    .tweet(1, 10)
                    .authors(&[10])
                    .viewer(VIEWER, viewer);
                let explanation = explainer(sources, no_copy()).explain(&ask).await;
                assert!(
                    matches!(explanation.viewer, ViewerFacts::LoggedIn { .. }),
                    "{:?}",
                    explanation.viewer
                );
                explanation.in_nsfw_gating_country
            }
        };
        assert_eq!(
            [
                gated(Some("DE"), "us").await,
                gated(Some("us"), "de").await,
                gated(None, "de").await,
                gated(None, "us").await,
            ],
            [true, false, true, false]
        );
    }

    #[tokio::test]
    async fn nsfw_gating_reads_the_request_country_for_every_viewer_without_a_profile() {
        let world = || InMemorySources::default().tweet(1, 10).authors(&[10]);
        let filter_all = Route::new(Rpc::FilterTweets, SafetyLevel::FilterAll).unwrap();
        let level_82 = Route::new(Rpc::EvaluateTweets, SafetyLevel::TimelineHomeHydration).unwrap();
        let arm = |facts: &ViewerFacts| match facts {
            ViewerFacts::LoggedOut => "logged out",
            ViewerFacts::NotRead { .. } => "not read",
            ViewerFacts::ReadFailed { .. } => "read failed",
            ViewerFacts::NoUser { .. } => "no user",
            ViewerFacts::LoggedIn { .. } => "logged in",
        };
        for (request, account, gated) in [("DE", "us", true), ("us", "de", false)] {
            let user = || {
                world().viewer(
                    VIEWER,
                    GizmoduckUser {
                        account: GizmoduckAccount {
                            country_code: Some(account.to_owned()),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                )
            };
            for (expected_arm, viewer, sources, route) in [
                ("logged out", 0, user(), level_82),
                ("not read", VIEWER, user(), filter_all),
                (
                    "read failed",
                    VIEWER,
                    user().fault(Source::GizmoduckViewer, Fault::Fails),
                    level_82,
                ),
                ("no user", VIEWER, world(), level_82),
            ] {
                let ask = Ask {
                    viewer: NonZeroU64::new(viewer),
                    post: 1,
                    country: Some(request.to_owned()),
                    client: Client::Web,
                    routes: [route],
                };
                let explanation = explainer(sources, no_copy()).explain(&ask).await;
                assert_eq!(arm(&explanation.viewer), expected_arm);
                assert_eq!(
                    explanation.in_nsfw_gating_country, gated,
                    "{expected_arm} in {request}"
                );
            }
        }
    }

    async fn viewer_on<const N: usize>(
        sources: InMemorySources,
        routes: [Route; N],
    ) -> ViewerFacts {
        explainer(sources, no_copy())
            .explain(&Ask {
                viewer: NonZeroU64::new(VIEWER),
                post: 1,
                country: Some("us".to_owned()),
                client: Client::Web,
                routes,
            })
            .await
            .viewer
    }

    #[tokio::test]
    async fn viewer_facts_say_which_routes_read_the_viewer_and_what_gizmoduck_answered() {
        let world = || InMemorySources::default().tweet(1, 10).authors(&[10]);
        let labeled = || GizmoduckUser {
            labels: Labels {
                labels: vec![Label {
                    label_value: LabelValue::AGE_VERIFIED_18.0,
                    created_at_msec: 0,
                }],
            },
            ..Default::default()
        };
        let filter_all = Route::new(Rpc::FilterTweets, SafetyLevel::FilterAll).unwrap();
        let level_8 = Route::new(Rpc::FilterTweets, SafetyLevel::TimelineHome).unwrap();
        let facts = viewer_on(world().viewer(VIEWER, labeled()), [filter_all]).await;
        assert!(matches!(facts, ViewerFacts::NotRead { .. }), "{facts:?}");
        let facts = viewer_on(
            world()
                .viewer(VIEWER, labeled())
                .fault(Source::GizmoduckViewer, Fault::Fails),
            home(),
        )
        .await;
        assert!(matches!(facts, ViewerFacts::ReadFailed { .. }), "{facts:?}");
        let facts = viewer_on(world(), home()).await;
        assert!(matches!(facts, ViewerFacts::NoUser { .. }), "{facts:?}");
        let suspended = GizmoduckUser {
            safety: Safety {
                suspended: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let ViewerFacts::LoggedIn { profile, .. } =
            viewer_on(world().viewer(VIEWER, suspended), home()).await
        else {
            panic!("a suspended viewer is a Gizmoduck user");
        };
        assert_eq!(profile.viewer_age, ViewerAge::Unknown);
        let suspended_aged_30 = GizmoduckUser {
            safety: Safety {
                suspended: true,
                ..Default::default()
            },
            extended_profile: Some(ExtendedProfile {
                age_in_years: Some(30),
            }),
            ..Default::default()
        };
        let ViewerFacts::LoggedIn { profile, .. } =
            viewer_on(world().viewer(VIEWER, suspended_aged_30), home()).await
        else {
            panic!("a suspended viewer is a Gizmoduck user");
        };
        assert_eq!(profile.viewer_age, ViewerAge::Known(30));
        let age_verified_18 = |facts| match facts {
            ViewerFacts::LoggedIn {
                age_verified_18, ..
            } => age_verified_18,
            facts @ (ViewerFacts::LoggedOut
            | ViewerFacts::NotRead { .. }
            | ViewerFacts::ReadFailed { .. }
            | ViewerFacts::NoUser { .. }) => panic!("{facts:?}"),
        };
        assert_eq!(
            [
                viewer_on(world().viewer(VIEWER, labeled()), [level_8]).await,
                viewer_on(world().viewer(VIEWER, labeled()), home()).await,
                viewer_on(world().viewer(VIEWER, GizmoduckUser::default()), home()).await,
            ]
            .map(age_verified_18),
            [Seen::NotHydrated, Seen::Yes, Seen::No]
        );
    }

    #[tokio::test]
    async fn the_post_card_shows_the_text_and_the_authors_profile_from_the_sources() {
        let author = GizmoduckUserResult {
            user: Some(GizmoduckUser {
                profile: GizmoduckUserProfile {
                    name: "Ada".to_owned(),
                    screen_name: "ada".to_owned(),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };
        let sources = InMemorySources::default()
            .pure_core(
                1,
                PureCoreData {
                    author_id: 10,
                    text: "hello".to_owned(),
                    ..Default::default()
                },
            )
            .user(10, author);
        let explanation = explainer(sources, no_copy())
            .explain(&ask(VIEWER, 1, Client::Web))
            .await;
        assert_eq!(
            explanation.post,
            Some(PostCard {
                id: 1,
                text: "hello".to_owned(),
                author: Author {
                    id: 10,
                    name: Some("Ada".to_owned()),
                    screen_name: Some("ada".to_owned()),
                    avatar_url: None,
                },
                media: Vec::new(),
            })
        );
        let missing = explainer(InMemorySources::default(), no_copy());
        assert_eq!(
            missing.explain(&ask(VIEWER, 1, Client::Web)).await.post,
            None
        );
        let tes_fails = InMemorySources::default()
            .pure_core(
                1,
                PureCoreData {
                    author_id: 10,
                    ..Default::default()
                },
            )
            .fault(Source::TesPureCoreData, Fault::Fails);
        assert_eq!(
            explainer(tes_fails, no_copy())
                .explain(&ask(VIEWER, 1, Client::Web))
                .await
                .post,
            None
        );
    }

    #[tokio::test]
    async fn a_failed_node_on_the_post_is_named_and_labelled_as_each_route_sends_it() {
        let world = InMemorySources::default()
            .tweet(1, 10)
            .authors(&[10])
            .fault(Source::SafetyLabels, Fault::Fails);
        let [_, level_61, level_82] = explainer(world, no_copy())
            .explain(&ask(VIEWER, 1, Client::Web))
            .await
            .levels;
        for level in [&level_61, &level_82] {
            let Judgement::Judged { failed, sent, .. } = &level.judgement else {
                panic!("{level:?}");
            };
            assert_eq!(
                (*failed, sent.is_none()),
                (Hydrators::of(Hydrator::TweetSafetyLabels), true),
                "{level:?}"
            );
        }
        assert_eq!(level_61.evaluate_tweets_sends(), None);
        assert_eq!(level_61.label(), ("allow".to_owned(), "none".to_owned()));
        assert_eq!(level_82.evaluate_tweets_sends(), Some(Sent::Failed));
        assert_eq!(level_82.label(), ("failed".to_owned(), "none".to_owned()));
    }

    #[tokio::test]
    async fn the_viewer_profile_comes_from_a_route_whose_plan_read_it() {
        let explainer = explainer(
            InMemorySources::default()
                .tweet(1, 10)
                .authors(&[10])
                .viewer(VIEWER, GizmoduckUser::default()),
            no_copy(),
        );
        let unread = Route::new(Rpc::FilterTweets, SafetyLevel::FilterAll).unwrap();
        let read = Route::new(Rpc::EvaluateTweets, SafetyLevel::TimelineHomeHydration).unwrap();
        let ask = Ask {
            viewer: NonZeroU64::new(VIEWER),
            post: 1,
            country: Some("us".to_owned()),
            client: Client::Web,
            routes: [unread, read],
        };
        let reads: Vec<bool> = join_all(ask.routes.map(|route| explainer.evaluate(&ask, route)))
            .await
            .iter()
            .map(|run| {
                run.hydrated
                    .hydration()
                    .planned()
                    .contains(Hydrator::ViewerProfile)
            })
            .collect();
        assert_eq!(reads, [false, true]);
        let facts = explainer.explain(&ask).await.viewer;
        let ViewerFacts::LoggedIn { profile, .. } = facts else {
            panic!("{facts:?}");
        };
        assert_eq!(profile.viewer_age, ViewerAge::NotStated);
    }
}
