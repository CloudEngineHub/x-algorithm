use std::collections::HashMap;
use std::time::{Duration, Instant};

use tonic::metadata::MetadataMap;
use tonic::{Request, Status};
use xai_stats_receiver::{global_stats_receiver, HistogramBuckets, StatsReceiverExt};

use crate::caller_identity;
use crate::hydration::{Cause, Hydrators, Lookup, INBOUND_ALLOWANCE};
use crate::models::{Evaluation, Verdict};
use crate::rules::SafetyLevel;
use crate::treatment;

const REQUESTS: &str = "vf_requests";
const LATENCY_MS: &str = "vf_request_latency_ms";
const DEADLINE: &str = "vf_request_deadline";
const BATCH_SIZE: &str = "vf_request_batch_size";
const VERDICTS: &str = "vf_verdicts";
const VERDICTS_BY_RULE: &str = "vf_verdicts_by_rule";
const EVALUATED_CANDIDATES: &str = "vf_evaluated_candidates";
const FAIL_OPEN_DEFAULTS_NONE: &str = "none";
const UNRESOLVED_TWEETS: &str = "vf_unresolved_tweets";
const LOGGED_OUT_VIEWER: &str = "vf_logged_out_viewer";
const VIEWER_ID_NORMALIZED: &str = "vf_viewer_id_normalized";
const PHASE_MS: &str = "vf_phase_ms";
const NOTIFICATION_RESULTS: &str = "filter_notifications_results";
const REQUESTS_BY_CALLER: &str = "vf_requests_by_caller";
const LATENCY_BY_CALLER_MS: &str = "vf_request_latency_by_caller_ms";
const UNKNOWN_IDENTITY: &str = "unknown";

#[derive(Clone, Copy, Debug, PartialEq, Eq, strum::IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub enum Rpc {
    FilterTweets,
    EvaluateTweets,
}

#[derive(Clone, Copy, strum::IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum Endpoint {
    FilterTweets,
    EvaluateTweets,
    GetSafetyLabels,
    FilterNotifications,
}

#[derive(Clone, Copy)]
pub(crate) enum RequestLevel {
    Named(SafetyLevel),
    Unknown,
    Absent,
}

impl RequestLevel {
    pub(crate) fn of(level: Option<SafetyLevel>) -> Self {
        level.map_or(Self::Unknown, Self::Named)
    }
}

