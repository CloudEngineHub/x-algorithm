use crate::models::{
    AuthorFeatures, AuthorLabel, ConversationControlFeatures, Decided, ExclusiveContentFeatures,
    HydratedTweetCandidate, LimitedEngagement, LimitedEngagementReason, MediaInterstitial,
    SafetyLabelType, TweetFeatures, Verdict, ViewerAge, ViewerAuthorRelationship, ViewerFeatures,
    ViewerProfile, Withholding,
};
use crate::rules::fixtures::{
    author_viewer, candidate, conversation_control, logged_out_viewer, sensitive_opt_in_viewer,
    viewer, viewer_with_profile, AUTHOR_ID, VIEWER_ID,
};
use crate::rules::{RuleEngine, SafetyLevel};
use crate::treatment::proto_action;
use prost::Message;
use std::collections::BTreeSet;
use xai_core_entities::entities::{
    ConversationControl, ConversationControlArm, EditControl, EditControlInitial, TakedownReason,
};
use xai_visibility_filtering::models::{
    Action, DropReason, FilteredReason, SafetyResult, SafetyResultReason,
};
use xai_x_thrift::action::InterstitialReason;
use SafetyLevel::{FilterAll, TimelineHome, TimelineHomeHydration, TimelineHomeRecommendations};

const REPLY_ROOT_AUTHOR_ID: u64 = 4242;

struct Case {
    name: &'static str,
    level: SafetyLevel,
    viewer: ViewerFeatures,
    candidate: HydratedTweetCandidate,
    expected: Verdict,
}

fn allow() -> Verdict {
    Verdict::Shown {
        media: None,
        engagement: None,
    }
}

fn dropped(reason: FilteredReason, by: &'static str) -> Verdict {
    Verdict::Withheld(Decided {
        value: Withholding::Drop(reason),
        by,
    })
}

fn blurred(reason: InterstitialReason, by: &'static str) -> Verdict {
    Verdict::Shown {
        media: Some(Decided {
            value: MediaInterstitial {
                legacy: FilteredReason::ContainNsfwMedia,
                reason,
            },
            by,
        }),
        engagement: None,
    }
}

fn limited(by: &'static str) -> Verdict {
    Verdict::Shown {
        media: None,
        engagement: Some(Decided {
            value: LimitedEngagement(LimitedEngagementReason::ConversationControl),
            by,
        }),
    }
}

fn deciders(verdict: &Verdict) -> Vec<&'static str> {
    match verdict {
        Verdict::Withheld(decided) => vec![decided.by],
        Verdict::Shown { media, engagement } => media
            .iter()
            .map(|blur| blur.by)
            .chain(engagement.iter().map(|limit| limit.by))
            .collect(),
    }
}

