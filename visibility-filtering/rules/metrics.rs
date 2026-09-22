use std::cell::Cell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use xai_stats_receiver::{global_stats_receiver, HistogramBuckets};

use crate::models::Verdict;
use crate::rules::SafetyLevel;
use crate::treatment;

const REQUESTS: &str = "filter_tweets_requests";
const LATENCY_MS: &str = "filter_tweets_latency_ms";
pub(crate) const BATCH_SIZE: &str = "filter_tweets_batch_size";
const VERDICTS: &str = "filter_tweets_verdicts";
const VERDICTS_BY_RULE: &str = "filter_tweets_verdicts_by_rule";
const LOGGED_OUT_VIEWER: &str = "filter_tweets_logged_out_viewer";
const VIEWER_ID_NORMALIZED: &str = "filter_tweets_viewer_id_normalized";
const PHASE_MS: &str = "filter_tweets_phase_ms";
const DEADLINE: &str = "filter_tweets_deadline";
const DEADLINE_REMAINING_MS: &str = "filter_tweets_deadline_remaining_ms";
const DEADLINE_OVERRUN_MS: &str = "filter_tweets_deadline_overrun_ms";

#[derive(Clone, Copy, strum::IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum Rpc {
    FilterTweets,
    EvaluateTweets,
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

pub(crate) struct RequestMetricsGuard {
    requests: &'static str,
    latency_ms: &'static str,
    start: Instant,
    outcome: Cell<&'static str>,
}

impl RequestMetricsGuard {
    pub(crate) fn new() -> Self {
        Self::named(REQUESTS, LATENCY_MS)
    }

    pub(crate) fn named(requests: &'static str, latency_ms: &'static str) -> Self {
        incr(requests, &[("outcome", "started")], 1);
        Self {
            requests,
            latency_ms,
            start: Instant::now(),
            outcome: Cell::new("cancelled"),
        }
    }

    pub(crate) fn mark_success(&self) {
        self.outcome.set("success");
    }

    pub(crate) fn mark_failure(&self) {
        self.outcome.set("failure");
    }

    pub(crate) fn record_deadline(&self, grpc_timeout: Option<Duration>) {
        let Some(deadline) = grpc_timeout else {
            incr(DEADLINE, &[("outcome", "absent")], 1);
            return;
        };
        let elapsed = self.start.elapsed();
        if elapsed <= deadline {
            incr(DEADLINE, &[("outcome", "within")], 1);
            observe_vm(DEADLINE_REMAINING_MS, &[], millis(deadline - elapsed));
        } else {
            incr(DEADLINE, &[("outcome", "overrun")], 1);
            observe_vm(DEADLINE_OVERRUN_MS, &[], millis(elapsed - deadline));
        }
    }
}

pub(crate) fn record_phase(rpc: Rpc, stage: &'static str, elapsed: Duration) {
    observe_vm(
        PHASE_MS,
        &[("stage", stage), ("rpc", rpc.into())],
        millis(elapsed),
    );
}

fn millis(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

impl Drop for RequestMetricsGuard {
    fn drop(&mut self) {
        incr(self.requests, &[("outcome", self.outcome.get())], 1);
        observe_vm(self.latency_ms, &[], millis(self.start.elapsed()));
    }
}

pub(crate) fn record_batch_size(metric: &str, size: usize) {
    observe(metric, &[], size as f64, HistogramBuckets::Bucket50To500);
}

fn incr_nonzero(metric: &str, labels: &[(&str, &str)], count: u64) {
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
        Decided, LimitedEngagement, LimitedEngagementReason, MediaInterstitial, Withholding,
    };
    use xai_visibility_filtering::models::FilteredReason;
    use xai_x_thrift::action::InterstitialReason;

    fn allow() -> Verdict {
        Verdict::Shown {
            media: None,
            engagement: None,
        }
    }

    fn drop_by(rule: &'static str) -> Verdict {
        Verdict::Withheld(Decided {
            value: Withholding::Drop(FilteredReason::UnspecifiedReason),
            by: rule,
        })
    }

    fn limit_by(rule: &'static str) -> Decided<LimitedEngagement> {
        Decided {
            value: LimitedEngagement(LimitedEngagementReason::ConversationControl),
            by: rule,
        }
    }

    #[test]
    fn dashboard_generator_pins_the_rpc_label() {
        let cargo = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/dashboard.py");
        let ws = "crates/x-product/xai-visibility-filtering-service/scripts/dashboard.py";
        let path = if std::path::Path::new(cargo).exists() {
            cargo
        } else {
            ws
        };
        let dashboard =
            std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
        let filter_tweets = <&str>::from(Rpc::FilterTweets);
        assert!(dashboard.contains(&format!("FT_RPC_FILTER = 'rpc=~\"{filter_tweets}|\"'")));
    }

    #[test]
    fn allows_only_on_mix_metric() {
        let a = allow();
        let b = allow();
        let d = drop_by("suspended_author");
        let aggregated = aggregate_verdicts([&a, &b, &d]);

        assert_eq!(aggregated.mix.get("allow"), Some(&2));
        assert_eq!(aggregated.mix.get("drop"), Some(&1));
        assert!(!aggregated.mix.contains_key("interstitial"));
        assert_eq!(aggregated.by_rule.len(), 1);
        assert_eq!(
            aggregated.by_rule.get(&("suspended_author", "drop")),
            Some(&1)
        );
    }

    #[test]
    fn unresolved_author_verdict_counts_on_by_rule() {
        let d = Verdict::unresolved_author();
        let a = allow();
        let aggregated = aggregate_verdicts([&d, &a]);

        assert_eq!(aggregated.mix.get("drop"), Some(&1));
        assert_eq!(aggregated.mix.get("allow"), Some(&1));
        assert_eq!(
            aggregated.by_rule.get(&("unresolved_author_id", "drop")),
            Some(&1)
        );
    }

    #[test]
    fn each_filled_slot_counts_its_own_row() {
        let d = drop_by("nsfw_media");
        let both = Verdict::Shown {
            media: Some(Decided {
                value: MediaInterstitial {
                    legacy: FilteredReason::ContainNsfwMedia,
                    reason: InterstitialReason::Sensitive(true),
                },
                by: "nsfw_media",
            }),
            engagement: Some(limit_by("conversation_control")),
        };
        let limit_only = Verdict::Shown {
            media: None,
            engagement: Some(limit_by("conversation_control")),
        };
        let aggregated = aggregate_verdicts([&d, &both, &limit_only]);

        assert_eq!(aggregated.mix.get("drop"), Some(&1));
        assert_eq!(aggregated.mix.get("tweet_interstitial"), Some(&1));
        assert_eq!(aggregated.mix.get("limited_engagement"), Some(&1));
        assert_eq!(aggregated.mix.values().sum::<u64>(), 3);
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
