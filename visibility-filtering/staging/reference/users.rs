use crate::hydration::request_context;
use crate::models::AuthorId;
use crate::post_evaluator::{PostEvaluator, UserResult};
use crate::rules::{RuleId, SafetyLevel};
use crate::staging::reference::vf_service::resolve_build_sha;
use crate::thrift_read::read_fields;
use futures::future::join;
use serde_json::json;
use std::collections::HashMap;
use std::env;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thrift::protocol::{
    TFieldIdentifier, TInputProtocol, TOutputProtocol, TSerializable, TStructIdentifier, TType,
};
use tokio::sync::Semaphore;
use tokio::time::{timeout, Instant};
use xai_build_version::current_build_information;
use xai_stats_receiver::global_stats_receiver;
use xai_strato::strato_thrift::{strato_decode, StratoResult};
use xai_strato::{encode, MValCodec, StratoGrpc};

const COLUMN: &str = "visibility/service/userVisibilityResult.User";
const LEVELS: [SafetyLevel; 2] = [
    SafetyLevel::NotificationsWriterV2,
    SafetyLevel::NotificationsFollow,
];
const MAX_ACTORS: usize = 10;
const MAX_IN_FLIGHT: usize = 16;
const SCALA_TIMEOUT: Duration = Duration::from_millis(1500);
const COMPARE: &str = "vf_user_reference_compare";
const BATCHES: &str = "vf_user_reference_batches";
const RECORD_LINE_MARKER: &str = "vf_user_reference_record";
const RECORD_CAP_PER_MINUTE: u32 = 20;
const COLUMN_CANNOT_FIRE: [RuleId; 3] = [
    RuleId::NoConfirmedEmail,
    RuleId::NoConfirmedPhone,
    RuleId::OnlyPeopleIFollow,
];

#[derive(Clone, Copy, strum::IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
enum BatchOutcome {
    Compared,
    NoActors,
    ScalaTimeout,
    Busy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, strum::IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
enum Scala {
    Allow,
    Drop,
    NotEvaluated,
    Missing,
    Failed,
}

type Bucket = (&'static str, &'static str, &'static str);

fn labels(result: UserResult) -> (&'static str, &'static str) {
    match result {
        UserResult::Allowed => ("allowed", "none"),
        UserResult::Dropped(rule) => ("dropped", rule.into()),
        UserResult::NotFound => ("dropped", "actor_not_found"),
        UserResult::Failed => ("failed", "none"),
    }
}

fn known_difference(result: UserResult) -> &'static str {
    match result {
        UserResult::Dropped(rule) if COLUMN_CANNOT_FIRE.contains(&rule) => "column_cannot_fire",
        UserResult::Allowed
        | UserResult::Dropped(_)
        | UserResult::NotFound
        | UserResult::Failed => "none",
    }
}

pub(super) struct UserReference {
    strato: Arc<StratoGrpc>,
    post_evaluator: Arc<PostEvaluator>,
    permits: Arc<Semaphore>,
    sampler: Mutex<(u64, HashMap<Bucket, u32>)>,
    build_sha: String,
}

impl UserReference {
    pub(super) fn new(strato: Arc<StratoGrpc>, post_evaluator: &Arc<PostEvaluator>) -> Self {
        Self {
            strato,
            post_evaluator: Arc::clone(post_evaluator),
            permits: Arc::new(Semaphore::new(MAX_IN_FLIGHT)),
            sampler: Mutex::default(),
            build_sha: resolve_build_sha(
                &current_build_information().git_commit_sha,
                env::var(super::ENV_IMAGE).ok().as_deref(),
            ),
        }
    }

    pub(super) fn spawn(
        self: &Arc<Self>,
        viewer_id: Option<u64>,
        level: SafetyLevel,
        tweet_ids: Vec<u64>,
    ) {
        let Some(viewer_id) = viewer_id else {
            return;
        };
        match level {
            SafetyLevel::TimelineHome | SafetyLevel::TimelineHomeRecommendations => {}
            SafetyLevel::FilterAll
            | SafetyLevel::TimelineHomeHydration
            | SafetyLevel::ImmersiveExpandedRecommendations
            | SafetyLevel::QuotedTweetRules
            | SafetyLevel::NotificationsFollow
            | SafetyLevel::NotificationsWriterV2 => return,
        }
        let Ok(permit) = Arc::clone(&self.permits).try_acquire_owned() else {
            self.count(BatchOutcome::Busy);
            return;
        };
        let reference = Arc::clone(self);
        tokio::spawn(async move {
            let _permit = permit;
            let outcome = reference.compare(viewer_id, &tweet_ids).await;
            reference.count(outcome);
        });
    }

