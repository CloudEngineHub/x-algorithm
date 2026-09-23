use crate::clients::socialgraph_client::SocialgraphClient;
use crate::hydration::batch::{Completeness, TweetHydrationBatch};
use crate::hydration::metrics::{record_batch_size, timed_results, timed_rpc, HydratorOutcome};
use crate::hydration::tes_hydrator::candidates_per_tweet;
use crate::models::{ConversationControlFeatures, TweetId};
use crate::rules::SafetyLevel;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use xai_core_entities::entities::ConversationControlArm;
use xai_core_entities::tweet_entity_service_client::TESClient;

const CLIENT_TIMEOUT: Duration = crate::hydration::HYDRATION_TIMEOUT;
const CLIENT: &str = "conversation_control";

pub struct ConversationControlHydrator {
    pub tes_client: Arc<dyn TESClient + Send + Sync>,
    pub sg_client: Arc<dyn SocialgraphClient + Send + Sync>,
}

impl ConversationControlHydrator {
    pub async fn hydrate(
        &self,
        tweet_ids: &[TweetId],
        viewer_id: Option<u64>,
        safety_level: SafetyLevel,
    ) -> TweetHydrationBatch<Completeness<ConversationControlFeatures>> {
        let candidate_count_by_key = candidates_per_tweet(tweet_ids);
        let raw_ids: Vec<u64> = candidate_count_by_key.keys().copied().collect();
        record_batch_size(CLIENT, candidate_count_by_key.len());
        let controls = timed_results(
            CLIENT,
            "get_conversation_controls",
            safety_level,
            &candidate_count_by_key,
            CLIENT_TIMEOUT,
            self.tes_client.get_conversation_controls(raw_ids),
        )
        .await
        .map_keys(TweetId);

        let Some(viewer_id) = viewer_id else {
            return controls.map(|control| {
                Completeness::Complete(ConversationControlFeatures {
                    control,
                    root_author_follows_viewer: None,
                    viewer_super_follows_root_author: None,
                })
            });
        };
        let root_authors = |arm: ConversationControlArm| -> Vec<u64> {
            tweet_ids
                .iter()
                .filter_map(|id| controls.get(id))
                .filter(|control| control.arm == arm)
                .map(|control| control.conversation_tweet_author_id)
                .collect::<HashSet<_>>()
                .into_iter()
                .collect()
        };
        let community_roots = root_authors(ConversationControlArm::Community);
        let subscriber_roots = root_authors(ConversationControlArm::Subscribers);
        let (followed_by, super_follows) = tokio::join!(
            edge_lookup(
                "batch_check_followed_by",
                safety_level,
                tweet_ids.len(),
                &community_roots,
                self.sg_client
                    .batch_check_followed_by(viewer_id, &community_roots),
            ),
            edge_lookup(
                "batch_check_super_follows",
                safety_level,
                tweet_ids.len(),
                &subscriber_roots,
                self.sg_client
                    .batch_check_super_follows(viewer_id, &subscriber_roots),
            ),
        );

        controls.map(|control| {
            let root = control.conversation_tweet_author_id;
            let edge = |lookup: &Option<HashMap<u64, bool>>| {
                lookup.as_ref().map(|edges| edges.get(&root) == Some(&true))
            };
            let (root_author_follows_viewer, viewer_super_follows_root_author, complete) =
                match control.arm {
                    ConversationControlArm::Community => {
                        (edge(&followed_by), None, followed_by.is_some())
                    }
                    ConversationControlArm::Subscribers => {
                        (None, edge(&super_follows), super_follows.is_some())
                    }
                    _ => (None, None, true),
                };
            Completeness::new(
                complete,
                ConversationControlFeatures {
                    control,
                    root_author_follows_viewer,
                    viewer_super_follows_root_author,
                },
            )
        })
    }
}

