use std::cell::Cell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use xai_stats_receiver::{global_stats_receiver, HistogramBuckets};

use crate::hydration::{Cause, Hydrators, Lookup, INBOUND_ALLOWANCE};
use crate::models::{Evaluation, Verdict};
use crate::rules::SafetyLevel;
use crate::treatment;

const REQUESTS: &str = "filter_tweets_requests";
const LATENCY_MS: &str = "filter_tweets_latency_ms";
pub(crate) const BATCH_SIZE: &str = "filter_tweets_batch_size";
const VERDICTS: &str = "filter_tweets_verdicts";
const VERDICTS_BY_RULE: &str = "filter_tweets_verdicts_by_rule";
const EVALUATED_CANDIDATES: &str = "filter_tweets_evaluated_candidates";
const FAIL_OPEN_DEFAULTS_NONE: &str = "none";
const UNRESOLVED_TWEETS: &str = "vf_unresolved_tweets";
const LOGGED_OUT_VIEWER: &str = "filter_tweets_logged_out_viewer";
const VIEWER_ID_NORMALIZED: &str = "filter_tweets_viewer_id_normalized";
const PHASE_MS: &str = "filter_tweets_phase_ms";
const DEADLINE: &str = "filter_tweets_deadline";
const DEADLINE_REMAINING_MS: &str = "filter_tweets_deadline_remaining_ms";
const DEADLINE_OVERRUN_MS: &str = "filter_tweets_deadline_overrun_ms";

#[derive(Clone, Copy, Debug, PartialEq, Eq, strum::IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub enum Rpc {
    FilterTweets,
    EvaluateTweets,
}

#[derive(Clone, Copy, strum::IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum RequestOutcome {
    Started,
    Cancelled,
    Success,
    Failure,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, strum::IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum DeadlineOutcome {
    Absent,
    Within(Duration),
    Overrun(Duration),
    CancelledEarly,
    CancelledAtDeadline,
    CancelledNoDeadline,
}

impl DeadlineOutcome {
    pub(crate) fn of(
        outcome: RequestOutcome,
        grpc_timeout: Option<Duration>,
        elapsed: Duration,
    ) -> Option<Self> {
        match (outcome, grpc_timeout) {
            (RequestOutcome::Started | RequestOutcome::Failure, _) => None,
            (RequestOutcome::Success, None) => Some(Self::Absent),
            (RequestOutcome::Cancelled, None) => Some(Self::CancelledNoDeadline),
            (RequestOutcome::Success, Some(deadline)) => {
                Some(match deadline.checked_sub(elapsed) {
                    Some(remaining) => Self::Within(remaining),
                    None => Self::Overrun(elapsed - deadline),
                })
            }
            (RequestOutcome::Cancelled, Some(deadline)) => {
                Some(if elapsed + INBOUND_ALLOWANCE < deadline {
                    Self::CancelledEarly
                } else {
                    Self::CancelledAtDeadline
                })
            }
        }
    }
}

#[derive(Clone, Copy, strum::IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum Stage {
    Hydration,
    PostHydration,
}

pub(crate) fn record_viewer_state(raw: Option<u64>, normalized: Option<u64>) {
    if normalized.is_some() {
        return;
    }
    incr(LOGGED_OUT_VIEWER, &[], 1);
    if raw.is_some() {
        incr(VIEWER_ID_NORMALIZED, &[], 1);
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct AggregatedVerdicts {
    pub mix: HashMap<&'static str, u64>,
    pub by_rule: HashMap<(&'static str, &'static str), u64>,
}

pub(crate) fn aggregate_verdicts<'a>(
    verdicts: impl IntoIterator<Item = &'a Verdict>,
) -> AggregatedVerdicts {
    let mut out = AggregatedVerdicts::default();
    for verdict in verdicts {
        *out.mix.entry(treatment::metric_label(verdict)).or_default() += 1;
        for row in treatment::decided_rows(verdict) {
            *out.by_rule.entry(row).or_default() += 1;
        }
    }
    out
}

pub(crate) fn record_verdicts<'a>(
    rpc: Rpc,
    safety_level: SafetyLevel,
    verdicts: impl IntoIterator<Item = &'a Verdict>,
) {
    let aggregated = aggregate_verdicts(verdicts);
    let level = <&str>::from(safety_level);
    let rpc = rpc.into();
    for (action, count) in &aggregated.mix {
        incr_nonzero(
            VERDICTS,
            &[("action", action), ("safety_level", level), ("rpc", rpc)],
            *count,
        );
    }
    for ((rule, action), count) in &aggregated.by_rule {
        incr_nonzero(
            VERDICTS_BY_RULE,
            &[
                ("rule", rule),
                ("action", action),
                ("safety_level", level),
                ("rpc", rpc),
            ],
            *count,
        );
    }
}

