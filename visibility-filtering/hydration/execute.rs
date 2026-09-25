use crate::clients::socialgraph_client::{EdgeDirection, EdgeQuery, Graph};
use crate::hydration::batch::{AuthorHydrationBatch, Completeness, Hydrated, TweetHydrationBatch};
use crate::hydration::gizmoduck_hydrator::{decode_authors, DecodedAuthor};
use crate::hydration::metrics::{
    self, batch_outcome, record_batch_size, record_hydrator_request, record_viewer_country,
    timed_all_or_none, timed_results, timed_rpc, HydratorOutcome,
};
use crate::hydration::plan::{Group, KeyOrigin, Part, Source};
use crate::hydration::safety_label_hydrator::SafetyLabelHydration;
use crate::hydration::sources::Sources;
use crate::hydration::tes_composite::TweetForVisibility;
use crate::hydration::tes_hydrator::{build_tweet_features, candidates_per_tweet, pure_core};
use crate::hydration::viewer_hydrator::viewer_profile;
use crate::hydration::{
    keyed_by_author, tweets_per_author, HydrationOutput, HydrationPlan, HydrationRequest, Hydrator,
    HYDRATION_TIMEOUT,
};
use crate::models::{
    resolve_candidates, ConversationControlFeatures, HydratedTweetCandidate, PureCore,
    RawCandidate, TweetCandidateInput, TweetId, Viewer, ViewerFeatures, ViewerProfile,
};
use futures::stream::{FuturesUnordered, StreamExt};
use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::warn;
use xai_core_entities::entities::{ConversationControl, ConversationControlArm};

#[derive(Debug, Default, strum::IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
enum ViewerCountry {
    NoAllowedList,
    Found(Arc<str>),
    NoRow,
    #[default]
    Failed,
}

enum Reply {
    PureCores(TweetHydrationBatch<PureCore>),
    Tweets(TweetHydrationBatch<TweetForVisibility>),
    Controls(TweetHydrationBatch<ConversationControl>),
    Labels(SafetyLabelHydration),
    Viewer(Completeness<ViewerProfile>),
    Authors(AuthorHydrationBatch<Completeness<DecodedAuthor>>),
    Edges(Option<Vec<HashSet<u64>>>),
    ViewerCountry(ViewerCountry),
}

struct Select<'a> {
    group: &'a Group,
    queries: Vec<(Graph, EdgeDirection, Vec<KeyOrigin>)>,
    answers: Option<Vec<HashSet<u64>>>,
}

type Call<'a> = Pin<Box<dyn Future<Output = (&'a Group, Reply)> + Send + 'a>>;

#[derive(Default)]
struct Store<'a> {
    viewer_id: Option<u64>,
    tweet_ids: Vec<TweetId>,
    pure_cores: TweetHydrationBatch<PureCore>,
    candidates: Vec<TweetCandidateInput>,
    tweets: Option<TweetHydrationBatch<TweetForVisibility>>,
    controls: Option<TweetHydrationBatch<ConversationControl>>,
    labels: Option<SafetyLabelHydration>,
    viewer: Option<Completeness<ViewerProfile>>,
    authors: Option<AuthorHydrationBatch<Completeness<DecodedAuthor>>>,
    selects: Vec<Select<'a>>,
    viewer_country: Option<ViewerCountry>,
    core_elapsed: Duration,
    composite_elapsed: Option<Duration>,
}

impl HydrationPlan {
    pub(crate) async fn hydrate(
        &self,
        sources: &dyn Sources,
        request: HydrationRequest<'_>,
    ) -> HydrationOutput {
        let started = Instant::now();
        let mut store = Store {
            viewer_id: request.viewer_id,
            tweet_ids: request.raw_candidates.iter().map(|c| c.tweet_id).collect(),
            ..Store::default()
        };
        let mut running: FuturesUnordered<Call<'_>> = FuturesUnordered::new();
        let mut ready: Vec<&Group> = self.groups().filter(|g| g.input.is_none()).collect();
        loop {
            while let Some(group) = ready.pop() {
                match self.start(group, &store, sources) {
                    Some(call) => running.push(call),
                    None => ready.extend(self.waiting_on(group)),
                }
            }
            let Some((group, reply)) = running.next().await else {
                break;
            };
            store.write(group, reply, request.raw_candidates, started);
            ready.extend(self.waiting_on(group));
        }
        metrics::record_tes_join_latency(
            self.level(),
            store
                .composite_elapsed
                .map_or(store.core_elapsed, |composite| {
                    composite.max(store.core_elapsed)
                }),
        );
        store.assemble(request)
    }

