use crate::models::{
    Decided, HydratedTweetCandidate, LimitedEngagement, MediaInterstitial, Verdict, ViewerFeatures,
    Withholding,
};
use crate::params::NsfwGatingCountries;
use crate::rules::rule_spec::{ActionSpec, RuleClause};
use crate::rules::RuleContext;
use crate::rules::{author_rules, tweet_rules};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, strum::IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub enum SafetyLevel {
    FilterAll,
    TimelineHome,
    TimelineHomeRecommendations,
    TimelineHomeHydration,
}

pub(super) struct Policy<'a> {
    rules: &'a [&'a [RuleClause]],
    additional_rules: &'a [&'a [RuleClause]],
}

impl<'a> Policy<'a> {
    pub(super) const fn new(rules: &'a [&'a [RuleClause]]) -> Self {
        Self {
            rules,
            additional_rules: &[],
        }
    }

    fn rules(&self) -> impl Iterator<Item = &'a RuleClause> + '_ {
        self.rules
            .iter()
            .chain(self.additional_rules)
            .copied()
            .flatten()
    }

    pub(super) fn evaluate(&self, context: &RuleContext<'_>) -> Verdict {
        let mut media = None;
        let mut engagement = None;

        for rule in self.rules() {
            if !rule.applies(context) {
                continue;
            }
            match &rule.action {
                ActionSpec::Drop(reason) => {
                    return Verdict::Withheld(Decided {
                        value: Withholding::Drop(reason.clone()),
                        by: rule.rule_name,
                    });
                }
                ActionSpec::Tombstone(reason) => {
                    return Verdict::Withheld(Decided {
                        value: Withholding::Tombstone(*reason),
                        by: rule.rule_name,
                    });
                }
                ActionSpec::Interstitial {
                    legacy,
                    media: reason,
                } => {
                    media.get_or_insert_with(|| Decided {
                        value: MediaInterstitial {
                            legacy: legacy.clone(),
                            reason: reason.clone(),
                        },
                        by: rule.rule_name,
                    });
                }
                ActionSpec::LimitedEngagement(reason) => {
                    engagement.get_or_insert(Decided {
                        value: LimitedEngagement(*reason),
                        by: rule.rule_name,
                    });
                }
            }
        }

        Verdict::Shown { media, engagement }
    }

    fn rule_names(&self) -> impl Iterator<Item = &'static str> + '_ {
        let mut previous: Option<&'static str> = None;
        self.rules().filter_map(move |rule| {
            let repeat = previous == Some(rule.rule_name);
            previous = Some(rule.rule_name);
            (!repeat).then_some(rule.rule_name)
        })
    }

    fn len(&self) -> usize {
        self.rule_names().count()
    }
}

static FILTER_ALL_POLICY: Policy = Policy::new(&[tweet_rules::FILTER_ALL]);

static TIMELINE_HOME_SHARED_RULES: [&[RuleClause]; 10] = [
    author_rules::AUTHOR_STATE_DROPS,
    author_rules::SOCIALGRAPH_DROPS,
    tweet_rules::TWEET_LABEL_DROPS,
    tweet_rules::NULLCAST_DROP,
    tweet_rules::STALE_TWEET_DROP,
    tweet_rules::TAKEDOWN_DROPS,
    tweet_rules::SENSITIVE_VIEWER_DROPS,
    tweet_rules::EXCLUSIVE_TWEET_DROP,
    tweet_rules::NSFW_MEDIA_INTERSTITIALS,
    tweet_rules::NSFW_AUTHOR_INTERSTITIAL,
];

static TIMELINE_HOME_RECOMMENDATION_ONLY_RULES: [&[RuleClause]; 5] = [
    tweet_rules::RECS_MEDIA_DROPS,
    author_rules::OON_NSFW_AUTHOR_DROPS,
    tweet_rules::OON_TWEET_FLAG_DROPS,
    tweet_rules::OON_TWEET_LABEL_DROPS,
    author_rules::OON_USER_LABEL_DROPS,
];

static TIMELINE_HOME_POLICY: Policy = Policy::new(&TIMELINE_HOME_SHARED_RULES);
static TIMELINE_HOME_RECOMMENDATIONS_POLICY: Policy = Policy {
    rules: &TIMELINE_HOME_SHARED_RULES,
    additional_rules: &TIMELINE_HOME_RECOMMENDATION_ONLY_RULES,
};