async fn edge_lookup(
    method: &'static str,
    safety_level: SafetyLevel,
    candidate_count: usize,
    root_author_ids: &[u64],
    lookup: impl Future<Output = Option<HashMap<u64, bool>>>,
) -> Option<HashMap<u64, bool>> {
    if root_author_ids.is_empty() {
        return Some(HashMap::new());
    }
    timed_rpc(
        CLIENT,
        method,
        safety_level,
        candidate_count,
        CLIENT_TIMEOUT,
        |edges: &Option<_>| match edges {
            Some(_) => HydratorOutcome::Success,
            None => HydratorOutcome::Error,
        },
        lookup,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hydration::batch::Hydrated;
    use crate::models::ViewerAuthorRelationship;
    use std::sync::Mutex;
    use tonic::async_trait;
    use xai_core_entities::entities::ConversationControl;
    use xai_core_entities::tweet_entity_service_client::MockTESClient;

    const VIEWER_ID: u64 = 50;
    const COMMUNITY_ROOT: u64 = 10;
    const SUBSCRIBERS_ROOT: u64 = 11;

    fn control(arm: ConversationControlArm, root: u64) -> ConversationControl {
        ConversationControl {
            arm,
            conversation_tweet_author_id: root,
            invited_user_ids: vec![],
            invite_via_mention: None,
            allowed_country_codes: vec![],
        }
    }

    fn tes() -> Arc<MockTESClient> {
        Arc::new(MockTESClient {
            conversation_controls: HashMap::from([
                (
                    1,
                    Some(control(ConversationControlArm::Community, COMMUNITY_ROOT)),
                ),
                (
                    2,
                    Some(control(ConversationControlArm::Community, COMMUNITY_ROOT)),
                ),
                (
                    3,
                    Some(control(
                        ConversationControlArm::Subscribers,
                        SUBSCRIBERS_ROOT,
                    )),
                ),
                (4, Some(control(ConversationControlArm::ByInvitation, 12))),
                (5, None),
            ]),
            ..Default::default()
        })
    }

    enum EdgeLookup {
        Fails,
        Hangs,
        Succeeds,
    }

    struct RecordingSocialgraph {
        followed_by: Mutex<Vec<Vec<u64>>>,
        super_follows: Mutex<Vec<Vec<u64>>>,
        lookup: EdgeLookup,
    }

    impl RecordingSocialgraph {
        fn with(lookup: EdgeLookup) -> Arc<Self> {
            Arc::new(Self {
                followed_by: Mutex::default(),
                super_follows: Mutex::default(),
                lookup,
            })
        }

        async fn answer(&self, ids: &[u64]) -> Option<HashMap<u64, bool>> {
            match self.lookup {
                EdgeLookup::Fails => None,
                EdgeLookup::Hangs => std::future::pending().await,
                EdgeLookup::Succeeds => Some(ids.iter().map(|&id| (id, true)).collect()),
            }
        }
    }

    #[async_trait]
    impl SocialgraphClient for RecordingSocialgraph {
        async fn batch_check_relationships(
            &self,
            _: u64,
            _: &[u64],
        ) -> HashMap<u64, ViewerAuthorRelationship> {
            unreachable!("conversation control never checks viewer→author relationships")
        }

        async fn batch_check_super_follows(
            &self,
            _: u64,
            ids: &[u64],
        ) -> Option<HashMap<u64, bool>> {
            self.super_follows.lock().unwrap().push(ids.to_vec());
            self.answer(ids).await
        }

        async fn batch_check_followed_by(&self, _: u64, ids: &[u64]) -> Option<HashMap<u64, bool>> {
            self.followed_by.lock().unwrap().push(ids.to_vec());
            self.answer(ids).await
        }
    }

    async fn hydrate_batch(
        tes_client: Arc<dyn TESClient + Send + Sync>,
        sg_client: Arc<dyn SocialgraphClient + Send + Sync>,
        viewer_id: Option<u64>,
    ) -> TweetHydrationBatch<Completeness<ConversationControlFeatures>> {
        let ids = [1, 2, 3, 4, 5, 1].map(TweetId);
        tokio::time::timeout(
            CLIENT_TIMEOUT * 2,
            ConversationControlHydrator {
                tes_client,
                sg_client,
            }
            .hydrate(&ids, viewer_id, SafetyLevel::TimelineHomeHydration),
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn successful_lookups_complete_every_controlled_tweet_and_dedup_roots() {
        let sg = RecordingSocialgraph::with(EdgeLookup::Succeeds);
        let out = hydrate_batch(tes(), sg.clone(), Some(VIEWER_ID)).await;

        let edges = |id| {
            let features = out.get(&TweetId(id)).unwrap();
            (
                features.is_complete(),
                features.value().root_author_follows_viewer,
                features.value().viewer_super_follows_root_author,
            )
        };
        assert_eq!(edges(1), (true, Some(true), None));
        assert_eq!(edges(3), (true, None, Some(true)));
        assert_eq!(edges(4), (true, None, None));
        assert!(out.get(&TweetId(2)).is_some_and(Completeness::is_complete));
        assert_eq!(out.hydrated(&TweetId(5)), Some(&Hydrated::NotFound));
        assert_eq!(*sg.followed_by.lock().unwrap(), vec![vec![COMMUNITY_ROOT]]);
        assert_eq!(
            *sg.super_follows.lock().unwrap(),
            vec![vec![SUBSCRIBERS_ROOT]]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn failed_edge_lookup_keeps_the_control_but_marks_its_arm_incomplete() {
        for lookup in [EdgeLookup::Fails, EdgeLookup::Hangs] {
            let out =
                hydrate_batch(tes(), RecordingSocialgraph::with(lookup), Some(VIEWER_ID)).await;

            for id in [1, 3] {
                let hydrated = out.get(&TweetId(id)).unwrap();
                assert!(!hydrated.is_complete(), "tweet {id}");
                assert_eq!(hydrated.value().root_author_follows_viewer, None);
                assert_eq!(hydrated.value().viewer_super_follows_root_author, None);
            }
            assert!(out.get(&TweetId(4)).is_some_and(Completeness::is_complete));
            assert_eq!(out.hydrated(&TweetId(5)), Some(&Hydrated::NotFound));
        }
    }

    #[tokio::test]
    async fn logged_out_viewer_skips_the_edge_lookups_and_keeps_the_control() {
        let sg = RecordingSocialgraph::with(EdgeLookup::Fails);
        let out = hydrate_batch(tes(), sg.clone(), None).await;

        let community = out.get(&TweetId(1)).unwrap();
        assert!(community.is_complete());
        assert_eq!(community.value().root_author_follows_viewer, None);
        assert!(sg.followed_by.lock().unwrap().is_empty());
        assert!(sg.super_follows.lock().unwrap().is_empty());
    }
}
