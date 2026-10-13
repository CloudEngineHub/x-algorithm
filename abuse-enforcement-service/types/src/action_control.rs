use serde::{Deserialize, Serialize};

pub const ACTION_CONTROL_RECORD_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ControlMode {
        Rules,
            Recording,
        Live,
}

impl ControlMode {
    pub fn as_str(self) -> &'static str {
        match self {
            ControlMode::Rules => "rules",
            ControlMode::Recording => "recording",
            ControlMode::Live => "live",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum Lifetime {
    #[serde(rename = "7d")]
    Days7,
    #[serde(rename = "30d")]
    Days30,
    #[serde(rename = "permanent")]
    Permanent,
}

impl Lifetime {
        pub fn ttl_msec(self) -> Option<i64> {
        match self {
            Lifetime::Days7 => Some(7 * 86_400_000),
            Lifetime::Days30 => Some(30 * 86_400_000),
            Lifetime::Permanent => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PageAction {
        Suspend {},
        LivenessCheck {},
        SpamHighRecall { lifetime: Lifetime },
        PostSpamHighRecall { lifetime: Lifetime },
        SuspendAuthor {},
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ControlEntity {
    User,
    Post,
}

impl ControlEntity {
    pub fn as_str(self) -> &'static str {
        match self {
            ControlEntity::User => "user",
            ControlEntity::Post => "post",
        }
    }
}

impl PageAction {
    pub fn entity(self) -> ControlEntity {
        match self {
            PageAction::Suspend {}
            | PageAction::LivenessCheck {}
            | PageAction::SpamHighRecall { .. } => ControlEntity::User,
            PageAction::PostSpamHighRecall { .. } | PageAction::SuspendAuthor {} => {
                ControlEntity::Post
            }
        }
    }

            pub fn menu(entity: ControlEntity) -> Vec<PageAction> {
        match entity {
            ControlEntity::User => vec![
                PageAction::Suspend {},
                PageAction::LivenessCheck {},
                PageAction::SpamHighRecall {
                    lifetime: Lifetime::Days7,
                },
                PageAction::SpamHighRecall {
                    lifetime: Lifetime::Days30,
                },
                PageAction::SpamHighRecall {
                    lifetime: Lifetime::Permanent,
                },
            ],
            ControlEntity::Post => vec![
                PageAction::PostSpamHighRecall {
                    lifetime: Lifetime::Days30,
                },
                PageAction::PostSpamHighRecall {
                    lifetime: Lifetime::Permanent,
                },
                PageAction::SuspendAuthor {},
            ],
        }
    }

            pub fn kind(self) -> &'static str {
        match self {
            PageAction::Suspend {} => "suspend",
            PageAction::LivenessCheck {} => "liveness_check",
            PageAction::SpamHighRecall { .. } => "spam_high_recall",
            PageAction::PostSpamHighRecall { .. } => "post_spam_high_recall",
            PageAction::SuspendAuthor {} => "suspend_author",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ActionControlRecord {
    pub v: u32,
    pub entity_type: ControlEntity,
    pub head: String,
    #[serde(default)]
    pub classifier: Option<String>,
    #[serde(default)]
    pub band: Option<String>,
    pub mode: ControlMode,
    #[serde(default)]
    pub actions: Vec<PageAction>,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub updated_by: String,
    #[serde(default)]
    pub key_id: String,
    #[serde(default)]
    pub updated_at_ms: i64,
        pub version: u64,
        #[serde(default)]
    pub write_nonce: u64,
}

impl ActionControlRecord {
            pub const FIELDS: &'static [&'static str] = &[
        "v",
        "entity_type",
        "head",
        "classifier",
        "band",
        "mode",
        "actions",
        "reason",
        "updated_by",
        "key_id",
        "updated_at_ms",
        "version",
        "write_nonce",
    ];

            pub fn parse_strict(bytes: &[u8]) -> Result<Self, String> {
        let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if let Some(obj) = value.as_object()
            && let Some(k) = obj.keys().find(|k| !Self::FIELDS.contains(&k.as_str()))
        {
            return Err(format!("unknown field `{k}`"));
        }
        serde_json::from_value(value).map_err(|e| e.to_string())
    }

        pub fn scope(&self) -> &'static str {
        match (self.classifier.is_some(), self.band.is_some()) {
            (false, false) => "head",
            (true, false) => "classifier",
            (false, true) => "band",
            (true, true) => "classifier+band",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ActionControlView {
    #[serde(flatten)]
    pub record: ActionControlRecord,
    pub scope: String,
    pub lkey: String,
        pub effective_mode: ControlMode,
                #[serde(default)]
    pub forced_recording_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct ActionControlPutRequest {
            pub entity_type: ControlEntity,
    pub head: String,
    #[serde(default)]
    pub classifier: Option<String>,
    #[serde(default)]
    pub band: Option<String>,
    pub mode: ControlMode,
    #[serde(default)]
    pub actions: Vec<PageAction>,
    #[serde(default)]
    pub reason: String,
        #[serde(default)]
    pub expected_version: Option<u64>,
        #[serde(default)]
    pub confirm: Option<String>,
        #[serde(default)]
    pub updated_by: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ActionControlSeen {
    pub head: String,
    pub classifier: String,
        pub band: String,
    pub entity_type: ControlEntity,
        #[serde(default)]
    pub factory: bool,
        #[serde(default)]
    pub cse: bool,
    pub last_seen_ms: i64,
        #[serde(default)]
    pub rule_id: String,
            #[serde(default)]
    #[cfg_attr(feature = "openapi", schema(value_type = Object))]
    pub action_specs: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SafetyCheckView {
    pub id: String,
    pub reason: String,
    pub when: String,
        pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SafetyChecksStatus {
    pub ok: bool,
    pub problems: Vec<String>,
    pub checks: Vec<SafetyCheckView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SafetyChecksByEntity {
    pub user: SafetyChecksStatus,
    pub post: SafetyChecksStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AllowedActions {
    pub user: Vec<PageAction>,
    pub post: Vec<PageAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CseLists {
    pub heads: Vec<String>,
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct UnreadableRecord {
    pub lkey: String,
    pub error: String,
            #[serde(default)]
    pub version: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ActionControlListResponse {
    pub feature_version: u32,
    pub env: String,
    pub pod: String,
                    pub enabled: bool,
        pub ignore_store: bool,
        pub source: String,
    #[serde(default)]
    pub error: Option<String>,
        pub snapshot_age_ms: i64,
    pub stale: bool,
    pub stale_after_ms: i64,
    pub records: Vec<ActionControlView>,
    pub unreadable: Vec<UnreadableRecord>,
    pub seen: Vec<ActionControlSeen>,
    #[serde(default)]
    pub seen_error: Option<String>,
    pub safety_checks: SafetyChecksByEntity,
    pub allowed_actions: AllowedActions,
    pub cse: CseLists,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ActionControlHistoryEntry {
    pub at_ms: i64,
    pub version: u64,
    pub lkey: String,
    pub scope: String,
    pub updated_by: String,
    pub key_id: String,
    pub reason: String,
    pub env: String,
    pub before: Option<ActionControlRecord>,
    pub after: ActionControlRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ActionControlHistoryResponse {
    pub head: String,
    pub entries: Vec<ActionControlHistoryEntry>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PodView {
    pub pod: String,
    pub env: String,
    pub image: String,
    pub action_control: PodActionControlView,
    pub off_switch: PodOffSwitchView,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PodActionControlView {
    pub loaded: bool,
    pub snapshot_age_ms: i64,
    pub stale: bool,
    pub records: usize,
        pub page_managed: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PodOffSwitchView {
    pub snapshot_age_ms: i64,
        pub active: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_actions_parse_from_the_documented_json() {
        let a: Vec<PageAction> = serde_json::from_str(
            r#"[{"kind":"suspend"},{"kind":"liveness_check"},
                {"kind":"spam_high_recall","lifetime":"7d"},
                {"kind":"post_spam_high_recall","lifetime":"permanent"},
                {"kind":"suspend_author"}]"#,
        )
        .unwrap();
        assert_eq!(
            a,
            vec![
                PageAction::Suspend {},
                PageAction::LivenessCheck {},
                PageAction::SpamHighRecall {
                    lifetime: Lifetime::Days7
                },
                PageAction::PostSpamHighRecall {
                    lifetime: Lifetime::Permanent
                },
                PageAction::SuspendAuthor {},
            ]
        );
    }

    #[test]
    fn actions_outside_the_menu_do_not_parse() {
        for bad in [
            r#"{"kind":"suspend","perm":true}"#,
            r#"{"kind":"suspend","policy":"Cse"}"#,
            r#"{"kind":"spam_high_recall","lifetime":"90d"}"#,
            r#"{"kind":"label","labels":["SpamHighRecall"]}"#,
            r#"{"kind":"arkose"}"#,
        ] {
            assert!(serde_json::from_str::<PageAction>(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn record_with_an_unknown_field_is_unreadable() {
        let ok = r#"{"v":1,"entity_type":"user","head":"H","mode":"recording","version":1}"#;
        let r: ActionControlRecord = serde_json::from_str(ok).unwrap();
        assert_eq!(r.scope(), "head");
        assert!(r.actions.is_empty());
        let newer = r#"{"v":2,"entity_type":"user","head":"H","mode":"recording","version":1,"new_field":true}"#;
        assert!(ActionControlRecord::parse_strict(newer.as_bytes()).is_err());
        let retired = r#"{"v":1,"entity_type":"user","head":"H","mode":"recording","version":1,"dedup_strength":"suspend"}"#;
        assert!(ActionControlRecord::parse_strict(retired.as_bytes()).is_err());
    }

    #[test]
    fn fields_list_matches_the_record_and_the_view_round_trips() {
        let r = ActionControlRecord::parse_strict(
            br#"{"v":1,"entity_type":"post","head":"H","classifier":"c","band":"b","mode":"live",
                "actions":[{"kind":"suspend_author"}],"reason":"r","updated_by":"u","key_id":"k",
                "updated_at_ms":5,"version":2,"write_nonce":9}"#,
        )
        .unwrap();
        let keys: Vec<String> = serde_json::to_value(&r)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        let mut want: Vec<&str> = ActionControlRecord::FIELDS.to_vec();
        want.sort_unstable();
        let mut got: Vec<&str> = keys.iter().map(String::as_str).collect();
        got.sort_unstable();
        assert_eq!(got, want);
        let view = ActionControlView {
            record: r,
            scope: "classifier+band".into(),
            lkey: "post/cb/H/c/b".into(),
            effective_mode: ControlMode::Recording,
            forced_recording_reason: Some("kill_switch".into()),
        };
        let back: ActionControlView =
            serde_json::from_str(&serde_json::to_string(&view).unwrap()).unwrap();
        assert_eq!(back, view);
    }

    #[test]
    fn record_and_put_need_the_entity() {
        let no_entity = r#"{"v":1,"head":"H","mode":"recording","version":1}"#;
        assert!(serde_json::from_str::<ActionControlRecord>(no_entity).is_err());
        let put = r#"{"head":"H","mode":"recording","reason":"r"}"#;
        assert!(serde_json::from_str::<ActionControlPutRequest>(put).is_err());
        let put = r#"{"entity_type":"post","head":"H","mode":"recording","reason":"r"}"#;
        let p: ActionControlPutRequest = serde_json::from_str(put).unwrap();
        assert_eq!(p.entity_type, ControlEntity::Post);
        let retired = r#"{"entity_type":"post","head":"H","mode":"recording","reason":"r","dedup_strength":"auto"}"#;
        assert!(serde_json::from_str::<ActionControlPutRequest>(retired).is_err());
    }

    #[test]
    fn scopes() {
        let mut r: ActionControlRecord = serde_json::from_str(
            r#"{"v":1,"entity_type":"user","head":"H","mode":"rules","version":1}"#,
        )
        .unwrap();
        r.classifier = Some("c".into());
        assert_eq!(r.scope(), "classifier");
        r.band = Some("b".into());
        assert_eq!(r.scope(), "classifier+band");
        r.classifier = None;
        assert_eq!(r.scope(), "band");
    }
}