static TIMELINE_HOME_HYDRATION_POLICY: Policy = Policy::new(&[
    tweet_rules::TWEET_LABEL_DROPS,
    tweet_rules::EXCLUSIVE_TWEET_DROP,
    tweet_rules::TAKEDOWN_DROPS,
    tweet_rules::SENSITIVE_VIEWER_DROPS,
    tweet_rules::NSFW_MEDIA_INTERSTITIALS,
    tweet_rules::NSFW_AUTHOR_INTERSTITIAL,
]);

pub struct RuleEngine {
    nsfw_gating_countries: Arc<NsfwGatingCountries>,
}

impl RuleEngine {
    #[cfg(test)]
    pub(crate) fn for_tests() -> Self {
        Self::with_nsfw_gating_countries(Arc::new(NsfwGatingCountries::starting_at_default()))
    }

    pub fn with_nsfw_gating_countries(gating_countries: Arc<NsfwGatingCountries>) -> Self {
        Self {
            nsfw_gating_countries: gating_countries,
        }
    }

    fn select(level: SafetyLevel) -> &'static Policy<'static> {
        match level {
            SafetyLevel::FilterAll => &FILTER_ALL_POLICY,
            SafetyLevel::TimelineHome => &TIMELINE_HOME_POLICY,
            SafetyLevel::TimelineHomeRecommendations => &TIMELINE_HOME_RECOMMENDATIONS_POLICY,
            SafetyLevel::TimelineHomeHydration => &TIMELINE_HOME_HYDRATION_POLICY,
        }
    }

    pub fn evaluate(
        &self,
        level: SafetyLevel,
        viewer: &ViewerFeatures,
        candidate: &HydratedTweetCandidate,
    ) -> Verdict {
        let context = RuleContext::new(viewer, candidate, &self.nsfw_gating_countries);
        Self::select(level).evaluate(&context)
    }

