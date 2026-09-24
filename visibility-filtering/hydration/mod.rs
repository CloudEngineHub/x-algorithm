pub mod batch;
pub mod conversation_control_hydrator;
pub mod exclusive_content_hydrator;
pub(crate) mod fallback_cache;
pub mod gizmoduck_hydrator;
pub mod metrics;
pub mod safety_label_hydrator;
pub mod socialgraph_hydrator;
pub mod tes_composite;
pub mod tes_hydrator;
pub mod viewer_hydrator;

use crate::clients::gizmoduck_client::GizmoduckLookup;
use crate::clients::socialgraph_client::SocialgraphClient;
use crate::models::{
    assemble, resolve_candidates, AuthorFeatures, AuthorId, ConversationControlFeatures,
    ExclusiveContentFeatures, HydratedTweetCandidate, PureCore, RawCandidate, SafetyLabelMap,
    TweetCandidateInput, TweetFeatures, TweetId, Viewer, ViewerAuthorRelationship, ViewerFeatures,
    ViewerProfile,
};
use crate::rules::{RuleEngine, SafetyLevel};
use crate::safety_label_source::SafetyLabelSource;
use batch::{Completeness, Hydrated, TweetHydrationBatch};
use conversation_control_hydrator::ConversationControlHydrator;
use exclusive_content_hydrator::ExclusiveContentHydrator;
use fallback_cache::FallbackCache;
use gizmoduck_hydrator::GizmoduckAuthorHydrator;
use safety_label_hydrator::{SafetyLabelHydration, SafetyLabelHydrator};
use socialgraph_hydrator::SocialgraphHydrator;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tes_composite::TweetForVisibilitySource;
use tes_hydrator::{PureCoreFallbackCache, TesHydrator};
use viewer_hydrator::ViewerHydrator;
use xai_core_entities::gizmoduck_client::GizmoduckClient;
use xai_core_entities::tweet_entity_service_client::TESClient;
use xai_visibility_filtering_proto as vf_pb;

pub(crate) const HYDRATION_TIMEOUT: Duration = Duration::from_secs(1);
pub(crate) const INBOUND_ALLOWANCE: Duration = Duration::from_millis(10);

