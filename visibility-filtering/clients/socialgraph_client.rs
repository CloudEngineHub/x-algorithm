use crate::models::ViewerAuthorRelationship;
use std::collections::{HashMap, HashSet};
use tonic::async_trait;
use tracing::warn;
use xai_flock_client::FlockClient;
use xai_flock_proto::{
    EdgeState, LongList, Page, QueryTerm, Results, SelectOperation, SelectOperationType,
    SelectQuery, SelectRequest,
};
use EdgeDirection::{Forward, Reverse};

const FOLLOWS_GRAPH_ID: i32 = 1;
const BLOCKS_GRAPH_ID: i32 = 3;
const MUTE_GRAPH_ID: i32 = 23;
const MUTE_RETWEETS_GRAPH_ID: i32 = 10;
const SUPER_FOLLOWS_GRAPH_ID: i32 = 55;
const REVERSE_EDGE_CHUNK_SIZE: usize = 500;

#[async_trait]
pub trait SocialgraphClient: Send + Sync {
    async fn batch_check_relationships(
        &self,
        viewer_id: u64,
        author_ids: &[u64],
    ) -> HashMap<u64, ViewerAuthorRelationship>;

    async fn batch_check_super_follows(
        &self,
        viewer_id: u64,
        author_ids: &[u64],
    ) -> Option<HashMap<u64, bool>>;

    async fn batch_check_followed_by(
        &self,
        viewer_id: u64,
        user_ids: &[u64],
    ) -> Option<HashMap<u64, bool>>;
}

#[cfg(test)]
pub struct FakeSocialgraphClient;

#[cfg(test)]
#[async_trait]
impl SocialgraphClient for FakeSocialgraphClient {
    async fn batch_check_relationships(
        &self,
        _viewer_id: u64,
        author_ids: &[u64],
    ) -> HashMap<u64, ViewerAuthorRelationship> {
        author_ids
            .iter()
            .map(|&author_id| (author_id, ViewerAuthorRelationship::default()))
            .collect()
    }

    async fn batch_check_super_follows(
        &self,
        _viewer_id: u64,
        author_ids: &[u64],
    ) -> Option<HashMap<u64, bool>> {
        Some(
            author_ids
                .iter()
                .map(|&author_id| (author_id, false))
                .collect(),
        )
    }

    async fn batch_check_followed_by(
        &self,
        _viewer_id: u64,
        user_ids: &[u64],
    ) -> Option<HashMap<u64, bool>> {
        Some(user_ids.iter().map(|&user_id| (user_id, false)).collect())
    }
}

#[derive(Clone, Copy)]
enum EdgeDirection {
    Forward,
    Reverse,
}

fn decode_packed_ids(packed: &[u8]) -> HashSet<u64> {
    let (chunks, _remainder) = packed.as_chunks::<8>();
    chunks
        .iter()
        .map(|&arr| i64::from_le_bytes(arr).cast_unsigned())
        .collect()
}

