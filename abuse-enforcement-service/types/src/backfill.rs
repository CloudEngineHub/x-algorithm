use serde::{Deserialize, Serialize};

use crate::action_control::{ControlEntity, LevelRow, PageAction};

pub const BACKFILL_RUN_RECORD_VERSION: u32 = 1;

pub const MAX_WINDOW_MS: i64 = 30 * 86_400_000;

pub const MAX_LEFT_OUT: usize = 10_000;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Running,
    Paused,
    Stopped,
    Done,
}

impl RunState {
    pub fn as_str(self) -> &'static str {
        match self {
            RunState::Running => "running",
            RunState::Paused => "paused",
            RunState::Stopped => "stopped",
            RunState::Done => "done",
        }
    }

    pub fn active(self) -> bool {
        matches!(self, RunState::Running | RunState::Paused)
    }
}

pub const DONE_DRAIN_MS: i64 = 3_600_000;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum RunSource {
    LoggedOnly,
    LighterAction,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum PausedReason {
    Person,
    OffSwitch,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RunTransition {
    pub at_ms: i64,
    pub by: String,
    #[serde(default)]
    pub from: Option<RunState>,
    pub to: RunState,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct BackfillRunRecord {
    pub v: u32,
    pub run_id: String,
    pub entity: ControlEntity,
    pub head: String,
    #[serde(default)]
    pub classifier: Option<String>,
    pub source: RunSource,
    pub levels: Vec<LevelRow>,
    pub window_start_ms: i64,
    pub window_end_ms: i64,
    pub actions: Vec<PageAction>,
    pub rate_per_sec: u32,
    pub expected_count: u64,
    #[serde(default)]
    pub left_out: Vec<i64>,
    #[serde(default)]
    pub include_genuine_liveness_passes: bool,
    pub reason: String,
    pub created_by: String,
    pub created_by_key: String,
    pub created_at_ms: i64,
    pub state: RunState,
    #[serde(default)]
    pub paused_reason: Option<PausedReason>,
    pub state_changed_by: String,
    pub state_changed_at_ms: i64,
    #[serde(default)]
    pub paused_at_ms: Option<i64>,
    #[serde(default)]
    pub done_at_ms: Option<i64>,
    pub version: u64,
    #[serde(default)]
    pub write_nonce: u64,
    #[serde(default)]
    pub history: Vec<RunTransition>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct BackfillRunStartRequest {
    pub run_id: String,
    pub entity: ControlEntity,
    pub head: String,
    #[serde(default)]
    pub classifier: Option<String>,
    pub source: RunSource,
    pub levels: Vec<LevelRow>,
    pub window_start_ms: i64,
    pub window_end_ms: i64,
    pub actions: Vec<PageAction>,
    pub rate_per_sec: u32,
    pub expected_count: u64,
    #[serde(default)]
    pub left_out: Vec<i64>,
    #[serde(default)]
    pub include_genuine_liveness_passes: bool,
    pub reason: String,
    #[serde(default)]
    pub confirm: Option<String>,
    #[serde(default)]
    pub expected_control_version: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct BackfillRunChangeRequest {
    pub expected_version: u64,
    #[serde(default)]
    pub confirm: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct BackfillRunListResponse {
    pub runs: Vec<BackfillRunRecord>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_round_trips_and_refuses_unknown_fields() {
        let rec = BackfillRunRecord {
            v: 1,
            run_id: "run-00000001".into(),
            entity: ControlEntity::User,
            head: "H".into(),
            classifier: None,
            source: RunSource::LoggedOnly,
            levels: vec![LevelRow::Mid],
            window_start_ms: 1,
            window_end_ms: 2,
            actions: vec![PageAction::Suspend {}],
            rate_per_sec: 200,
            expected_count: 3,
            left_out: vec![],
            include_genuine_liveness_passes: false,
            reason: "r".into(),
            created_by: "a@example.com".into(),
            created_by_key: "page".into(),
            created_at_ms: 1,
            state: RunState::Running,
            paused_reason: None,
            state_changed_by: "a@example.com".into(),
            state_changed_at_ms: 1,
            paused_at_ms: None,
            done_at_ms: None,
            version: 1,
            write_nonce: 7,
            history: vec![],
        };
        let json = serde_json::to_value(&rec).unwrap();
        assert_eq!(json["actions"], serde_json::json!([{"kind": "suspend"}]));
        assert_eq!(json["state"], "running");
        assert_eq!(json["source"], "logged_only");
        let back: BackfillRunRecord = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(back, rec);
        let mut extra = json;
        extra["surprise"] = true.into();
        assert!(serde_json::from_value::<BackfillRunRecord>(extra).is_err());
    }
}