pub(crate) fn request_context(
    entered: tokio::time::Instant,
    grpc_timeout: Option<Duration>,
) -> xai_x_rpc::CallContext {
    xai_x_rpc::CallContext {
        deadline: Some(
            entered
                + grpc_timeout
                    .unwrap_or(HYDRATION_TIMEOUT)
                    .saturating_sub(INBOUND_ALLOWANCE),
        ),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hydrator {
    ViewerProfile,
    TweetSafetyLabels,
    Tweet,
    ExclusiveContent,
    Author,
    Relationship,
    ConversationControl,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hydrators(u8);

impl Hydrators {
    pub const fn empty() -> Self {
        Self(0)
    }

    #[cfg(test)]
    pub const fn all() -> Self {
        Self::empty()
            .with(Hydrator::ViewerProfile)
            .with(Hydrator::TweetSafetyLabels)
            .with(Hydrator::Tweet)
            .with(Hydrator::ExclusiveContent)
            .with(Hydrator::Author)
            .with(Hydrator::Relationship)
            .with(Hydrator::ConversationControl)
    }

    pub const fn of(hydrator: Hydrator) -> Self {
        Self(1 << hydrator as u8)
    }

    pub const fn with(self, hydrator: Hydrator) -> Self {
        self.union(Self::of(hydrator))
    }

    #[cfg(test)]
    pub const fn without(self, hydrator: Hydrator) -> Self {
        Self(self.0 & !Self::of(hydrator).0)
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn contains(self, hydrator: Hydrator) -> bool {
        self.0 & Self::of(hydrator).0 != 0
    }
}

pub(crate) struct HydrationRequest<'a> {
    viewer_id: Option<u64>,
    country_code: Option<String>,
    raw_candidates: &'a [RawCandidate],
    safety_level: SafetyLevel,
}

impl<'a> HydrationRequest<'a> {
    pub(crate) fn new(
        viewer_id: Option<u64>,
        country_code: Option<String>,
        raw_candidates: &'a [RawCandidate],
        safety_level: SafetyLevel,
    ) -> Self {
        Self {
            viewer_id,
            country_code,
            raw_candidates,
            safety_level,
        }
    }
}

struct CandidateFeatures {
    tweet_features: HashMap<TweetId, TweetFeatures>,
    author_features: TweetHydrationBatch<Completeness<AuthorFeatures>>,
    safety_labels: HashMap<TweetId, SafetyLabelMap>,
    relationships: TweetHydrationBatch<ViewerAuthorRelationship>,
    exclusive_content: HashMap<TweetId, Completeness<ExclusiveContentFeatures>>,
    conversation_control: TweetHydrationBatch<Completeness<ConversationControlFeatures>>,
}

impl CandidateFeatures {
    fn assemble(self, candidates: &[TweetCandidateInput]) -> Vec<HydratedTweetCandidate> {
        candidates
            .iter()
            .map(|c| {
                assemble(
                    c,
                    self.tweet_features
                        .get(&c.tweet_id)
                        .cloned()
                        .unwrap_or_default(),
                    self.author_features
                        .get_or_default(&c.tweet_id)
                        .into_value(),
                    self.safety_labels
                        .get(&c.tweet_id)
                        .cloned()
                        .unwrap_or_default(),
                    self.relationships.get_or_default(&c.tweet_id),
                    self.exclusive_content
                        .get(&c.tweet_id)
                        .map(|exclusive| exclusive.value().clone()),
                    self.conversation_control
                        .get(&c.tweet_id)
                        .map(|control| control.value().clone()),
                )
            })
            .collect()
    }
}

pub(crate) fn tweets_per_author(candidates: &[TweetCandidateInput]) -> HashMap<AuthorId, usize> {
    let mut candidate_count_by_key = HashMap::with_capacity(candidates.len());
    for candidate in candidates {
        *candidate_count_by_key
            .entry(candidate.author_id)
            .or_default() += 1;
    }
    candidate_count_by_key
}

pub(crate) fn keyed_by_author<V>(
    expected: &HashMap<AuthorId, usize>,
    response: HashMap<u64, V>,
) -> HashMap<AuthorId, V> {
    let author_by_raw: HashMap<u64, AuthorId> = expected
        .keys()
        .map(|&author| (author.get(), author))
        .collect();
    response
        .into_iter()
        .filter_map(|(id, value)| author_by_raw.get(&id).map(|&author| (author, value)))
        .collect()
}

pub(crate) struct HydrationPipeline {
    viewer_hydrator: ViewerHydrator,
    tes_hydrator: TesHydrator,
    gizmoduck_author_hydrator: GizmoduckAuthorHydrator,
    socialgraph_hydrator: SocialgraphHydrator,
    safety_label_hydrator: SafetyLabelHydrator,
    exclusive_content_hydrator: ExclusiveContentHydrator,
    conversation_control_hydrator: ConversationControlHydrator,
}

pub(crate) struct HydrationOutput {
    pub(crate) viewer_features: ViewerFeatures,
    pub(crate) candidates: Vec<HydratedTweetCandidate>,
    pub(crate) safety_labels: HashMap<TweetId, Arc<vf_pb::SafetyLabelMap>>,
    pub(crate) failed_ids: HashSet<TweetId>,
    pub(crate) pure_cores: TweetHydrationBatch<PureCore>,
}

impl HydrationPipeline {
    pub(crate) fn new(
        tes_client: Arc<dyn TESClient + Send + Sync>,
        tweet_source: Arc<dyn TweetForVisibilitySource>,
        gizmoduck_client: Arc<dyn GizmoduckClient + Send + Sync>,
        socialgraph_client: Arc<dyn SocialgraphClient + Send + Sync>,
        safety_label_source: Arc<SafetyLabelSource>,
        fallback_cache: Option<FallbackCache<AuthorId, Completeness<AuthorFeatures>>>,
        pure_core_fallback_cache: Option<PureCoreFallbackCache>,
    ) -> Self {
        Self {
            viewer_hydrator: ViewerHydrator {
                gizmoduck_client: gizmoduck_client.clone(),
            },
            tes_hydrator: TesHydrator::new(
                tes_client.clone(),
                tweet_source,
                pure_core_fallback_cache,
            ),
            gizmoduck_author_hydrator: GizmoduckAuthorHydrator::new(
                GizmoduckLookup::new(gizmoduck_client),
                fallback_cache,
            ),
            socialgraph_hydrator: SocialgraphHydrator {
                sg_client: socialgraph_client.clone(),
            },
            safety_label_hydrator: SafetyLabelHydrator {
                source: safety_label_source,
            },
            exclusive_content_hydrator: ExclusiveContentHydrator {
                sg_client: socialgraph_client.clone(),
            },
            conversation_control_hydrator: ConversationControlHydrator {
                tes_client,
                sg_client: socialgraph_client,
            },
        }
    }

    pub(crate) async fn hydrate(&self, request: HydrationRequest<'_>) -> HydrationOutput {
        self.hydrate_with(RuleEngine::hydrators_for(request.safety_level), request)
            .await
    }

    async fn hydrate_with(
        &self,
        hydrators: Hydrators,
        request: HydrationRequest<'_>,
    ) -> HydrationOutput {
        let HydrationRequest {
            viewer_id,
            country_code,
            raw_candidates,
            safety_level,
        } = request;
        let viewer_hydration = async {
            match (viewer_id, hydrators.contains(Hydrator::ViewerProfile)) {
                (None, _) => Completeness::Complete(Viewer::LoggedOut),
                (Some(id), false) => Completeness::Complete(Viewer::LoggedIn {
                    id,
                    profile: ViewerProfile::default(),
                }),
                (Some(id), true) => self
                    .viewer_hydrator
                    .hydrate(id, safety_level)
                    .await
                    .map(|profile| Viewer::LoggedIn { id, profile }),
            }
        };
        let candidate_hydration = async {
            let tweet_ids: Vec<TweetId> = raw_candidates.iter().map(|c| c.tweet_id).collect();
            let tes_started = Instant::now();
            let (exclusive_tx, exclusive_rx) = tokio::sync::oneshot::channel();
            let independent_group = async {
                tokio::join!(
                    async {
                        if hydrators.contains(Hydrator::TweetSafetyLabels) {
                            self.safety_label_hydrator
                                .hydrate(&tweet_ids, safety_level)
                                .await
                        } else {
                            SafetyLabelHydration::default()
                        }
                    },
                    async {
                        if !hydrators.contains(Hydrator::Tweet) {
                            return (TweetHydrationBatch::default(), None);
                        }
                        let tweets = self
                            .tes_hydrator
                            .hydrate_tweets(&tweet_ids, safety_level)
                            .await;
                        let tes_elapsed = tes_started.elapsed();
                        let conversation_authors = tweet_ids
                            .iter()
                            .filter_map(|id| {
                                tweets
                                    .get(id)?
                                    .exclusive_conversation_author_id
                                    .map(|author| (*id, author))
                            })
                            .collect();
                        let _ = exclusive_tx.send(conversation_authors);
                        (tweets, Some(tes_elapsed))
                    },
                    async {
                        if !hydrators.contains(Hydrator::ExclusiveContent) {
                            return HashMap::new();
                        }
                        let conversation_authors = exclusive_rx.await.unwrap_or_default();
                        self.exclusive_content_hydrator
                            .hydrate(
                                conversation_authors,
                                tweet_ids.len(),
                                viewer_id,
                                safety_level,
                            )
                            .await
                    },
                    async {
                        if !hydrators.contains(Hydrator::ConversationControl) {
                            return TweetHydrationBatch::default();
                        }
                        self.conversation_control_hydrator
                            .hydrate(&tweet_ids, viewer_id, safety_level)
                            .await
                    },
                )
            };

            let author_hop = async {
                let pure_cores = self
                    .tes_hydrator
                    .fetch_pure_core(&tweet_ids, safety_level)
                    .await;
                let tes_elapsed = tes_started.elapsed();
                let candidates = resolve_candidates(raw_candidates, &pure_cores);
                let (author_features, relationships) = tokio::join!(
                    async {
                        if hydrators.contains(Hydrator::Author) {
                            self.gizmoduck_author_hydrator
                                .hydrate(&candidates, safety_level)
                                .await
                        } else {
                            TweetHydrationBatch::default()
                        }
                    },
                    async {
                        if hydrators.contains(Hydrator::Relationship) {
                            self.socialgraph_hydrator
                                .hydrate(&candidates, viewer_id, safety_level)
                                .await
                        } else {
                            TweetHydrationBatch::default()
                        }
                    },
                );
                (
                    pure_cores,
                    candidates,
                    author_features,
                    relationships,
                    tes_elapsed,
                )
            };

            let (
                (
                    safety_labels,
                    (tes_tweet_keyed, composite_elapsed),
                    exclusive_content,
                    conversation_control,
                ),
                (pure_cores, candidates, author_features, relationships, core_elapsed),
            ) = tokio::join!(independent_group, author_hop);
            metrics::record_tes_join_latency(
                safety_level,
                composite_elapsed.map_or(core_elapsed, |composite| core_elapsed.max(composite)),
            );

            let SafetyLabelHydration {
                label_types,
                label_response,
            } = safety_labels;

            let tweet_features = self
                .tes_hydrator
                .assemble_tweet_features(&candidates, &tes_tweet_keyed);

            let failed_ids: HashSet<TweetId> = candidates
                .iter()
                .map(|candidate| candidate.tweet_id)
                .filter(|id| {
                    pure_cores.is_failed(id)
                        || (hydrators.contains(Hydrator::Author)
                            && !matches!(
                                author_features.hydrated(id),
                                Some(
                                    Hydrated::Found(Completeness::Complete(_)) | Hydrated::NotFound
                                )
                            ))
                        || (hydrators.contains(Hydrator::Relationship)
                            && relationships.is_failed(id))
                        || (hydrators.contains(Hydrator::Tweet) && tes_tweet_keyed.is_failed(id))
                        || (hydrators.contains(Hydrator::TweetSafetyLabels)
                            && !label_response.contains_key(id))
                        || exclusive_content
                            .get(id)
                            .is_some_and(|exclusive| !exclusive.is_complete())
                        || (hydrators.contains(Hydrator::ConversationControl)
                            && !matches!(
                                conversation_control.hydrated(id),
                                Some(
                                    Hydrated::Found(Completeness::Complete(_)) | Hydrated::NotFound
                                )
                            ))
                })
                .collect();
            let features = CandidateFeatures {
                tweet_features,
                author_features,
                safety_labels: label_types,
                relationships,
                exclusive_content,
                conversation_control,
            };
            let hydrated_candidates = features.assemble(&candidates);

            (hydrated_candidates, label_response, failed_ids, pure_cores)
        };

        let (viewer, (candidates, safety_labels, mut failed_ids, pure_cores)) =
            tokio::join!(viewer_hydration, candidate_hydration);
        if !viewer.is_complete() {
            failed_ids.extend(candidates.iter().map(|c| TweetId(c.tweet_id)));
        }

        HydrationOutput {
            viewer_features: ViewerFeatures::from_request(viewer.into_value(), country_code),
            candidates,
            safety_labels,
            failed_ids,
            pure_cores,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clients::socialgraph_client::FakeSocialgraphClient;
    use crate::filter::test_support::{
        exclusive_tweet, safety_labels, safety_labels_with_manhattan, PendingTes,
    };
    use crate::hydration::tes_composite::MockTweetForVisibilitySource;
    use crate::hydration::viewer_hydrator::tests::{BrokenViewerClient, ViewerLookup};
    use crate::safety_label_source::lookup::{LookupError, ManhattanLookup};
    use crate::safety_label_source::types::{FailureKind, ManhattanOutcome};
    use xai_core_entities::entities::{
        ConversationControl, ConversationControlArm, GizmoduckUserResult, PureCoreData,
        UserResponseState,
    };
    use xai_core_entities::gizmoduck_client::{GizmoduckClient, MockGizmoduckClient, ViewerData};
    use xai_core_entities::tweet_entity_service_client::{MockTESClient, TESClient};

    #[test]
    fn request_context_budget_is_header_or_hang_guard_minus_allowance() {
        let entered = tokio::time::Instant::now();

        let header = Duration::from_millis(400);
        assert_eq!(
            request_context(entered, Some(header)).deadline,
            Some(entered + header - INBOUND_ALLOWANCE)
        );
        assert_eq!(
            request_context(entered, None).deadline,
            Some(entered + HYDRATION_TIMEOUT - INBOUND_ALLOWANCE)
        );
    }

    #[test]
    fn assemble_handles_mismatched_cardinality_without_mispairing() {
        let candidates = vec![TweetCandidateInput {
            tweet_id: TweetId(2),
            author_id: AuthorId(200),
        }];

        let results = CandidateFeatures {
            tweet_features: HashMap::from([
                (TweetId(1), TweetFeatures::default()),
                (
                    TweetId(2),
                    TweetFeatures {
                        source_tweet_id: Some(2),
                        ..Default::default()
                    },
                ),
            ]),
            author_features: TweetHydrationBatch::from_results(
                [TweetId(2)],
                HashMap::from([(
                    TweetId(2),
                    Ok::<_, anyhow::Error>(Some(Completeness::Complete(AuthorFeatures {
                        is_suspended: true,
                        ..Default::default()
                    }))),
                )]),
            ),
            safety_labels: HashMap::from([
                (TweetId(1), SafetyLabelMap::default()),
                (TweetId(2), SafetyLabelMap::default()),
            ]),
            relationships: TweetHydrationBatch::from_results(
                [TweetId(2)],
                HashMap::from([(
                    TweetId(2),
                    Ok::<_, anyhow::Error>(Some(ViewerAuthorRelationship {
                        viewer_follows_author: true,
                        ..Default::default()
                    })),
                )]),
            ),
            exclusive_content: HashMap::new(),
            conversation_control: TweetHydrationBatch::default(),
        };

        let assembled = results.assemble(&candidates);

        assert_eq!(assembled.len(), 1);
        let c = &assembled[0];
        assert_eq!(c.tweet_id, 2);
        assert_eq!(c.author_id, 200);
        assert_eq!(c.tweet_features.source_tweet_id, Some(2));
        assert!(c.author_features.is_suspended);
        assert!(c.relationship.viewer_follows_author);
    }

    fn raw(tweet_id: u64, request_author_id: Option<u64>) -> RawCandidate {
        RawCandidate {
            tweet_id: TweetId(tweet_id),
            request_author_id,
        }
    }

    fn pipeline_with_viewer_data(id: u64, data: ViewerData) -> HydrationPipeline {
        let mut gizmoduck = MockGizmoduckClient::default();
        gizmoduck.viewer_data.insert(id, data);
        HydrationPipeline::new(
            Arc::new(MockTESClient::default()),
            Arc::new(MockTweetForVisibilitySource::default()),
            Arc::new(gizmoduck),
            Arc::new(FakeSocialgraphClient),
            safety_labels(),
            None,
            None,
        )
    }

    #[tokio::test]
    async fn logged_out_viewer_carries_no_profile_and_a_lower_cased_request_country() {
        let pipeline = pipeline_with_viewer_data(50, ViewerData::default());
        let request = HydrationRequest::new(None, Some("US".into()), &[], SafetyLevel::FilterAll);

        let viewer = pipeline
            .hydrate_with(Hydrators::of(Hydrator::ViewerProfile), request)
            .await
            .viewer_features;

        assert_eq!(viewer.viewer, Viewer::LoggedOut);
        assert_eq!(viewer.country_code.as_deref(), Some("us"));
    }

    #[tokio::test]
    async fn logged_in_viewer_gets_the_gizmoduck_profile_only_when_its_level_derives_it() {
        let pipeline = &pipeline_with_viewer_data(
            50,
            ViewerData {
                user_exists: true,
                age_in_years: Some(30),
                nsfw_view: Some(true),
                ..Default::default()
            },
        );
        let hydrate = |hydrators| async move {
            pipeline
                .hydrate_with(
                    hydrators,
                    HydrationRequest::new(Some(50), None, &[], SafetyLevel::FilterAll),
                )
                .await
                .viewer_features
                .viewer
        };

        assert_eq!(
            hydrate(Hydrators::empty()).await,
            Viewer::LoggedIn {
                id: 50,
                profile: ViewerProfile::default(),
            }
        );
        let Viewer::LoggedIn { id: 50, profile } =
            hydrate(Hydrators::of(Hydrator::ViewerProfile)).await
        else {
            panic!("a logged-in request stays logged in")
        };
        assert_ne!(profile, ViewerProfile::default());
    }

    struct DegradedSocialgraph;

    #[tonic::async_trait]
    impl SocialgraphClient for DegradedSocialgraph {
        async fn batch_check_relationships(
            &self,
            _: u64,
            authors: &[u64],
        ) -> HashMap<u64, ViewerAuthorRelationship> {
            authors
                .iter()
                .filter(|&&author| author != 10)
                .map(|&author| (author, ViewerAuthorRelationship::default()))
                .collect()
        }

        async fn batch_check_super_follows(&self, _: u64, _: &[u64]) -> Option<HashMap<u64, bool>> {
            None
        }

        async fn batch_check_followed_by(&self, _: u64, _: &[u64]) -> Option<HashMap<u64, bool>> {
            None
        }
    }

    struct FailingManhattan;

    #[tonic::async_trait]
    impl ManhattanLookup for FailingManhattan {
        async fn get(&self, ids: &[u64]) -> HashMap<u64, ManhattanOutcome> {
            ids.iter()
                .map(|&id| {
                    (
                        id,
                        ManhattanOutcome::Failure(LookupError::new(
                            FailureKind::ManhattanFetch,
                            "manhattan unavailable",
                        )),
                    )
                })
                .collect()
        }
    }

    fn tes(
        core: &[(u64, u64)],
        conversation_controls: HashMap<u64, Option<ConversationControl>>,
    ) -> Arc<MockTESClient> {
        Arc::new(MockTESClient {
            core_data: core
                .iter()
                .map(|&(tweet_id, author_id)| {
                    (
                        tweet_id,
                        Some(PureCoreData {
                            author_id,
                            ..Default::default()
                        }),
                    )
                })
                .collect(),
            conversation_controls,
            ..Default::default()
        })
    }

    struct Deps {
        tes: Arc<dyn TESClient + Send + Sync>,
        composite: MockTweetForVisibilitySource,
        gizmoduck: Arc<dyn GizmoduckClient + Send + Sync>,
        socialgraph: Arc<dyn SocialgraphClient + Send + Sync>,
        labels: Arc<SafetyLabelSource>,
    }

    impl Default for Deps {
        fn default() -> Self {
            Self {
                tes: tes(&[(1, 10), (2, 20)], HashMap::new()),
                composite: MockTweetForVisibilitySource::default(),
                gizmoduck: Arc::new(MockGizmoduckClient::default()),
                socialgraph: Arc::new(FakeSocialgraphClient),
                labels: safety_labels(),
            }
        }
    }

    impl Deps {
        fn pipeline(self) -> HydrationPipeline {
            HydrationPipeline::new(
                self.tes,
                Arc::new(self.composite),
                self.gizmoduck,
                self.socialgraph,
                self.labels,
                None,
                None,
            )
        }
    }

    #[tokio::test(start_paused = true)]
    async fn failed_ids_reports_exactly_the_candidates_each_clause_flags() {
        let community = ConversationControl {
            arm: ConversationControlArm::Community,
            conversation_tweet_author_id: 30,
            invited_user_ids: vec![],
            invite_via_mention: None,
            allowed_country_codes: vec![],
        };
        let rows = [
            ("healthy", Hydrators::all(), Deps::default(), vec![]),
            (
                "failed pure core",
                Hydrators::empty(),
                Deps {
                    tes: Arc::new(PendingTes),
                    ..Default::default()
                },
                vec![1, 2],
            ),
            (
                "incomplete author",
                Hydrators::of(Hydrator::Author),
                Deps {
                    gizmoduck: Arc::new(MockGizmoduckClient {
                        users: HashMap::from([(
                            10,
                            Some(GizmoduckUserResult {
                                user: None,
                                response_state: Some(UserResponseState::Failed),
                            }),
                        )]),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                vec![1],
            ),
            (
                "failed relationships",
                Hydrators::of(Hydrator::Relationship),
                Deps {
                    socialgraph: Arc::new(DegradedSocialgraph),
                    ..Default::default()
                },
                vec![1],
            ),
            (
                "failed tweet-keyed TES",
                Hydrators::of(Hydrator::Tweet),
                Deps {
                    composite: MockTweetForVisibilitySource {
                        errors: HashMap::from([(1, "tes unavailable".into())]),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                vec![1],
            ),
            (
                "missing label response",
                Hydrators::of(Hydrator::TweetSafetyLabels),
                Deps {
                    labels: safety_labels_with_manhattan(Arc::new(FailingManhattan)),
                    ..Default::default()
                },
                vec![1],
            ),
            (
                "incomplete exclusive content",
                Hydrators::of(Hydrator::Tweet).with(Hydrator::ExclusiveContent),
                Deps {
                    composite: MockTweetForVisibilitySource {
                        tweets: HashMap::from([(1, Some(exclusive_tweet()))]),
                        ..Default::default()
                    },
                    socialgraph: Arc::new(DegradedSocialgraph),
                    ..Default::default()
                },
                vec![1],
            ),
            (
                "incomplete conversation control",
                Hydrators::of(Hydrator::ConversationControl),
                Deps {
                    tes: tes(&[(1, 10), (2, 20)], HashMap::from([(1, Some(community))])),
                    socialgraph: Arc::new(DegradedSocialgraph),
                    ..Default::default()
                },
                vec![1],
            ),
            (
                "incomplete viewer",
                Hydrators::of(Hydrator::ViewerProfile),
                Deps {
                    gizmoduck: Arc::new(BrokenViewerClient(ViewerLookup::Fails)),
                    ..Default::default()
                },
                vec![1, 2],
            ),
        ];
        let raw = [raw(1, Some(10)), raw(2, Some(20))];
        for (name, hydrators, deps, expected) in rows {
            let hydrated = deps
                .pipeline()
                .hydrate_with(
                    hydrators,
                    HydrationRequest::new(Some(50), None, &raw, SafetyLevel::TimelineHome),
                )
                .await;
            assert_eq!(
                hydrated.failed_ids,
                expected.into_iter().map(TweetId).collect::<HashSet<_>>(),
                "{name}"
            );
        }
    }

    #[tokio::test]
    async fn conversation_control_arm_populates_the_candidate_and_fails_it_on_a_lost_edge() {
        struct LostEdge;

        #[tonic::async_trait]
        impl SocialgraphClient for LostEdge {
            async fn batch_check_relationships(
                &self,
                _: u64,
                _: &[u64],
            ) -> HashMap<u64, ViewerAuthorRelationship> {
                unreachable!("the conversation-control arm reads no viewer→author edge")
            }

            async fn batch_check_super_follows(
                &self,
                _: u64,
                _: &[u64],
            ) -> Option<HashMap<u64, bool>> {
                unreachable!("no Subscribers-arm tweet in the batch")
            }

            async fn batch_check_followed_by(
                &self,
                _: u64,
                _: &[u64],
            ) -> Option<HashMap<u64, bool>> {
                None
            }
        }

        let control = ConversationControl {
            arm: ConversationControlArm::Community,
            conversation_tweet_author_id: 30,
            invited_user_ids: vec![],
            invite_via_mention: None,
            allowed_country_codes: vec![],
        };
        let tes = Arc::new(MockTESClient {
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
            conversation_controls: HashMap::from([(1, Some(control)), (2, None)]),
            ..Default::default()
        });
        let raw = [raw(1, None), raw(2, None)];
        let request = || HydrationRequest::new(Some(50), None, &raw, SafetyLevel::FilterAll);
        let pipeline = |sg: Arc<dyn SocialgraphClient + Send + Sync>| {
            HydrationPipeline::new(
                tes.clone(),
                Arc::new(MockTweetForVisibilitySource::default()),
                Arc::new(MockGizmoduckClient::default()),
                sg,
                crate::filter::test_support::safety_labels(),
                None,
                None,
            )
        };

        let hydrated = pipeline(Arc::new(FakeSocialgraphClient))
            .hydrate_with(Hydrators::of(Hydrator::ConversationControl), request())
            .await;
        assert!(hydrated.candidates[0].conversation_control.is_some());
        assert!(hydrated.candidates[1].conversation_control.is_none());
        assert!(hydrated.failed_ids.is_empty());

        let lost = pipeline(Arc::new(LostEdge))
            .hydrate_with(Hydrators::of(Hydrator::ConversationControl), request())
            .await;
        assert!(lost.candidates[0].conversation_control.is_some());
        assert_eq!(lost.failed_ids, HashSet::from([TweetId(1)]));
    }
}
