use crate::clients::vm_ranker_client::{VMRankerClient, VMRankerCluster};
use crate::models::candidate::PostCandidate;
use crate::models::query::ScoredPostsQuery;
use crate::params::*;
use crate::scorers::vm_ranker_request::{write_candidate_inputs, write_value_model_inputs};
use rustc_hash::FxHashMap;
use std::sync::Arc;
use tonic::async_trait;
use xai_candidate_pipeline::scorer::Scorer;
use xai_stats_receiver::global_stats_receiver;
use xai_vm_ranker_proto::{DppParams, RankCandidate, RankRequest, RankResponse};

const DPP_VALUE_MODEL_ID: &str = "dpp";
const METRIC_PREFIX: &str = "VMRanker";

pub struct VMRanker {
    pub client: Arc<dyn VMRankerClient>,
    pub xds_client: Option<Arc<dyn VMRankerClient>>,
}

impl VMRanker {
    async fn rank(
        &self,
        query: &ScoredPostsQuery,
        cluster: VMRankerCluster,
        request: RankRequest,
    ) -> Result<RankResponse, String> {
        let use_xds = self.xds_client.is_some()
            && crate::util::xds::use_xds_for_vm_ranker_cluster(query, &cluster.gate_name());

        if use_xds {
            let xds = self.xds_client.as_ref().expect("checked is_some above");
            match xds.rank(cluster, request.clone()).await {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    if !query.params.get(VMRankerEnableFallback) {
                        return Err(format!(
                            "VMRanker xDS gRPC call failed (fallback disabled): {e}"
                        ));
                    }
                    tracing::warn!(cluster = ?cluster, error = %e, "VMRanker xDS rank failed; falling back to DNS");
                }
            }
        }

        self.client
            .rank(cluster, request)
            .await
            .map_err(|e| format!("VMRanker gRPC call failed: {e}"))
    }
}

#[async_trait]
impl Scorer<ScoredPostsQuery, PostCandidate> for VMRanker {
    fn enable(&self, query: &ScoredPostsQuery) -> bool {
        query.params.get(EnableVMRanker)
    }

    async fn score(
        &self,
        query: &ScoredPostsQuery,
        candidates: &[PostCandidate],
    ) -> Vec<Result<PostCandidate, String>> {
        let cluster = VMRankerCluster::parse(&query.params.get(VMRankerClusterId));
        let send_value_model_inputs = query.params.get(VMRankerSendValueModelInputs);
        let compute_value_model =
            send_value_model_inputs && query.params.get(VMRankerComputeValueModel);
        record_request_mode(if compute_value_model {
            "compute_value_model"
        } else if send_value_model_inputs {
            "send_inputs"
        } else {
            "dpp_only"
        });

        let request = build_request(
            query,
            candidates,
            send_value_model_inputs,
            compute_value_model,
        );

        let response = match self.rank(query, cluster, request).await {
            Ok(resp) => resp,
            Err(msg) => {
                record_fallback("rpc_error", candidates.len());
                return vec![Err(msg); candidates.len()];
            }
        };

        let score_map: FxHashMap<u64, f64> = response
            .candidates
            .iter()
            .map(|sc| (sc.tweet_id, sc.score))
            .collect();

        let mut missing = 0;
        let scored = candidates
            .iter()
            .map(|c| {
                let score = match score_map.get(&c.tweet_id) {
                    Some(&score) => Some(score),
                    None => {
                        missing += 1;
                        c.score
                    }
                };
                Ok(PostCandidate {
                    score,
                    ..Default::default()
                })
            })
            .collect();
        if missing > 0 {
            record_fallback("missing_candidate", missing);
        }
        scored
    }

    fn update(&self, candidate: &mut PostCandidate, scored: PostCandidate) {
        candidate.score = scored.score;
    }
}

fn record_request_mode(mode: &str) {
    if let Some(receiver) = global_stats_receiver() {
        receiver.incr(&format!("{METRIC_PREFIX}.request"), &[("mode", mode)], 1);
    }
}

fn record_fallback(reason: &str, candidate_count: usize) {
    if let Some(receiver) = global_stats_receiver() {
        receiver.incr(
            &format!("{METRIC_PREFIX}.local_score_fallback"),
            &[("reason", reason)],
            candidate_count as u64,
        );
    }
}

fn build_request(
    query: &ScoredPostsQuery,
    candidates: &[PostCandidate],
    send_value_model_inputs: bool,
    compute_value_model: bool,
) -> RankRequest {
    let proto_candidates: Vec<RankCandidate> = candidates
        .iter()
        .map(|c| {
            let mut rank_candidate = RankCandidate {
                tweet_id: c.tweet_id,
                retweeted_tweet_id: c.retweeted_tweet_id.unwrap_or(0),
                score: c.score,
                ..Default::default()
            };
            if send_value_model_inputs {
                write_candidate_inputs(c, &mut rank_candidate);
            }
            rank_candidate
        })
        .collect();

    let dpp_theta = query.params.get(VMRankerDppTheta);
    let dpp_max_selected_rank = query.params.get(VMRankerDppMaxSelectedRank);

    let dpp_params = if dpp_theta > 0.0 || dpp_max_selected_rank > 0 {
        Some(DppParams {
            theta: dpp_theta,
            max_selected_rank: dpp_max_selected_rank,
        })
    } else {
        None
    };

    let mut request = RankRequest {
        viewer_id: query.user_id,
        candidates: proto_candidates,
        value_model_id: DPP_VALUE_MODEL_ID.to_string(),
        dpp_params,
        ..Default::default()
    };

    if send_value_model_inputs {
        write_value_model_inputs(query, compute_value_model, &mut request);
    }

    request
}
