use crate::clients::about_this_account_client::AboutThisAccountClient;
use crate::clients::gizmoduck_client::GizmoduckLookup;
use crate::clients::socialgraph_client::{EdgeQuery, SocialgraphClient};
use crate::hydration::gizmoduck_hydrator::AuthorFallbackCache;
use crate::hydration::tes_composite::{TweetForVisibility, TweetForVisibilitySource};
use crate::hydration::tes_hydrator::PureCoreFallbackCache;
use crate::safety_label_source::lookup::LookupError;
use crate::safety_label_source::SafetyLabelSource;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use xai_core_entities::entities::{ConversationControl, GizmoduckUserResult, PureCoreData};
use xai_core_entities::gizmoduck_client::{GizmoduckClient, QueryFields, ViewerData};
use xai_core_entities::tweet_entity_service_client::TESClient;
use xai_visibility_filtering_proto as vf_pb;

pub(crate) type Keyed<V> = HashMap<u64, anyhow::Result<Option<V>>>;

#[tonic::async_trait]
pub(crate) trait Sources: Send + Sync {
    async fn pure_cores(&self, tweet_ids: Vec<u64>) -> Keyed<PureCoreData>;

    async fn tweets(&self, tweet_ids: Vec<u64>) -> Keyed<TweetForVisibility>;

    async fn conversation_controls(&self, tweet_ids: Vec<u64>) -> Keyed<ConversationControl>;

    async fn safety_labels(
        &self,
        tweet_ids: Vec<u64>,
    ) -> HashMap<u64, Result<Arc<vf_pb::SafetyLabelMap>, LookupError>>;

    async fn viewer(&self, viewer_id: u64, fields: &[QueryFields]) -> anyhow::Result<ViewerData>;

    async fn users(&self, user_ids: Vec<u64>, fields: &[QueryFields])
        -> Keyed<GizmoduckUserResult>;

    async fn select_edges(
        &self,
        viewer_id: u64,
        queries: &[EdgeQuery],
    ) -> Option<Vec<HashSet<u64>>>;

    async fn viewer_country(&self, viewer_id: u64) -> anyhow::Result<Option<String>>;

    fn pure_core_cache(&self) -> Option<&PureCoreFallbackCache> {
        None
    }

    fn author_cache(&self) -> Option<&AuthorFallbackCache> {
        None
    }
}

pub(crate) struct ProdSources {
    tes: Arc<dyn TESClient + Send + Sync>,
    composite: Arc<dyn TweetForVisibilitySource>,
    gizmoduck: Arc<dyn GizmoduckClient + Send + Sync>,
    authors: GizmoduckLookup,
    socialgraph: Arc<dyn SocialgraphClient + Send + Sync>,
    about_this_account: Arc<dyn AboutThisAccountClient>,
    safety_labels: Arc<SafetyLabelSource>,
    author_cache: Option<AuthorFallbackCache>,
    pure_core_cache: Option<PureCoreFallbackCache>,
}

impl ProdSources {
    #[expect(
        clippy::too_many_arguments,
        reason = "one argument per backend and cache"
    )]
    pub(crate) fn new(
        tes: Arc<dyn TESClient + Send + Sync>,
        composite: Arc<dyn TweetForVisibilitySource>,
        gizmoduck: Arc<dyn GizmoduckClient + Send + Sync>,
        socialgraph: Arc<dyn SocialgraphClient + Send + Sync>,
        about_this_account: Arc<dyn AboutThisAccountClient>,
        safety_labels: Arc<SafetyLabelSource>,
        author_cache: Option<AuthorFallbackCache>,
        pure_core_cache: Option<PureCoreFallbackCache>,
    ) -> Self {
        Self {
            tes,
            composite,
            authors: GizmoduckLookup::new(gizmoduck.clone()),
            gizmoduck,
            socialgraph,
            about_this_account,
            safety_labels,
            author_cache,
            pure_core_cache,
        }
    }
}

#[tonic::async_trait]
impl Sources for ProdSources {
    async fn pure_cores(&self, tweet_ids: Vec<u64>) -> Keyed<PureCoreData> {
        self.tes.get_tweet_core_datas(tweet_ids).await
    }

    async fn tweets(&self, tweet_ids: Vec<u64>) -> Keyed<TweetForVisibility> {
        self.composite.get_tweets_for_visibility(&tweet_ids).await
    }

    async fn conversation_controls(&self, tweet_ids: Vec<u64>) -> Keyed<ConversationControl> {
        self.tes.get_conversation_controls(tweet_ids).await
    }

    async fn safety_labels(
        &self,
        tweet_ids: Vec<u64>,
    ) -> HashMap<u64, Result<Arc<vf_pb::SafetyLabelMap>, LookupError>> {
        self.safety_labels.get(&tweet_ids).await
    }

    async fn viewer(&self, viewer_id: u64, fields: &[QueryFields]) -> anyhow::Result<ViewerData> {
        self.gizmoduck
            .get_viewer_data_with_fields(viewer_id, fields)
            .await
    }