pub(crate) fn record_unresolved<'a>(
    rpc: Rpc,
    safety_level: SafetyLevel,
    evaluations: impl IntoIterator<Item = &'a Evaluation>,
) {
    let mut counts: HashMap<(Lookup, Cause), u64> = HashMap::new();
    for evaluation in evaluations {
        let unresolved = match *evaluation {
            Evaluation::NotFound(lookup) => (lookup, Cause::NotFound),
            Evaluation::Failed(lookup) => (lookup, Cause::Failed),
            Evaluation::Complete { .. } | Evaluation::Partial { .. } => continue,
        };
        *counts.entry(unresolved).or_default() += 1;
    }
    let level = <&str>::from(safety_level);
    let rpc = rpc.into();
    for ((lookup, cause), count) in counts {
        incr(
            UNRESOLVED_TWEETS,
            &[
                ("lookup", lookup.into()),
                ("cause", cause.into()),
                ("rpc", rpc),
                ("safety_level", level),
            ],
            count,
        );
    }
}

pub(crate) fn record_fail_open_defaults(
    rpc: Rpc,
    safety_level: SafetyLevel,
    fail_open_defaults: impl IntoIterator<Item = Hydrators>,
) {
    let mut on_none = 0;
    let mut on_failed: HashMap<Hydrators, u64> = HashMap::new();
    for nodes in fail_open_defaults {
        if nodes.is_empty() {
            on_none += 1;
        } else {
            *on_failed.entry(nodes).or_default() += 1;
        }
    }
    let level = <&str>::from(safety_level);
    let rpc = rpc.into();
    incr_nonzero(
        EVALUATED_CANDIDATES,
        &[
            ("fail_open_defaults", FAIL_OPEN_DEFAULTS_NONE),
            ("safety_level", level),
            ("rpc", rpc),
        ],
        on_none,
    );
    for (nodes, count) in on_failed {
        let nodes: Vec<&str> = nodes.iter().map(<&str>::from).collect();
        incr(
            EVALUATED_CANDIDATES,
            &[
                ("fail_open_defaults", &nodes.join("+")),
                ("safety_level", level),
                ("rpc", rpc),
            ],
            count,
        );
    }
}

enum DeadlineCount {
    Skip,
    Count { grpc_timeout: Option<Duration> },
}

pub(crate) struct RequestMetricsGuard {
    requests: &'static str,
    latency_ms: &'static str,
    deadline: DeadlineCount,
    start: Instant,
    outcome: Cell<RequestOutcome>,
}

impl RequestMetricsGuard {
    pub(crate) fn new(grpc_timeout: Option<Duration>) -> Self {
        Self::start(REQUESTS, LATENCY_MS, DeadlineCount::Count { grpc_timeout })
    }

    pub(crate) fn named(requests: &'static str, latency_ms: &'static str) -> Self {
        Self::start(requests, latency_ms, DeadlineCount::Skip)
    }

    fn start(requests: &'static str, latency_ms: &'static str, deadline: DeadlineCount) -> Self {
        incr(requests, &[("outcome", RequestOutcome::Started.into())], 1);
        Self {
            requests,
            latency_ms,
            deadline,
            start: Instant::now(),
            outcome: Cell::new(RequestOutcome::Cancelled),
        }
    }

    pub(crate) fn mark_success(&self) {
        self.outcome.set(RequestOutcome::Success);
    }