    fn waiting_on<'a>(&'a self, done: &'a Group) -> impl Iterator<Item = &'a Group> + 'a {
        self.groups()
            .filter(move |group| group.input.is_some_and(|input| done.nodes.contains(input)))
    }

    fn start<'a>(
        &'a self,
        group: &'a Group,
        store: &Store<'_>,
        sources: &'a dyn Sources,
    ) -> Option<Call<'a>> {
        let level = self.level();
        let (client, method) = group.label();
        if store.viewer_id.is_none() && group.nodes.iter().all(Hydrator::needs_viewer) {
            return None;
        }
        let reply: Pin<Box<dyn Future<Output = Reply> + Send + 'a>> = match group.source {
            Source::TesPureCore => {
                let counts = candidates_per_tweet(&store.tweet_ids);
                if counts.is_empty() {
                    return None;
                }
                record_batch_size(&client, counts.len());
                let cache = sources
                    .pure_core_cache()
                    .map(|cache| (cache, cache.begin_request()));
                Box::pin(async move {
                    let ids = counts.keys().copied().collect();
                    let fetched = timed_results(
                        &client,
                        &method,
                        level,
                        &counts,
                        HYDRATION_TIMEOUT,
                        sources.pure_cores(ids),
                    )
                    .await
                    .map_keys(TweetId)
                    .map(|core| pure_core(&core));
                    Reply::PureCores(match cache {
                        Some((cache, generation)) => {
                            cache.resolve_hydration_batch(generation, fetched)
                        }
                        None => fetched,
                    })
                })
            }
            Source::TesComposite => {
                let counts = candidates_per_tweet(&store.tweet_ids);
                if counts.is_empty() {
                    return None;
                }
                Box::pin(async move {
                    let ids = counts.keys().copied().collect();
                    let tweets = timed_results(
                        &client,
                        &method,
                        level,
                        &counts,
                        HYDRATION_TIMEOUT,
                        sources.tweets(ids),
                    )
                    .await;
                    Reply::Tweets(tweets.map_keys(TweetId))
                })
            }
            Source::TesConversationControl => {
                let counts = candidates_per_tweet(&store.tweet_ids);
                if counts.is_empty() {
                    return None;
                }
                record_batch_size(&client, counts.len());
                Box::pin(async move {
                    let ids = counts.keys().copied().collect();
                    let controls = timed_results(
                        &client,
                        &method,
                        level,
                        &counts,
                        HYDRATION_TIMEOUT,
                        sources.conversation_controls(ids),
                    )
                    .await;
                    Reply::Controls(controls.map_keys(TweetId))
                })
            }
            Source::SafetyLabels => {
                let tweet_ids = store.tweet_ids.clone();
                if tweet_ids.is_empty() {
                    return None;
                }
                record_batch_size(&client, tweet_ids.len());
                Box::pin(async move {
                    let call_started = Instant::now();
                    let resolved = sources
                        .safety_labels(tweet_ids.iter().map(|id| id.0).collect())
                        .await;
                    record_hydrator_request(
                        &client,
                        &method,
                        level,
                        batch_outcome(&resolved),
                        tweet_ids.len(),
                        call_started.elapsed().as_secs_f64() * 1000.0,
                    );
                    Reply::Labels(SafetyLabelHydration::new(&tweet_ids, &resolved))
                })
            }
            Source::GizmoduckViewer => {
                let viewer_id = store.viewer_id?;
                let fields = group.fields();
                Box::pin(async move {
                    let data = timed_rpc(
                        &client,
                        &method,
                        level,
                        1,
                        HYDRATION_TIMEOUT,
                        |data: &Option<anyhow::Result<_>>| match data {
                            Some(Ok(_)) => HydratorOutcome::Success,
                            _ => HydratorOutcome::Error,
                        },
                        async { Some(sources.viewer(viewer_id, &fields).await) },
                    )
                    .await;
                    Reply::Viewer(match data {
                        Some(Ok(data)) => Completeness::Complete(viewer_profile(data)),
                        Some(Err(error)) => {
                            warn!(%error, "Gizmoduck viewer lookup failed; failing open");
                            Completeness::Incomplete(ViewerProfile::default())
                        }
                        None => {
                            warn!("Gizmoduck viewer lookup timed out; failing open");
                            Completeness::Incomplete(ViewerProfile::default())
                        }
                    })
                })
            }
            Source::GizmoduckAuthor => {
                let counts = tweets_per_author(&store.candidates);
                if counts.is_empty() {
                    return None;
                }
                record_batch_size(&client, counts.len());
                let fields = group.fields();
                let cache = sources
                    .author_cache()
                    .map(|cache| (cache, cache.begin_request()));
                Box::pin(async move {
                    let ids = counts.keys().map(|author| author.get()).collect();
                    let users =
                        timed_results(&client, &method, level, &counts, HYDRATION_TIMEOUT, async {
                            keyed_by_author(&counts, sources.users(ids, &fields).await)
                        })
                        .await;
                    let authors = decode_authors(users);
                    Reply::Authors(match cache {
                        Some((cache, generation)) => {
                            cache.resolve_hydration_batch(generation, authors)
                        }
                        None => authors,
                    })
                })
            }
            Source::Flock => {
                let viewer_id = store.viewer_id?;
                let queries: Vec<EdgeQuery> = group
                    .edges()
                    .into_iter()
                    .map(|(graph, direction, origins)| {
                        let mut destinations: Vec<u64> = origins
                            .iter()
                            .flat_map(|&origin| store.keys(origin))
                            .collect::<HashSet<_>>()
                            .into_iter()
                            .collect();
                        destinations.sort_unstable();
                        EdgeQuery {
                            graph,
                            direction,
                            destination_ids: destinations,
                        }
                    })
                    .collect();
                if queries.iter().all(|query| query.destination_ids.is_empty()) {
                    return None;
                }
                let select = async move {
                    let count = queries.len();
                    let sets = sources.select_edges(viewer_id, &queries).await;
                    sets.filter(|sets| sets.len() == count)
                };
                if group.input == Some(Hydrator::PureCore) {
                    let counts = tweets_per_author(&store.candidates);
                    record_batch_size(&client, store.candidates.len());
                    Box::pin(async move {
                        let edges = timed_all_or_none(
                            &client,
                            &method,
                            level,
                            &counts,
                            HYDRATION_TIMEOUT,
                            select,
                        )
                        .await;
                        Reply::Edges(edges)
                    })
                } else {
                    let candidate_count = store.tweet_ids.len();
                    record_batch_size(&client, candidate_count);
                    Box::pin(async move {
                        let edges = timed_rpc(
                            &client,
                            &method,
                            level,
                            candidate_count,
                            HYDRATION_TIMEOUT,
                            |edges: &Option<_>| match edges {
                                Some(_) => HydratorOutcome::Success,
                                None => HydratorOutcome::Error,
                            },
                            select,
                        )
                        .await;
                        Reply::Edges(edges)
                    })
                }
            }
            Source::ViewerCountry => {
                let Some(&viewer_id) = store.keys(KeyOrigin::ViewerForCoAllowedList).first() else {
                    let has_co = store
                        .controls()
                        .any(|control| control.arm == ConversationControlArm::Co);
                    if has_co {
                        record_viewer_country(ViewerCountry::NoAllowedList.into(), level);
                    }
                    return None;
                };
                Box::pin(async move {
                    let country = timed_rpc(
                        &client,
                        &method,
                        level,
                        1,
                        HYDRATION_TIMEOUT,
                        |country: &ViewerCountry| match country {
                            ViewerCountry::Failed => HydratorOutcome::Error,
                            _ => HydratorOutcome::Success,
                        },
                        async {
                            match sources.viewer_country(viewer_id).await {
                                Ok(Some(country)) => ViewerCountry::Found(country.into()),
                                Ok(None) => ViewerCountry::NoRow,
                                Err(error) => {
                                    warn!(%error, "tfe_top_country lookup failed");
                                    ViewerCountry::Failed
                                }
                            }
                        },
                    )
                    .await;
                    record_viewer_country((&country).into(), level);
                    Reply::ViewerCountry(country)
                })
            }
        };
        Some(Box::pin(async move { (group, reply.await) }))
    }
}