    async fn users(
        &self,
        user_ids: Vec<u64>,
        fields: &[QueryFields],
    ) -> Keyed<GizmoduckUserResult> {
        self.authors.get_users(user_ids, fields).await
    }

    async fn select_edges(
        &self,
        viewer_id: u64,
        queries: &[EdgeQuery],
    ) -> Option<Vec<HashSet<u64>>> {
        self.socialgraph.select_edges(viewer_id, queries).await
    }

    async fn viewer_country(&self, viewer_id: u64) -> anyhow::Result<Option<String>> {
        self.about_this_account.tfe_top_country(viewer_id).await
    }

    fn pure_core_cache(&self) -> Option<&PureCoreFallbackCache> {
        self.pure_core_cache.as_ref()
    }

    fn author_cache(&self) -> Option<&AuthorFallbackCache> {
        self.author_cache.as_ref()
    }
}

#[cfg(test)]
pub(crate) use in_memory::{Fault, InMemorySources};

#[cfg(test)]
mod in_memory {
    use super::*;
    use crate::clients::socialgraph_client::{EdgeDirection, Graph};
    use crate::hydration::plan::Source;
    use crate::safety_label_source::types::FailureKind;
    use std::sync::Mutex;

    #[derive(Clone, Copy, Debug)]
    pub(crate) enum Fault {
        Fails,
        Hangs,
    }

    #[derive(Default)]
    pub(crate) struct InMemorySources {
        pure_cores: HashMap<u64, PureCoreData>,
        tweets: HashMap<u64, TweetForVisibility>,
        controls: HashMap<u64, ConversationControl>,
        viewers: HashMap<u64, ViewerData>,
        users: HashMap<u64, GizmoduckUserResult>,
        edges: HashSet<(Graph, u64, u64)>,
        countries: HashMap<u64, String>,
        faults: Mutex<Vec<(Source, Fault)>>,
        failed_keys: HashSet<(Source, u64)>,
        failed_graphs: HashSet<Graph>,
        author_cache: Option<AuthorFallbackCache>,
        pure_core_cache: Option<PureCoreFallbackCache>,
        calls: Mutex<Vec<(Source, Vec<u64>)>>,
        selects: Mutex<Vec<Vec<EdgeQuery>>>,
        fields: Mutex<Vec<(Source, Vec<QueryFields>)>>,
    }

    impl InMemorySources {
        pub(crate) fn tweet(self, tweet_id: u64, author_id: u64) -> Self {
            self.pure_core(
                tweet_id,
                PureCoreData {
                    author_id,
                    ..Default::default()
                },
            )
        }

        pub(crate) fn pure_core(mut self, tweet_id: u64, core: PureCoreData) -> Self {
            self.pure_cores.insert(tweet_id, core);
            self
        }

        pub(crate) fn composite(mut self, tweet_id: u64, tweet: TweetForVisibility) -> Self {
            self.tweets.insert(tweet_id, tweet);
            self
        }

        pub(crate) fn control(mut self, tweet_id: u64, control: ConversationControl) -> Self {
            self.controls.insert(tweet_id, control);
            self
        }

        pub(crate) fn viewer(mut self, viewer_id: u64, data: ViewerData) -> Self {
            self.viewers.insert(viewer_id, data);
            self
        }

        pub(crate) fn user(mut self, user_id: u64, user: GizmoduckUserResult) -> Self {
            self.users.insert(user_id, user);
            self
        }

        pub(crate) fn edge(mut self, graph: Graph, source: u64, destination: u64) -> Self {
            self.edges.insert((graph, source, destination));
            self
        }

        pub(crate) fn country(mut self, viewer_id: u64, country: &str) -> Self {
            self.countries.insert(viewer_id, country.to_owned());
            self
        }

        pub(crate) fn fault(self, source: Source, fault: Fault) -> Self {
            self.break_source(source, fault);
            self
        }

        pub(crate) fn break_source(&self, source: Source, fault: Fault) {
            self.faults.lock().unwrap().push((source, fault));
        }

        pub(crate) fn fail_graph(mut self, graph: Graph) -> Self {
            self.failed_graphs.insert(graph);
            self
        }

        pub(crate) fn fail_key(mut self, source: Source, key: u64) -> Self {
            self.failed_keys.insert((source, key));
            self
        }

        pub(crate) fn with_author_cache(mut self, cache: AuthorFallbackCache) -> Self {
            self.author_cache = Some(cache);
            self
        }

        pub(crate) fn with_pure_core_cache(mut self, cache: PureCoreFallbackCache) -> Self {
            self.pure_core_cache = Some(cache);
            self
        }

        pub(crate) fn calls(&self) -> Vec<Source> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .map(|(source, _)| *source)
                .collect()
        }