    async fn compare(&self, viewer_id: u64, tweet_ids: &[u64]) -> BatchOutcome {
        let mut actors = self.post_evaluator.post_authors(tweet_ids).await;
        actors.retain(|&actor| actor != AuthorId(viewer_id));
        actors.sort_unstable_by_key(|actor| actor.get());
        actors.dedup();
        actors.truncate(MAX_ACTORS);
        if actors.is_empty() {
            return BatchOutcome::NoActors;
        }
        for level in LEVELS {
            let rust = request_context(Instant::now(), None).scope(
                self.post_evaluator
                    .evaluate_users(level, viewer_id, actors.clone()),
            );
            let scala = timeout(SCALA_TIMEOUT, self.scala(level, viewer_id, &actors));
            let (rust, scala) = join(rust, scala).await;
            let Ok(scala) = scala else {
                return BatchOutcome::ScalaTimeout;
            };
            let pairs: Vec<(AuthorId, UserResult, Scala)> = actors
                .iter()
                .zip(scala)
                .map(|(&actor, scala)| {
                    let result = rust.get(&actor).copied().unwrap_or(UserResult::Failed);
                    (actor, result, scala)
                })
                .collect();
            self.record(level, viewer_id, &pairs);
        }
        BatchOutcome::Compared
    }

    async fn scala(&self, level: SafetyLevel, viewer_id: u64, actors: &[AuthorId]) -> Vec<Scala> {
        let Some(safety_level) = level.thrift_number() else {
            return vec![Scala::Failed; actors.len()];
        };
        let view = LookupContext {
            safety_level,
            for_user_id: viewer_id,
        };
        let calls = actors
            .iter()
            .map(|actor| {
                (
                    COLUMN.to_owned(),
                    "fetch".to_owned(),
                    vec![encode(&(actor.get().cast_signed(), view))],
                )
            })
            .collect();
        self.strato
            .batch_call(calls, None)
            .await
            .into_iter()
            .map(|answer| answer.map_or(Scala::Failed, |bytes| scala_answer(&bytes)))
            .collect()
    }

    fn record(&self, level: SafetyLevel, viewer_id: u64, pairs: &[(AuthorId, UserResult, Scala)]) {
        let minute = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs() / 60);
        let mut counts: HashMap<(&str, &str, &str, &str), u64> = HashMap::new();
        let mut sampler = self.sampler.lock().unwrap_or_else(PoisonError::into_inner);
        if sampler.0 != minute {
            *sampler = (minute, HashMap::new());
        }
        for &(actor, result, scala) in pairs {
            let (rust, rule) = labels(result);
            let scala: &'static str = scala.into();
            let seen = sampler.1.entry((level.into(), rust, scala)).or_default();
            *seen += 1;
            if *seen <= RECORD_CAP_PER_MINUTE {
                #[expect(clippy::print_stdout, reason = "stdout is the record sink")]
                {
                    println!(
                        "{}",
                        json!({
                            "h": RECORD_LINE_MARKER,
                            "v": 1,
                            "level": <&str>::from(level),
                            "build": self.build_sha,
                            "recipient": viewer_id,
                            "actor": actor.get(),
                            "vf": rust,
                            "vf_rule": rule,
                            "scala": scala,
                        })
                    );
                }
            }
            *counts
                .entry((rust, rule, scala, known_difference(result)))
                .or_default() += 1;
        }
        drop(sampler);
        let Some(stats) = global_stats_receiver() else {
            return;
        };
        for ((rust, rule, scala, difference), count) in counts {
            stats.incr(
                COMPARE,
                &[
                    ("safety_level", level.into()),
                    ("vf", rust),
                    ("vf_rule", rule),
                    ("scala", scala),
                    ("known_difference", difference),
                    ("build_sha", &self.build_sha),
                ],
                count,
            );
        }
    }

