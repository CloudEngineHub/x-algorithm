use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tracing::{error, warn};
use xai_abuse_enforcement_types::action_control::{ControlEntity, LevelRow, PageAction};
pub use xai_abuse_enforcement_types::backfill::*;
use xai_abuse_proto::enforcement as abuse_proto;

use crate::action_control::{
    ActionControl, CONFIRM_WORD, ControlConfig, ControlStore, DRY_RUN_LABEL_PREFIX, FACTORY_LABEL,
    entity_of, lkey_for,
};
use crate::backfill_claims::{ClaimStore, KeyLocks};
use crate::facts::{EntityType, Facts, GizmoduckFacts};
use crate::off_switch::{OffSwitch, valid_name};
use crate::overturn_hold::{GateMode, HoldGateConfig, OnProbeError};

pub const BACKFILL_TOPIC: &str = "abuse.enforcement.backfill.v1";
pub const BACKFILL_TOPIC_PREFIX: &str = "abuse.enforcement.backfill.";

pub fn is_backfill_topic(topic: &str) -> bool {
    topic.starts_with(BACKFILL_TOPIC_PREFIX)
}
pub const RUN_PKEY: &str = "backfill_run";
pub const HOLD_PKEY: &str = "backfill_run_hold";
pub const STOP_PKEY: &str = "backfill_run_stop";
pub const CONFIG_KEY: &str = "backfill_runs";
pub const RUN_TTL_SECS: u64 = 10 * 365 * 86_400;
pub const HOLD_TTL_SECS: u64 = RUN_TTL_SECS;
pub const DEFAULT_MAX_RATE_PER_SEC: u32 = 200;
pub const HARD_MAX_RATE_PER_SEC: u32 = 1_000;
const MAX_REASON_LEN: usize = 1_000;
const MAX_HISTORY: usize = 200;
const CLOCK_SKEW_MS: i64 = 300_000;
pub const MAX_PAUSE_MS: i64 = 30 * 86_400_000;
const START_SETTLE_WAITS: u32 = 20;
const START_SETTLE_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);


pub const STATUS_ID_ON_LIVE_TOPIC: &str = "backfill_run_id_on_live_topic";
pub const STATUS_ID_MISSING: &str = "backfill_run_id_missing";
pub const STATUS_RUN_UNKNOWN: &str = "backfill_run_unknown";
pub const STATUS_RUN_PAUSED: &str = "backfill_run_paused";
pub const STATUS_RUN_STOPPED: &str = "backfill_run_stopped";
pub const STATUS_RUN_DONE: &str = "backfill_run_done";
pub const STATUS_NOT_IN_RUN: &str = "backfill_not_in_run";
pub const STATUS_BAD_MESSAGE: &str = "backfill_bad_message";
pub const STATUS_ALREADY_FINISHED: &str = "backfill_already_finished";
pub const STATUS_ALREADY_APPLIED: &str = "backfill_already_applied";
pub const STATUS_CLAIM_TAKEN: &str = "backfill_claim_taken";
pub const STATUS_BUDGET_REACHED: &str = "backfill_budget_reached";
pub const STATUS_RUN_UNREADABLE: &str = "backfill_run_unreadable";

pub fn leaves_claim(status: &str) -> bool {
    matches!(
        status,
        STATUS_RUN_PAUSED
            | STATUS_RUN_STOPPED
            | STATUS_RUN_DONE
            | STATUS_RUN_UNKNOWN
            | STATUS_CLAIM_TAKEN
            | STATUS_BUDGET_REACHED
            | STATUS_ALREADY_FINISHED
            | STATUS_RUN_UNREADABLE
            | "rule_eval_error"
            | "dedup_skipped"
            | crate::overturn_hold::STATUS_HOLD_LOOKUP_FAILED
            | crate::off_switch::STATUS_SWITCHED_OFF
            | "dry_run"
    )
}


#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackfillConfig {
        pub page_keys: HashSet<String>,
        pub sender_keys: HashSet<String>,
    pub max_rate_per_sec: u32,
    pub max_user_decisions_per_day: u64,
    pub max_post_decisions_per_day: u64,
}

impl Default for BackfillConfig {
    fn default() -> Self {
        Self {
            page_keys: HashSet::new(),
            sender_keys: HashSet::new(),
            max_rate_per_sec: DEFAULT_MAX_RATE_PER_SEC,
            max_user_decisions_per_day: 0,
            max_post_decisions_per_day: 0,
        }
    }
}

impl BackfillConfig {
                pub fn from_config(config: Option<&Value>) -> Self {
        static LAST_BAD: Mutex<Option<String>> = Mutex::new(None);
        let mut out = Self::default();
        let Some(v) = config
            .and_then(|c| c.get(CONFIG_KEY))
            .filter(|v| !v.is_null())
        else {
            return out;
        };
        let mut bad = Vec::new();
        if !v.is_object() {
            bad.push(v.to_string());
        }
        let keys = |name: &str, bad: &mut Vec<String>| -> HashSet<String> {
            match v.get(name) {
                None | Some(Value::Null) => HashSet::new(),
                Some(Value::Array(arr)) => {
                    let parsed: Option<HashSet<String>> = arr
                        .iter()
                        .map(|k| {
                            k.as_str()
                                .map(str::trim)
                                .filter(|s| !s.is_empty())
                                .map(str::to_owned)
                        })
                        .collect();
                    parsed.unwrap_or_else(|| {
                        bad.push(format!("{name}={}", v[name]));
                        HashSet::new()
                    })
                }
                Some(other) => {
                    bad.push(format!("{name}={other}"));
                    HashSet::new()
                }
            }
        };
        out.page_keys = keys("page_keys", &mut bad);
        out.sender_keys = keys("sender_keys", &mut bad);
        match v.get("max_rate_per_sec") {
            None | Some(Value::Null) => {}
            Some(x) => match x.as_u64() {
                Some(n) if n >= 1 => {
                    out.max_rate_per_sec = n.min(u64::from(HARD_MAX_RATE_PER_SEC)) as u32
                }
                _ => bad.push(format!("max_rate_per_sec={x}")),
            },
        }
        for (name, slot) in [
            (
                "max_user_decisions_per_day",
                &mut out.max_user_decisions_per_day,
            ),
            (
                "max_post_decisions_per_day",
                &mut out.max_post_decisions_per_day,
            ),
        ] {
            match v.get(name) {
                None | Some(Value::Null) => {}
                Some(x) => match x.as_u64() {
                    Some(n) => *slot = n,
                    None => bad.push(format!("{name}={x}")),
                },
            }
        }
        if !bad.is_empty() {
            let bad = bad.join(", ");
            let mut last = LAST_BAD.lock().unwrap_or_else(|p| p.into_inner());
            if last.as_deref() != Some(bad.as_str()) {
                error!(value = %bad, "GrowthBook {CONFIG_KEY} is malformed; the bad parts read as their closed defaults");
                *last = Some(bad);
            }
        }
        out
    }

        pub fn budget(&self, entity: EntityType) -> u64 {
        match entity {
            EntityType::User => self.max_user_decisions_per_day,
            EntityType::Post => self.max_post_decisions_per_day,
        }
    }
}

pub fn run_gate_cfg(cfg: &HoldGateConfig) -> Option<HoldGateConfig> {
    if cfg.mode == GateMode::Off {
        return None;
    }
    Some(HoldGateConfig {
        mode: GateMode::Enforce,
        on_probe_error: OnProbeError::Skip,
        topics: Vec::new(),
        ..cfg.clone()
    })
}


#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct HoldMarker {
    at_ms: i64,
    head: String,
            run_version: u64,
    #[serde(default)]
    pod: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct StopMarker {
    at_ms: i64,
    by: String,
            #[serde(default)]
    start_conflict: bool,
}

fn effective(
    mut rec: BackfillRunRecord,
    hold: Option<&HoldMarker>,
    stop: Option<&StopMarker>,
) -> BackfillRunRecord {
    if let Some(stop) = stop {
        if rec.state != RunState::Stopped {
            rec.state = RunState::Stopped;
            rec.paused_reason = None;
            rec.state_changed_by = stop.by.clone();
            rec.state_changed_at_ms = stop.at_ms;
        }
        return rec;
    }
    if rec.state == RunState::Running
        && let Some(h) = hold
        && h.run_version == rec.version
    {
        rec.state = RunState::Paused;
        rec.paused_reason = Some(PausedReason::OffSwitch);
        rec.paused_at_ms = Some(h.at_ms);
    }
    rec
}

fn parse_record(bytes: &[u8]) -> anyhow::Result<BackfillRunRecord> {
    let rec: BackfillRunRecord = serde_json::from_slice(bytes)?;
    if rec.v > BACKFILL_RUN_RECORD_VERSION {
        anyhow::bail!("run record v{} is newer than this build reads", rec.v);
    }
    Ok(rec)
}

pub fn gate(rec: &BackfillRunRecord, now_ms: i64) -> Result<(), &'static str> {
    match rec.state {
        RunState::Running => Ok(()),
        RunState::Paused => Err(STATUS_RUN_PAUSED),
        RunState::Stopped => Err(STATUS_RUN_STOPPED),
        RunState::Done
            if rec
                .done_at_ms
                .is_some_and(|d| now_ms.saturating_sub(d) <= DONE_DRAIN_MS) =>
        {
            Ok(())
        }
        RunState::Done => Err(STATUS_RUN_DONE),
    }
}

pub fn check_message(
    rec: &BackfillRunRecord,
    score: &abuse_proto::ScoreResult,
) -> Result<(), &'static str> {
    let labels: &[String] = score.summary.as_ref().map_or(&[], |s| s.labels.as_slice());
    if score.dry_run
        || score.skip_author_credibility_prechecks
        || !score.requested_actions.is_empty()
        || !score.policy_version.is_empty()
        || labels
            .iter()
            .any(|l| l.starts_with(DRY_RUN_LABEL_PREFIX) || l == FACTORY_LABEL)
    {
        return Err(STATUS_BAD_MESSAGE);
    }
    let (entity, entity_id) = Facts::resolve_target(score);
    let (row, _) = crate::confidence::row_of(score.confidence);
    let in_run = entity_of(entity) == rec.entity
        && crate::facts::head_label(score) == rec.head
        && rec
            .classifier
            .as_deref()
            .is_none_or(|c| c == score.model_version)
        && rec.levels.contains(&row)
        && (rec.window_start_ms..=rec.window_end_ms).contains(&score.score_ts_ms)
        && !rec.left_out.contains(&entity_id);
    if in_run {
        Ok(())
    } else {
        Err(STATUS_NOT_IN_RUN)
    }
}