impl<'a> Store<'a> {
    fn controls(&self) -> impl Iterator<Item = &ConversationControl> {
        self.tweet_ids
            .iter()
            .filter_map(|id| self.controls.as_ref()?.get(id))
    }

    fn keys(&self, origin: KeyOrigin) -> Vec<u64> {
        match origin {
            KeyOrigin::RequestTweets => self.tweet_ids.iter().map(|id| id.0).collect(),
            KeyOrigin::Viewer => self.viewer_id.into_iter().collect(),
            KeyOrigin::ViewerForCoAllowedList => {
                let needs_country = self.controls().any(|control| {
                    control.arm == ConversationControlArm::Co
                        && !control.allowed_country_codes.is_empty()
                });
                self.viewer_id
                    .filter(|_| needs_country)
                    .into_iter()
                    .collect()
            }
            KeyOrigin::PureCoreAuthor => self
                .candidates
                .iter()
                .map(|candidate| candidate.author_id.get())
                .collect(),
            KeyOrigin::PureCoreReplyRoot => self
                .candidates
                .iter()
                .filter_map(|candidate| self.reply_root(candidate))
                .collect(),
            KeyOrigin::ExclusiveConversationAuthor => self
                .tweet_ids
                .iter()
                .filter_map(|id| self.exclusive_author(id))
                .collect(),
            KeyOrigin::ConversationRoot(arms) => self
                .controls()
                .filter(|control| arms.contains(&control.arm))
                .map(|control| control.conversation_tweet_author_id)
                .collect(),
        }
    }

    fn key(&self, origin: KeyOrigin, candidate: &TweetCandidateInput) -> Option<u64> {
        match origin {
            KeyOrigin::RequestTweets | KeyOrigin::Viewer | KeyOrigin::ViewerForCoAllowedList => {
                None
            }
            KeyOrigin::PureCoreAuthor => Some(candidate.author_id.get()),
            KeyOrigin::PureCoreReplyRoot => self.reply_root(candidate),
            KeyOrigin::ExclusiveConversationAuthor => self.exclusive_author(&candidate.tweet_id),
            KeyOrigin::ConversationRoot(arms) => self
                .controls
                .as_ref()?
                .get(&candidate.tweet_id)
                .filter(|control| arms.contains(&control.arm))
                .map(|control| control.conversation_tweet_author_id),
        }
    }

    fn reply_root(&self, candidate: &TweetCandidateInput) -> Option<u64> {
        self.pure_cores
            .get(&candidate.tweet_id)?
            .direct_reply_root_author_id
            .map(|author| author.get())
    }

    fn exclusive_author(&self, id: &TweetId) -> Option<u64> {
        self.tweets
            .as_ref()?
            .get(id)?
            .exclusive_conversation_author_id
    }

    fn write(&mut self, group: &'a Group, reply: Reply, raw: &[RawCandidate], started: Instant) {
        match reply {
            Reply::PureCores(pure_cores) => {
                self.core_elapsed = started.elapsed();
                self.candidates = resolve_candidates(raw, &pure_cores);
                self.pure_cores = pure_cores;
            }
            Reply::Tweets(tweets) => {
                self.composite_elapsed = Some(started.elapsed());
                self.tweets = Some(tweets);
            }
            Reply::Controls(controls) => self.controls = Some(controls),
            Reply::Labels(labels) => self.labels = Some(labels),
            Reply::Viewer(viewer) => self.viewer = Some(viewer),
            Reply::Authors(authors) => self.authors = Some(authors),
            Reply::Edges(answers) => self.selects.push(Select {
                group,
                queries: group.edges(),
                answers,
            }),
            Reply::ViewerCountry(country) => self.viewer_country = Some(country),
        }
    }

