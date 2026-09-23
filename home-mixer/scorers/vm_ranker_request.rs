use crate::models::candidate::{PhoenixScores, PostCandidate};
use crate::models::fs_recipient::FsRecipientInputs;
use crate::models::query::ScoredPostsQuery;
use crate::params::*;
use crate::scorers::ranking_scorer::RankingScorer;
use crate::scorers::vm_ranker_debias_payload::{candidate_payload, request_payload};
use xai_vm_ranker_proto as pb;

pub(crate) struct OptionalInputs {
    value_model: bool,
    compute_value_model: bool,
    cached_weighted_score: bool,
    debias: bool,
}

impl OptionalInputs {
    pub(crate) fn from_query(query: &ScoredPostsQuery) -> Self {
        let value_model = query.params.get(VMRankerSendValueModelInputs);
        let cached_weighted_score =
            value_model && RankingScorer::reuses_cached_weighted_score(query);
        Self {
            value_model,
            compute_value_model: value_model && query.params.get(VMRankerComputeValueModel),
            cached_weighted_score,
            debias: value_model
                && !cached_weighted_score
                && query.params.get(VMRankerSendDebiasInputs),
        }
    }

    pub(crate) fn mode(&self) -> &'static str {
        if self.compute_value_model {
            "compute_value_model"
        } else if self.value_model {
            "send_inputs"
        } else {
            "dpp_only"
        }
    }

    pub(crate) fn write_request(
        &self,
        query: &ScoredPostsQuery,
        candidates: &[PostCandidate],
        request: &mut pb::RankRequest,
    ) {
        if self.value_model {
            write_value_model_inputs(query, self.compute_value_model, request);
        }
        if self.debias {
            request.experiment_payload = request_payload(query, candidates).into();
        }
    }

    pub(crate) fn write_candidate(&self, candidate: &PostCandidate, out: &mut pb::RankCandidate) {
        if self.value_model {
            write_candidate_inputs(candidate, out);
        }
        if self.cached_weighted_score {
            out.weighted_score = candidate.weighted_score;
        }
        if self.debias {
            out.experiment_payload = candidate_payload(candidate).into();
        }
    }
}

fn write_value_model_inputs(
    query: &ScoredPostsQuery,
    compute_value_model: bool,
    request: &mut pb::RankRequest,
) {
    request.compute_value_model = compute_value_model;
    request.viewer = query.fs_recipient_inputs.as_ref().map(viewer_context_proto);
    request.topic_request = query.is_topic_request();
}

fn write_candidate_inputs(candidate: &PostCandidate, out: &mut pb::RankCandidate) {
    out.author_id = candidate.author_id;
    out.in_network = candidate.in_network.unwrap_or(false);
    out.is_retweet = candidate.retweeted_tweet_id.is_some();
    out.is_reply = candidate.in_reply_to_tweet_id.is_some();
    out.is_mutual_follow_author = candidate.is_mutual_follow_author == Some(true);
    out.author_policy_zeroed = candidate.author_policy_zeroed;
    out.cold_start_lift_to_rank = candidate.cold_start_lift_to_rank;
    out.min_video_duration_ms = candidate.min_video_duration_ms;
    out.phoenix_scores = Some(phoenix_scores_proto(&candidate.phoenix_scores));
}

fn viewer_context_proto(inputs: &FsRecipientInputs) -> pb::ViewerContext {
    pb::ViewerContext {
        user_id: inputs.user_id,
        country_code: inputs.country_code.clone(),
        language_code: inputs.language_code.clone(),
        client_app_id: inputs.client_app_id,
        client_version: inputs.client_version.clone(),
        user_roles: inputs.user_roles.clone(),
        datacenter: inputs.datacenter.clone(),
        has_phone_number: inputs.has_phone_number,
        resurrection_time_ms: inputs.resurrection_time_ms,
        product: inputs.product.clone(),
        now_ms: inputs.now_ms,
        fs_overrides: inputs.fs_overrides.clone(),
    }
}

fn phoenix_scores_proto(s: &PhoenixScores) -> pb::PhoenixScores {
    pb::PhoenixScores {
        favorite_score: s.favorite_score,
        reply_score: s.reply_score,
        retweet_score: s.retweet_score,
        photo_expand_score: s.photo_expand_score,
        click_score: s.click_score,
        profile_click_score: s.profile_click_score,
        vqv_score: s.vqv_score,
        share_score: s.share_score,
        share_via_dm_score: s.share_via_dm_score,
        share_via_copy_link_score: s.share_via_copy_link_score,
        dwell_score: s.dwell_score,
        quote_score: s.quote_score,
        quoted_click_score: s.quoted_click_score,
        follow_author_score: s.follow_author_score,
        not_interested_score: s.not_interested_score,
        block_author_score: s.block_author_score,
        mute_author_score: s.mute_author_score,
        report_score: s.report_score,
        dwell_time: s.dwell_time,
        click_dwell_time: s.click_dwell_time,
        not_dwelled_score: s.not_dwelled_score,
        video_open_score: s.video_open_score,
        open_link_score: s.open_link_score,
        quoted_vqv_score: s.quoted_vqv_score,
        post_unexplored_score: s.post_unexplored_score,
        active_secs_5m_residual_norm: s.active_secs_5m_residual_norm,
    }
}