pub fn already_applied(actions: &[PageAction], account: &GizmoduckFacts) -> bool {
    actions.iter().all(|a| match a {
        PageAction::Suspend {} | PageAction::SuspendAuthor {} => account.suspended,
        PageAction::SpamHighRecall { .. } => account.labels.iter().any(|l| l == "SpamHighRecall"),
        PageAction::PhoenixSkip { .. } => account
            .labels
            .iter()
            .any(|l| l == crate::gizmoduck_labels::CLIENT_EVENT_PHOENIX_SKIP),
        PageAction::LivenessCheck {} | PageAction::PostSpamHighRecall { .. } => false,
    })
}

pub fn info_keys(
    rec: &BackfillRunRecord,
    score: &abuse_proto::ScoreResult,
) -> Vec<(String, String)> {
    vec![
        ("backfill_run_id".into(), rec.run_id.clone()),
        ("backfill_run_by".into(), rec.created_by.clone()),
        (
            "backfill_run_created_at_ms".into(),
            rec.created_at_ms.to_string(),
        ),
        (
            "backfill_run_actions".into(),
            serde_json::to_string(&rec.actions).unwrap_or_else(|_| "[]".into()),
        ),
        (
            "original_decided_at_ms".into(),
            score.score_ts_ms.to_string(),
        ),
    ]
}

fn valid_run_id(id: &str) -> bool {
    (8..=64).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}