    fn assemble(self, request: HydrationRequest<'_>) -> HydrationOutput {
        let mut failed_ids = HashSet::new();
        let candidates = self
            .candidates
            .iter()
            .map(|input| {
                let (candidate, complete) = self.candidate(input);
                if !complete {
                    failed_ids.insert(input.tweet_id);
                }
                candidate
            })
            .collect::<Vec<_>>();
        let viewer = match (request.viewer_id, self.viewer) {
            (None, _) => Completeness::Complete(Viewer::LoggedOut),
            (Some(id), None) => Completeness::Complete(Viewer::LoggedIn {
                id,
                profile: ViewerProfile::default(),
            }),
            (Some(id), Some(profile)) => profile.map(|profile| Viewer::LoggedIn { id, profile }),
        };
        if !viewer.is_complete() {
            failed_ids.extend(self.candidates.iter().map(|c| c.tweet_id));
        }
        HydrationOutput {
            viewer_features: ViewerFeatures::from_request(
                viewer.into_value(),
                request.country_code,
            ),
            candidates,
            safety_labels: self
                .labels
                .map(|labels| labels.label_response)
                .unwrap_or_default(),
            failed_ids,
            pure_cores: self.pure_cores,
        }
    }

    fn candidate(&self, input: &TweetCandidateInput) -> (HydratedTweetCandidate, bool) {
        let id = &input.tweet_id;
        let mut candidate = HydratedTweetCandidate {
            tweet_id: id.0,
            author_id: input.author_id.get(),
            ..Default::default()
        };
        let mut complete = !self.pure_cores.is_failed(id);
        if let Some(tweets) = &self.tweets {
            candidate.tweet_features = build_tweet_features(tweets.get(id));
            complete &= !tweets.is_failed(id);
        }
        if let Some(labels) = &self.labels {
            candidate.safety_labels = labels.label_types.get(id).cloned().unwrap_or_default();
            complete &= labels.label_response.contains_key(id);
        }
        if let Some(authors) = &self.authors {
            let author = authors.hydrated(&input.author_id);
            if let Some(Hydrated::Found(found)) = author {
                (candidate.author_features, candidate.author_labels) = *found.value();
            }
            complete &= matches!(
                author,
                Some(Hydrated::Found(Completeness::Complete(_)) | Hydrated::NotFound)
            );
        }
        if let Some(controls) = &self.controls {
            complete &= !controls.is_failed(id);
            candidate.conversation_control =
                controls
                    .get(id)
                    .cloned()
                    .map(|control| ConversationControlFeatures {
                        control,
                        root_author_follows_viewer: None,
                        viewer_super_follows_root_author: None,
                        viewer_country: None,
                    });
        }
        for select in &self.selects {
            for node in select.group.nodes.iter() {
                let spec = node.spec();
                let Some(key) = self.key(spec.key, input) else {
                    continue;
                };
                let edge = select
                    .queries
                    .iter()
                    .position(|(graph, direction, _)| spec.part == Part::Edge(*graph, *direction))
                    .zip(select.answers.as_ref())
                    .and_then(|(query, sets)| sets.get(query))
                    .map(|set| set.contains(&key));
                complete &= edge.is_some();
                write_edge(&mut candidate, node, edge);
            }
        }
        if let Some(features) = candidate
            .conversation_control
            .as_mut()
            .filter(|features| features.control.arm == ConversationControlArm::Co)
        {
            match &self.viewer_country {
                Some(ViewerCountry::Found(country)) => {
                    features.viewer_country = Some(country.clone());
                }
                Some(ViewerCountry::Failed) => {
                    complete &= features.control.allowed_country_codes.is_empty();
                }
                _ => {}
            }
        }
        (candidate, complete)
    }
}