    pub(crate) fn mark_failure(&self) {
        self.outcome.set(RequestOutcome::Failure);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, strum::IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum RetweetSources {
    #[strum(to_string = "none")]
    NoSource,
    InBatch,
    Fetched,
}

pub(crate) fn record_phase(rpc: Rpc, stage: Stage, sources: RetweetSources, elapsed: Duration) {
    observe_vm(
        PHASE_MS,
        &[
            ("stage", stage.into()),
            ("rpc", rpc.into()),
            ("sources", sources.into()),
        ],
        millis(elapsed),
    );
}

fn millis(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

impl Drop for RequestMetricsGuard {
    fn drop(&mut self) {
        let elapsed = self.start.elapsed();
        let outcome = self.outcome.get();
        incr(self.requests, &[("outcome", outcome.into())], 1);
        observe_vm(self.latency_ms, &[], millis(elapsed));
        let DeadlineCount::Count { grpc_timeout } = self.deadline else {
            return;
        };
        let Some(deadline_outcome) = DeadlineOutcome::of(outcome, grpc_timeout, elapsed) else {
            return;
        };
        incr(DEADLINE, &[("outcome", deadline_outcome.into())], 1);
        match deadline_outcome {
            DeadlineOutcome::Within(remaining) => {
                observe_vm(DEADLINE_REMAINING_MS, &[], millis(remaining));
            }
            DeadlineOutcome::Overrun(overrun) => {
                observe_vm(DEADLINE_OVERRUN_MS, &[], millis(overrun));
            }
            DeadlineOutcome::Absent
            | DeadlineOutcome::CancelledEarly
            | DeadlineOutcome::CancelledAtDeadline
            | DeadlineOutcome::CancelledNoDeadline => {}
        }
    }
}

pub(crate) fn record_batch_size(metric: &str, size: usize) {
    observe(metric, &[], size as f64, HistogramBuckets::Bucket50To500);
}

pub(crate) fn incr_nonzero(metric: &str, labels: &[(&str, &str)], count: u64) {
    if count > 0 {
        incr(metric, labels, count);
    }
}

fn incr(metric: &str, labels: &[(&str, &str)], count: u64) {
    if let Some(sr) = global_stats_receiver() {
        sr.incr(metric, labels, count);
    }
}

fn observe(metric: &str, labels: &[(&str, &str)], value: f64, buckets: HistogramBuckets) {
    if let Some(sr) = global_stats_receiver() {
        sr.observe(metric, labels, value, buckets);
    }
}

fn observe_vm(metric: &str, labels: &[(&str, &str)], value: f64) {
    if let Some(sr) = global_stats_receiver() {
        sr.observe_vm(metric, labels, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        Decided, DropReason, LimitedEngagement, LimitedEngagementReason, MediaInterstitial,
        MediaRestriction, Withholding,
    };
    use std::fs;
    use std::path::Path;
    use xai_visibility_filtering::models::FilteredReason;
    use xai_visibility_filtering::vf_client::FILTER_TWEETS_LATENCY_VM_METRIC;
    use xai_x_thrift::action::InterstitialReason;

    fn allow() -> Verdict {
        Verdict::Shown {
            notice: None,
            media: None,
            engagement: None,
        }
    }

    fn drop_by(rule: &'static str) -> Verdict {
        Verdict::Withheld(Decided {
            value: Withholding::Drop(DropReason::Legacy(FilteredReason::UnspecifiedReason)),
            by: rule,
        })
    }

    fn limit_by(rule: &'static str) -> Decided<LimitedEngagement> {
        Decided {
            value: LimitedEngagement::new(LimitedEngagementReason::ConversationControl),
            by: rule,
        }
    }

    #[test]
    fn dashboard_generator_pins_the_evaluated_candidates_unresolved_tweets_and_deadline_metrics() {
        let cargo = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/dashboard.py");
        let ws = "crates/x-product/xai-visibility-filtering-service/scripts/dashboard.py";
        let path = if Path::new(cargo).exists() { cargo } else { ws };
        let dashboard = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
        assert!(dashboard.contains(&format!(
            "FT_EVALUATED_CANDIDATES_METRIC = \"{EVALUATED_CANDIDATES}\""
        )));
        assert!(dashboard.contains(&format!(
            "FT_FAIL_OPEN_DEFAULTS_FILTER = 'fail_open_defaults!~\"{FAIL_OPEN_DEFAULTS_NONE}|\"'"
        )));
        assert!(dashboard.contains(&format!(
            "UNRESOLVED_TWEETS_METRIC = \"{UNRESOLVED_TWEETS}\""
        )));
        assert!(dashboard.contains(&format!("FT_DEADLINE_METRIC = \"{DEADLINE}\"")));
        assert!(dashboard.contains(&format!(
            "FT_DEADLINE_REMAINING_METRIC = \"{DEADLINE_REMAINING_MS}\""
        )));
        assert!(dashboard.contains(&format!(
            "VF_CLIENT_FT_LATENCY_VM_METRIC = \"{FILTER_TWEETS_LATENCY_VM_METRIC}\""
        )));
        for outcome in [
            DeadlineOutcome::CancelledEarly,
            DeadlineOutcome::CancelledAtDeadline,
            DeadlineOutcome::CancelledNoDeadline,
            DeadlineOutcome::Overrun(Duration::ZERO),
        ] {
            let outcome: &str = outcome.into();
            assert!(dashboard.contains(&format!("'outcome=\"{outcome}\"'")));
        }
    }

    #[test]
    fn a_cancel_counts_whether_the_caller_hung_up_before_its_deadline() {
        let deadline = Some(Duration::from_millis(400));
        let of = |outcome, ms| DeadlineOutcome::of(outcome, deadline, Duration::from_millis(ms));
        assert_eq!(
            of(RequestOutcome::Cancelled, 389),
            Some(DeadlineOutcome::CancelledEarly)
        );
        assert_eq!(
            of(RequestOutcome::Cancelled, 390),
            Some(DeadlineOutcome::CancelledAtDeadline)
        );
        assert_eq!(
            of(RequestOutcome::Cancelled, 401),
            Some(DeadlineOutcome::CancelledAtDeadline)
        );
        assert_eq!(
            DeadlineOutcome::of(RequestOutcome::Cancelled, None, Duration::ZERO),
            Some(DeadlineOutcome::CancelledNoDeadline)
        );
        assert_eq!(
            DeadlineOutcome::of(RequestOutcome::Success, None, Duration::ZERO),
            Some(DeadlineOutcome::Absent)
        );
        assert_eq!(
            of(RequestOutcome::Success, 400),
            Some(DeadlineOutcome::Within(Duration::ZERO))
        );
        assert_eq!(
            of(RequestOutcome::Success, 401),
            Some(DeadlineOutcome::Overrun(Duration::from_millis(1)))
        );
        assert_eq!(of(RequestOutcome::Failure, 5), None);
    }

    #[test]
    fn each_filled_slot_counts_its_own_row() {
        let a = allow();
        let b = allow();
        let d = drop_by("nsfw_media");
        let both = Verdict::Shown {
            notice: None,
            media: Some(Decided {
                value: MediaRestriction::MediaInterstitial(MediaInterstitial {
                    legacy: FilteredReason::ContainNsfwMedia,
                    reason: InterstitialReason::Sensitive(true),
                    prompt: None,
                }),
                by: "nsfw_media",
            }),
            engagement: Some(limit_by("conversation_control")),
        };
        let limit_only = Verdict::Shown {
            notice: None,
            media: None,
            engagement: Some(limit_by("conversation_control")),
        };
        let aggregated = aggregate_verdicts([&a, &b, &d, &both, &limit_only]);

        assert_eq!(aggregated.mix.get("allow"), Some(&2));
        assert_eq!(aggregated.mix.get("drop"), Some(&1));
        assert_eq!(aggregated.mix.get("tweet_interstitial"), Some(&1));
        assert_eq!(aggregated.mix.get("limited_engagement"), Some(&1));
        assert_eq!(aggregated.mix.values().sum::<u64>(), 5);
        assert_eq!(aggregated.by_rule.values().sum::<u64>(), 4);
        assert_eq!(aggregated.by_rule.get(&("nsfw_media", "drop")), Some(&1));
        assert_eq!(
            aggregated.by_rule.get(&("nsfw_media", "interstitial")),
            Some(&1)
        );
        assert_eq!(
            aggregated
                .by_rule
                .get(&("conversation_control", "limited_engagement")),
            Some(&2)
        );
    }
}