#[derive(Debug)]
pub enum RunError {
        Forbidden(&'static str, String),
        Invalid(&'static str, String),
        NotFound,
        Conflict(&'static str, String, Option<Box<BackfillRunRecord>>),
        Store(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Pause,
    Resume,
    Stop,
    Done,
}

impl Change {
    pub fn as_str(self) -> &'static str {
        match self {
            Change::Pause => "pause",
            Change::Resume => "resume",
            Change::Stop => "stop",
            Change::Done => "done",
        }
    }
}


pub struct Backfill {
    store: Arc<dyn ControlStore>,
    claims: Option<Arc<dyn ClaimStore>>,
        pub key_locks: KeyLocks,
    pod: String,
}

impl Backfill {
    pub fn new(
        store: Arc<dyn ControlStore>,
        claims: Option<Arc<dyn ClaimStore>>,
        pod: String,
    ) -> Self {
        Self {
            store,
            claims,
            key_locks: KeyLocks::default(),
            pod,
        }
    }

            pub fn claims(&self) -> Option<&Arc<dyn ClaimStore>> {
        self.claims.as_ref()
    }

    pub fn pod(&self) -> &str {
        &self.pod
    }

    async fn stored(&self, run_id: &str) -> anyhow::Result<Option<BackfillRunRecord>> {
        match self.store.get(RUN_PKEY, run_id).await? {
            None => Ok(None),
            Some(b) => parse_record(&b).map(Some),
        }
    }

    async fn hold(&self, run_id: &str) -> anyhow::Result<Option<HoldMarker>> {
        Ok(self
            .store
            .get(HOLD_PKEY, run_id)
            .await?
            .and_then(|b| serde_json::from_slice(&b).ok()))
    }

        pub async fn run(&self, run_id: &str) -> anyhow::Result<Option<BackfillRunRecord>> {
        let Some(rec) = self.stored(run_id).await? else {
            return Ok(None);
        };
        let stop = self.stop_marker(run_id).await?;
        let hold = if rec.state == RunState::Running && stop.is_none() {
            self.hold(run_id).await?
        } else {
            None
        };
        Ok(Some(effective(rec, hold.as_ref(), stop.as_ref())))
    }

    async fn stop_marker(&self, run_id: &str) -> anyhow::Result<Option<StopMarker>> {
        Ok(self
            .store
            .get(STOP_PKEY, run_id)
            .await?
            .and_then(|b| serde_json::from_slice(&b).ok()))
    }

        pub async fn list(&self) -> anyhow::Result<Vec<BackfillRunRecord>> {
        let holds: std::collections::HashMap<String, HoldMarker> = self
            .store
            .scan(HOLD_PKEY)
            .await?
            .into_iter()
            .filter_map(|(k, v)| Some((k, serde_json::from_slice(&v).ok()?)))
            .collect();
        let stops: std::collections::HashMap<String, StopMarker> = self
            .store
            .scan(STOP_PKEY)
            .await?
            .into_iter()
            .filter_map(|(k, v)| Some((k, serde_json::from_slice(&v).ok()?)))
            .collect();
        let mut runs: Vec<BackfillRunRecord> = self
            .store
            .scan(RUN_PKEY)
            .await?
            .into_iter()
            .filter_map(|(lkey, v)| match parse_record(&v) {
                Ok(r) => Some(r),
                Err(e) => {
                    warn!(lkey, "unreadable backfill run record: {e}");
                    None
                }
            })
            .map(|r| {
                let hold = holds.get(&r.run_id);
                let stop = stops.get(&r.run_id);
                effective(r, hold, stop)
            })
            .collect();
        runs.sort_by(|a, b| b.created_at_ms.cmp(&a.created_at_ms));
        Ok(runs)
    }

                    pub async fn hold_for_off_switch(
        &self,
        run: &BackfillRunRecord,
        now_ms: i64,
    ) -> anyhow::Result<()> {
        let run_id = run.run_id.as_str();
        let marker = HoldMarker {
            at_ms: now_ms,
            head: run.head.clone(),
            run_version: run.version,
            pod: self.pod.clone(),
        };
        self.store
            .put(
                HOLD_PKEY,
                run_id,
                serde_json::to_vec(&marker).unwrap_or_default(),
                HOLD_TTL_SECS,
            )
            .await
            .map_err(|e| anyhow::anyhow!("backfill run {run_id} off-switch hold write failed: {e}"))
    }

    async fn write(&self, rec: &BackfillRunRecord) -> Result<BackfillRunRecord, RunError> {
        self.store
            .put(
                RUN_PKEY,
                &rec.run_id,
                serde_json::to_vec(rec).unwrap_or_default(),
                RUN_TTL_SECS,
            )
            .await
            .map_err(|e| RunError::Store(e.to_string()))?;
        let back = self
            .stored(&rec.run_id)
            .await
            .map_err(|e| RunError::Store(e.to_string()))?;
        match back {
            Some(b) if b.write_nonce == rec.write_nonce => Ok(b),
            other => Err(RunError::Conflict(
                "lost_race",
                "another change to this run landed at the same moment; read the run and try again"
                    .into(),
                other.map(Box::new),
            )),
        }
    }

        #[allow(clippy::too_many_arguments)]
    pub async fn start(
        &self,
        req: BackfillRunStartRequest,
        who: Option<&str>,
        key_id: &str,
        cfg: &BackfillConfig,
        control: &ActionControl,
        control_cfg: &ControlConfig,
        off_switch: &OffSwitch,
        now_ms: i64,
    ) -> Result<(BackfillRunRecord, bool), RunError> {
        if !cfg.page_keys.contains(key_id) {
            return Err(RunError::Forbidden(
                "not_page_key",
                format!("key {key_id} is not in GrowthBook {CONFIG_KEY}.page_keys"),
            ));
        }
        let Some(who) = who else {
            return Err(RunError::Forbidden(
                "person_required",
                "the X-Updated-By header must name the signed-in person".into(),
            ));
        };
        validate_start(&req, cfg, now_ms)?;
        if let Some(existing) = self
            .stored(&req.run_id)
            .await
            .map_err(|e| RunError::Store(e.to_string()))?
        {
            if same_run(&existing, &req) {
                let stop = self
                    .stop_marker(&req.run_id)
                    .await
                    .map_err(|e| RunError::Store(e.to_string()))?;
                let never_ran = stop.as_ref().is_some_and(|s| s.start_conflict)
                    || (!started(&existing)
                        && (stop.is_some() || existing.state == RunState::Stopped));
                if never_ran {
                    return Err(RunError::Conflict(
                        "start_failed",
                        "this run lost a start race and never ran; start again with a new run_id"
                            .into(),
                        Some(Box::new(effective(existing, None, stop.as_ref()))),
                    ));
                }
                if existing.version == 0 && existing.state == RunState::Paused {
                    return self.finish_start(existing).await;
                }
                let hold = if existing.state == RunState::Running && stop.is_none() {
                    self.hold(&req.run_id)
                        .await
                        .map_err(|e| RunError::Store(e.to_string()))?
                } else {
                    None
                };
                return Ok((effective(existing, hold.as_ref(), stop.as_ref()), false));
            }
            return Err(RunError::Conflict(
                "run_id_conflict",
                "a different run already has this run_id".into(),
                Some(Box::new(existing)),
            ));
        }
        let entity = match req.entity {
            ControlEntity::User => EntityType::User,
            ControlEntity::Post => EntityType::Post,
        };
        let scope = lkey_for(req.entity, &req.head, req.classifier.as_deref(), None);
        let current = control
            .stored_version(&scope)
            .await
            .map_err(|e| RunError::Store(e.to_string()))?;
        if current != req.expected_control_version {
            return Err(RunError::Conflict(
                "changed_since_you_looked",
                format!(
                    "the model's setting {scope} is at version {current:?}, the page read {:?}",
                    req.expected_control_version
                ),
                None,
            ));
        }
        if off_switch
            .lookup(&req.head, req.classifier.as_deref().unwrap_or(""), now_ms)
            .is_some()
        {
            return Err(RunError::Conflict(
                "switched_off",
                "the model is switched off".into(),
                None,
            ));
        }
        let weak = control.weak_rows(
            entity,
            &req.head,
            req.classifier.as_deref(),
            &req.levels,
            control_cfg,
            now_ms,
        );
        if !weak.is_empty() {
            return Err(RunError::Conflict(
                "weak_evidence",
                format!(
                    "the evidence of {} doesn't clear its floor now",
                    weak.iter()
                        .map(|r| r.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                None,
            ));
        }
        let runs = self
            .list()
            .await
            .map_err(|e| RunError::Store(e.to_string()))?;
        if let Some(busy) = runs.into_iter().find(|r| {
            r.state.active()
                && r.entity == req.entity
                && r.head == req.head
                && (r.classifier.is_none()
                    || req.classifier.is_none()
                    || r.classifier == req.classifier)
        }) {
            return Err(RunError::Conflict(
                "run_in_progress",
                format!("run {} is still {}", busy.run_id, busy.state.as_str()),
                Some(Box::new(busy)),
            ));
        }
        let rec = BackfillRunRecord {
            v: BACKFILL_RUN_RECORD_VERSION,
            run_id: req.run_id,
            entity: req.entity,
            head: req.head,
            classifier: req.classifier,
            source: req.source,
            levels: req.levels,
            window_start_ms: req.window_start_ms,
            window_end_ms: req.window_end_ms,
            actions: req.actions,
            rate_per_sec: req.rate_per_sec,
            expected_count: req.expected_count,
            left_out: req.left_out,
            include_genuine_liveness_passes: req.include_genuine_liveness_passes,
            reason: req.reason.trim().to_owned(),
            created_by: who.to_owned(),
            created_by_key: key_id.to_owned(),
            created_at_ms: now_ms,
            state: RunState::Paused,
            paused_reason: Some(PausedReason::Person),
            state_changed_by: who.to_owned(),
            state_changed_at_ms: now_ms,
            paused_at_ms: Some(now_ms),
            done_at_ms: None,
            version: 0,
            write_nonce: crate::action_control::new_nonce(),
            history: Vec::new(),
        };
        let written = self.write(&rec).await?;
        self.finish_start(written).await
    }

                                            async fn finish_start(
        &self,
        written: BackfillRunRecord,
    ) -> Result<(BackfillRunRecord, bool), RunError> {
        let mut waits = 0;
        loop {
            let runs = self
                .list()
                .await
                .map_err(|e| RunError::Store(e.to_string()))?;
            if let Some(mine) = runs.iter().find(|r| r.run_id == written.run_id) {
                if mine.state == RunState::Stopped {
                    return Err(start_stopped(mine.clone()));
                }
                if mine.version > 0 {
                    return Ok((mine.clone(), false));
                }
            }
            let rivals: Vec<&BackfillRunRecord> = runs
                .iter()
                .filter(|r| r.run_id != written.run_id && overlaps(r, &written))
                .collect();
            if rivals.is_empty() {
                break;
            }
            let holder = rivals
                .iter()
                .find(|r| r.version > 0 || start_order(r) < start_order(&written));
            if let Some(holder) = holder.or_else(|| {
                (waits >= START_SETTLE_WAITS)
                    .then(|| rivals.first())
                    .flatten()
            }) {
                return Err(self.lose_start(&written, holder).await);
            }
            waits += 1;
            tokio::time::sleep(START_SETTLE_INTERVAL).await;
        }
        let mut running = written.clone();
        running.state = RunState::Running;
        running.paused_reason = None;
        running.paused_at_ms = None;
        running.state_changed_at_ms = written.state_changed_at_ms + 1;
        running.version = written.version + 1;
        running.write_nonce = crate::action_control::new_nonce();
        running.history.push(RunTransition {
            at_ms: running.state_changed_at_ms,
            by: written.created_by.clone(),
            from: None,
            to: RunState::Running,
            note: "start".into(),
        });
        let running = self.write(&running).await?;
        let stop = self
            .stop_marker(&running.run_id)
            .await
            .map_err(|e| RunError::Store(e.to_string()))?;
        if stop.is_some() {
            return Err(start_stopped(effective(running, None, stop.as_ref())));
        }
        Ok((running, true))
    }

                async fn lose_start(
        &self,
        written: &BackfillRunRecord,
        holder: &BackfillRunRecord,
    ) -> RunError {
        let at_ms = written.state_changed_at_ms + 1;
        let marker = StopMarker {
            at_ms,
            by: written.created_by.clone(),
            start_conflict: true,
        };
        match self
            .store
            .put(
                STOP_PKEY,
                &written.run_id,
                serde_json::to_vec(&marker).unwrap_or_default(),
                RUN_TTL_SECS,
            )
            .await
        {
            Err(e) => {
                warn!(run_id = %written.run_id, "stopping a run that lost its start race failed (it stays paused): {e}");
            }
            Ok(()) => {
                let mut stopped = written.clone();
                stopped.state = RunState::Stopped;
                stopped.paused_reason = None;
                stopped.state_changed_at_ms = at_ms;
                stopped.version = written.version + 1;
                stopped.write_nonce = crate::action_control::new_nonce();
                stopped.history.push(RunTransition {
                    at_ms,
                    by: written.created_by.clone(),
                    from: None,
                    to: RunState::Stopped,
                    note: format!("start_conflict with {}", holder.run_id),
                });
                if let Err(e) = self.write(&stopped).await {
                    warn!(run_id = %written.run_id, "stop marker written, record write after a lost start failed: {e:?}");
                }
            }
        }
        RunError::Conflict(
            "run_in_progress",
            format!(
                "run {} started on this model at the same moment",
                holder.run_id
            ),
            Some(Box::new(holder.clone())),
        )
    }

        #[allow(clippy::too_many_arguments)]
    pub async fn change(
        &self,
        run_id: &str,
        change: Change,
        req: BackfillRunChangeRequest,
        who: Option<&str>,
        key_id: &str,
        cfg: &BackfillConfig,
        control: &ActionControl,
        control_cfg: &ControlConfig,
        off_switch: &OffSwitch,
        now_ms: i64,
    ) -> Result<BackfillRunRecord, RunError> {
        let allowed = cfg.page_keys.contains(key_id)
            || (change == Change::Done && cfg.sender_keys.contains(key_id));
        if !allowed {
            return Err(RunError::Forbidden(
                "key_not_allowed",
                format!("key {key_id} may not {} a run", change.as_str()),
            ));
        }
        let who = match (who, change) {
            (Some(w), _) => w.to_owned(),
            (None, Change::Done) => key_id.to_owned(),
            (None, _) => {
                return Err(RunError::Forbidden(
                    "person_required",
                    "the X-Updated-By header must name the signed-in person".into(),
                ));
            }
        };
        if change == Change::Resume && req.confirm.as_deref() != Some(CONFIRM_WORD) {
            return Err(RunError::Invalid(
                "confirm_required",
                format!("resuming a run needs confirm: \"{CONFIRM_WORD}\""),
            ));
        }
        let stored = self
            .stored(run_id)
            .await
            .map_err(|e| RunError::Store(e.to_string()))?
            .ok_or(RunError::NotFound)?;
        let hold = self
            .hold(run_id)
            .await
            .map_err(|e| RunError::Store(e.to_string()))?;
        let stop = self
            .stop_marker(run_id)
            .await
            .map_err(|e| RunError::Store(e.to_string()))?;
        let now = effective(stored.clone(), hold.as_ref(), stop.as_ref());
        if now.state == RunState::Stopped {
            return Err(RunError::Conflict(
                "already_final",
                "the run is stopped".into(),
                Some(Box::new(now)),
            ));
        }
        if stored.version != req.expected_version {
            return Err(RunError::Conflict(
                "version_conflict",
                format!(
                    "the run is at version {}, not {}",
                    stored.version, req.expected_version
                ),
                Some(Box::new(now)),
            ));
        }
        let conflict = |code: &'static str, detail: String| {
            Err(RunError::Conflict(
                code,
                detail,
                Some(Box::new(now.clone())),
            ))
        };
        let to = match change {
            Change::Pause if stored.state == RunState::Running => RunState::Paused,
            Change::Pause => {
                return conflict("not_running", format!("the run is {}", now.state.as_str()));
            }
            Change::Resume if now.state == RunState::Paused => RunState::Running,
            Change::Resume => {
                return conflict("not_paused", format!("the run is {}", now.state.as_str()));
            }
            Change::Stop if stored.state.active() => RunState::Stopped,
            Change::Stop => {
                return conflict(
                    "already_final",
                    format!("the run is {}", now.state.as_str()),
                );
            }
            Change::Done if now.state == RunState::Running => RunState::Done,
            Change::Done => {
                return conflict("not_running", format!("the run is {}", now.state.as_str()));
            }
        };
        if change == Change::Resume {
            if !started(&stored) {
                return conflict(
                    "start_failed",
                    "this run's start never finished; start a new run".into(),
                );
            }
            let runs = self
                .list()
                .await
                .map_err(|e| RunError::Store(e.to_string()))?;
            if let Some(other) = runs
                .iter()
                .find(|r| r.run_id != stored.run_id && overlaps(r, &stored))
            {
                return conflict(
                    "run_in_progress",
                    format!(
                        "run {} is {} on this model",
                        other.run_id,
                        other.state.as_str()
                    ),
                );
            }
            let paused_at = now.paused_at_ms.unwrap_or(now.state_changed_at_ms);
            if now_ms.saturating_sub(paused_at) > MAX_PAUSE_MS {
                return conflict(
                    "paused_too_long",
                    "This run was paused more than 30 days ago, so it can't be resumed. Start a new run \
                     instead: it leaves out accounts that were already acted on."
                        .into(),
                );
            }
            if off_switch
                .lookup(
                    &stored.head,
                    stored.classifier.as_deref().unwrap_or(""),
                    now_ms,
                )
                .is_some()
            {
                return conflict("switched_off", "the model is still switched off".into());
            }
            let entity = match stored.entity {
                ControlEntity::User => EntityType::User,
                ControlEntity::Post => EntityType::Post,
            };
            let weak = control.weak_rows(
                entity,
                &stored.head,
                stored.classifier.as_deref(),
                &stored.levels,
                control_cfg,
                now_ms,
            );
            if !weak.is_empty() {
                return conflict(
                    "weak_evidence",
                    format!(
                        "the evidence of {} doesn't clear its floor now",
                        weak.iter()
                            .map(|r| r.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                );
            }
        }
        let mut next = stored.clone();
        next.state = to;
        next.state_changed_by = who.clone();
        next.state_changed_at_ms = now_ms.max(stored.state_changed_at_ms + 1);
        next.paused_reason = (to == RunState::Paused).then_some(PausedReason::Person);
        if to == RunState::Paused {
            next.paused_at_ms = Some(next.state_changed_at_ms);
        }
        if to == RunState::Done {
            next.done_at_ms = Some(next.state_changed_at_ms);
        }
        next.version = stored.version + 1;
        next.write_nonce = crate::action_control::new_nonce();
        next.history.push(RunTransition {
            at_ms: next.state_changed_at_ms,
            by: who,
            from: Some(now.state),
            to,
            note: change.as_str().into(),
        });
        if next.history.len() > MAX_HISTORY {
            let drop = next.history.len() - MAX_HISTORY;
            next.history.drain(1..=drop);
        }
        if to == RunState::Stopped {
            let marker = StopMarker {
                at_ms: next.state_changed_at_ms,
                by: next.state_changed_by.clone(),
                start_conflict: false,
            };
            self.store
                .put(
                    STOP_PKEY,
                    run_id,
                    serde_json::to_vec(&marker).unwrap_or_default(),
                    RUN_TTL_SECS,
                )
                .await
                .map_err(|e| RunError::Store(e.to_string()))?;
            return match self.write(&next).await {
                Ok(written) => Ok(written),
                Err(e) => {
                    warn!(
                        run_id,
                        "stop marker written, run record write failed: {e:?}"
                    );
                    self.run(run_id)
                        .await
                        .map_err(|e| RunError::Store(e.to_string()))?
                        .ok_or(RunError::NotFound)
                }
            };
        }
        let written = self.write(&next).await?;
        if change == Change::Resume
            && hold.is_some()
            && let Err(e) = self.store.delete(HOLD_PKEY, run_id).await
        {
            warn!(run_id, "backfill run hold delete after resume failed: {e}");
        }
        Ok(written)
    }
}

fn validate_start(
    req: &BackfillRunStartRequest,
    cfg: &BackfillConfig,
    now_ms: i64,
) -> Result<(), RunError> {
    let bad = |code: &'static str, detail: String| Err(RunError::Invalid(code, detail));
    if req.confirm.as_deref() != Some(CONFIRM_WORD) {
        return bad(
            "confirm_required",
            format!("starting a run needs confirm: \"{CONFIRM_WORD}\""),
        );
    }
    if !valid_run_id(&req.run_id) {
        return bad("bad_run_id", "run_id is 8-64 of [a-z0-9-]".into());
    }
    if !valid_name(&req.head) {
        return bad(
            "bad_head",
            format!("head {:?} is not a valid name", req.head),
        );
    }
    if req.classifier.as_deref().is_some_and(|c| !valid_name(c)) {
        return bad("bad_classifier", "classifier is not a valid name".into());
    }
    if req.levels.is_empty() {
        return bad("no_levels", "pick at least one level".into());
    }
    let mut levels: Vec<LevelRow> = req.levels.clone();
    levels.sort();
    levels.dedup();
    if levels.len() != req.levels.len() {
        return bad("duplicate_levels", "a level is listed twice".into());
    }
    if req.window_start_ms >= req.window_end_ms
        || req.window_end_ms - req.window_start_ms > MAX_WINDOW_MS
        || req.window_end_ms > now_ms + CLOCK_SKEW_MS
        || req.window_start_ms < now_ms - MAX_WINDOW_MS - CLOCK_SKEW_MS
    {
        return bad(
            "bad_window",
            "the window must end by now and start at most 30 days ago".into(),
        );
    }
    if req.actions.is_empty() {
        return bad("no_actions", "pick at least one action".into());
    }
    if req.actions.iter().any(|a| a.entity() != req.entity) {
        return bad(
            "wrong_entity",
            format!("every action must be a {} action", req.entity.as_str()),
        );
    }
    let kinds: HashSet<std::mem::Discriminant<PageAction>> =
        req.actions.iter().map(std::mem::discriminant).collect();
    if kinds.len() != req.actions.len() {
        return bad("duplicate_actions", "an action is listed twice".into());
    }
    if req.source == RunSource::LighterAction && req.entity != ControlEntity::User {
        return bad(
            "lighter_action_is_accounts_only",
            "a lighter-action run covers accounts only".into(),
        );
    }
    if req.rate_per_sec == 0 || req.rate_per_sec > cfg.max_rate_per_sec {
        return bad(
            "bad_rate",
            format!("rate_per_sec must be 1 to {}", cfg.max_rate_per_sec),
        );
    }
    if req.reason.trim().is_empty() || req.reason.len() > MAX_REASON_LEN {
        return bad("bad_reason", "say why, in at most 1000 bytes".into());
    }
    if req.left_out.len() > MAX_LEFT_OUT {
        return bad(
            "too_many_left_out",
            format!("at most {MAX_LEFT_OUT} may be left out"),
        );
    }
    Ok(())
}

fn overlaps(a: &BackfillRunRecord, b: &BackfillRunRecord) -> bool {
    a.state.active()
        && a.entity == b.entity
        && a.head == b.head
        && (a.classifier.is_none() || b.classifier.is_none() || a.classifier == b.classifier)
}

fn start_order(r: &BackfillRunRecord) -> (i64, &str) {
    (r.created_at_ms, r.run_id.as_str())
}

fn start_stopped(run: BackfillRunRecord) -> RunError {
    RunError::Conflict(
        "start_failed",
        "this run was stopped before its start finished; start again with a new run_id".into(),
        Some(Box::new(run)),
    )
}

fn started(rec: &BackfillRunRecord) -> bool {
    rec.history
        .iter()
        .any(|t| t.to == RunState::Running && t.note == "start")
}

fn same_run(rec: &BackfillRunRecord, req: &BackfillRunStartRequest) -> bool {
    rec.entity == req.entity
        && rec.head == req.head
        && rec.classifier == req.classifier
        && rec.source == req.source
        && rec.levels == req.levels
        && rec.window_start_ms == req.window_start_ms
        && rec.window_end_ms == req.window_end_ms
        && rec.actions == req.actions
        && rec.rate_per_sec == req.rate_per_sec
        && rec.expected_count == req.expected_count
        && rec.left_out == req.left_out
        && rec.include_genuine_liveness_passes == req.include_genuine_liveness_passes
        && rec.reason == req.reason.trim()
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    pub(crate) fn config(page: &str, sender: &str, user_budget: u64) -> BackfillConfig {
        BackfillConfig {
            page_keys: [page.to_owned()].into(),
            sender_keys: [sender.to_owned()].into(),
            max_rate_per_sec: 1_000,
            max_user_decisions_per_day: user_budget,
            max_post_decisions_per_day: user_budget,
        }
    }

    pub(crate) fn start_request(run_id: &str, head: &str, now_ms: i64) -> BackfillRunStartRequest {
        BackfillRunStartRequest {
            run_id: run_id.into(),
            entity: ControlEntity::User,
            head: head.into(),
            classifier: None,
            source: RunSource::LoggedOnly,
            levels: vec![LevelRow::Missing],
            window_start_ms: now_ms - 86_400_000,
            window_end_ms: now_ms,
            actions: vec![PageAction::Suspend {}],
            rate_per_sec: 200,
            expected_count: 10,
            left_out: vec![],
            include_genuine_liveness_passes: false,
            reason: "acting on logged decisions".into(),
            confirm: Some(CONFIRM_WORD.into()),
            expected_control_version: None,
        }
    }

        pub(crate) fn put_running(
        store: &crate::action_control::test_support::MemControlStore,
        run_id: &str,
        head: &str,
        actions: Vec<PageAction>,
        levels: Vec<LevelRow>,
    ) -> BackfillRunRecord {
        let now = crate::now_millis();
        let rec = BackfillRunRecord {
            v: BACKFILL_RUN_RECORD_VERSION,
            run_id: run_id.into(),
            entity: actions.first().map_or(ControlEntity::User, |a| a.entity()),
            head: head.into(),
            classifier: None,
            source: RunSource::LoggedOnly,
            levels,
            window_start_ms: now - 86_400_000,
            window_end_ms: now,
            actions,
            rate_per_sec: 200,
            expected_count: 1,
            left_out: vec![],
            include_genuine_liveness_passes: false,
            reason: "r".into(),
            created_by: "operator@example.com".into(),
            created_by_key: "page".into(),
            created_at_ms: now - 1_000,
            state: RunState::Running,
            paused_reason: None,
            state_changed_by: "operator@example.com".into(),
            state_changed_at_ms: now - 1_000,
            paused_at_ms: None,
            done_at_ms: None,
            version: 1,
            write_nonce: 1,
            history: vec![],
        };
        store.rows.lock().unwrap().insert(
            (RUN_PKEY.into(), run_id.into()),
            serde_json::to_vec(&rec).unwrap(),
        );
        rec
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;
    use crate::action_control::test_support::MemControlStore;

    const NOW: i64 = 1_800_000_000_000;

    fn parts() -> (Arc<MemControlStore>, Backfill, ActionControl, OffSwitch) {
        let store = Arc::new(MemControlStore::default());
        let bf = Backfill::new(store.clone(), None, "pod-0".into());
        let ac = ActionControl::new(
            Arc::new(MemControlStore::default()),
            "test".into(),
            String::new(),
        );
        let sw = OffSwitch::new(
            Arc::new(crate::off_switch::test_support::MemStore::default()),
            Arc::new(crate::off_switch::test_support::MemStore::default()),
        );
        (store, bf, ac, sw)
    }

    async fn start(
        bf: &Backfill,
        ac: &ActionControl,
        sw: &OffSwitch,
        req: BackfillRunStartRequest,
        key: &str,
        who: Option<&str>,
    ) -> Result<(BackfillRunRecord, bool), RunError> {
        bf.start(
            req,
            who,
            key,
            &config("page", "sender", 10),
            ac,
            &ControlConfig::default(),
            sw,
            NOW,
        )
        .await
    }

    async fn change(
        bf: &Backfill,
        ac: &ActionControl,
        sw: &OffSwitch,
        id: &str,
        c: Change,
        version: u64,
        key: &str,
        confirm: Option<&str>,
        at: i64,
    ) -> Result<BackfillRunRecord, RunError> {
        bf.change(
            id,
            c,
            BackfillRunChangeRequest {
                expected_version: version,
                confirm: confirm.map(str::to_owned),
            },
            Some("operator@example.com"),
            key,
            &config("page", "sender", 10),
            ac,
            &ControlConfig::default(),
            sw,
            at,
        )
        .await
    }

    #[test]
    fn config_is_closed_by_default_and_on_bad_values() {
        let d = BackfillConfig::from_config(None);
        assert!(d.page_keys.is_empty() && d.sender_keys.is_empty());
        assert_eq!((d.max_rate_per_sec, d.max_user_decisions_per_day), (200, 0));
        let c = BackfillConfig::from_config(Some(&serde_json::json!({CONFIG_KEY: {
            "page_keys": ["pv2"], "sender_keys": ["sender"], "max_rate_per_sec": 5000,
            "max_user_decisions_per_day": 1000, "max_post_decisions_per_day": "lots"
        }})));
        assert!(c.page_keys.contains("pv2"));
        assert!(c.sender_keys.contains("sender"));
        assert_eq!(c.max_rate_per_sec, HARD_MAX_RATE_PER_SEC);
        assert_eq!(c.max_user_decisions_per_day, 1000);
        assert_eq!(c.max_post_decisions_per_day, 0, "malformed reads as closed");
        let bad = BackfillConfig::from_config(Some(&serde_json::json!({CONFIG_KEY: {
            "page_keys": "pv2"
        }})));
        assert!(bad.page_keys.is_empty());
    }

    #[test]
    fn backfill_topics_are_recognised() {
        assert!(is_backfill_topic(BACKFILL_TOPIC));
        assert!(is_backfill_topic("abuse.enforcement.backfill.staging.v1"));
        assert!(!is_backfill_topic("abuse.enforcement.decisions.v1"));
        assert!(!is_backfill_topic("abuse.embeddings.user_decisions"));
    }

    #[test]
    fn gate_cfg_for_runs_fails_closed_and_covers_every_topic() {
        let mut live = HoldGateConfig::default();
        assert_eq!(run_gate_cfg(&live), None, "off: a run can't be checked");
        live.mode = GateMode::Shadow;
        live.topics = vec!["abuse.v3.score_results".into()];
        let run = run_gate_cfg(&live).unwrap();
        assert_eq!(run.mode, GateMode::Enforce);
        assert_eq!(run.on_probe_error, OnProbeError::Skip);
        assert!(run.covers_topic(BACKFILL_TOPIC));
        assert_eq!(
            live.on_probe_error,
            OnProbeError::Allow,
            "live config untouched"
        );
    }

    #[tokio::test]
    async fn only_a_page_key_with_a_named_person_starts_a_run() {
        let (_, bf, ac, sw) = parts();
        let req = start_request("run-00000001", "H", NOW);
        assert!(matches!(
            start(&bf, &ac, &sw, req.clone(), "sender", Some("a@example.com")).await,
            Err(RunError::Forbidden("not_page_key", _))
        ));
        assert!(matches!(
            start(&bf, &ac, &sw, req.clone(), "page", None).await,
            Err(RunError::Forbidden("person_required", _))
        ));
        let mut no_confirm = req.clone();
        no_confirm.confirm = None;
        assert!(matches!(
            start(&bf, &ac, &sw, no_confirm, "page", Some("a@example.com")).await,
            Err(RunError::Invalid("confirm_required", _))
        ));
        let (rec, created) = start(&bf, &ac, &sw, req.clone(), "page", Some("a@example.com"))
            .await
            .unwrap();
        assert!(created);
        assert_eq!(rec.state, RunState::Running);
        assert_eq!(rec.created_by, "a@example.com");
        assert_eq!(rec.version, 1);
        let (again, created) = start(&bf, &ac, &sw, req.clone(), "page", Some("a@example.com"))
            .await
            .unwrap();
        assert!(!created);
        assert_eq!(again, rec);
        let mut other = req.clone();
        other.rate_per_sec = 300;
        assert!(matches!(
            start(&bf, &ac, &sw, other, "page", Some("a@example.com")).await,
            Err(RunError::Conflict("run_id_conflict", ..))
        ));
        let mut by_classifier = start_request("run-00000003", "H", NOW);
        by_classifier.classifier = Some("clf_v1".into());
        assert!(matches!(
            start(&bf, &ac, &sw, by_classifier, "page", Some("a@example.com")).await,
            Err(RunError::Conflict("run_in_progress", ..))
        ));
        let mut lighter = start_request("run-00000004", "H", NOW);
        lighter.source = RunSource::LighterAction;
        assert!(matches!(
            start(&bf, &ac, &sw, lighter, "page", Some("a@example.com")).await,
            Err(RunError::Conflict("run_in_progress", ..))
        ));
        let second = start_request("run-00000002", "H", NOW);
        assert!(matches!(
            start(&bf, &ac, &sw, second, "page", Some("a@example.com")).await,
            Err(RunError::Conflict("run_in_progress", ..))
        ));
    }

            struct RacingStore {
        inner: Arc<MemControlStore>,
        raced: std::sync::atomic::AtomicBool,
    }

    #[async_trait::async_trait]
    impl ControlStore for RacingStore {
        async fn scan(&self, pkey: &str) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
            self.inner.scan(pkey).await
        }
        async fn get(&self, pkey: &str, lkey: &str) -> anyhow::Result<Option<Vec<u8>>> {
            self.inner.get(pkey, lkey).await
        }
        async fn put(
            &self,
            pkey: &str,
            lkey: &str,
            value: Vec<u8>,
            ttl: u64,
        ) -> anyhow::Result<()> {
            self.inner.put(pkey, lkey, value, ttl).await?;
            if pkey == RUN_PKEY && !self.raced.swap(true, std::sync::atomic::Ordering::SeqCst) {
                test_support::put_running(
                    &self.inner,
                    "run-other001",
                    "H",
                    vec![PageAction::Suspend {}],
                    vec![LevelRow::Missing],
                );
            }
            Ok(())
        }
        async fn delete(&self, pkey: &str, lkey: &str) -> anyhow::Result<()> {
            self.inner.delete(pkey, lkey).await
        }
    }

    #[tokio::test]
    async fn two_starts_at_once_never_leave_two_active_runs() {
        let inner = Arc::new(MemControlStore::default());
        let bf = Backfill::new(
            Arc::new(RacingStore {
                inner: inner.clone(),
                raced: Default::default(),
            }),
            None,
            String::new(),
        );
        let (_, _, ac, sw) = parts();
        let req = start_request("run-00000001", "H", NOW);
        assert!(matches!(
            start(&bf, &ac, &sw, req, "page", Some("a@example.com")).await,
            Err(RunError::Conflict("run_in_progress", ..))
        ));
        let runs = bf.list().await.unwrap();
        let active: Vec<&str> = runs
            .iter()
            .filter(|r| r.state.active())
            .map(|r| r.run_id.as_str())
            .collect();
        assert_eq!(active, vec!["run-other001"]);
        let mine = runs.iter().find(|r| r.run_id == "run-00000001").unwrap();
        assert_eq!(mine.state, RunState::Stopped);
    }

    #[tokio::test]
    async fn a_resent_start_finishes_or_refuses_an_unfinished_one() {
        let (store, bf, ac, sw) = parts();
        let req = start_request("run-00000001", "H", NOW);
        let first = {
            let failing = Backfill::new(
                Arc::new(FailSecondRunWrite {
                    inner: store.clone(),
                    writes: Default::default(),
                }),
                None,
                String::new(),
            );
            start(
                &failing,
                &ac,
                &sw,
                req.clone(),
                "page",
                Some("a@example.com"),
            )
            .await
        };
        assert!(matches!(first, Err(RunError::Store(_))), "{first:?}");
        let left = bf.run("run-00000001").await.unwrap().unwrap();
        assert_eq!((left.state, left.version), (RunState::Paused, 0));
        let (rec, created) = start(&bf, &ac, &sw, req, "page", Some("a@example.com"))
            .await
            .unwrap();
        assert!(created);
        assert_eq!((rec.state, rec.version), (RunState::Running, 1));
        let mut lost = rec.clone();
        lost.run_id = "run-00000009".into();
        lost.state = RunState::Stopped;
        lost.history.clear();
        store.rows.lock().unwrap().insert(
            (RUN_PKEY.into(), "run-00000009".into()),
            serde_json::to_vec(&lost).unwrap(),
        );
        let resend = start_request("run-00000009", "H", NOW);
        assert!(matches!(
            start(&bf, &ac, &sw, resend, "page", Some("a@example.com")).await,
            Err(RunError::Conflict("start_failed", ..))
        ));
    }

        struct FailSecondRunWrite {
        inner: Arc<MemControlStore>,
        writes: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl ControlStore for FailSecondRunWrite {
        async fn scan(&self, pkey: &str) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
            self.inner.scan(pkey).await
        }
        async fn get(&self, pkey: &str, lkey: &str) -> anyhow::Result<Option<Vec<u8>>> {
            self.inner.get(pkey, lkey).await
        }
        async fn put(
            &self,
            pkey: &str,
            lkey: &str,
            value: Vec<u8>,
            ttl: u64,
        ) -> anyhow::Result<()> {
            if pkey == RUN_PKEY
                && self
                    .writes
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                    == 1
            {
                anyhow::bail!("put failed (test)");
            }
            self.inner.put(pkey, lkey, value, ttl).await
        }
        async fn delete(&self, pkey: &str, lkey: &str) -> anyhow::Result<()> {
            self.inner.delete(pkey, lkey).await
        }
    }

        fn put_record(store: &MemControlStore, rec: &BackfillRunRecord) {
        store.rows.lock().unwrap().insert(
            (RUN_PKEY.into(), rec.run_id.clone()),
            serde_json::to_vec(rec).unwrap(),
        );
    }

    #[tokio::test]
    async fn a_lost_start_stays_stopped_whatever_lands_later() {
        let inner = Arc::new(MemControlStore::default());
        let bf = Backfill::new(
            Arc::new(RacingStore {
                inner: inner.clone(),
                raced: Default::default(),
            }),
            None,
            String::new(),
        );
        let (_, _, ac, sw) = parts();
        let req = start_request("run-00000001", "H", NOW);
        assert!(matches!(
            start(&bf, &ac, &sw, req.clone(), "page", Some("a@example.com")).await,
            Err(RunError::Conflict("run_in_progress", ..))
        ));
        let marker: StopMarker =
            serde_json::from_slice(&inner.get(STOP_PKEY, "run-00000001").await.unwrap().unwrap())
                .unwrap();
        assert!(marker.start_conflict);
        let mut late = bf.run("run-00000001").await.unwrap().unwrap();
        late.state = RunState::Running;
        late.version = 1;
        late.history = vec![RunTransition {
            at_ms: NOW,
            by: "a@example.com".into(),
            from: None,
            to: RunState::Running,
            note: "start".into(),
        }];
        put_record(&inner, &late);
        let now = bf.run("run-00000001").await.unwrap().unwrap();
        assert_eq!(now.state, RunState::Stopped);
        assert_eq!(gate(&now, NOW), Err(STATUS_RUN_STOPPED));
        assert!(matches!(
            start(&bf, &ac, &sw, req, "page", Some("a@example.com")).await,
            Err(RunError::Conflict("start_failed", ..))
        ));
        assert!(matches!(
            change(
                &bf,
                &ac,
                &sw,
                "run-00000001",
                Change::Resume,
                1,
                "page",
                Some(CONFIRM_WORD),
                NOW
            )
            .await,
            Err(RunError::Conflict("already_final", ..))
        ));
    }

            struct StopDuringSwitch {
        inner: Arc<MemControlStore>,
        run_puts: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl ControlStore for StopDuringSwitch {
        async fn scan(&self, pkey: &str) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
            self.inner.scan(pkey).await
        }
        async fn get(&self, pkey: &str, lkey: &str) -> anyhow::Result<Option<Vec<u8>>> {
            self.inner.get(pkey, lkey).await
        }
        async fn put(
            &self,
            pkey: &str,
            lkey: &str,
            value: Vec<u8>,
            ttl: u64,
        ) -> anyhow::Result<()> {
            self.inner.put(pkey, lkey, value, ttl).await?;
            if pkey == RUN_PKEY
                && self
                    .run_puts
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                    == 1
            {
                let marker = StopMarker {
                    at_ms: NOW,
                    by: "a@example.com".into(),
                    start_conflict: true,
                };
                self.inner
                    .put(STOP_PKEY, lkey, serde_json::to_vec(&marker)?, RUN_TTL_SECS)
                    .await?;
            }
            Ok(())
        }
        async fn delete(&self, pkey: &str, lkey: &str) -> anyhow::Result<()> {
            self.inner.delete(pkey, lkey).await
        }
    }

    #[tokio::test]
    async fn a_stop_landing_during_the_switch_to_running_is_never_reported_as_started() {
        let inner = Arc::new(MemControlStore::default());
        let bf = Backfill::new(
            Arc::new(StopDuringSwitch {
                inner: inner.clone(),
                run_puts: Default::default(),
            }),
            None,
            String::new(),
        );
        let (_, _, ac, sw) = parts();
        let req = start_request("run-00000001", "H", NOW);
        let got = start(&bf, &ac, &sw, req.clone(), "page", Some("a@example.com")).await;
        assert!(
            matches!(got, Err(RunError::Conflict("start_failed", ..))),
            "{got:?}"
        );
        let now = bf.run("run-00000001").await.unwrap().unwrap();
        assert_eq!(now.state, RunState::Stopped);
        assert_eq!(gate(&now, NOW), Err(STATUS_RUN_STOPPED));
        assert!(matches!(
            start(&bf, &ac, &sw, req, "page", Some("a@example.com")).await,
            Err(RunError::Conflict("start_failed", ..))
        ));
    }

                struct BothWrittenStore {
        inner: Arc<MemControlStore>,
        barrier: Arc<tokio::sync::Barrier>,
        waited: std::sync::atomic::AtomicBool,
    }

    #[async_trait::async_trait]
    impl ControlStore for BothWrittenStore {
        async fn scan(&self, pkey: &str) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
            self.inner.scan(pkey).await
        }
        async fn get(&self, pkey: &str, lkey: &str) -> anyhow::Result<Option<Vec<u8>>> {
            self.inner.get(pkey, lkey).await
        }
        async fn put(
            &self,
            pkey: &str,
            lkey: &str,
            value: Vec<u8>,
            ttl: u64,
        ) -> anyhow::Result<()> {
            let first =
                pkey == RUN_PKEY && !self.waited.swap(true, std::sync::atomic::Ordering::SeqCst);
            if first {
                self.barrier.wait().await;
            }
            self.inner.put(pkey, lkey, value, ttl).await?;
            if first {
                self.barrier.wait().await;
            }
            Ok(())
        }
        async fn delete(&self, pkey: &str, lkey: &str) -> anyhow::Result<()> {
            self.inner.delete(pkey, lkey).await
        }
    }

    #[tokio::test]
    async fn two_fresh_starts_at_once_leave_exactly_one_running() {
        let inner = Arc::new(MemControlStore::default());
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let pod = |name: &str| {
            Backfill::new(
                Arc::new(BothWrittenStore {
                    inner: inner.clone(),
                    barrier: barrier.clone(),
                    waited: Default::default(),
                }),
                None,
                name.into(),
            )
        };
        let (a, b) = (pod("pod-a"), pod("pod-b"));
        let (_, _, ac, sw) = parts();
        let (got_a, got_b) = tokio::join!(
            start(
                &a,
                &ac,
                &sw,
                start_request("run-00000001", "H", NOW),
                "page",
                Some("a@example.com")
            ),
            start(
                &b,
                &ac,
                &sw,
                start_request("run-00000002", "H", NOW),
                "page",
                Some("b@example.com")
            ),
        );
        let (won, created) = got_a.unwrap();
        assert!(created);
        assert_eq!(won.state, RunState::Running);
        assert!(
            matches!(got_b, Err(RunError::Conflict("run_in_progress", ..))),
            "{got_b:?}"
        );
        let runs = a.list().await.unwrap();
        let active: Vec<(&str, RunState)> = runs
            .iter()
            .filter(|r| r.state.active())
            .map(|r| (r.run_id.as_str(), r.state))
            .collect();
        assert_eq!(active, vec![("run-00000001", RunState::Running)]);
    }

            struct LateRivalStore {
        inner: Arc<MemControlStore>,
        run_scans: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl ControlStore for LateRivalStore {
        async fn scan(&self, pkey: &str) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
            if pkey == RUN_PKEY {
                let n = self
                    .run_scans
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if n == 1 || n == 2 {
                    let mut rival = test_support::put_running(
                        &self.inner,
                        "run-zzzz0001",
                        "H",
                        vec![PageAction::Suspend {}],
                        vec![LevelRow::Missing],
                    );
                    rival.created_at_ms = NOW + 5;
                    if n == 1 {
                        rival.state = RunState::Paused;
                        rival.paused_reason = Some(PausedReason::Person);
                        rival.paused_at_ms = Some(NOW + 5);
                        rival.version = 0;
                    }
                    put_record(&self.inner, &rival);
                }
            }
            self.inner.scan(pkey).await
        }
        async fn get(&self, pkey: &str, lkey: &str) -> anyhow::Result<Option<Vec<u8>>> {
            self.inner.get(pkey, lkey).await
        }
        async fn put(
            &self,
            pkey: &str,
            lkey: &str,
            value: Vec<u8>,
            ttl: u64,
        ) -> anyhow::Result<()> {
            self.inner.put(pkey, lkey, value, ttl).await
        }
        async fn delete(&self, pkey: &str, lkey: &str) -> anyhow::Result<()> {
            self.inner.delete(pkey, lkey).await
        }
    }

    #[tokio::test]
    async fn an_earlier_start_waits_for_a_later_unstarted_one_before_running() {
        let inner = Arc::new(MemControlStore::default());
        let bf = Backfill::new(
            Arc::new(LateRivalStore {
                inner: inner.clone(),
                run_scans: Default::default(),
            }),
            None,
            String::new(),
        );
        let (_, _, ac, sw) = parts();
        let got = start(
            &bf,
            &ac,
            &sw,
            start_request("run-00000001", "H", NOW),
            "page",
            Some("a@example.com"),
        )
        .await;
        assert!(
            matches!(got, Err(RunError::Conflict("run_in_progress", ..))),
            "{got:?}"
        );
        let runs = bf.list().await.unwrap();
        let active: Vec<&str> = runs
            .iter()
            .filter(|r| r.state.active())
            .map(|r| r.run_id.as_str())
            .collect();
        assert_eq!(active, vec!["run-zzzz0001"]);
        assert_eq!(
            bf.run("run-00000001").await.unwrap().unwrap().state,
            RunState::Stopped
        );
    }

    #[tokio::test]
    async fn a_resent_start_shows_an_off_switch_pause() {
        let (_, bf, ac, sw) = parts();
        let req = start_request("run-00000001", "H", NOW);
        let (rec, _) = start(&bf, &ac, &sw, req.clone(), "page", Some("a@example.com"))
            .await
            .unwrap();
        bf.hold_for_off_switch(&rec, NOW).await.unwrap();
        let (again, created) = start(&bf, &ac, &sw, req, "page", Some("a@example.com"))
            .await
            .unwrap();
        assert!(!created);
        assert_eq!(again.state, RunState::Paused);
        assert_eq!(again.paused_reason, Some(PausedReason::OffSwitch));
        assert_eq!(again, bf.run("run-00000001").await.unwrap().unwrap());
    }

    #[tokio::test]
    async fn start_refuses_bad_input() {
        let (_, bf, ac, sw) = parts();
        let base = start_request("run-00000001", "H", NOW);
        let cases: Vec<(&str, BackfillRunStartRequest)> = vec![
            (
                "bad_run_id",
                BackfillRunStartRequest {
                    run_id: "UPPER_case".into(),
                    ..base.clone()
                },
            ),
            (
                "bad_window",
                BackfillRunStartRequest {
                    window_start_ms: NOW - 31 * 86_400_000,
                    ..base.clone()
                },
            ),
            (
                "bad_window",
                BackfillRunStartRequest {
                    window_end_ms: NOW + 86_400_000,
                    ..base.clone()
                },
            ),
            (
                "no_actions",
                BackfillRunStartRequest {
                    actions: vec![],
                    ..base.clone()
                },
            ),
            (
                "wrong_entity",
                BackfillRunStartRequest {
                    actions: vec![PageAction::SuspendAuthor {}],
                    ..base.clone()
                },
            ),
            (
                "duplicate_actions",
                BackfillRunStartRequest {
                    actions: vec![PageAction::Suspend {}, PageAction::Suspend {}],
                    ..base.clone()
                },
            ),
            (
                "bad_rate",
                BackfillRunStartRequest {
                    rate_per_sec: 0,
                    ..base.clone()
                },
            ),
            (
                "bad_reason",
                BackfillRunStartRequest {
                    reason: "  ".into(),
                    ..base.clone()
                },
            ),
            (
                "no_levels",
                BackfillRunStartRequest {
                    levels: vec![],
                    ..base.clone()
                },
            ),
            (
                "lighter_action_is_accounts_only",
                BackfillRunStartRequest {
                    entity: ControlEntity::Post,
                    source: RunSource::LighterAction,
                    actions: vec![PageAction::SuspendAuthor {}],
                    ..base.clone()
                },
            ),
        ];
        for (code, req) in cases {
            match start(&bf, &ac, &sw, req, "page", Some("a@example.com")).await {
                Err(RunError::Invalid(c, _)) => assert_eq!(c, code),
                other => panic!("{code}: {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn start_refuses_a_changed_setting_and_a_switched_off_model() {
        let (_, bf, ac, sw) = parts();
        let mut req = start_request("run-00000001", "H", NOW);
        req.expected_control_version = Some(3);
        assert!(matches!(
            start(&bf, &ac, &sw, req, "page", Some("a@example.com")).await,
            Err(RunError::Conflict("changed_since_you_looked", ..))
        ));
        let switches = Arc::new(crate::off_switch::test_support::MemStore::default());
        switches.put_record(&crate::off_switch::test_support::record(
            "H", None, true, None,
        ));
        let sw = OffSwitch::new(
            switches,
            Arc::new(crate::off_switch::test_support::MemStore::default()),
        );
        sw.refresh().await.unwrap();
        let req = start_request("run-00000001", "H", NOW);
        assert!(matches!(
            start(&bf, &ac, &sw, req, "page", Some("a@example.com")).await,
            Err(RunError::Conflict("switched_off", ..))
        ));
    }

    #[tokio::test]
    async fn pause_resume_stop_done_follow_the_states() {
        let (_, bf, ac, sw) = parts();
        let req = start_request("run-00000001", "H", NOW);
        let (rec, _) = start(&bf, &ac, &sw, req, "page", Some("a@example.com"))
            .await
            .unwrap();
        let id = rec.run_id.as_str();
        assert!(matches!(
            change(&bf, &ac, &sw, id, Change::Pause, 1, "sender", None, NOW + 1).await,
            Err(RunError::Forbidden(..))
        ));
        assert!(matches!(
            change(&bf, &ac, &sw, id, Change::Pause, 7, "page", None, NOW + 1).await,
            Err(RunError::Conflict("version_conflict", ..))
        ));
        let p = change(&bf, &ac, &sw, id, Change::Pause, 1, "page", None, NOW + 1)
            .await
            .unwrap();
        assert_eq!(
            (p.state, p.paused_reason, p.version),
            (RunState::Paused, Some(PausedReason::Person), 2)
        );
        assert_eq!(gate(&p, NOW + 2), Err(STATUS_RUN_PAUSED));
        assert!(matches!(
            change(&bf, &ac, &sw, id, Change::Done, 2, "sender", None, NOW + 2).await,
            Err(RunError::Conflict("not_running", ..))
        ));
        assert!(matches!(
            change(&bf, &ac, &sw, id, Change::Resume, 2, "page", None, NOW + 2).await,
            Err(RunError::Invalid("confirm_required", _))
        ));
        let r = change(
            &bf,
            &ac,
            &sw,
            id,
            Change::Resume,
            2,
            "page",
            Some("ENFORCE"),
            NOW + 2,
        )
        .await
        .unwrap();
        assert_eq!(r.state, RunState::Running);
        let d = change(&bf, &ac, &sw, id, Change::Done, 3, "sender", None, NOW + 3)
            .await
            .unwrap();
        assert_eq!(d.state, RunState::Done);
        assert_eq!(d.state_changed_by, "operator@example.com");
        let done_at = d.done_at_ms.unwrap();
        assert_eq!(
            gate(&d, done_at + DONE_DRAIN_MS),
            Ok(()),
            "drains for an hour"
        );
        assert_eq!(gate(&d, done_at + DONE_DRAIN_MS + 1), Err(STATUS_RUN_DONE));
        assert!(matches!(
            change(&bf, &ac, &sw, id, Change::Stop, 4, "page", None, NOW + 4).await,
            Err(RunError::Conflict("already_final", ..))
        ));
        assert_eq!(d.history.len(), 4);
    }

    #[tokio::test]
    async fn stop_is_final() {
        let (_, bf, ac, sw) = parts();
        let req = start_request("run-00000001", "H", NOW);
        let (rec, _) = start(&bf, &ac, &sw, req, "page", Some("a@example.com"))
            .await
            .unwrap();
        let s = change(
            &bf,
            &ac,
            &sw,
            &rec.run_id,
            Change::Stop,
            1,
            "page",
            None,
            NOW + 1,
        )
        .await
        .unwrap();
        assert_eq!(gate(&s, NOW + 2), Err(STATUS_RUN_STOPPED));
        for c in [Change::Pause, Change::Resume, Change::Stop, Change::Done] {
            let r = change(
                &bf,
                &ac,
                &sw,
                &rec.run_id,
                c,
                2,
                "page",
                Some("ENFORCE"),
                NOW + 2,
            )
            .await;
            assert!(matches!(r, Err(RunError::Conflict(..))), "{c:?}: {r:?}");
        }
    }

        struct TtlStore {
        inner: Arc<MemControlStore>,
        ttls: Mutex<Vec<(String, u64)>>,
    }

    #[async_trait::async_trait]
    impl ControlStore for TtlStore {
        async fn scan(&self, pkey: &str) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
            self.inner.scan(pkey).await
        }
        async fn get(&self, pkey: &str, lkey: &str) -> anyhow::Result<Option<Vec<u8>>> {
            self.inner.get(pkey, lkey).await
        }
        async fn put(
            &self,
            pkey: &str,
            lkey: &str,
            value: Vec<u8>,
            ttl: u64,
        ) -> anyhow::Result<()> {
            self.ttls.lock().unwrap().push((pkey.to_owned(), ttl));
            self.inner.put(pkey, lkey, value, ttl).await
        }
        async fn delete(&self, pkey: &str, lkey: &str) -> anyhow::Result<()> {
            self.inner.delete(pkey, lkey).await
        }
    }

            #[tokio::test]
    async fn holds_and_stops_live_as_long_as_the_run() {
        let (store, _, ac, sw) = parts();
        let ttl_store = Arc::new(TtlStore {
            inner: store,
            ttls: Mutex::new(Vec::new()),
        });
        let bf = Backfill::new(ttl_store.clone(), None, "pod-0".into());
        let req = start_request("run-00000001", "H", NOW);
        let (rec, _) = start(&bf, &ac, &sw, req, "page", Some("a@example.com"))
            .await
            .unwrap();
        bf.hold_for_off_switch(&rec, NOW + 1).await.unwrap();
        change(
            &bf,
            &ac,
            &sw,
            &rec.run_id,
            Change::Stop,
            1,
            "page",
            None,
            NOW + 2,
        )
        .await
        .unwrap();
        let ttls = ttl_store.ttls.lock().unwrap().clone();
        let ttl_of = |pkey: &str| ttls.iter().find(|(p, _)| p == pkey).map(|(_, t)| *t);
        assert_eq!(ttl_of(RUN_PKEY), Some(RUN_TTL_SECS));
        assert_eq!(ttl_of(HOLD_PKEY), Some(RUN_TTL_SECS));
        assert_eq!(ttl_of(STOP_PKEY), Some(RUN_TTL_SECS));
    }

            #[tokio::test]
    async fn resume_refuses_while_another_run_holds_the_model() {
        let (store, bf, ac, sw) = parts();
        let req = start_request("run-00000001", "H", NOW);
        let (rec, _) = start(&bf, &ac, &sw, req, "page", Some("a@example.com"))
            .await
            .unwrap();
        change(
            &bf,
            &ac,
            &sw,
            &rec.run_id,
            Change::Pause,
            1,
            "page",
            None,
            NOW + 1,
        )
        .await
        .unwrap();
        test_support::put_running(
            &store,
            "run-other001",
            "H",
            vec![PageAction::Suspend {}],
            vec![LevelRow::Missing],
        );
        assert!(matches!(
            change(
                &bf,
                &ac,
                &sw,
                &rec.run_id,
                Change::Resume,
                2,
                "page",
                Some("ENFORCE"),
                NOW + 2
            )
            .await,
            Err(RunError::Conflict("run_in_progress", ..))
        ));
        let mut unstarted = rec.clone();
        unstarted.run_id = "run-unstart1".into();
        unstarted.head = "Other".into();
        unstarted.state = RunState::Paused;
        unstarted.version = 0;
        unstarted.history.clear();
        store.rows.lock().unwrap().insert(
            (RUN_PKEY.into(), unstarted.run_id.clone()),
            serde_json::to_vec(&unstarted).unwrap(),
        );
        assert!(matches!(
            change(
                &bf,
                &ac,
                &sw,
                "run-unstart1",
                Change::Resume,
                0,
                "page",
                Some("ENFORCE"),
                NOW + 2
            )
            .await,
            Err(RunError::Conflict("start_failed", ..))
        ));
    }

            #[tokio::test]
    async fn a_stop_holds_against_a_later_write_of_the_record() {
        let (store, bf, ac, sw) = parts();
        let req = start_request("run-00000001", "H", NOW);
        let (rec, _) = start(&bf, &ac, &sw, req, "page", Some("a@example.com"))
            .await
            .unwrap();
        let stopped = change(
            &bf,
            &ac,
            &sw,
            &rec.run_id,
            Change::Stop,
            1,
            "page",
            None,
            NOW + 1,
        )
        .await
        .unwrap();
        assert_eq!(stopped.state, RunState::Stopped);
        let mut late = rec.clone();
        late.version = 2;
        late.write_nonce = 99;
        store.rows.lock().unwrap().insert(
            (RUN_PKEY.into(), rec.run_id.clone()),
            serde_json::to_vec(&late).unwrap(),
        );
        let now = bf.run(&rec.run_id).await.unwrap().unwrap();
        assert_eq!(now.state, RunState::Stopped);
        assert_eq!(gate(&now, NOW + 2), Err(STATUS_RUN_STOPPED));
        assert_eq!(bf.list().await.unwrap()[0].state, RunState::Stopped);
        assert!(matches!(
            change(
                &bf,
                &ac,
                &sw,
                &rec.run_id,
                Change::Pause,
                2,
                "page",
                None,
                NOW + 3
            )
            .await,
            Err(RunError::Conflict("already_final", ..))
        ));
    }

    #[tokio::test]
    async fn an_off_switch_hold_pauses_until_a_person_resumes() {
        let (_, bf, ac, sw) = parts();
        let req = start_request("run-00000001", "H", NOW);
        let (rec, _) = start(&bf, &ac, &sw, req, "page", Some("a@example.com"))
            .await
            .unwrap();
        bf.hold_for_off_switch(&rec, NOW + 5).await.unwrap();
        let held = bf.run(&rec.run_id).await.unwrap().unwrap();
        assert_eq!(
            (held.state, held.paused_reason),
            (RunState::Paused, Some(PausedReason::OffSwitch))
        );
        assert_eq!(held.version, 1, "the hold doesn't write the run record");
        assert_eq!(gate(&held, NOW + 6), Err(STATUS_RUN_PAUSED));
        let listed = bf.list().await.unwrap();
        assert_eq!(listed[0].state, RunState::Paused);
        assert!(matches!(
            change(
                &bf,
                &ac,
                &sw,
                &rec.run_id,
                Change::Done,
                1,
                "sender",
                None,
                NOW + 6
            )
            .await,
            Err(RunError::Conflict("not_running", ..))
        ));
        let r = change(
            &bf,
            &ac,
            &sw,
            &rec.run_id,
            Change::Resume,
            1,
            "page",
            Some("ENFORCE"),
            NOW + 7,
        )
        .await
        .unwrap();
        assert_eq!(r.state, RunState::Running);
        assert_eq!(
            bf.run(&rec.run_id).await.unwrap().unwrap().state,
            RunState::Running
        );
        bf.hold_for_off_switch(&rec, NOW + 8).await.unwrap();
        assert_eq!(
            bf.run(&rec.run_id).await.unwrap().unwrap().state,
            RunState::Running
        );
        bf.hold_for_off_switch(&r, NOW + 9).await.unwrap();
        assert_eq!(
            bf.run(&rec.run_id).await.unwrap().unwrap().state,
            RunState::Paused
        );
    }

    #[tokio::test]
    async fn a_failed_resume_write_leaves_an_off_switch_pause_in_place() {
        let (store, bf, ac, sw) = parts();
        let req = start_request("run-00000001", "H", NOW);
        let (rec, _) = start(&bf, &ac, &sw, req, "page", Some("a@example.com"))
            .await
            .unwrap();
        bf.hold_for_off_switch(&rec, NOW + 5).await.unwrap();
        store
            .fail_put
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let r = change(
            &bf,
            &ac,
            &sw,
            &rec.run_id,
            Change::Resume,
            1,
            "page",
            Some("ENFORCE"),
            NOW + 7,
        )
        .await;
        assert!(matches!(r, Err(RunError::Store(_))), "{r:?}");
        store
            .fail_put
            .store(false, std::sync::atomic::Ordering::Relaxed);
        let now = bf.run(&rec.run_id).await.unwrap().unwrap();
        assert_eq!(
            (now.state, now.paused_reason),
            (RunState::Paused, Some(PausedReason::OffSwitch))
        );
    }

    #[tokio::test]
    async fn a_run_paused_over_30_days_cant_be_resumed() {
        let (_, bf, ac, sw) = parts();
        let req = start_request("run-00000001", "H", NOW);
        let (rec, _) = start(&bf, &ac, &sw, req, "page", Some("a@example.com"))
            .await
            .unwrap();
        let p = change(
            &bf,
            &ac,
            &sw,
            &rec.run_id,
            Change::Pause,
            1,
            "page",
            None,
            NOW + 1,
        )
        .await
        .unwrap();
        let late = p.paused_at_ms.unwrap() + MAX_PAUSE_MS + 1;
        match change(
            &bf,
            &ac,
            &sw,
            &rec.run_id,
            Change::Resume,
            2,
            "page",
            Some("ENFORCE"),
            late,
        )
        .await
        {
            Err(RunError::Conflict("paused_too_long", detail, _)) => {
                assert!(detail.contains("more than 30 days"), "{detail}");
                assert!(detail.contains("Start a new run"), "{detail}");
            }
            other => panic!("{other:?}"),
        }
        let ok = p.paused_at_ms.unwrap() + MAX_PAUSE_MS;
        let r = change(
            &bf,
            &ac,
            &sw,
            &rec.run_id,
            Change::Resume,
            2,
            "page",
            Some("ENFORCE"),
            ok,
        )
        .await
        .unwrap();
        assert_eq!(r.state, RunState::Running);
        bf.hold_for_off_switch(&r, ok + 10).await.unwrap();
        assert!(matches!(
            change(
                &bf,
                &ac,
                &sw,
                &rec.run_id,
                Change::Resume,
                3,
                "page",
                Some("ENFORCE"),
                ok + 10 + MAX_PAUSE_MS + 1
            )
            .await,
            Err(RunError::Conflict("paused_too_long", ..))
        ));
        assert!(
            change(
                &bf,
                &ac,
                &sw,
                &rec.run_id,
                Change::Stop,
                3,
                "page",
                None,
                ok + 11 + MAX_PAUSE_MS
            )
            .await
            .is_ok()
        );
    }

    #[test]
    fn message_checks() {
        let rec = BackfillRunRecord {
            classifier: Some("clf".into()),
            levels: vec![LevelRow::Mid],
            left_out: vec![5],
            ..start_record()
        };
        let mut s = abuse_proto::ScoreResult {
            user_id: 4,
            score_ts_ms: rec.window_end_ms - 1,
            model_version: "clf".into(),
            confidence: abuse_proto::Confidence::Mid as i32,
            summary: Some(abuse_proto::SummaryCounters {
                fired_heads: vec![abuse_proto::FiredHead {
                    name: "H".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(check_message(&rec, &s), Ok(()));
        let mut other = s.clone();
        other.model_version = "clf2".into();
        assert_eq!(check_message(&rec, &other), Err(STATUS_NOT_IN_RUN));
        let mut low = s.clone();
        low.confidence = abuse_proto::Confidence::Low as i32;
        assert_eq!(check_message(&rec, &low), Err(STATUS_NOT_IN_RUN));
        let mut early = s.clone();
        early.score_ts_ms = rec.window_start_ms - 1;
        assert_eq!(check_message(&rec, &early), Err(STATUS_NOT_IN_RUN));
        let mut left_out = s.clone();
        left_out.user_id = 5;
        assert_eq!(check_message(&rec, &left_out), Err(STATUS_NOT_IN_RUN));
        s.skip_author_credibility_prechecks = true;
        assert_eq!(check_message(&rec, &s), Err(STATUS_BAD_MESSAGE));
    }

    fn start_record() -> BackfillRunRecord {
        let store = MemControlStore::default();
        test_support::put_running(
            &store,
            "run-00000001",
            "H",
            vec![PageAction::Suspend {}],
            vec![LevelRow::Mid],
        )
    }

    #[test]
    fn already_applied_reads_back_suspensions_and_labels_only() {
        let suspended = GizmoduckFacts {
            suspended: true,
            ..Default::default()
        };
        let labelled = GizmoduckFacts {
            labels: vec!["SpamHighRecall".into()],
            ..Default::default()
        };
        let shr = PageAction::SpamHighRecall {
            lifetime: xai_abuse_enforcement_types::action_control::Lifetime::Days7,
        };
        assert!(already_applied(&[PageAction::Suspend {}], &suspended));
        assert!(!already_applied(&[PageAction::Suspend {}], &labelled));
        assert!(already_applied(&[shr], &labelled));
        assert!(!already_applied(
            &[shr, PageAction::LivenessCheck {}],
            &labelled
        ));
        let phoenix = PageAction::PhoenixSkip {
            lifetime: xai_abuse_enforcement_types::action_control::Lifetime::Days7,
        };
        let phoenix_labelled = GizmoduckFacts {
            labels: vec!["ClientEventPhoenixSkip".into()],
            ..Default::default()
        };
        assert!(already_applied(&[phoenix], &phoenix_labelled));
        assert!(!already_applied(&[phoenix], &labelled));
        assert!(!already_applied(&[shr], &phoenix_labelled));
    }
}