fn write_edge(candidate: &mut HydratedTweetCandidate, node: Hydrator, edge: Option<bool>) {
    let has_edge = edge.unwrap_or(false);
    let control = candidate.conversation_control.as_mut();
    match node {
        Hydrator::Follows => candidate.relationship.viewer_follows_author = has_edge,
        Hydrator::Blocks => candidate.relationship.viewer_blocks_author = has_edge,
        Hydrator::Mutes => candidate.relationship.viewer_mutes_author = has_edge,
        Hydrator::MuteRetweets => {
            candidate.relationship.viewer_mutes_retweets_from_author = has_edge;
        }
        Hydrator::BlockedByAuthor => candidate.blocked_by.author = has_edge,
        Hydrator::BlockedByReplyRoot => candidate.blocked_by.root_author = has_edge,
        Hydrator::SuperFollowsExclusive => {
            candidate.viewer_super_follows_exclusive_author = has_edge;
        }
        Hydrator::RootFollowsViewer => {
            if let Some(control) = control {
                control.root_author_follows_viewer = edge;
            }
        }
        Hydrator::SuperFollowsRoot => {
            if let Some(control) = control {
                control.viewer_super_follows_root_author = edge;
            }
        }
        Hydrator::PureCore
        | Hydrator::Tweet
        | Hydrator::ConversationControl
        | Hydrator::TweetSafetyLabels
        | Hydrator::ViewerProfile
        | Hydrator::AuthorSafety
        | Hydrator::AuthorLabels
        | Hydrator::ViewerCountry => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::test_support::exclusive_tweet;
    use crate::hydration::gizmoduck_hydrator::fallback_cache;
    use crate::hydration::sources::{Fault, InMemorySources};
    use crate::hydration::tes_hydrator::pure_core_fallback_cache;
    use crate::rules::{RuleEngine, SafetyLevel};
    use xai_core_entities::entities::{
        GizmoduckUser, GizmoduckUserResult, PureCoreData, Safety, UserResponseState,
    };
    use xai_core_entities::gizmoduck_client::{QueryFields, ViewerData};

    const VIEWER: u64 = 50;

    fn raw(tweet_id: u64, request_author_id: Option<u64>) -> RawCandidate {
        RawCandidate {
            tweet_id: TweetId(tweet_id),
            request_author_id,
        }
    }

    async fn hydrate(
        sources: &InMemorySources,
        level: SafetyLevel,
        viewer_id: Option<u64>,
        raw: &[RawCandidate],
    ) -> HydrationOutput {
        RuleEngine::for_tests()
            .plan(level)
            .hydrate(
                sources,
                HydrationRequest::new(viewer_id, Some("US".into()), raw),
            )
            .await
    }

    fn ids(ids: &[u64]) -> HashSet<TweetId> {
        ids.iter().copied().map(TweetId).collect()
    }

    fn suspended() -> GizmoduckUserResult {
        GizmoduckUserResult {
            user: Some(GizmoduckUser {
                safety: Safety {
                    suspended: true,
                    ..Default::default()
                },
                ..Default::default()
            }),
            response_state: Some(UserResponseState::Found),
        }
    }

    fn control(arm: ConversationControlArm, root: u64, countries: &[&str]) -> ConversationControl {
        ConversationControl {
            arm,
            conversation_tweet_author_id: root,
            invited_user_ids: vec![],
            invite_via_mention: None,
            allowed_country_codes: countries.iter().map(|c| (*c).to_owned()).collect(),
        }
    }

    #[tokio::test]
    async fn failed_ids_reports_exactly_the_candidates_each_node_flags() {
        use ConversationControlArm::{Co, Community};
        use SafetyLevel::{TimelineHome, TimelineHomeHydration};
        let world = || InMemorySources::default().tweet(1, 10).tweet(2, 20);
        let rows = [
            ("healthy home", TimelineHome, world(), vec![]),
            ("healthy level 82", TimelineHomeHydration, world(), vec![]),
            (
                "failed pure core",
                TimelineHome,
                world().fault(Source::TesPureCore, Fault::Fails),
                vec![1, 2],
            ),
            (
                "incomplete author",
                TimelineHome,
                world().user(
                    10,
                    GizmoduckUserResult {
                        response_state: Some(UserResponseState::Failed),
                        ..suspended()
                    },
                ),
                vec![1],
            ),
            (
                "failed author-keyed select",
                TimelineHome,
                world().fail_graph(Graph::Mutes),
                vec![1, 2],
            ),
            (
                "failed blocked-by select",
                TimelineHomeHydration,
                world().fail_graph(Graph::Blocks),
                vec![1, 2],
            ),
            (
                "failed composite row",
                TimelineHome,
                world().fail_key(Source::TesComposite, 1),
                vec![1],
            ),
            (
                "failed label lookup",
                TimelineHome,
                world().fail_key(Source::SafetyLabels, 1),
                vec![1],
            ),
            (
                "failed exclusive select",
                TimelineHome,
                world()
                    .composite(1, exclusive_tweet())
                    .fail_graph(Graph::SuperFollows),
                vec![1],
            ),
            (
                "failed root-edge select",
                TimelineHomeHydration,
                world()
                    .control(1, control(Community, 30, &[]))
                    .fail_graph(Graph::Follows),
                vec![1],
            ),
            (
                "failed conversation-control row",
                TimelineHomeHydration,
                world().fail_key(Source::TesConversationControl, 1),
                vec![1],
            ),
            (
                "failed country lookup",
                TimelineHomeHydration,
                world()
                    .control(1, control(Co, 30, &["us"]))
                    .control(2, control(Co, 30, &[]))
                    .fault(Source::ViewerCountry, Fault::Fails),
                vec![1],
            ),
            (
                "failed viewer",
                TimelineHome,
                world().fault(Source::GizmoduckViewer, Fault::Fails),
                vec![1, 2],
            ),
        ];
        let raw = [raw(1, Some(10)), raw(2, Some(20))];
        for (name, level, sources, expected) in rows {
            let hydrated = hydrate(&sources, level, Some(VIEWER), &raw).await;
            assert_eq!(hydrated.failed_ids, ids(&expected), "{name}");
        }
    }

    fn author_keyed(sources: &InMemorySources) -> bool {
        sources.calls().contains(&Source::GizmoduckAuthor) || !sources.selects().is_empty()
    }

    #[tokio::test(start_paused = true)]
    async fn author_keyed_calls_wait_for_pure_core() {
        let sources = InMemorySources::default()
            .tweet(1, 10)
            .fault(Source::TesPureCore, Fault::Hangs);
        let raw = [raw(1, None)];
        let hydration = hydrate(&sources, SafetyLevel::TimelineHome, Some(VIEWER), &raw);
        tokio::pin!(hydration);
        let early = tokio::time::timeout(HYDRATION_TIMEOUT / 2, &mut hydration).await;
        assert!(early.is_err());
        assert!(!author_keyed(&sources));
        let hydrated = hydration.await;
        assert!(!author_keyed(&sources));
        assert!(hydrated.candidates.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn author_calls_do_not_wait_for_the_composite() {
        let sources = InMemorySources::default()
            .tweet(1, 10)
            .user(10, suspended())
            .edge(Graph::Follows, VIEWER, 10)
            .fault(Source::TesComposite, Fault::Hangs);
        let raw = [raw(1, None)];
        let started = tokio::time::Instant::now();
        let hydration = hydrate(&sources, SafetyLevel::TimelineHome, Some(VIEWER), &raw);
        tokio::pin!(hydration);
        let early = tokio::time::timeout(HYDRATION_TIMEOUT / 2, &mut hydration).await;
        assert!(early.is_err());
        assert_eq!(sources.keys(Source::GizmoduckAuthor), [vec![10]]);
        assert_eq!(sources.selects().len(), 1);

        let hydrated = hydration.await;
        assert_eq!(started.elapsed(), HYDRATION_TIMEOUT);
        let candidate = &hydrated.candidates[0];
        assert!(candidate.author_features.is_suspended);
        assert!(candidate.relationship.viewer_follows_author);
        assert_eq!(candidate.tweet_features, Default::default());
        assert_eq!(hydrated.failed_ids, ids(&[1]));
    }

    fn sorted(mut calls: Vec<Source>) -> Vec<String> {
        let mut names: Vec<String> = calls.drain(..).map(|s| format!("{s:?}")).collect();
        names.sort();
        names
    }

    #[tokio::test]
    async fn empty_key_sets_and_logged_out_viewers_send_no_call() {
        let raw = [raw(1, None)];
        let logged_out = InMemorySources::default().tweet(1, 10);
        hydrate(&logged_out, SafetyLevel::TimelineHomeHydration, None, &raw).await;
        assert_eq!(
            sorted(logged_out.calls()),
            [
                "GizmoduckAuthor",
                "SafetyLabels",
                "TesComposite",
                "TesConversationControl",
                "TesPureCore"
            ]
        );

        let logged_in = InMemorySources::default().tweet(1, 10);
        hydrate(
            &logged_in,
            SafetyLevel::TimelineHomeHydration,
            Some(VIEWER),
            &raw,
        )
        .await;
        assert!(!logged_in.calls().contains(&Source::ViewerCountry));
        assert_eq!(
            logged_in.selects(),
            [vec![EdgeQuery {
                graph: Graph::Blocks,
                direction: EdgeDirection::Reverse,
                destination_ids: vec![10],
            }]]
        );
    }

    #[tokio::test]
    async fn filter_all_calls_pure_core_only_and_keeps_the_request_side_viewer() {
        let sources = InMemorySources::default()
            .tweet(1, 10)
            .composite(1, exclusive_tweet());
        let hydrated = hydrate(
            &sources,
            SafetyLevel::FilterAll,
            Some(VIEWER),
            &[raw(1, None)],
        )
        .await;
        assert_eq!(sources.calls(), [Source::TesPureCore]);
        assert_eq!(
            hydrated.viewer_features.viewer,
            Viewer::LoggedIn {
                id: VIEWER,
                profile: ViewerProfile::default(),
            }
        );
        assert_eq!(hydrated.viewer_features.country_code.as_deref(), Some("us"));
        assert_eq!(hydrated.candidates[0].author_id, 10);
        assert!(hydrated.safety_labels.is_empty());
        assert!(hydrated.failed_ids.is_empty());
    }

    #[tokio::test]
    async fn a_level_that_plans_the_viewer_profile_decodes_it() {
        let sources = InMemorySources::default().viewer(
            VIEWER,
            ViewerData {
                user_exists: true,
                age_in_years: Some(30),
                ..Default::default()
            },
        );
        let hydrated = hydrate(&sources, SafetyLevel::TimelineHome, Some(VIEWER), &[]).await;
        let Viewer::LoggedIn { profile, .. } = hydrated.viewer_features.viewer else {
            panic!("a logged-in request stays logged in")
        };
        assert_ne!(profile, ViewerProfile::default());
    }

    #[tokio::test]
    async fn exclusive_edges_dedup_conversation_authors() {
        let sources = || {
            InMemorySources::default()
                .tweet(1, 10)
                .tweet(2, 20)
                .composite(1, exclusive_tweet())
                .composite(2, exclusive_tweet())
                .edge(Graph::SuperFollows, VIEWER, 30)
        };
        let raw = [raw(1, None), raw(2, None), raw(1, None), raw(3, Some(40))];
        for viewer_id in [Some(VIEWER), None] {
            let sources = sources();
            let hydrated = hydrate(&sources, SafetyLevel::TimelineHome, viewer_id, &raw).await;
            assert_eq!(sources.keys(Source::TesPureCore), [vec![1, 2, 3]]);
            assert_eq!(sources.keys(Source::TesComposite), [vec![1, 2, 3]]);
            let exclusive = (Some(30), viewer_id.is_some());
            assert_eq!(
                hydrated
                    .candidates
                    .iter()
                    .map(|c| (
                        c.tweet_features.exclusive_conversation_author_id,
                        c.viewer_super_follows_exclusive_author
                    ))
                    .collect::<Vec<_>>(),
                [exclusive, exclusive, exclusive, (None, false)]
            );
            let super_follows: Vec<Vec<u64>> = sources
                .selects()
                .into_iter()
                .flatten()
                .filter(|query| query.graph == Graph::SuperFollows)
                .map(|query| query.destination_ids)
                .collect();
            let expected: &[Vec<u64>] = if viewer_id.is_some() {
                &[vec![30]]
            } else {
                &[]
            };
            assert_eq!(super_follows, expected);
        }
    }

    fn root_edges(sources: &InMemorySources) -> Vec<Vec<EdgeQuery>> {
        sources
            .selects()
            .into_iter()
            .filter(|queries| queries.iter().any(|query| query.graph == Graph::Follows))
            .collect()
    }

    #[tokio::test]
    async fn one_select_carries_both_root_edges_and_a_failure_fails_every_tweet_it_keyed() {
        use ConversationControlArm::{ByInvitation, Community, MyNetwork, Subscribers};
        let world = || {
            InMemorySources::default()
                .tweet(1, 10)
                .tweet(2, 10)
                .tweet(3, 10)
                .tweet(4, 10)
                .control(1, control(Community, 30, &[]))
                .control(2, control(MyNetwork, 30, &[]))
                .control(3, control(Subscribers, 40, &[]))
                .control(4, control(ByInvitation, 40, &[]))
                .edge(Graph::Follows, 30, VIEWER)
                .edge(Graph::SuperFollows, VIEWER, 40)
        };
        let raw = [raw(1, None), raw(2, None), raw(3, None), raw(4, None)];
        let facts = |hydrated: &HydrationOutput| {
            hydrated
                .candidates
                .iter()
                .map(|c| {
                    let features = c.conversation_control.as_ref().unwrap();
                    (
                        features.root_author_follows_viewer,
                        features.viewer_super_follows_root_author,
                    )
                })
                .collect::<Vec<_>>()
        };

        let healthy = world();
        let hydrated = hydrate(
            &healthy,
            SafetyLevel::TimelineHomeHydration,
            Some(VIEWER),
            &raw,
        )
        .await;
        assert_eq!(
            root_edges(&healthy),
            [vec![
                EdgeQuery {
                    graph: Graph::Follows,
                    direction: EdgeDirection::Reverse,
                    destination_ids: vec![30],
                },
                EdgeQuery {
                    graph: Graph::SuperFollows,
                    direction: EdgeDirection::Forward,
                    destination_ids: vec![40],
                },
            ]]
        );
        assert_eq!(
            facts(&hydrated),
            [
                (Some(true), None),
                (Some(true), None),
                (None, Some(true)),
                (None, None)
            ]
        );
        assert!(hydrated.failed_ids.is_empty());

        let failed = world().fail_graph(Graph::Follows);
        let hydrated = hydrate(
            &failed,
            SafetyLevel::TimelineHomeHydration,
            Some(VIEWER),
            &raw,
        )
        .await;
        assert_eq!(facts(&hydrated), [(None, None); 4]);
        assert_eq!(hydrated.failed_ids, ids(&[1, 2, 3]));

        let logged_out = world();
        let hydrated = hydrate(&logged_out, SafetyLevel::TimelineHomeHydration, None, &raw).await;
        assert!(root_edges(&logged_out).is_empty());
        assert_eq!(facts(&hydrated), [(None, None); 4]);
        assert!(hydrated.failed_ids.is_empty());
    }

    #[tokio::test]
    async fn one_country_lookup_reaches_every_co_tweet_that_needs_it() {
        use ConversationControlArm::Co;
        let raw = [raw(1, None), raw(2, None)];
        let world = |countries: &[&str]| {
            InMemorySources::default()
                .tweet(1, 10)
                .tweet(2, 10)
                .control(1, control(Co, 30, countries))
                .control(2, control(Co, 30, countries))
                .country(VIEWER, "us")
        };
        let country = |hydrated: &HydrationOutput| {
            hydrated
                .candidates
                .iter()
                .map(|c| {
                    c.conversation_control
                        .as_ref()
                        .unwrap()
                        .viewer_country
                        .as_deref()
                        .map(str::to_owned)
                })
                .collect::<Vec<_>>()
        };

        let listed = world(&["us"]);
        let hydrated = hydrate(
            &listed,
            SafetyLevel::TimelineHomeHydration,
            Some(VIEWER),
            &raw,
        )
        .await;
        assert_eq!(listed.keys(Source::ViewerCountry), [vec![VIEWER]]);
        assert_eq!(
            country(&hydrated),
            [Some("us".to_owned()), Some("us".to_owned())]
        );

        for (sources, viewer_id) in [(world(&[]), Some(VIEWER)), (world(&["us"]), None)] {
            let hydrated = hydrate(
                &sources,
                SafetyLevel::TimelineHomeHydration,
                viewer_id,
                &raw,
            )
            .await;
            assert!(sources.keys(Source::ViewerCountry).is_empty());
            assert_eq!(country(&hydrated), [None, None]);
        }
    }

    #[tokio::test]
    async fn authors_share_one_key_and_a_missing_user_is_complete() {
        let sources = InMemorySources::default().tweet(1, 10).tweet(2, 10);
        let raw = [raw(1, None), raw(2, None)];
        let hydrated = hydrate(&sources, SafetyLevel::TimelineHome, None, &raw).await;
        assert_eq!(sources.keys(Source::GizmoduckAuthor), [vec![10]]);
        assert!(hydrated
            .candidates
            .iter()
            .all(|c| !c.author_features.is_suspended));
        assert!(hydrated.failed_ids.is_empty());
    }

    #[tokio::test]
    async fn the_author_cache_serves_the_last_known_author_when_the_call_fails() {
        let sources = InMemorySources::default()
            .tweet(1, 10)
            .user(10, suspended())
            .with_author_cache(fallback_cache());
        let raw = [raw(1, None)];
        let first = hydrate(&sources, SafetyLevel::TimelineHome, None, &raw).await;
        assert!(first.candidates[0].author_features.is_suspended);

        sources.break_source(Source::GizmoduckAuthor, Fault::Fails);
        let second = hydrate(&sources, SafetyLevel::TimelineHome, None, &raw).await;
        assert!(second.candidates[0].author_features.is_suspended);
        assert!(second.failed_ids.is_empty());
    }

    #[tokio::test]
    async fn the_pure_core_cache_serves_the_last_known_core_when_the_call_fails() {
        let sources = InMemorySources::default()
            .tweet(1, 10)
            .with_pure_core_cache(pure_core_fallback_cache(8));
        let raw = [raw(1, None)];
        let first = hydrate(&sources, SafetyLevel::TimelineHome, None, &raw).await;
        assert_eq!(first.candidates[0].author_id, 10);

        sources.break_source(Source::TesPureCore, Fault::Fails);
        let second = hydrate(&sources, SafetyLevel::TimelineHome, None, &raw).await;
        assert_eq!(second.candidates[0].author_id, 10);
        assert!(second.failed_ids.is_empty());
    }

    #[tokio::test]
    async fn gizmoduck_calls_ask_for_every_field_any_level_reads() {
        use QueryFields::{ACCOUNT, EXTENDED_PROFILE, LABELS, SAFETY};
        for level in [
            SafetyLevel::TimelineHome,
            SafetyLevel::TimelineHomeHydration,
        ] {
            let sources = InMemorySources::default().tweet(1, 10);
            hydrate(&sources, level, Some(VIEWER), &[raw(1, None)]).await;
            assert_eq!(
                sources.fields(Source::GizmoduckViewer),
                [vec![ACCOUNT, EXTENDED_PROFILE, SAFETY]],
                "{level:?}"
            );
            assert_eq!(
                sources.fields(Source::GizmoduckAuthor),
                [vec![SAFETY, LABELS]],
                "{level:?}"
            );
        }
    }

    fn call_labels(sources: &InMemorySources) -> Vec<String> {
        let mut labels: Vec<String> = sources
            .calls()
            .into_iter()
            .filter(|source| *source != Source::Flock)
            .map(|source| format!("{source:?}"))
            .chain(sources.selects().into_iter().map(|queries| {
                let graphs: Vec<&str> = queries.iter().map(|q| <&str>::from(q.graph)).collect();
                format!("Flock {}", graphs.join(","))
            }))
            .collect();
        labels.sort();
        labels
    }

    #[tokio::test(start_paused = true)]
    async fn a_hung_source_delays_only_the_calls_waiting_on_it() {
        use ConversationControlArm::{Co, Community};
        use SafetyLevel::{TimelineHome, TimelineHomeHydration};
        let world = || {
            InMemorySources::default()
                .pure_core(
                    1,
                    PureCoreData {
                        author_id: 10,
                        conversation_id: Some(100),
                        in_reply_to_tweet_id: Some(100),
                        in_reply_to_user_id: Some(30),
                        ..Default::default()
                    },
                )
                .tweet(2, 20)
                .composite(1, exclusive_tweet())
                .control(1, control(Community, 30, &[]))
                .control(2, control(Co, 30, &["us"]))
        };
        let rows: [(SafetyLevel, Source, &[&str]); 14] = [
            (
                TimelineHome,
                Source::TesPureCore,
                &[
                    "Flock follows,blocks,mutes,mute_retweets",
                    "GizmoduckAuthor",
                ],
            ),
            (TimelineHome, Source::TesComposite, &["Flock super_follows"]),
            (TimelineHome, Source::SafetyLabels, &[]),
            (TimelineHome, Source::GizmoduckViewer, &[]),
            (TimelineHome, Source::GizmoduckAuthor, &[]),
            (TimelineHome, Source::Flock, &[]),
            (
                TimelineHomeHydration,
                Source::TesPureCore,
                &["Flock blocks", "GizmoduckAuthor"],
            ),
            (
                TimelineHomeHydration,
                Source::TesComposite,
                &["Flock super_follows"],
            ),
            (
                TimelineHomeHydration,
                Source::TesConversationControl,
                &["Flock follows,super_follows", "ViewerCountry"],
            ),
            (TimelineHomeHydration, Source::SafetyLabels, &[]),
            (TimelineHomeHydration, Source::GizmoduckViewer, &[]),
            (TimelineHomeHydration, Source::GizmoduckAuthor, &[]),
            (TimelineHomeHydration, Source::Flock, &[]),
            (TimelineHomeHydration, Source::ViewerCountry, &[]),
        ];
        let raw = [raw(1, None), raw(2, None)];
        for (level, hung, waiting) in rows {
            let healthy = world();
            hydrate(&healthy, level, Some(VIEWER), &raw).await;
            let every_call = call_labels(&healthy);
            let groups = RuleEngine::for_tests().plan(level).groups().count();
            assert_eq!(every_call.len(), groups, "{level:?}: {every_call:?}");
            assert!(
                waiting
                    .iter()
                    .all(|call| every_call.iter().any(|c| c == call)),
                "{level:?} {hung:?}: {every_call:?}"
            );

            let sources = world().fault(hung, Fault::Hangs);
            let started = tokio::time::Instant::now();
            let hydration = hydrate(&sources, level, Some(VIEWER), &raw);
            tokio::pin!(hydration);
            let early = tokio::time::timeout(HYDRATION_TIMEOUT / 2, &mut hydration).await;
            assert!(early.is_err(), "{level:?} {hung:?}");
            let expected: Vec<String> = every_call
                .iter()
                .filter(|call| !waiting.contains(&call.as_str()))
                .cloned()
                .collect();
            assert_eq!(call_labels(&sources), expected, "{level:?} {hung:?}");
            if hung != Source::SafetyLabels {
                hydration.await;
                assert_eq!(started.elapsed(), HYDRATION_TIMEOUT, "{level:?} {hung:?}");
            }
        }
    }
}