fn edge_membership_query(
    source_id: u64,
    graph_id: i32,
    direction: EdgeDirection,
    destination_ids: &[i64],
) -> SelectQuery {
    SelectQuery {
        operations: vec![SelectOperation {
            operation_type: SelectOperationType::SimpleQuery as i32,
            term: Some(QueryTerm {
                source_id: source_id.cast_signed(),
                graph_id,
                is_forward: matches!(direction, Forward),
                destination_ids: Some(LongList {
                    ids: destination_ids.to_vec(),
                }),
                state_ids: vec![EdgeState::Positive as i32],
                size_hint: None,
                cursor_hint: None,
            }),
        }],
        page: Some(Page {
            count: i32::try_from(destination_ids.len()).unwrap_or(i32::MAX),
            cursor: -1,
        }),
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct RelationshipEdges {
    follows: HashSet<u64>,
    blocks: HashSet<u64>,
    mutes: HashSet<u64>,
    mute_retweets: HashSet<u64>,
}

fn relationship_select_request(viewer_id: u64, destination_ids: &[i64]) -> SelectRequest {
    SelectRequest {
        queries: vec![
            edge_membership_query(viewer_id, FOLLOWS_GRAPH_ID, Forward, destination_ids),
            edge_membership_query(viewer_id, BLOCKS_GRAPH_ID, Forward, destination_ids),
            edge_membership_query(viewer_id, MUTE_GRAPH_ID, Forward, destination_ids),
            edge_membership_query(viewer_id, MUTE_RETWEETS_GRAPH_ID, Forward, destination_ids),
        ],
        ancestor_client_id: None,
        service_account: None,
        quota_name: None,
    }
}

fn next_edge_set(results: &mut impl Iterator<Item = Results>) -> HashSet<u64> {
    results
        .next()
        .map(|r| decode_packed_ids(&r.ids))
        .unwrap_or_default()
}

fn decode_relationship_edges(results: impl IntoIterator<Item = Results>) -> RelationshipEdges {
    let mut results = results.into_iter();
    RelationshipEdges {
        follows: next_edge_set(&mut results),
        blocks: next_edge_set(&mut results),
        mutes: next_edge_set(&mut results),
        mute_retweets: next_edge_set(&mut results),
    }
}

fn followed_by_queries(viewer_id: u64, user_ids: &[i64]) -> Vec<SelectQuery> {
    user_ids
        .chunks(REVERSE_EDGE_CHUNK_SIZE)
        .map(|chunk| edge_membership_query(viewer_id, FOLLOWS_GRAPH_ID, Reverse, chunk))
        .collect()
}

fn merge_edge_sets(results: impl IntoIterator<Item = Results>) -> HashSet<u64> {
    results
        .into_iter()
        .flat_map(|r| decode_packed_ids(&r.ids))
        .collect()
}

fn membership_map(ids: &[u64], edge_set: &HashSet<u64>) -> HashMap<u64, bool> {
    ids.iter().map(|&id| (id, edge_set.contains(&id))).collect()
}

async fn select_edge_set(
    client: &FlockClient,
    queries: Vec<SelectQuery>,
    label: &'static str,
) -> Option<HashSet<u64>> {
    let request = SelectRequest {
        queries,
        ancestor_client_id: None,
        service_account: None,
        quota_name: None,
    };
    let mut request = tonic::Request::new(request);
    xai_x_rpc::apply_call_deadline(&mut request);
    match client.inner().clone().select(request).await {
        Ok(resp) => Some(merge_edge_sets(resp.into_inner().results)),
        Err(e) => {
            warn!(error = %e, label, "FlockDB select failed, defaulting to empty set");
            None
        }
    }
}

pub struct ProdSocialgraphClient {
    flock_client: FlockClient,
}

impl ProdSocialgraphClient {
    pub async fn new(
        datacenter: &str,
        ca_cert_path: &str,
        client_cert_path: &str,
        client_key_path: &str,
    ) -> anyhow::Result<Self> {
        let flock_client =
            FlockClient::from_s2s(datacenter, ca_cert_path, client_cert_path, client_key_path)
                .await?;
        Ok(Self { flock_client })
    }
}

#[async_trait]
impl SocialgraphClient for ProdSocialgraphClient {
    async fn batch_check_relationships(
        &self,
        viewer_id: u64,
        author_ids: &[u64],
    ) -> HashMap<u64, ViewerAuthorRelationship> {
        if author_ids.is_empty() {
            return HashMap::new();
        }

        let dest_ids: Vec<i64> = author_ids.iter().map(|&id| id.cast_signed()).collect();
        let mut request = tonic::Request::new(relationship_select_request(viewer_id, &dest_ids));
        xai_x_rpc::apply_call_deadline(&mut request);

        let edges = match self.flock_client.inner().clone().select(request).await {
            Ok(resp) => decode_relationship_edges(resp.into_inner().results),
            Err(e) => {
                warn!(
                    error = %e,
                    "FlockDB multi-query select failed, returning no relationships"
                );
                return HashMap::new();
            }
        };

        author_ids
            .iter()
            .map(|&author_id| {
                (
                    author_id,
                    ViewerAuthorRelationship {
                        viewer_follows_author: edges.follows.contains(&author_id),
                        viewer_blocks_author: edges.blocks.contains(&author_id),
                        viewer_mutes_author: edges.mutes.contains(&author_id),
                        viewer_mutes_retweets_from_author: edges.mute_retweets.contains(&author_id),
                    },
                )
            })
            .collect()
    }

    async fn batch_check_super_follows(
        &self,
        viewer_id: u64,
        author_ids: &[u64],
    ) -> Option<HashMap<u64, bool>> {
        let dest_ids: Vec<i64> = author_ids.iter().map(|&id| id.cast_signed()).collect();

        let super_set = select_edge_set(
            &self.flock_client,
            vec![edge_membership_query(
                viewer_id,
                SUPER_FOLLOWS_GRAPH_ID,
                Forward,
                &dest_ids,
            )],
            "super_follows",
        )
        .await?;

        Some(membership_map(author_ids, &super_set))
    }

    async fn batch_check_followed_by(
        &self,
        viewer_id: u64,
        user_ids: &[u64],
    ) -> Option<HashMap<u64, bool>> {
        if user_ids.is_empty() {
            return Some(HashMap::new());
        }

        let mut seen = HashSet::new();
        let dest_ids: Vec<i64> = user_ids
            .iter()
            .filter(|&&id| seen.insert(id))
            .map(|&id| id.cast_signed())
            .collect();

        let follower_set = select_edge_set(
            &self.flock_client,
            followed_by_queries(viewer_id, &dest_ids),
            "followed_by",
        )
        .await?;

        Some(membership_map(user_ids, &follower_set))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_packed_ids_decodes_little_endian_chunks_and_ignores_trailing_bytes() {
        let packed = [
            0x08, 0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01, 0xff, 0xee, 0xdd, 0xcc, 0xbb, 0xaa,
            0x99, 0x88, 0x42, 0x24,
        ];
        assert_eq!(
            decode_packed_ids(&packed),
            HashSet::from([0x0102_0304_0506_0708, 0x8899_aabb_ccdd_eeff])
        );
        assert!(decode_packed_ids(&[]).is_empty());
    }

    #[test]
    fn relationship_select_request_packs_four_graphs_in_order() {
        let dest = vec![10i64, 20];
        let request = relationship_select_request(999, &dest);
        assert_eq!(request.queries.len(), 4);
        let expected_graphs = [
            FOLLOWS_GRAPH_ID,
            BLOCKS_GRAPH_ID,
            MUTE_GRAPH_ID,
            MUTE_RETWEETS_GRAPH_ID,
        ];
        for (query, &graph_id) in request.queries.iter().zip(expected_graphs.iter()) {
            let term = query.operations[0].term.as_ref().unwrap();
            assert_eq!(term.source_id, 999);
            assert_eq!(term.graph_id, graph_id);
            assert_eq!(term.destination_ids.as_ref().unwrap().ids, dest);
        }
    }

    #[test]
    fn followed_by_queries_reverse_follows_graph_in_chunks() {
        let user_ids: Vec<i64> = (1..=REVERSE_EDGE_CHUNK_SIZE as i64 + 1).collect();
        let queries = followed_by_queries(999, &user_ids);
        assert_eq!(queries.len(), 2);
        for (query, chunk) in queries.iter().zip(user_ids.chunks(REVERSE_EDGE_CHUNK_SIZE)) {
            let term = query.operations[0].term.as_ref().unwrap();
            assert_eq!(term.graph_id, FOLLOWS_GRAPH_ID);
            assert!(!term.is_forward);
            assert_eq!(term.destination_ids.as_ref().unwrap().ids, chunk);
        }
    }

    #[test]
    fn merged_edge_set_membership_marks_only_response_ids() {
        let pack = |ids: &[i64]| -> Results {
            Results {
                ids: ids.iter().flat_map(|id| id.to_le_bytes()).collect(),
                next_cursor: 0,
                prev_cursor: 0,
            }
        };
        let edge_set = merge_edge_sets([pack(&[2]), pack(&[4])]);
        assert_eq!(
            membership_map(&[1, 2, 3, 4], &edge_set),
            HashMap::from([(1, false), (2, true), (3, false), (4, true)])
        );
    }

    #[test]
    fn decode_relationship_edges_maps_named_fields_and_missing_slots_fail_open() {
        let pack = |ids: &[i64]| -> Results {
            Results {
                ids: ids.iter().flat_map(|id| id.to_le_bytes()).collect(),
                next_cursor: 0,
                prev_cursor: 0,
            }
        };
        assert_eq!(
            decode_relationship_edges([pack(&[1, 2]), pack(&[3]), pack(&[]), pack(&[4, 5, 6])]),
            RelationshipEdges {
                follows: HashSet::from([1, 2]),
                blocks: HashSet::from([3]),
                mutes: HashSet::new(),
                mute_retweets: HashSet::from([4, 5, 6]),
            }
        );
        assert_eq!(
            decode_relationship_edges([pack(&[1])]),
            RelationshipEdges {
                follows: HashSet::from([1]),
                ..RelationshipEdges::default()
            }
        );
    }
}