impl From<RequestLevel> for &'static str {
    fn from(level: RequestLevel) -> Self {
        match level {
            RequestLevel::Named(level) => level.into(),
            RequestLevel::Unknown => "unknown",
            RequestLevel::Absent => "none",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, strum::IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub(crate) enum NotificationKind {
    Follow,
    FollowRequestAccepted,
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
    Within,
    Overrun,
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
            (RequestOutcome::Success, Some(deadline)) => Some(if elapsed <= deadline {
                Self::Within
            } else {
                Self::Overrun
            }),
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

pub(crate) fn record_verdicts<'a>(
    endpoint: Endpoint,
    safety_level: SafetyLevel,
    verdicts: impl IntoIterator<Item = &'a Verdict>,
) {
    let mut mix: HashMap<&str, u64> = HashMap::new();
    let mut by_rule: HashMap<(&str, &str), u64> = HashMap::new();
    for verdict in verdicts {
        *mix.entry(treatment::metric_label(verdict)).or_default() += 1;
        for row in treatment::decided_rows(verdict) {
            *by_rule.entry(row).or_default() += 1;
        }
    }
    let level = <&str>::from(safety_level);
    let rpc = endpoint.into();
    for (action, count) in &mix {
        incr_nonzero(
            VERDICTS,
            &[("action", action), ("safety_level", level), ("rpc", rpc)],
            *count,
        );
    }
    for ((rule, action), count) in &by_rule {
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

pub(crate) fn record_sent<'a>(
    endpoint: Endpoint,
    safety_level: SafetyLevel,
    evaluations: impl Iterator<Item = &'a Evaluation> + Clone,
    unevaluated: usize,
) {
    static LOOKUP_FAILED: Verdict = Verdict::lookup_failed();
    record_verdicts(
        endpoint,
        safety_level,
        evaluations
            .clone()
            .map(Evaluation::verdict)
            .chain(std::iter::repeat_n(&LOOKUP_FAILED, unevaluated)),
    );
    match endpoint {
        Endpoint::FilterTweets | Endpoint::EvaluateTweets => {
            record_unresolved(endpoint, safety_level, evaluations.clone());
        }
        Endpoint::GetSafetyLabels | Endpoint::FilterNotifications => {}
    }
    record_fail_open_defaults(
        endpoint,
        safety_level,
        evaluations.map(Evaluation::fail_open_defaults),
    );
}

fn record_unresolved<'a>(
    endpoint: Endpoint,
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
    let rpc = endpoint.into();
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

fn record_fail_open_defaults(
    endpoint: Endpoint,
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
    let rpc = endpoint.into();
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

pub(crate) async fn observe_request<R, T, F>(
    endpoint: Endpoint,
    level: RequestLevel,
    request: Request<R>,
    handle: impl FnOnce(Request<R>, xai_x_rpc::CallContext) -> F,
) -> Result<T, Status>
where
    F: Future<Output = Result<T, Status>>,
{
    let entered = tokio::time::Instant::now();
    let grpc_timeout = parse_grpc_timeout(request.metadata());
    incr(
        REQUESTS,
        &[
            ("rpc", endpoint.into()),
            ("safety_level", level.into()),
            ("outcome", RequestOutcome::Started.into()),
        ],
        1,
    );
    let mut guard = RequestMetricsGuard {
        endpoint,
        level,
        caller_identity: caller_identity::identity(&request),
        grpc_timeout,
        start: Instant::now(),
        outcome: RequestOutcome::Cancelled,
    };
    let result = handle(
        request,
        crate::hydration::request_context(entered, grpc_timeout),
    )
    .await;
    guard.outcome = if result.is_ok() {
        RequestOutcome::Success
    } else {
        RequestOutcome::Failure
    };
    result
}

pub(crate) fn parse_grpc_timeout(metadata: &MetadataMap) -> Option<Duration> {
    let raw = metadata.get("grpc-timeout")?.to_str().ok()?;
    let (digits, unit) = raw.split_at(raw.len().checked_sub(1)?);
    if digits.is_empty() || digits.len() > 8 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let value: u64 = digits.parse().ok()?;
    let nanos_per_unit = match unit {
        "H" => 3_600_000_000_000,
        "M" => 60_000_000_000,
        "S" => 1_000_000_000,
        "m" => 1_000_000,
        "u" => 1_000,
        "n" => 1,
        _ => return None,
    };
    Some(Duration::from_nanos(value.checked_mul(nanos_per_unit)?))
}

struct RequestMetricsGuard {
    endpoint: Endpoint,
    level: RequestLevel,
    caller_identity: Option<String>,
    grpc_timeout: Option<Duration>,
    start: Instant,
    outcome: RequestOutcome,
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

impl RequestMetricsGuard {
    fn emit(&self, sr: &dyn StatsReceiverExt, elapsed: Duration) {
        let outcome: &str = self.outcome.into();
        let rpc = ("rpc", self.endpoint.into());
        let level = ("safety_level", self.level.into());
        sr.incr(REQUESTS, &[rpc, level, ("outcome", outcome)], 1);
        let caller = (
            "caller_identity",
            self.caller_identity.as_deref().unwrap_or(UNKNOWN_IDENTITY),
        );
        sr.incr(REQUESTS_BY_CALLER, &[caller, rpc, ("outcome", outcome)], 1);
        if let RequestOutcome::Success = self.outcome {
            sr.observe_vm(LATENCY_MS, &[rpc], millis(elapsed));
            sr.observe_vm(LATENCY_BY_CALLER_MS, &[caller, rpc], millis(elapsed));
        }
        if let Some(deadline) = DeadlineOutcome::of(self.outcome, self.grpc_timeout, elapsed) {
            sr.incr(DEADLINE, &[rpc, level, ("outcome", deadline.into())], 1);
        }
    }
}

impl Drop for RequestMetricsGuard {
    fn drop(&mut self) {
        if let Some(sr) = global_stats_receiver() {
            self.emit(&*sr, self.start.elapsed());
        }
    }
}

pub(crate) fn record_notification_results(
    notification_type: &str,
    results: impl IntoIterator<Item = (&'static str, &'static str)>,
) {
    let mut counts: HashMap<(&str, &str), u64> = HashMap::new();
    for result in results {
        *counts.entry(result).or_default() += 1;
    }
    for ((outcome, reason), count) in counts {
        incr(
            NOTIFICATION_RESULTS,
            &[
                ("notification_type", notification_type),
                ("outcome", outcome),
                ("reason", reason),
            ],
            count,
        );
    }
}

pub(crate) fn record_batch_size(endpoint: Endpoint, size: usize) {
    observe(
        BATCH_SIZE,
        &[("rpc", endpoint.into())],
        size as f64,
        HistogramBuckets::Bucket50To500,
    );
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
pub(crate) mod thread_recorded {
    use std::cell::RefCell;
    use std::sync::Arc;
    use xai_stats_receiver::{HistogramBuckets, StatsReceiverExt};

    thread_local! {
        static SAMPLES: RefCell<Option<Vec<Sample>>> = const { RefCell::new(None) };
    }

    #[derive(Clone, Debug, PartialEq)]
    pub(crate) struct Sample {
        pub(crate) name: String,
        pub(crate) labels: Vec<(String, String)>,
        pub(crate) value: f64,
    }

    impl Sample {
        pub(crate) fn label(&self, name: &str) -> Option<&str> {
            self.labels
                .iter()
                .find(|(label, _)| label == name)
                .map(|(_, value)| value.as_str())
        }
    }

    struct ThreadRecorded;

    fn push(name: &str, scopes: &[(&str, &str)], value: f64) {
        SAMPLES.with_borrow_mut(|samples| {
            if let Some(samples) = samples {
                samples.push(Sample {
                    name: name.to_owned(),
                    labels: scopes
                        .iter()
                        .map(|&(label, value)| (label.to_owned(), value.to_owned()))
                        .collect(),
                    value,
                });
            }
        });
    }

    impl StatsReceiverExt for ThreadRecorded {
        fn incr(&self, name: &str, scopes: &[(&str, &str)], count: u64) {
            push(name, scopes, count as f64);
        }
        fn observe(&self, name: &str, scopes: &[(&str, &str)], value: f64, _: HistogramBuckets) {
            push(name, scopes, value);
        }
        fn observe_expo(&self, _: &str, _: &[(&str, &str)], _: f64) {}
        fn observe_vm(&self, name: &str, scopes: &[(&str, &str)], value: f64) {
            push(name, scopes, value);
        }
        fn gauge(&self, _: &str, _: &[(&str, &str)], _: f64) {}
    }

    pub(crate) fn start() {
        xai_stats_receiver::init_stats_receiver_with(Arc::new(ThreadRecorded));
        SAMPLES.set(Some(Vec::new()));
    }

    pub(crate) fn take(name: &str) -> Vec<Sample> {
        let samples = SAMPLES.take().unwrap_or_default();
        samples
            .into_iter()
            .filter(|sample| sample.name == name)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        Decided, DropReason, LimitedEngagement, LimitedEngagementReason, MediaInterstitial,
        MediaRestriction, Withholding,
    };
    use crate::quote::QUOTE_PASS;
    use crate::rules::fixtures::allow;
    use crate::rules::RuleId;
    use crate::safety_label_source::metrics::EXPIRED_LABELS;
    use xai_visibility_filtering::models::FilteredReason;
    use xai_visibility_filtering::vf_client::FILTER_TWEETS_LATENCY_VM_METRIC;
    use xai_x_thrift::action::InterstitialReason;

    fn drop_by(rule: &'static str) -> Verdict {
        Verdict::Withheld(Decided {
            value: Withholding::Drop(DropReason::Legacy(FilteredReason::UnspecifiedReason)),
            id: RuleId::FilterAll,
            by: rule,
        })
    }

    fn limit_by(rule: &'static str) -> Decided<LimitedEngagement> {
        Decided {
            value: LimitedEngagement::new(LimitedEngagementReason::ConversationControl),
            id: RuleId::FilterAll,
            by: rule,
        }
    }

    #[test]
    fn dashboard_generator_pins_the_server_metrics() {
        let dashboard =
            std::fs::read_to_string(crate::pinned::crate_path("scripts/dashboard.py")).unwrap();
        for (constant, family) in [
            ("VF_REQUESTS_METRIC", REQUESTS),
            ("VF_REQUEST_LATENCY_METRIC", LATENCY_MS),
            ("VF_REQUEST_DEADLINE_METRIC", DEADLINE),
            ("VF_REQUEST_BATCH_SIZE_METRIC", BATCH_SIZE),
            ("VF_VERDICTS_METRIC", VERDICTS),
            ("VF_VERDICTS_BY_RULE_METRIC", VERDICTS_BY_RULE),
            ("VF_EVALUATED_CANDIDATES_METRIC", EVALUATED_CANDIDATES),
            ("VF_PHASE_METRIC", PHASE_MS),
            ("UNRESOLVED_TWEETS_METRIC", UNRESOLVED_TWEETS),
            ("EXPIRED_LABELS_METRIC", EXPIRED_LABELS),
        ] {
            assert!(
                dashboard.contains(&format!("{constant} = \"{family}\"")),
                "{constant}"
            );
        }
        assert!(dashboard.contains(&format!(
            "FT_FAIL_OPEN_DEFAULTS_FILTER = 'fail_open_defaults!~\"{FAIL_OPEN_DEFAULTS_NONE}|\"'"
        )));
        assert!(dashboard.contains(&format!("QUOTE_PASS_METRIC = \"{QUOTE_PASS}\"")));
        assert!(dashboard.contains(&format!(
            "REQUESTS_BY_CALLER_METRIC = \"{REQUESTS_BY_CALLER}\""
        )));
        assert!(dashboard.contains(&format!(
            "VF_LATENCY_BY_CALLER_METRIC = \"{LATENCY_BY_CALLER_MS}\""
        )));
        let [success, failure, cancelled] = [
            RequestOutcome::Success,
            RequestOutcome::Failure,
            RequestOutcome::Cancelled,
        ]
        .map(<&str>::from);
        assert!(dashboard.contains(&format!(
            "REQUESTS_BY_CALLER_SUCCESS_FILTER = 'outcome=\"{success}\"'"
        )));
        assert!(dashboard.contains(&format!(
            "REQUESTS_BY_CALLER_NON_SUCCESS_FILTER = 'outcome=~\"{failure}|{cancelled}\"'"
        )));
        let hydration: &str = Stage::Hydration.into();
        assert!(dashboard.contains(&format!("{{stage=\"{hydration}\",")));
        assert!(dashboard.contains(&format!(
            "VF_CLIENT_FT_LATENCY_VM_METRIC = \"{FILTER_TWEETS_LATENCY_VM_METRIC}\""
        )));
        for outcome in [
            DeadlineOutcome::CancelledEarly,
            DeadlineOutcome::CancelledAtDeadline,
            DeadlineOutcome::CancelledNoDeadline,
            DeadlineOutcome::Overrun,
        ] {
            let outcome: &str = outcome.into();
            assert!(dashboard.contains(&format!("'outcome=\"{outcome}\"'")));
        }
    }

    #[derive(Default)]
    struct Recorded(std::sync::Mutex<Vec<String>>);

    impl Recorded {
        fn push(&self, kind: &str, name: &str, scopes: &[(&str, &str)]) {
            let labels: Vec<String> = scopes.iter().map(|(k, v)| format!("{k}={v}")).collect();
            let line = format!("{kind} {name}{{{}}}", labels.join(","));
            self.0.lock().unwrap().push(line);
        }
    }

    impl StatsReceiverExt for Recorded {
        fn incr(&self, name: &str, scopes: &[(&str, &str)], _: u64) {
            self.push("incr", name, scopes);
        }
        fn observe(&self, _: &str, _: &[(&str, &str)], _: f64, _: HistogramBuckets) {}
        fn observe_expo(&self, _: &str, _: &[(&str, &str)], _: f64) {}
        fn observe_vm(&self, name: &str, scopes: &[(&str, &str)], _: f64) {
            self.push("observe_vm", name, scopes);
        }
        fn gauge(&self, _: &str, _: &[(&str, &str)], _: f64) {}
    }

    #[test]
    fn each_outcome_emits_its_request_deadline_and_caller_series() {
        let recorded = Recorded::default();
        for outcome in [
            RequestOutcome::Success,
            RequestOutcome::Failure,
            RequestOutcome::Cancelled,
        ] {
            let guard = RequestMetricsGuard {
                endpoint: Endpoint::FilterNotifications,
                level: RequestLevel::Named(SafetyLevel::NotificationsFollow),
                caller_identity: None,
                grpc_timeout: Some(Duration::from_millis(400)),
                start: Instant::now(),
                outcome,
            };
            guard.emit(&recorded, Duration::from_millis(100));
        }
        assert_eq!(
            recorded.0.into_inner().unwrap(),
            [
                "incr vf_requests{rpc=filter_notifications,safety_level=notifications_follow,outcome=success}",
                "incr vf_requests_by_caller{caller_identity=unknown,rpc=filter_notifications,outcome=success}",
                "observe_vm vf_request_latency_ms{rpc=filter_notifications}",
                "observe_vm vf_request_latency_by_caller_ms{caller_identity=unknown,rpc=filter_notifications}",
                "incr vf_request_deadline{rpc=filter_notifications,safety_level=notifications_follow,outcome=within}",
                "incr vf_requests{rpc=filter_notifications,safety_level=notifications_follow,outcome=failure}",
                "incr vf_requests_by_caller{caller_identity=unknown,rpc=filter_notifications,outcome=failure}",
                "incr vf_requests{rpc=filter_notifications,safety_level=notifications_follow,outcome=cancelled}",
                "incr vf_requests_by_caller{caller_identity=unknown,rpc=filter_notifications,outcome=cancelled}",
                "incr vf_request_deadline{rpc=filter_notifications,safety_level=notifications_follow,outcome=cancelled_early}",
            ]
        );
    }

    #[tokio::test]
    async fn each_endpoint_counts_started_and_its_outcome_at_one_level() {
        for (endpoint, level, label) in [
            (
                Endpoint::FilterTweets,
                RequestLevel::Named(SafetyLevel::TimelineHome),
                "timeline_home",
            ),
            (Endpoint::EvaluateTweets, RequestLevel::Unknown, "unknown"),
            (Endpoint::GetSafetyLabels, RequestLevel::Absent, "none"),
            (
                Endpoint::FilterNotifications,
                RequestLevel::Named(SafetyLevel::NotificationsWriterV2),
                "notifications_writer_v2",
            ),
        ] {
            thread_recorded::start();
            let served = observe_request(endpoint, level, Request::new(()), |_, _| async {
                Ok::<_, Status>(())
            })
            .await;
            assert!(served.is_ok());
            let recorded: Vec<Vec<(String, String)>> = thread_recorded::take(REQUESTS)
                .into_iter()
                .map(|sample| sample.labels)
                .collect();
            let rpc: &str = endpoint.into();
            let labels = |outcome: &str| {
                [("rpc", rpc), ("safety_level", label), ("outcome", outcome)]
                    .map(|(name, value)| (name.to_owned(), value.to_owned()))
                    .to_vec()
            };
            assert_eq!(recorded, [labels("started"), labels("success")], "{rpc}");
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
            Some(DeadlineOutcome::Within)
        );
        assert_eq!(
            of(RequestOutcome::Success, 401),
            Some(DeadlineOutcome::Overrun)
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
                id: RuleId::FilterAll,
                by: "nsfw_media",
            }),
            engagement: Some(limit_by("conversation_control")),
        };
        let limit_only = Verdict::Shown {
            notice: None,
            media: None,
            engagement: Some(limit_by("conversation_control")),
        };
        let recorded = |family: &str| {
            thread_recorded::start();
            record_verdicts(
                Endpoint::FilterTweets,
                SafetyLevel::FilterAll,
                [&a, &b, &d, &both, &limit_only],
            );
            let mut counts: Vec<(Option<String>, String, f64)> = thread_recorded::take(family)
                .into_iter()
                .map(|sample| {
                    (
                        sample.label("rule").map(str::to_owned),
                        sample.label("action").unwrap().to_owned(),
                        sample.value,
                    )
                })
                .collect();
            counts.sort_by(|x, y| (&x.0, &x.1).cmp(&(&y.0, &y.1)));
            counts
        };
        let row = |rule: Option<&str>, action: &str, count: f64| {
            (rule.map(str::to_owned), action.to_owned(), count)
        };

        assert_eq!(
            recorded(VERDICTS),
            [
                row(None, "allow", 2.0),
                row(None, "drop", 1.0),
                row(None, "limited_engagement", 1.0),
                row(None, "tweet_interstitial", 1.0),
            ]
        );
        assert_eq!(
            recorded(VERDICTS_BY_RULE),
            [
                row(Some("conversation_control"), "limited_engagement", 2.0),
                row(Some("nsfw_media"), "drop", 1.0),
                row(Some("nsfw_media"), "interstitial", 1.0),
            ]
        );
    }

    #[test]
    fn parse_grpc_timeout_units_and_garbage() {
        let parsed = |value: &str| {
            let mut metadata = MetadataMap::new();
            metadata.insert("grpc-timeout", value.parse().unwrap());
            parse_grpc_timeout(&metadata)
        };
        assert_eq!(parsed("400m"), Some(Duration::from_millis(400)));
        assert_eq!(parsed("1S"), Some(Duration::from_secs(1)));
        assert_eq!(parsed("2M"), Some(Duration::from_secs(120)));
        assert_eq!(parsed("500u"), Some(Duration::from_micros(500)));
        assert_eq!(parsed("3H"), Some(Duration::from_secs(10_800)));
        assert_eq!(parsed("9n"), Some(Duration::from_nanos(9)));
        assert_eq!(parsed("400"), None);
        assert_eq!(parsed("m"), None);
        assert_eq!(parsed("400x"), None);
        assert_eq!(parsed("+400m"), None);
        assert_eq!(parsed("12345678m"), Some(Duration::from_millis(12_345_678)));
        assert_eq!(parsed("123456789m"), None);
        assert_eq!(parse_grpc_timeout(&MetadataMap::new()), None);
    }
}