    #[cfg(test)]
    pub(crate) fn wired_rule_names(&self, level: SafetyLevel) -> Vec<&'static str> {
        Self::select(level).rule_names().collect()
    }

    pub fn rule_counts(&self) -> (usize, usize) {
        (
            TIMELINE_HOME_POLICY.len(),
            TIMELINE_HOME_RECOMMENDATIONS_POLICY.len(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::fixtures::{candidate, viewer, VIEWER_ID};

    #[test]
    fn refreshed_config_country_reaches_the_wired_rule() {
        let gating_countries = Arc::new(NsfwGatingCountries::starting_at_default());
        let rule_engine = RuleEngine::with_nsfw_gating_countries(Arc::clone(&gating_countries));
        let candidate = candidate()
            .with_label(crate::models::SafetyLabelType::NSFW_HIGH_PRECISION)
            .with_media()
            .build();
        let viewer = ViewerFeatures {
            viewer_age: crate::models::ViewerAge::NotStated,
            country_code: Some("us".into()),
            ..viewer(VIEWER_ID)
        };

        let verdict = rule_engine.evaluate(SafetyLevel::TimelineHome, &viewer, &candidate);
        assert!(!matches!(verdict, Verdict::Withheld(_)));

        gating_countries.refresh_from(
            &xai_feature_switches::FeatureSwitches::load_string(
                r#"
rust_vf:
  parameters:
    rust_vf_nsfw_gating_countries:
      type: array
      default:
      - "us"
"#,
            )
            .unwrap(),
        );
        let verdict = rule_engine.evaluate(SafetyLevel::TimelineHome, &viewer, &candidate);
        assert!(matches!(
            verdict,
            Verdict::Withheld(Decided {
                value: Withholding::Drop(_),
                by: "SensitiveViewerNoStatedAgeDropRule",
            })
        ));
    }

    #[test]
    fn wired_rule_order_matches_pre_migration_sequence() {
        let rule_engine = RuleEngine::for_tests();
        assert_eq!(rule_engine.rule_counts(), (30, 56));
        assert_eq!(
            rule_engine.wired_rule_names(SafetyLevel::FilterAll),
            vec!["FilterAllRule"]
        );
        let home = rule_engine.wired_rule_names(SafetyLevel::TimelineHome);
        assert_eq!(
            home,
            vec![
                "SuspendedAuthorRule",
                "DeactivatedAuthorRule",
                "ErasedAuthorRule",
                "OffboardedAuthorRule",
                "ProtectedAuthorDropRule",
                "ViewerBlocksAuthorRule",
                "ViewerMutesAuthorRule",
                "MutedRetweetsRule",
                "PdnaTweetLabelRule",
                "BounceTweetLabelRule",
                "SpamTweetLabelRule",
                "ForEmergencyUseOnlyDropRule",
                "FosnrHatefulConductDropRule",
                "FosnrViolentSpeechDropRule",
                "FosnrAbuseDropRule",
                "FosnrCivicIntegrityDropRule",
                "NullcastedTweetDropRule",
                "DropStaleTweetsRule",
                "DropLegalTakendownPostRule",
                "DropLocalLawsTakendownPostRule",
                "SensitiveViewerLoggedOutDropRule",
                "SensitiveViewerUnderageDropRule",
                "SensitiveViewerNoStatedAgeDropRule",
                "DropExclusiveTweetContentRule",
                "NsfwHighPrecisionAdultInterstitialRule",
                "NsfwHighPrecisionInterstitialRule",
                "GoreAndViolenceInterstitialRule",
                "NsfwCardImageInterstitialRule",
                "NsfwAdminInterstitialRule",
                "NsfwUserInterstitialRule",
            ]
        );
        let mut recs = home.clone();
        recs.extend([
            "DropTweetsWithDmcaMediaRule",
            "DropTweetsWithGeoRestrictedMediaRule",
            "DropNsfwUserAuthorRule",
            "DropNsfwAdminAuthorRule",
            "TweetNsfwUserDropRule",
            "TweetNsfwAdminDropRule",
            "NsfwHighRecallDropRule",
            "NsfwHighPrecisionOonDropRule",
            "GoreAndViolenceOonDropRule",
            "NsfwCardImageOonDropRule",
            "DoNotAmplifyOonDropRule",
            "MaliciousUrlOonDropRule",
            "SpamHighRecallDropRule",
            "NsfwTextTweetLabelDropRule",
            "FosnrAbuseInsultsOonDropRule",
            "NsfwHighRecallUserLabelRule",
            "NsfwHighPrecisionUserLabelRule",
            "SpamHighRecallUserLabelRule",
            "CompromisedUserLabelRule",
            "ReadOnlyUserLabelRule",
            "ImpersonationHighPrecisionUserLabelRule",
            "NsfwAvatarImageRule",
            "NsfwBannerImageRule",
            "AbusiveHighRecallRule",
            "NsfwNearPerfectAuthorRule",
            "DoNotAmplifyNonFollowerRule",
        ]);
        assert_eq!(
            rule_engine.wired_rule_names(SafetyLevel::TimelineHomeRecommendations),
            recs
        );
    }

    #[test]
    fn home_hydration_wires_only_its_ordered_baseline_rules() {
        assert_eq!(
            RuleEngine::for_tests().wired_rule_names(SafetyLevel::TimelineHomeHydration),
            vec![
                "PdnaTweetLabelRule",
                "BounceTweetLabelRule",
                "SpamTweetLabelRule",
                "ForEmergencyUseOnlyDropRule",
                "FosnrHatefulConductDropRule",
                "FosnrViolentSpeechDropRule",
                "FosnrAbuseDropRule",
                "FosnrCivicIntegrityDropRule",
                "DropExclusiveTweetContentRule",
                "DropLegalTakendownPostRule",
                "DropLocalLawsTakendownPostRule",
                "SensitiveViewerLoggedOutDropRule",
                "SensitiveViewerUnderageDropRule",
                "SensitiveViewerNoStatedAgeDropRule",
                "NsfwHighPrecisionAdultInterstitialRule",
                "NsfwHighPrecisionInterstitialRule",
                "GoreAndViolenceInterstitialRule",
                "NsfwCardImageInterstitialRule",
                "NsfwAdminInterstitialRule",
                "NsfwUserInterstitialRule",
            ]
        );
        assert_terminals_precede_restrictions(
            &TIMELINE_HOME_HYDRATION_POLICY,
            "TimelineHomeHydration",
        );
    }

    #[derive(Debug, PartialEq, Eq)]
    enum RowClass {
        Terminal,
        Restriction,
    }

    fn row_class(spec: &RuleClause) -> RowClass {
        match spec.action {
            ActionSpec::Drop(_) | ActionSpec::Tombstone(_) => RowClass::Terminal,
            ActionSpec::Interstitial { .. } | ActionSpec::LimitedEngagement(_) => {
                RowClass::Restriction
            }
        }
    }

    fn assert_terminals_precede_restrictions(policy: &Policy, name: &str) {
        let mut seen_restriction = false;
        for spec in policy.rules() {
            match row_class(spec) {
                RowClass::Restriction => seen_restriction = true,
                RowClass::Terminal if seen_restriction => {
                    panic!(
                        "{name}: terminal {} follows a restriction row",
                        spec.rule_name
                    );
                }
                RowClass::Terminal => {}
            }
        }
    }

    #[test]
    fn terminals_precede_restrictions_and_recommendation_only_rules_are_drops() {
        assert_terminals_precede_restrictions(&FILTER_ALL_POLICY, "FilterAll");
        assert_terminals_precede_restrictions(&TIMELINE_HOME_POLICY, "TimelineHome");
        for spec in TIMELINE_HOME_RECOMMENDATION_ONLY_RULES
            .iter()
            .copied()
            .flatten()
        {
            assert!(
                matches!(spec.action, ActionSpec::Drop(_)),
                "recommendation-only row {} must be a drop",
                spec.rule_name
            );
        }
    }

    #[test]
    fn opaque_conditions_are_three_documented_takedown_joins() {
        use crate::rules::rule_spec::Condition;
        let mut opaque = Vec::new();
        for spec in TIMELINE_HOME_RECOMMENDATIONS_POLICY
            .rules()
            .chain(FILTER_ALL_POLICY.rules())
        {
            for condition in spec.when {
                if let Condition::Opaque { id, doc, .. } = condition {
                    assert!(
                        !doc.trim().is_empty(),
                        "opaque condition {id} must carry a doc"
                    );
                    opaque.push(*id);
                }
            }
        }
        assert_eq!(
            opaque,
            vec![
                "legal_takedown_in_viewer_country",
                "local_laws_takedown_in_viewer_country",
                "media_geo_restricted_in_viewer_country",
            ]
        );
    }

    mod engine {
        use super::super::*;
        use crate::models::{
            HydratedTweetCandidate, LimitedEngagementReason, TombstoneReason, ViewerFeatures,
        };
        use crate::rules::rule_spec::{ActionSpec, Audience, Condition};
        use crate::rules::test_context;
        use xai_visibility_filtering::models::FilteredReason;
        use xai_x_thrift::action::InterstitialReason;

        const fn always(name: &'static str, action: ActionSpec) -> RuleClause {
            RuleClause {
                rule_name: name,
                when: &[],
                applies_to: Audience::Everyone,
                action,
            }
        }

        const NEVER: Condition = Condition::Opaque {
            id: "never",
            doc: "test leaf that never holds",
            eval: |_| false,
        };

        const UNREACHABLE: Condition = Condition::Opaque {
            id: "unreachable",
            doc: "test leaf that must not be evaluated",
            eval: |_| panic!("a rule after a terminal action must never be evaluated"),
        };

        const DROP_SUSPENDED: ActionSpec = ActionSpec::Drop(FilteredReason::AuthorIsSuspended);
        const TOMBSTONE: ActionSpec = ActionSpec::Tombstone(TombstoneReason::LocalRegulations);
        const INTERSTITIAL_NSFW: ActionSpec = ActionSpec::Interstitial {
            legacy: FilteredReason::ContainNsfwMedia,
            media: InterstitialReason::Sensitive(true),
        };
        const INTERSTITIAL_UNSPECIFIED: ActionSpec = ActionSpec::Interstitial {
            legacy: FilteredReason::UnspecifiedReason,
            media: InterstitialReason::Nudity(true),
        };
        const LIMIT: ActionSpec =
            ActionSpec::LimitedEngagement(LimitedEngagementReason::ConversationControl);

        fn context_inputs() -> (ViewerFeatures, HydratedTweetCandidate) {
            (ViewerFeatures::default(), HydratedTweetCandidate::default())
        }

        fn withheld(value: Withholding, by: &'static str) -> Verdict {
            Verdict::Withheld(Decided { value, by })
        }

        static SHORT_CIRCUIT_ROWS: [RuleClause; 3] = [
            RuleClause {
                rule_name: "allow",
                when: &[NEVER],
                applies_to: Audience::Everyone,
                action: DROP_SUSPENDED,
            },
            always("drop", DROP_SUSPENDED),
            RuleClause {
                rule_name: "after_drop",
                when: &[UNREACHABLE],
                applies_to: Audience::Everyone,
                action: TOMBSTONE,
            },
        ];
        static SHORT_CIRCUIT: Policy = Policy::new(&[&SHORT_CIRCUIT_ROWS]);

        static TOMBSTONE_FIRST_ROWS: [RuleClause; 2] = [
            always("tombstone", TOMBSTONE),
            always("drop", DROP_SUSPENDED),
        ];
        static TOMBSTONE_FIRST: Policy = Policy::new(&[&TOMBSTONE_FIRST_ROWS]);

        static RESTRICTION_ROWS: [RuleClause; 4] = [
            always("first_interstitial", INTERSTITIAL_NSFW),
            always("first_limit", LIMIT),
            always("second_interstitial", INTERSTITIAL_UNSPECIFIED),
            always("second_limit", LIMIT),
        ];
        static RESTRICTIONS: Policy = Policy::new(&[&RESTRICTION_ROWS]);

        static DROP_AFTER_RESTRICTIONS_ROWS: [RuleClause; 3] = [
            always("interstitial", INTERSTITIAL_NSFW),
            always("limit", LIMIT),
            always("drop", DROP_SUSPENDED),
        ];
        static DROP_AFTER_RESTRICTIONS: Policy = Policy::new(&[&DROP_AFTER_RESTRICTIONS_ROWS]);

        static ALL_ALLOWS_ROWS: [RuleClause; 2] = [
            RuleClause {
                rule_name: "a",
                when: &[NEVER],
                applies_to: Audience::Everyone,
                action: DROP_SUSPENDED,
            },
            RuleClause {
                rule_name: "b",
                when: &[NEVER],
                applies_to: Audience::Everyone,
                action: LIMIT,
            },
        ];
        static ALL_ALLOWS: Policy = Policy::new(&[&ALL_ALLOWS_ROWS]);

        #[test]
        fn first_terminal_returns_before_later_rules() {
            let (viewer, candidate) = context_inputs();

            let verdict = SHORT_CIRCUIT.evaluate(&test_context(&viewer, &candidate));

            assert_eq!(
                verdict,
                withheld(Withholding::Drop(FilteredReason::AuthorIsSuspended), "drop")
            );
        }

        #[test]
        fn tombstone_is_terminal_in_policy_order() {
            let (viewer, candidate) = context_inputs();

            let verdict = TOMBSTONE_FIRST.evaluate(&test_context(&viewer, &candidate));

            assert_eq!(
                verdict,
                withheld(
                    Withholding::Tombstone(TombstoneReason::LocalRegulations),
                    "tombstone"
                )
            );
        }

        #[test]
        fn each_slot_keeps_its_first_restriction() {
            let (viewer, candidate) = context_inputs();

            let verdict = RESTRICTIONS.evaluate(&test_context(&viewer, &candidate));

            assert_eq!(
                verdict,
                Verdict::Shown {
                    media: Some(Decided {
                        value: MediaInterstitial {
                            legacy: FilteredReason::ContainNsfwMedia,
                            reason: InterstitialReason::Sensitive(true),
                        },
                        by: "first_interstitial",
                    }),
                    engagement: Some(Decided {
                        value: LimitedEngagement(LimitedEngagementReason::ConversationControl),
                        by: "first_limit",
                    }),
                }
            );
        }

        #[test]
        fn terminal_after_restrictions_discards_them() {
            let (viewer, candidate) = context_inputs();

            let verdict = DROP_AFTER_RESTRICTIONS.evaluate(&test_context(&viewer, &candidate));

            assert_eq!(
                verdict,
                withheld(Withholding::Drop(FilteredReason::AuthorIsSuspended), "drop")
            );
        }

        #[test]
        fn no_applying_rule_is_shown_unrestricted() {
            let (viewer, candidate) = context_inputs();

            let verdict = ALL_ALLOWS.evaluate(&test_context(&viewer, &candidate));

            assert_eq!(
                verdict,
                Verdict::Shown {
                    media: None,
                    engagement: None,
                }
            );
        }
    }
}