    fn count(&self, outcome: BatchOutcome) {
        if let Some(stats) = global_stats_receiver() {
            stats.incr(
                BATCHES,
                &[("outcome", outcome.into()), ("build_sha", &self.build_sha)],
                1,
            );
        }
    }
}

fn scala_answer(bytes: &[u8]) -> Scala {
    match strato_decode::<UserVisibilityResult>(bytes) {
        Ok(StratoResult::Ok { value: None, .. }) => Scala::Missing,
        Ok(StratoResult::Ok {
            value: Some(result),
            ..
        }) => result.0,
        Ok(StratoResult::Err { .. }) | Err(_) => Scala::Failed,
    }
}

#[derive(Clone, Copy)]
struct LookupContext {
    safety_level: i32,
    for_user_id: u64,
}

impl MValCodec for LookupContext {
    fn thrift_type() -> TType {
        TType::Struct
    }

    #[expect(clippy::unimplemented, reason = "a view is only encoded")]
    fn from_thrift(_proto: &mut dyn TInputProtocol) -> Self {
        unimplemented!("a view is only encoded")
    }

    #[expect(
        clippy::unwrap_used,
        reason = "`xai_strato::encode` writes into a Vec, which cannot fail"
    )]
    fn to_thrift(&self, proto: &mut dyn TOutputProtocol) {
        proto
            .write_struct_begin(&TStructIdentifier::new("VisibilityFilteringLookupContext"))
            .unwrap();
        proto
            .write_field_begin(&TFieldIdentifier::new("safety_level", TType::I32, 1))
            .unwrap();
        proto.write_i32(self.safety_level).unwrap();
        proto.write_field_end().unwrap();
        proto
            .write_field_begin(&TFieldIdentifier::new("for_user_id", TType::I64, 2))
            .unwrap();
        proto.write_i64(self.for_user_id.cast_signed()).unwrap();
        proto.write_field_end().unwrap();
        proto.write_field_stop().unwrap();
        proto.write_struct_end().unwrap();
    }
}

struct UserVisibilityResult(Scala);

impl TSerializable for UserVisibilityResult {
    fn read_from_in_protocol(i_prot: &mut dyn TInputProtocol) -> thrift::Result<Self> {
        let mut answer = Scala::NotEvaluated;
        read_fields(i_prot, |i_prot, field| {
            if field.id != Some(1) || field.field_type != TType::Struct {
                return Ok(false);
            }
            read_fields(i_prot, |_, action| {
                answer = match action.id {
                    Some(2) => Scala::Allow,
                    Some(3) => Scala::Drop,
                    _ => Scala::NotEvaluated,
                };
                Ok(false)
            })?;
            Ok(true)
        })?;
        Ok(Self(answer))
    }

    fn write_to_out_protocol(&self, _o_prot: &mut dyn TOutputProtocol) -> thrift::Result<()> {
        Err(thrift::new_protocol_error(
            thrift::ProtocolErrorKind::NotImplemented,
            "a result is only decoded",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clients::strato_answer::{found, no_row, Thrift};
    use crate::rules::RuleEngine;

    #[test]
    fn the_known_differences_are_rules_both_levels_wire() {
        let engine = RuleEngine::for_tests();
        for level in LEVELS {
            let wired: Vec<RuleId> = engine
                .wired_rule_names(level)
                .into_iter()
                .filter_map(RuleId::of_rule_name)
                .collect();
            for rule in COLUMN_CANNOT_FIRE {
                assert!(wired.contains(&rule), "{level:?} wires no {rule:?}");
            }
        }
    }

    #[test]
    fn the_column_answer_reads_as_its_user_action() {
        use Thrift::Struct;
        let action = |field| Struct(vec![(1, Struct(vec![(field, Struct(vec![]))]))]);
        let answer = |value: Option<Thrift>| scala_answer(&value.map_or_else(no_row, found));
        assert_eq!(answer(Some(action(2))), Scala::Allow);
        assert_eq!(answer(Some(action(3))), Scala::Drop);
        assert_eq!(answer(Some(action(1))), Scala::NotEvaluated);
        assert_eq!(answer(Some(Struct(vec![]))), Scala::NotEvaluated);
        assert_eq!(answer(None), Scala::Missing);
        assert_eq!(scala_answer(b"not thrift"), Scala::Failed);
    }
}