#[test]
fn golden_corpus_pins_policy_verdicts() {
    let rule_engine = RuleEngine::for_tests();
    let cases = cases();
    let names: BTreeSet<&'static str> = cases.iter().map(|c| c.name).collect();
    assert_eq!(names.len(), cases.len(), "duplicate corpus case name");
    let mut failures = Vec::new();
    for case in cases {
        let verdict = rule_engine.evaluate(case.level, &case.viewer, &case.candidate);
        if matches!(&case.expected, Verdict::Shown { media: Some(_), .. }) {
            let (action, reason) = proto_action(verdict.clone());
            assert_eq!(action.encode_to_vec(), [0x20, 0x01], "{}", case.name);
            assert_eq!(
                reason.unwrap().encode_to_vec(),
                [0x08, 0x01],
                "{}",
                case.name
            );
        }
        if verdict != case.expected {
            failures.push(format!(
                "{} [{:?}]:\n  expected {:?}\n  got      {:?}",
                case.name, case.level, case.expected, verdict,
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} corpus case(s) diverged:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn every_wired_rule_decides_a_corpus_case() {
    let rule_engine = RuleEngine::for_tests();
    let wired: BTreeSet<&'static str> = [
        FilterAll,
        TimelineHome,
        TimelineHomeRecommendations,
        TimelineHomeHydration,
    ]
    .into_iter()
    .flat_map(|level| rule_engine.wired_rule_names(level))
    .collect();
    let deciders: BTreeSet<&'static str> =
        cases().iter().flat_map(|c| deciders(&c.expected)).collect();
    let missing: Vec<&&'static str> = wired.difference(&deciders).collect();
    assert!(
        missing.is_empty(),
        "rules wired in RuleEngine but never the decider of any corpus case: {missing:?}"
    );
}

fn cases() -> Vec<Case> {
    let mut cases = filter_all_cases();
    cases.extend(baseline_cases());
    cases.extend(author_state_cases());
    cases.extend(relationship_cases());
    cases.extend(tweet_label_cases());
    cases.extend(tweet_shape_cases());
    cases.extend(age_gating_cases());
    cases.extend(exclusive_content_cases());
    cases.extend(conversation_control_cases());
    cases.extend(interstitial_cases());
    cases.extend(oon_media_cases());
    cases.extend(oon_tweet_label_cases());
    cases.extend(oon_user_label_cases());
    cases.extend(interaction_cases());
    cases
}

fn author_candidate(set: fn(&mut AuthorFeatures)) -> HydratedTweetCandidate {
    let mut features = AuthorFeatures::default();
    set(&mut features);
    candidate().with_author_features(features).build()
}

fn tweet_candidate(set: fn(&mut TweetFeatures)) -> HydratedTweetCandidate {
    let mut features = TweetFeatures::default();
    set(&mut features);
    candidate().with_tweet_features(features).build()
}

fn relationship_candidate(set: fn(&mut ViewerAuthorRelationship)) -> HydratedTweetCandidate {
    let mut relationship = ViewerAuthorRelationship::default();
    set(&mut relationship);
    candidate().with_relationship(relationship).build()
}

fn labeled(label: SafetyLabelType) -> HydratedTweetCandidate {
    candidate().with_label(label).build()
}

fn labeled_media(label: SafetyLabelType) -> HydratedTweetCandidate {
    candidate().with_label(label).with_media().build()
}

fn user_labeled(label: AuthorLabel) -> HydratedTweetCandidate {
    candidate().with_author_user_label(label).build()
}

fn user_labeled_follower(label: AuthorLabel) -> HydratedTweetCandidate {
    candidate().with_author_user_label(label).followed().build()
}

fn stale_candidate() -> HydratedTweetCandidate {
    tweet_candidate(|t| {
        t.edit_control = Some(EditControl::Initial(EditControlInitial {
            edit_tweet_ids: vec![1, 2],
            ..Default::default()
        }))
    })
}

fn takedown_candidate(reason: TakedownReason) -> HydratedTweetCandidate {
    candidate()
        .with_tweet_features(TweetFeatures {
            takedown_reasons: vec![reason],
            ..Default::default()
        })
        .build()
}

fn exclusive_candidate(viewer_super_follows_author: bool) -> HydratedTweetCandidate {
    let mut c = candidate().build();
    c.exclusive_content = Some(ExclusiveContentFeatures {
        conversation_author_id: 42,
        viewer_super_follows_author,
    });
    c
}

fn controlled_root(arm: ConversationControlArm) -> ConversationControlFeatures {
    conversation_control(arm, AUTHOR_ID)
}

fn controlled_candidate(features: ConversationControlFeatures) -> HydratedTweetCandidate {
    candidate().with_conversation_control(features).build()
}

fn viewer_in_country(code: &str) -> ViewerFeatures {
    ViewerFeatures {
        country_code: Some(code.to_string()),
        ..viewer(VIEWER_ID)
    }
}

fn viewer_with_age(age: ViewerAge) -> ViewerFeatures {
    viewer_with_profile(ViewerProfile {
        viewer_age: age,
        ..ViewerProfile::default()
    })
}

fn no_stated_age_viewer(account_country_code: &str) -> ViewerFeatures {
    viewer_with_profile(ViewerProfile {
        viewer_age: ViewerAge::NotStated,
        account_country_code: Some(account_country_code.to_string()),
        ..ViewerProfile::default()
    })
}

fn nsfw_high_precision_reason() -> FilteredReason {
    FilteredReason::SafetyResult(SafetyResult {
        reason: Some(SafetyResultReason::NsfwHighPrecision),
        action: Action::Drop(DropReason {}),
    })
}

fn filter_all_cases() -> Vec<Case> {
    vec![
        Case {
            name: "filter_all_drops_pristine_candidate",
            level: FilterAll,
            viewer: viewer(VIEWER_ID),
            candidate: candidate().build(),
            expected: dropped(FilteredReason::UnspecifiedReason, "FilterAllRule"),
        },
        Case {
            name: "filter_all_drops_even_self_view",
            level: FilterAll,
            viewer: author_viewer(),
            candidate: candidate().build(),
            expected: dropped(FilteredReason::UnspecifiedReason, "FilterAllRule"),
        },
    ]
}

fn baseline_cases() -> Vec<Case> {
    vec![
        Case {
            name: "home_hydration_allows_stale_tweet",
            level: TimelineHomeHydration,
            viewer: viewer(VIEWER_ID),
            candidate: stale_candidate(),
            expected: allow(),
        },
        Case {
            name: "home_hydration_emergency_drops_even_self_view",
            level: TimelineHomeHydration,
            viewer: author_viewer(),
            candidate: labeled(SafetyLabelType::FOR_EMERGENCY_USE_ONLY),
            expected: dropped(
                FilteredReason::UnspecifiedReason,
                "ForEmergencyUseOnlyDropRule",
            ),
        },
        Case {
            name: "home_hydration_nsfw_label_blurs_non_follower",
            level: TimelineHomeHydration,
            viewer: viewer(VIEWER_ID),
            candidate: labeled_media(SafetyLabelType::NSFW_HIGH_PRECISION),
            expected: blurred(
                InterstitialReason::Sensitive(true),
                "NsfwHighPrecisionInterstitialRule",
            ),
        },
        Case {
            name: "home_allows_pristine_candidate",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: candidate().build(),
            expected: allow(),
        },
        Case {
            name: "recommendations_allow_pristine_candidate",
            level: TimelineHomeRecommendations,
            viewer: viewer(VIEWER_ID),
            candidate: candidate().build(),
            expected: allow(),
        },
        Case {
            name: "home_allows_pristine_candidate_for_logged_out",
            level: TimelineHome,
            viewer: logged_out_viewer(),
            candidate: candidate().build(),
            expected: allow(),
        },
        Case {
            name: "home_allows_egregious_nsfw_tweet_label",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: labeled(SafetyLabelType::EGREGIOUS_NSFW),
            expected: allow(),
        },
        Case {
            name: "recommendations_allow_egregious_nsfw_tweet_label",
            level: TimelineHomeRecommendations,
            viewer: viewer(VIEWER_ID),
            candidate: labeled(SafetyLabelType::EGREGIOUS_NSFW),
            expected: allow(),
        },
    ]
}

fn author_state_cases() -> Vec<Case> {
    vec![
        Case {
            name: "suspended_author_drops",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: author_candidate(|a| a.is_suspended = true),
            expected: dropped(FilteredReason::AuthorIsSuspended, "SuspendedAuthorRule"),
        },
        Case {
            name: "suspended_author_allows_self_view",
            level: TimelineHome,
            viewer: author_viewer(),
            candidate: author_candidate(|a| a.is_suspended = true),
            expected: allow(),
        },
        Case {
            name: "deactivated_author_drops",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: author_candidate(|a| a.is_deactivated = true),
            expected: dropped(FilteredReason::AuthorIsDeactivated, "DeactivatedAuthorRule"),
        },
        Case {
            name: "erased_author_drops",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: author_candidate(|a| a.is_erased = true),
            expected: dropped(FilteredReason::AuthorAccountIsInactive, "ErasedAuthorRule"),
        },
        Case {
            name: "offboarded_author_drops",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: author_candidate(|a| a.is_offboarded = true),
            expected: dropped(
                FilteredReason::AuthorAccountIsInactive,
                "OffboardedAuthorRule",
            ),
        },
        Case {
            name: "protected_author_drops_non_follower",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: author_candidate(|a| a.is_protected = true),
            expected: dropped(FilteredReason::AuthorIsProtected, "ProtectedAuthorDropRule"),
        },
        Case {
            name: "protected_author_allows_follower",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: {
                let features = AuthorFeatures {
                    is_protected: true,
                    ..Default::default()
                };
                candidate()
                    .with_author_features(features)
                    .followed()
                    .build()
            },
            expected: allow(),
        },
        Case {
            name: "protected_author_drops_logged_out",
            level: TimelineHome,
            viewer: logged_out_viewer(),
            candidate: author_candidate(|a| a.is_protected = true),
            expected: dropped(FilteredReason::AuthorIsProtected, "ProtectedAuthorDropRule"),
        },
    ]
}

fn relationship_cases() -> Vec<Case> {
    vec![
        Case {
            name: "viewer_blocking_author_drops",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: relationship_candidate(|r| r.viewer_blocks_author = true),
            expected: dropped(FilteredReason::ViewerBlocksAuthor, "ViewerBlocksAuthorRule"),
        },
        Case {
            name: "viewer_muting_author_drops",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: relationship_candidate(|r| r.viewer_mutes_author = true),
            expected: dropped(FilteredReason::ViewerMutesAuthor, "ViewerMutesAuthorRule"),
        },
        Case {
            name: "block_decides_before_mute",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: relationship_candidate(|r| {
                r.viewer_blocks_author = true;
                r.viewer_mutes_author = true;
            }),
            expected: dropped(FilteredReason::ViewerBlocksAuthor, "ViewerBlocksAuthorRule"),
        },
        Case {
            name: "muted_retweets_drop_retweet",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: {
                let relationship = ViewerAuthorRelationship {
                    viewer_mutes_retweets_from_author: true,
                    ..Default::default()
                };
                candidate()
                    .with_relationship(relationship)
                    .retweet_of(2)
                    .build()
            },
            expected: dropped(FilteredReason::UnspecifiedReason, "MutedRetweetsRule"),
        },
        Case {
            name: "muted_retweets_allow_original_tweet",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: relationship_candidate(|r| r.viewer_mutes_retweets_from_author = true),
            expected: allow(),
        },
    ]
}

fn tweet_label_cases() -> Vec<Case> {
    vec![
        Case {
            name: "pdna_label_drops",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: labeled(SafetyLabelType::PDNA),
            expected: dropped(nsfw_high_precision_reason(), "PdnaTweetLabelRule"),
        },
        Case {
            name: "pdna_label_allows_self_view",
            level: TimelineHome,
            viewer: author_viewer(),
            candidate: labeled(SafetyLabelType::PDNA),
            expected: allow(),
        },
        Case {
            name: "bounce_label_drops",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: labeled(SafetyLabelType::BOUNCE),
            expected: dropped(FilteredReason::TweetIsBounced, "BounceTweetLabelRule"),
        },
        Case {
            name: "spam_label_drops",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: labeled(SafetyLabelType::SPAM),
            expected: dropped(FilteredReason::PossiblyUndesirable, "SpamTweetLabelRule"),
        },
        Case {
            name: "for_emergency_use_only_label_drops",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: labeled(SafetyLabelType::FOR_EMERGENCY_USE_ONLY),
            expected: dropped(
                FilteredReason::UnspecifiedReason,
                "ForEmergencyUseOnlyDropRule",
            ),
        },
        Case {
            name: "for_emergency_use_only_label_drops_even_self_view",
            level: TimelineHome,
            viewer: author_viewer(),
            candidate: labeled(SafetyLabelType::FOR_EMERGENCY_USE_ONLY),
            expected: dropped(
                FilteredReason::UnspecifiedReason,
                "ForEmergencyUseOnlyDropRule",
            ),
        },
        Case {
            name: "fosnr_hateful_conduct_label_drops",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: labeled(SafetyLabelType::FOSNR_HATEFUL_CONDUCT),
            expected: dropped(
                FilteredReason::PossiblyUndesirable,
                "FosnrHatefulConductDropRule",
            ),
        },
        Case {
            name: "fosnr_violent_speech_label_drops",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: labeled(SafetyLabelType::FOSNR_VIOLENT_SPEECH),
            expected: dropped(
                FilteredReason::PossiblyUndesirable,
                "FosnrViolentSpeechDropRule",
            ),
        },
        Case {
            name: "fosnr_abuse_label_drops",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: labeled(SafetyLabelType::FOSNR_ABUSE),
            expected: dropped(FilteredReason::PossiblyUndesirable, "FosnrAbuseDropRule"),
        },
        Case {
            name: "fosnr_civic_integrity_label_drops",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: labeled(SafetyLabelType::FOSNR_CIVIC_INTEGRITY),
            expected: dropped(
                FilteredReason::PossiblyUndesirable,
                "FosnrCivicIntegrityDropRule",
            ),
        },
    ]
}

fn tweet_shape_cases() -> Vec<Case> {
    vec![
        Case {
            name: "nullcast_tweet_drops",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: tweet_candidate(|t| t.is_nullcast = true),
            expected: dropped(FilteredReason::TweetIsNullcast, "NullcastedTweetDropRule"),
        },
        Case {
            name: "nullcast_retweet_allows",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: {
                let features = TweetFeatures {
                    is_nullcast: true,
                    ..Default::default()
                };
                candidate()
                    .with_tweet_features(features)
                    .retweet_of(2)
                    .build()
            },
            expected: allow(),
        },
        Case {
            name: "nullcast_community_tweet_allows",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: tweet_candidate(|t| {
                t.is_nullcast = true;
                t.is_community_tweet = true;
            }),
            expected: allow(),
        },
        Case {
            name: "nullcast_tweet_drops_even_self_view",
            level: TimelineHome,
            viewer: author_viewer(),
            candidate: tweet_candidate(|t| t.is_nullcast = true),
            expected: dropped(FilteredReason::TweetIsNullcast, "NullcastedTweetDropRule"),
        },
        Case {
            name: "stale_edit_tweet_drops",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: stale_candidate(),
            expected: dropped(FilteredReason::UnspecifiedReason, "DropStaleTweetsRule"),
        },
        Case {
            name: "stale_edit_retweet_allows",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: {
                let mut retweet = stale_candidate();
                retweet.tweet_features.source_tweet_id = Some(2);
                retweet
            },
            expected: allow(),
        },
        Case {
            name: "current_edit_tweet_allows",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: tweet_candidate(|t| {
                t.edit_control = Some(EditControl::Initial(EditControlInitial {
                    edit_tweet_ids: vec![1],
                    ..Default::default()
                }))
            }),
            expected: allow(),
        },
        Case {
            name: "legal_takedown_drops_in_withheld_country",
            level: TimelineHome,
            viewer: viewer_in_country("us"),
            candidate: takedown_candidate(TakedownReason::LegalRequest {
                country_code: "us".to_string(),
            }),
            expected: dropped(
                FilteredReason::UnspecifiedReason,
                "DropLegalTakendownPostRule",
            ),
        },
        Case {
            name: "legal_takedown_allows_other_country",
            level: TimelineHome,
            viewer: viewer_in_country("fr"),
            candidate: takedown_candidate(TakedownReason::LegalRequest {
                country_code: "us".to_string(),
            }),
            expected: allow(),
        },
        Case {
            name: "local_laws_takedown_drops_in_withheld_country",
            level: TimelineHome,
            viewer: viewer_in_country("de"),
            candidate: takedown_candidate(TakedownReason::BystanderReport {
                country_code: "de".to_string(),
            }),
            expected: dropped(
                FilteredReason::UnspecifiedReason,
                "DropLocalLawsTakendownPostRule",
            ),
        },
        Case {
            name: "legal_takedown_worldwide_drops_for_us_viewer",
            level: TimelineHome,
            viewer: viewer_in_country("us"),
            candidate: takedown_candidate(TakedownReason::LegalRequest {
                country_code: "xx".to_string(),
            }),
            expected: dropped(
                FilteredReason::UnspecifiedReason,
                "DropLegalTakendownPostRule",
            ),
        },
        Case {
            name: "legal_takedown_worldwide_drops_without_viewer_country",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: takedown_candidate(TakedownReason::LegalRequest {
                country_code: "xx".to_string(),
            }),
            expected: dropped(
                FilteredReason::UnspecifiedReason,
                "DropLegalTakendownPostRule",
            ),
        },
        Case {
            name: "legal_takedown_copyright_code_allows_without_viewer_country",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: takedown_candidate(TakedownReason::LegalRequest {
                country_code: "xy".to_string(),
            }),
            expected: allow(),
        },
        Case {
            name: "unspecified_takedown_worldwide_drops_without_viewer_country",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: takedown_candidate(TakedownReason::UnspecifiedReason {
                country_code: "xx".to_string(),
            }),
            expected: dropped(
                FilteredReason::UnspecifiedReason,
                "DropLegalTakendownPostRule",
            ),
        },
        Case {
            name: "unspecified_takedown_copyright_code_drops_without_viewer_country",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: takedown_candidate(TakedownReason::UnspecifiedReason {
                country_code: "xy".to_string(),
            }),
            expected: dropped(
                FilteredReason::UnspecifiedReason,
                "DropLegalTakendownPostRule",
            ),
        },
        Case {
            name: "local_laws_takedown_worldwide_allows_any_viewer",
            level: TimelineHome,
            viewer: viewer_in_country("us"),
            candidate: takedown_candidate(TakedownReason::BystanderReport {
                country_code: "xx".to_string(),
            }),
            expected: allow(),
        },
        Case {
            name: "dmca_takedown_drops_for_any_viewer",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: takedown_candidate(TakedownReason::Dmca),
            expected: dropped(
                FilteredReason::UnspecifiedReason,
                "DropLegalTakendownPostRule",
            ),
        },
        Case {
            name: "dmca_takedown_allows_author",
            level: TimelineHome,
            viewer: author_viewer(),
            candidate: takedown_candidate(TakedownReason::Dmca),
            expected: allow(),
        },
    ]
}

fn age_gating_cases() -> Vec<Case> {
    vec![
        Case {
            name: "logged_out_viewer_drops_sensitive_media",
            level: TimelineHome,
            viewer: logged_out_viewer(),
            candidate: labeled_media(SafetyLabelType::NSFW_HIGH_RECALL),
            expected: dropped(
                FilteredReason::ContainNsfwMedia,
                "SensitiveViewerLoggedOutDropRule",
            ),
        },
        Case {
            name: "underage_viewer_drops_sensitive_media",
            level: TimelineHome,
            viewer: viewer_with_age(ViewerAge::Known(17)),
            candidate: labeled_media(SafetyLabelType::NSFW_HIGH_RECALL),
            expected: dropped(
                FilteredReason::ContainNsfwMedia,
                "SensitiveViewerUnderageDropRule",
            ),
        },
        Case {
            name: "no_stated_age_in_gating_country_drops_sensitive_media",
            level: TimelineHome,
            viewer: no_stated_age_viewer("gb"),
            candidate: labeled_media(SafetyLabelType::NSFW_HIGH_RECALL),
            expected: dropped(
                FilteredReason::ContainNsfwMedia,
                "SensitiveViewerNoStatedAgeDropRule",
            ),
        },
        Case {
            name: "no_stated_age_outside_gating_country_allows_sensitive_media",
            level: TimelineHome,
            viewer: no_stated_age_viewer("us"),
            candidate: labeled_media(SafetyLabelType::NSFW_HIGH_RECALL),
            expected: allow(),
        },
        Case {
            name: "known_adult_age_allows_sensitive_media_in_network",
            level: TimelineHome,
            viewer: viewer_with_age(ViewerAge::Known(30)),
            candidate: labeled_media(SafetyLabelType::NSFW_HIGH_RECALL),
            expected: allow(),
        },
    ]
}

fn exclusive_content_cases() -> Vec<Case> {
    vec![
        Case {
            name: "exclusive_tweet_drops_non_subscriber",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: exclusive_candidate(false),
            expected: dropped(
                FilteredReason::ExclusiveTweet,
                "DropExclusiveTweetContentRule",
            ),
        },
        Case {
            name: "exclusive_tweet_allows_super_follower",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: exclusive_candidate(true),
            expected: allow(),
        },
        Case {
            name: "exclusive_tweet_drops_logged_out",
            level: TimelineHome,
            viewer: logged_out_viewer(),
            candidate: exclusive_candidate(false),
            expected: dropped(
                FilteredReason::ExclusiveTweet,
                "DropExclusiveTweetContentRule",
            ),
        },
    ]
}

fn conversation_control_cases() -> Vec<Case> {
    use ConversationControlArm::{ByInvitation, Community, Subscribers, Verified};
    vec![
        Case {
            name: "home_hydration_by_invitation_conversation_limits_replies",
            level: TimelineHomeHydration,
            viewer: viewer(VIEWER_ID),
            candidate: controlled_candidate(controlled_root(ByInvitation)),
            expected: limited("LimitRepliesByInvitationConversationRule"),
        },
        Case {
            name: "home_hydration_community_conversation_limits_replies",
            level: TimelineHomeHydration,
            viewer: viewer(VIEWER_ID),
            candidate: controlled_candidate(controlled_root(Community)),
            expected: limited("LimitRepliesCommunityConversationRule"),
        },
        Case {
            name: "home_hydration_subscribers_conversation_limits_replies",
            level: TimelineHomeHydration,
            viewer: viewer(VIEWER_ID),
            candidate: controlled_candidate(controlled_root(Subscribers)),
            expected: limited("LimitRepliesSubscribersConversationRule"),
        },
        Case {
            name: "home_hydration_verified_conversation_limits_replies",
            level: TimelineHomeHydration,
            viewer: viewer(VIEWER_ID),
            candidate: controlled_candidate(controlled_root(Verified)),
            expected: limited("LimitRepliesVerifiedConversationRule"),
        },
        Case {
            name: "home_hydration_reply_carrying_root_arm_limits_its_author",
            level: TimelineHomeHydration,
            viewer: author_viewer(),
            candidate: controlled_candidate(conversation_control(
                ByInvitation,
                REPLY_ROOT_AUTHOR_ID,
            )),
            expected: limited("LimitRepliesByInvitationConversationRule"),
        },
        Case {
            name: "home_hydration_conversation_control_exempts_root_author",
            level: TimelineHomeHydration,
            viewer: author_viewer(),
            candidate: controlled_candidate(controlled_root(ByInvitation)),
            expected: allow(),
        },
        Case {
            name: "home_hydration_conversation_control_exempts_invited_viewer",
            level: TimelineHomeHydration,
            viewer: viewer(VIEWER_ID),
            candidate: controlled_candidate(ConversationControlFeatures {
                control: ConversationControl {
                    invited_user_ids: vec![VIEWER_ID],
                    ..controlled_root(ByInvitation).control
                },
                ..controlled_root(ByInvitation)
            }),
            expected: allow(),
        },
        Case {
            name: "home_hydration_conversation_control_allows_logged_out",
            level: TimelineHomeHydration,
            viewer: logged_out_viewer(),
            candidate: controlled_candidate(controlled_root(ByInvitation)),
            expected: allow(),
        },
        Case {
            name: "home_hydration_conversation_control_allows_retweet",
            level: TimelineHomeHydration,
            viewer: viewer(VIEWER_ID),
            candidate: candidate()
                .retweet_of(2)
                .with_conversation_control(controlled_root(ByInvitation))
                .build(),
            expected: allow(),
        },
        Case {
            name: "home_hydration_community_conversation_exempts_followed_viewer",
            level: TimelineHomeHydration,
            viewer: viewer(VIEWER_ID),
            candidate: controlled_candidate(ConversationControlFeatures {
                root_author_follows_viewer: Some(true),
                ..controlled_root(Community)
            }),
            expected: allow(),
        },
        Case {
            name: "home_hydration_subscribers_conversation_exempts_super_follower",
            level: TimelineHomeHydration,
            viewer: viewer(VIEWER_ID),
            candidate: controlled_candidate(ConversationControlFeatures {
                viewer_super_follows_root_author: Some(true),
                ..controlled_root(Subscribers)
            }),
            expected: allow(),
        },
        Case {
            name: "home_hydration_verified_conversation_exempts_verified_viewer",
            level: TimelineHomeHydration,
            viewer: viewer_with_profile(ViewerProfile {
                has_verified_badge: true,
                ..ViewerProfile::default()
            }),
            candidate: controlled_candidate(controlled_root(Verified)),
            expected: allow(),
        },
    ]
}

fn interstitial_cases() -> Vec<Case> {
    const AT_CUTOFF: u64 = (1705536000000 - 1288834974657) << 22;
    vec![
        Case {
            name: "nsfw_high_precision_adult_label_interstitials_in_network",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: candidate()
                .tweet_id(AT_CUTOFF + (1 << 22))
                .with_label(SafetyLabelType::NSFW_HIGH_PRECISION)
                .build(),
            expected: blurred(
                InterstitialReason::Nudity(true),
                "NsfwHighPrecisionAdultInterstitialRule",
            ),
        },
        Case {
            name: "nsfw_high_precision_label_interstitials_in_network",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: candidate()
                .tweet_id(AT_CUTOFF)
                .with_label(SafetyLabelType::NSFW_HIGH_PRECISION)
                .build(),
            expected: blurred(
                InterstitialReason::Sensitive(true),
                "NsfwHighPrecisionInterstitialRule",
            ),
        },
        Case {
            name: "gore_and_violence_label_interstitials_in_network",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: labeled(SafetyLabelType::GORE_AND_VIOLENCE_HIGH_PRECISION),
            expected: blurred(
                InterstitialReason::Violence(true),
                "GoreAndViolenceInterstitialRule",
            ),
        },
        Case {
            name: "nsfw_card_image_label_interstitials_in_network",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: labeled(SafetyLabelType::NSFW_CARD_IMAGE),
            expected: blurred(
                InterstitialReason::Sensitive(true),
                "NsfwCardImageInterstitialRule",
            ),
        },
        Case {
            name: "nsfw_admin_with_media_interstitials_in_network",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: candidate()
                .with_author_features(AuthorFeatures {
                    is_nsfw_admin: true,
                    ..Default::default()
                })
                .with_media()
                .build(),
            expected: blurred(
                InterstitialReason::Sensitive(true),
                "NsfwAdminInterstitialRule",
            ),
        },
        Case {
            name: "nsfw_user_with_media_interstitials_in_network",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: {
                let features = AuthorFeatures {
                    is_nsfw_user: true,
                    ..Default::default()
                };
                candidate()
                    .with_author_features(features)
                    .with_media()
                    .build()
            },
            expected: blurred(
                InterstitialReason::SensitiveUser(true),
                "NsfwUserInterstitialRule",
            ),
        },
        Case {
            name: "tweet_nsfw_admin_flag_with_media_interstitials_in_network",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: tweet_candidate(|t| {
                t.nsfw.admin = true;
                t.media.has_media = true;
            }),
            expected: blurred(
                InterstitialReason::Sensitive(true),
                "NsfwAdminInterstitialRule",
            ),
        },
        Case {
            name: "tweet_nsfw_user_flag_with_media_interstitials_in_network",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: tweet_candidate(|t| {
                t.nsfw.user = true;
                t.media.has_media = true;
            }),
            expected: blurred(
                InterstitialReason::SensitiveUser(true),
                "NsfwUserInterstitialRule",
            ),
        },
        Case {
            name: "nsfw_admin_interstitial_beats_nsfw_user_when_author_has_both_flags",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: candidate()
                .with_author_features(AuthorFeatures {
                    is_nsfw_user: true,
                    is_nsfw_admin: true,
                    ..Default::default()
                })
                .with_media()
                .build(),
            expected: blurred(
                InterstitialReason::Sensitive(true),
                "NsfwAdminInterstitialRule",
            ),
        },
        Case {
            name: "two_interstitial_labels_keep_the_first_media_blur",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: candidate()
                .with_label(SafetyLabelType::GORE_AND_VIOLENCE_HIGH_PRECISION)
                .with_label(SafetyLabelType::NSFW_CARD_IMAGE)
                .build(),
            expected: blurred(
                InterstitialReason::Violence(true),
                "GoreAndViolenceInterstitialRule",
            ),
        },
        Case {
            name: "nsfw_author_without_media_allows_in_network",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: author_candidate(|a| a.is_nsfw_user = true),
            expected: allow(),
        },
        Case {
            name: "nsfw_interstitial_exempts_self_view",
            level: TimelineHome,
            viewer: author_viewer(),
            candidate: labeled(SafetyLabelType::NSFW_HIGH_PRECISION),
            expected: allow(),
        },
        Case {
            name: "nsfw_interstitial_exempts_sensitive_opt_in_viewer",
            level: TimelineHome,
            viewer: sensitive_opt_in_viewer(),
            candidate: labeled(SafetyLabelType::NSFW_HIGH_PRECISION),
            expected: allow(),
        },
    ]
}

fn oon_media_cases() -> Vec<Case> {
    vec![
        Case {
            name: "dmca_media_drops_oon",
            level: TimelineHomeRecommendations,
            viewer: viewer(VIEWER_ID),
            candidate: tweet_candidate(|t| t.media.has_dmca_media = true),
            expected: dropped(
                FilteredReason::UnspecifiedReason,
                "DropTweetsWithDmcaMediaRule",
            ),
        },
        Case {
            name: "dmca_media_allows_in_network",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: tweet_candidate(|t| t.media.has_dmca_media = true),
            expected: allow(),
        },
        Case {
            name: "geo_denied_media_drops_oon",
            level: TimelineHomeRecommendations,
            viewer: viewer_in_country("de"),
            candidate: tweet_candidate(|t| t.media.geo_deny_list = vec!["de".to_string()]),
            expected: dropped(
                FilteredReason::UnspecifiedReason,
                "DropTweetsWithGeoRestrictedMediaRule",
            ),
        },
        Case {
            name: "geo_allow_listed_media_drops_unknown_country_oon",
            level: TimelineHomeRecommendations,
            viewer: viewer(VIEWER_ID),
            candidate: tweet_candidate(|t| t.media.geo_allow_list = vec!["us".to_string()]),
            expected: dropped(
                FilteredReason::UnspecifiedReason,
                "DropTweetsWithGeoRestrictedMediaRule",
            ),
        },
        Case {
            name: "nsfw_user_author_drops_oon",
            level: TimelineHomeRecommendations,
            viewer: viewer(VIEWER_ID),
            candidate: author_candidate(|a| a.is_nsfw_user = true),
            expected: dropped(FilteredReason::ContainNsfwMedia, "DropNsfwUserAuthorRule"),
        },
        Case {
            name: "nsfw_admin_author_drops_oon",
            level: TimelineHomeRecommendations,
            viewer: viewer(VIEWER_ID),
            candidate: author_candidate(|a| a.is_nsfw_admin = true),
            expected: dropped(FilteredReason::ContainNsfwMedia, "DropNsfwAdminAuthorRule"),
        },
        Case {
            name: "tweet_nsfw_user_flag_drops_oon",
            level: TimelineHomeRecommendations,
            viewer: viewer(VIEWER_ID),
            candidate: tweet_candidate(|t| t.nsfw.user = true),
            expected: dropped(FilteredReason::ContainNsfwMedia, "TweetNsfwUserDropRule"),
        },
        Case {
            name: "tweet_nsfw_user_flag_drops_even_self_view_oon",
            level: TimelineHomeRecommendations,
            viewer: author_viewer(),
            candidate: tweet_candidate(|t| t.nsfw.user = true),
            expected: dropped(FilteredReason::ContainNsfwMedia, "TweetNsfwUserDropRule"),
        },
        Case {
            name: "tweet_nsfw_admin_flag_drops_oon",
            level: TimelineHomeRecommendations,
            viewer: viewer(VIEWER_ID),
            candidate: tweet_candidate(|t| t.nsfw.admin = true),
            expected: dropped(FilteredReason::ContainNsfwMedia, "TweetNsfwAdminDropRule"),
        },
        Case {
            name: "tweet_nsfw_admin_flag_drops_even_self_view_oon",
            level: TimelineHomeRecommendations,
            viewer: author_viewer(),
            candidate: tweet_candidate(|t| t.nsfw.admin = true),
            expected: dropped(FilteredReason::ContainNsfwMedia, "TweetNsfwAdminDropRule"),
        },
    ]
}

fn oon_tweet_label_cases() -> Vec<Case> {
    vec![
        Case {
            name: "nsfw_high_recall_label_drops_oon",
            level: TimelineHomeRecommendations,
            viewer: viewer(VIEWER_ID),
            candidate: labeled(SafetyLabelType::NSFW_HIGH_RECALL),
            expected: dropped(FilteredReason::ContainNsfwMedia, "NsfwHighRecallDropRule"),
        },
        Case {
            name: "nsfw_high_precision_label_drop_beats_interstitial_oon",
            level: TimelineHomeRecommendations,
            viewer: viewer(VIEWER_ID),
            candidate: labeled(SafetyLabelType::NSFW_HIGH_PRECISION),
            expected: dropped(
                FilteredReason::ContainNsfwMedia,
                "NsfwHighPrecisionOonDropRule",
            ),
        },
        Case {
            name: "gore_and_violence_label_drop_beats_interstitial_oon",
            level: TimelineHomeRecommendations,
            viewer: viewer(VIEWER_ID),
            candidate: labeled(SafetyLabelType::GORE_AND_VIOLENCE_HIGH_PRECISION),
            expected: dropped(
                FilteredReason::ContainNsfwMedia,
                "GoreAndViolenceOonDropRule",
            ),
        },
        Case {
            name: "nsfw_card_image_label_drop_beats_interstitial_oon",
            level: TimelineHomeRecommendations,
            viewer: viewer(VIEWER_ID),
            candidate: labeled(SafetyLabelType::NSFW_CARD_IMAGE),
            expected: dropped(FilteredReason::ContainNsfwMedia, "NsfwCardImageOonDropRule"),
        },
        Case {
            name: "sensitive_opt_in_does_not_save_oon_drop",
            level: TimelineHomeRecommendations,
            viewer: sensitive_opt_in_viewer(),
            candidate: labeled(SafetyLabelType::NSFW_HIGH_PRECISION),
            expected: dropped(
                FilteredReason::ContainNsfwMedia,
                "NsfwHighPrecisionOonDropRule",
            ),
        },
        Case {
            name: "do_not_amplify_label_drops_oon",
            level: TimelineHomeRecommendations,
            viewer: viewer(VIEWER_ID),
            candidate: labeled(SafetyLabelType::DO_NOT_AMPLIFY),
            expected: dropped(
                FilteredReason::PossiblyUndesirable,
                "DoNotAmplifyOonDropRule",
            ),
        },
        Case {
            name: "malicious_url_label_drops_oon",
            level: TimelineHomeRecommendations,
            viewer: viewer(VIEWER_ID),
            candidate: labeled(SafetyLabelType::MALICIOUS_URL),
            expected: dropped(
                FilteredReason::PossiblyUndesirable,
                "MaliciousUrlOonDropRule",
            ),
        },
        Case {
            name: "malicious_url_label_allows_in_network",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: labeled(SafetyLabelType::MALICIOUS_URL),
            expected: allow(),
        },
        Case {
            name: "malicious_url_label_allows_self_view_oon",
            level: TimelineHomeRecommendations,
            viewer: author_viewer(),
            candidate: labeled(SafetyLabelType::MALICIOUS_URL),
            expected: allow(),
        },
        Case {
            name: "spam_high_recall_label_drops_oon",
            level: TimelineHomeRecommendations,
            viewer: viewer(VIEWER_ID),
            candidate: labeled(SafetyLabelType::SPAM_HIGH_RECALL),
            expected: dropped(
                FilteredReason::PossiblyUndesirable,
                "SpamHighRecallDropRule",
            ),
        },
        Case {
            name: "spam_high_recall_label_allows_in_network",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: labeled(SafetyLabelType::SPAM_HIGH_RECALL),
            expected: allow(),
        },
        Case {
            name: "nsfw_text_label_drops_oon",
            level: TimelineHomeRecommendations,
            viewer: viewer(VIEWER_ID),
            candidate: labeled(SafetyLabelType::NSFW_TEXT),
            expected: dropped(nsfw_high_precision_reason(), "NsfwTextTweetLabelDropRule"),
        },
        Case {
            name: "fosnr_abuse_insults_label_drops_oon",
            level: TimelineHomeRecommendations,
            viewer: viewer(VIEWER_ID),
            candidate: labeled(SafetyLabelType::FOSNR_ABUSE_INSULTS),
            expected: dropped(
                FilteredReason::PossiblyUndesirable,
                "FosnrAbuseInsultsOonDropRule",
            ),
        },
        Case {
            name: "fosnr_abuse_insults_label_allows_in_network",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: labeled(SafetyLabelType::FOSNR_ABUSE_INSULTS),
            expected: allow(),
        },
    ]
}

fn oon_user_label_cases() -> Vec<Case> {
    vec![
        Case {
            name: "nsfw_high_recall_user_label_drops_oon",
            level: TimelineHomeRecommendations,
            viewer: viewer(VIEWER_ID),
            candidate: user_labeled(AuthorLabel::NsfwHighRecall),
            expected: dropped(
                FilteredReason::UnspecifiedReason,
                "NsfwHighRecallUserLabelRule",
            ),
        },
        Case {
            name: "nsfw_high_recall_user_label_allows_in_network",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: user_labeled(AuthorLabel::NsfwHighRecall),
            expected: allow(),
        },
        Case {
            name: "nsfw_high_recall_user_label_allows_self_view_oon",
            level: TimelineHomeRecommendations,
            viewer: author_viewer(),
            candidate: user_labeled(AuthorLabel::NsfwHighRecall),
            expected: allow(),
        },
        Case {
            name: "nsfw_high_precision_user_label_drops_oon",
            level: TimelineHomeRecommendations,
            viewer: viewer(VIEWER_ID),
            candidate: user_labeled(AuthorLabel::NsfwHighPrecision),
            expected: dropped(
                FilteredReason::UnspecifiedReason,
                "NsfwHighPrecisionUserLabelRule",
            ),
        },
        Case {
            name: "spam_high_recall_user_label_drops_oon",
            level: TimelineHomeRecommendations,
            viewer: viewer(VIEWER_ID),
            candidate: user_labeled(AuthorLabel::SpamHighRecall),
            expected: dropped(
                FilteredReason::UnspecifiedReason,
                "SpamHighRecallUserLabelRule",
            ),
        },
        Case {
            name: "compromised_user_label_drops_oon",
            level: TimelineHomeRecommendations,
            viewer: viewer(VIEWER_ID),
            candidate: user_labeled(AuthorLabel::Compromised),
            expected: dropped(
                FilteredReason::UnspecifiedReason,
                "CompromisedUserLabelRule",
            ),
        },
        Case {
            name: "read_only_user_label_drops_oon",
            level: TimelineHomeRecommendations,
            viewer: viewer(VIEWER_ID),
            candidate: user_labeled(AuthorLabel::ReadOnly),
            expected: dropped(FilteredReason::UnspecifiedReason, "ReadOnlyUserLabelRule"),
        },
        Case {
            name: "impersonation_user_label_drops_oon",
            level: TimelineHomeRecommendations,
            viewer: viewer(VIEWER_ID),
            candidate: user_labeled(AuthorLabel::ImpersonationHighPrecision),
            expected: dropped(
                FilteredReason::UnspecifiedReason,
                "ImpersonationHighPrecisionUserLabelRule",
            ),
        },
        Case {
            name: "nsfw_avatar_user_label_drops_oon",
            level: TimelineHomeRecommendations,
            viewer: viewer(VIEWER_ID),
            candidate: user_labeled(AuthorLabel::NsfwAvatarImage),
            expected: dropped(FilteredReason::UnspecifiedReason, "NsfwAvatarImageRule"),
        },
        Case {
            name: "nsfw_banner_user_label_drops_oon",
            level: TimelineHomeRecommendations,
            viewer: viewer(VIEWER_ID),
            candidate: user_labeled(AuthorLabel::NsfwBannerImage),
            expected: dropped(FilteredReason::UnspecifiedReason, "NsfwBannerImageRule"),
        },
        Case {
            name: "abusive_high_recall_user_label_drops_non_follower_oon",
            level: TimelineHomeRecommendations,
            viewer: viewer(VIEWER_ID),
            candidate: user_labeled(AuthorLabel::AbusiveHighRecall),
            expected: dropped(FilteredReason::UnspecifiedReason, "AbusiveHighRecallRule"),
        },
        Case {
            name: "abusive_high_recall_user_label_allows_follower_oon",
            level: TimelineHomeRecommendations,
            viewer: viewer(VIEWER_ID),
            candidate: user_labeled_follower(AuthorLabel::AbusiveHighRecall),
            expected: allow(),
        },
        Case {
            name: "nsfw_near_perfect_user_label_drops_oon",
            level: TimelineHomeRecommendations,
            viewer: viewer(VIEWER_ID),
            candidate: user_labeled(AuthorLabel::NsfwNearPerfect),
            expected: dropped(
                FilteredReason::UnspecifiedReason,
                "NsfwNearPerfectAuthorRule",
            ),
        },
        Case {
            name: "nsfw_near_perfect_user_label_allows_in_network",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: user_labeled(AuthorLabel::NsfwNearPerfect),
            expected: allow(),
        },
        Case {
            name: "do_not_amplify_user_label_drops_non_follower_oon",
            level: TimelineHomeRecommendations,
            viewer: viewer(VIEWER_ID),
            candidate: user_labeled(AuthorLabel::DoNotAmplify),
            expected: dropped(
                FilteredReason::UnspecifiedReason,
                "DoNotAmplifyNonFollowerRule",
            ),
        },
        Case {
            name: "do_not_amplify_user_label_allows_follower_oon",
            level: TimelineHomeRecommendations,
            viewer: viewer(VIEWER_ID),
            candidate: user_labeled_follower(AuthorLabel::DoNotAmplify),
            expected: allow(),
        },
    ]
}

fn interaction_cases() -> Vec<Case> {
    vec![
        Case {
            name: "drop_short_circuits_before_interstitial_attribution",
            level: TimelineHome,
            viewer: viewer(VIEWER_ID),
            candidate: {
                let features = AuthorFeatures {
                    is_suspended: true,
                    ..Default::default()
                };
                candidate()
                    .with_author_features(features)
                    .with_label(SafetyLabelType::NSFW_HIGH_PRECISION)
                    .with_media()
                    .build()
            },
            expected: dropped(FilteredReason::AuthorIsSuspended, "SuspendedAuthorRule"),
        },
        Case {
            name: "later_oon_drop_beats_earlier_nsfw_author_interstitial",
            level: TimelineHomeRecommendations,
            viewer: viewer(VIEWER_ID),
            candidate: {
                let features = AuthorFeatures {
                    is_nsfw_user: true,
                    ..Default::default()
                };
                candidate()
                    .with_author_features(features)
                    .with_media()
                    .build()
            },
            expected: dropped(FilteredReason::ContainNsfwMedia, "DropNsfwUserAuthorRule"),
        },
    ]
}