        pub(crate) fn keys(&self, source: Source) -> Vec<Vec<u64>> {
            self.calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(called, _)| *called == source)
                .map(|(_, keys)| keys.clone())
                .collect()
        }

        pub(crate) fn selects(&self) -> Vec<Vec<EdgeQuery>> {
            self.selects.lock().unwrap().clone()
        }

        pub(crate) fn fields(&self, source: Source) -> Vec<Vec<QueryFields>> {
            self.fields
                .lock()
                .unwrap()
                .iter()
                .filter(|(called, _)| *called == source)
                .map(|(_, fields)| fields.clone())
                .collect()
        }

        fn record_fields(&self, source: Source, fields: &[QueryFields]) {
            self.fields.lock().unwrap().push((source, fields.to_vec()));
        }

        async fn enter(&self, source: Source, keys: &[u64]) -> bool {
            let mut keys = keys.to_vec();
            keys.sort_unstable();
            self.calls.lock().unwrap().push((source, keys));
            let fault = self
                .faults
                .lock()
                .unwrap()
                .iter()
                .find(|(faulty, _)| *faulty == source)
                .map(|(_, fault)| *fault);
            match fault {
                Some(Fault::Hangs) => std::future::pending().await,
                Some(Fault::Fails) => true,
                None => false,
            }
        }

        async fn keyed<V: Clone>(
            &self,
            source: Source,
            ids: Vec<u64>,
            values: &HashMap<u64, V>,
        ) -> Keyed<V> {
            let fails = self.enter(source, &ids).await;
            ids.into_iter()
                .map(|id| {
                    let result = if fails || self.failed_keys.contains(&(source, id)) {
                        Err(anyhow::anyhow!("{source:?} unavailable"))
                    } else {
                        Ok(values.get(&id).cloned())
                    };
                    (id, result)
                })
                .collect()
        }
    }

    #[tonic::async_trait]
    impl Sources for InMemorySources {
        async fn pure_cores(&self, tweet_ids: Vec<u64>) -> Keyed<PureCoreData> {
            self.keyed(Source::TesPureCore, tweet_ids, &self.pure_cores)
                .await
        }

        async fn tweets(&self, tweet_ids: Vec<u64>) -> Keyed<TweetForVisibility> {
            self.keyed(Source::TesComposite, tweet_ids, &self.tweets)
                .await
        }

        async fn conversation_controls(&self, tweet_ids: Vec<u64>) -> Keyed<ConversationControl> {
            self.keyed(Source::TesConversationControl, tweet_ids, &self.controls)
                .await
        }

        async fn safety_labels(
            &self,
            tweet_ids: Vec<u64>,
        ) -> HashMap<u64, Result<Arc<vf_pb::SafetyLabelMap>, LookupError>> {
            let fails = self.enter(Source::SafetyLabels, &tweet_ids).await;
            tweet_ids
                .into_iter()
                .map(|id| {
                    let result = if fails || self.failed_keys.contains(&(Source::SafetyLabels, id))
                    {
                        Err(LookupError::new(
                            FailureKind::ManhattanFetch,
                            "labels unavailable",
                        ))
                    } else {
                        Ok(Arc::default())
                    };
                    (id, result)
                })
                .collect()
        }

        async fn viewer(
            &self,
            viewer_id: u64,
            fields: &[QueryFields],
        ) -> anyhow::Result<ViewerData> {
            self.record_fields(Source::GizmoduckViewer, fields);
            if self.enter(Source::GizmoduckViewer, &[viewer_id]).await {
                anyhow::bail!("gizmoduck unavailable");
            }
            Ok(self.viewers.get(&viewer_id).cloned().unwrap_or_default())
        }

        async fn users(
            &self,
            user_ids: Vec<u64>,
            fields: &[QueryFields],
        ) -> Keyed<GizmoduckUserResult> {
            self.record_fields(Source::GizmoduckAuthor, fields);
            self.keyed(Source::GizmoduckAuthor, user_ids, &self.users)
                .await
        }

        async fn select_edges(
            &self,
            viewer_id: u64,
            queries: &[EdgeQuery],
        ) -> Option<Vec<HashSet<u64>>> {
            let mut recorded = queries.to_vec();
            for query in &mut recorded {
                query.destination_ids.sort_unstable();
            }
            self.selects.lock().unwrap().push(recorded);
            let failed_graph = queries
                .iter()
                .any(|query| self.failed_graphs.contains(&query.graph));
            if self.enter(Source::Flock, &[viewer_id]).await || failed_graph {
                return None;
            }
            let answer = |query: &EdgeQuery| {
                query
                    .destination_ids
                    .iter()
                    .copied()
                    .filter(|&id| {
                        let edge = match query.direction {
                            EdgeDirection::Forward => (query.graph, viewer_id, id),
                            EdgeDirection::Reverse => (query.graph, id, viewer_id),
                        };
                        self.edges.contains(&edge)
                    })
                    .collect()
            };
            Some(queries.iter().map(answer).collect())
        }

        async fn viewer_country(&self, viewer_id: u64) -> anyhow::Result<Option<String>> {
            if self.enter(Source::ViewerCountry, &[viewer_id]).await {
                anyhow::bail!("tfe_top_country unavailable");
            }
            Ok(self.countries.get(&viewer_id).cloned())
        }

        fn pure_core_cache(&self) -> Option<&PureCoreFallbackCache> {
            self.pure_core_cache.as_ref()
        }

        fn author_cache(&self) -> Option<&AuthorFallbackCache> {
            self.author_cache.as_ref()
        }
    }
}
