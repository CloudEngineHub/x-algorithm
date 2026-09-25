use super::builders::labeled;
use super::{Role, Row};
use crate::models::SafetyLabelType;
use crate::rules::fixtures::{allow, dropped};
use crate::rules::SafetyLevel::{TimelineHome, TimelineHomeHydration};
use xai_visibility_filtering::models::{
    Action, DropReason, FilteredReason, SafetyResult, SafetyResultReason,
};

pub(super) fn rows() -> Vec<Row> {
    vec![
        Row {
            name: "for_emergency_use_only_label",
            post: labeled(SafetyLabelType::FOR_EMERGENCY_USE_ONLY),
            expect: vec![
                (
                    TimelineHome,
                    Role::NonFollower,
                    dropped(
                        FilteredReason::UnspecifiedReason,
                        "ForEmergencyUseOnlyDropRule",
                    ),
                ),
                (
                    TimelineHome,
                    Role::Follower,
                    dropped(
                        FilteredReason::UnspecifiedReason,
                        "ForEmergencyUseOnlyDropRule",
                    ),
                ),
                (
                    TimelineHome,
                    Role::Author,
                    dropped(
                        FilteredReason::UnspecifiedReason,
                        "ForEmergencyUseOnlyDropRule",
                    ),
                ),
                (
                    TimelineHomeHydration,
                    Role::Author,
                    dropped(
                        FilteredReason::UnspecifiedReason,
                        "ForEmergencyUseOnlyDropRule",
                    ),
                ),
            ],
        },
        Row {
            name: "pdna_label",
            post: labeled(SafetyLabelType::PDNA),
            expect: vec![
                (
                    TimelineHome,
                    Role::NonFollower,
                    dropped(nsfw_high_precision_reason(), "PdnaTweetLabelRule"),
                ),
                (
                    TimelineHome,
                    Role::Follower,
                    dropped(nsfw_high_precision_reason(), "PdnaTweetLabelRule"),
                ),
                (TimelineHome, Role::Author, allow()),
            ],
        },
        Row {
            name: "bounce_label",
            post: labeled(SafetyLabelType::BOUNCE),
            expect: vec![
                (
                    TimelineHome,
                    Role::NonFollower,
                    dropped(FilteredReason::TweetIsBounced, "BounceTweetLabelRule"),
                ),
                (
                    TimelineHome,
                    Role::Follower,
                    dropped(FilteredReason::TweetIsBounced, "BounceTweetLabelRule"),
                ),
                (TimelineHome, Role::Author, allow()),
            ],
        },
        Row {
            name: "spam_label",
            post: labeled(SafetyLabelType::SPAM),
            expect: vec![
                (
                    TimelineHome,
                    Role::NonFollower,
                    dropped(FilteredReason::PossiblyUndesirable, "SpamTweetLabelRule"),
                ),
                (
                    TimelineHome,
                    Role::Follower,
                    dropped(FilteredReason::PossiblyUndesirable, "SpamTweetLabelRule"),
                ),
                (TimelineHome, Role::Author, allow()),
            ],
        },
        Row {
            name: "fosnr_hateful_conduct_label",
            post: labeled(SafetyLabelType::FOSNR_HATEFUL_CONDUCT),
            expect: vec![
                (
                    TimelineHome,
                    Role::NonFollower,
                    dropped(
                        FilteredReason::PossiblyUndesirable,
                        "FosnrHatefulConductDropRule",
                    ),
                ),
                (
                    TimelineHome,
                    Role::Follower,
                    dropped(
                        FilteredReason::PossiblyUndesirable,
                        "FosnrHatefulConductDropRule",
                    ),
                ),
                (TimelineHome, Role::Author, allow()),
            ],
        },
        Row {
            name: "fosnr_violent_speech_label",
            post: labeled(SafetyLabelType::FOSNR_VIOLENT_SPEECH),
            expect: vec![
                (
                    TimelineHome,
                    Role::NonFollower,
                    dropped(
                        FilteredReason::PossiblyUndesirable,
                        "FosnrViolentSpeechDropRule",
                    ),
                ),
                (
                    TimelineHome,
                    Role::Follower,
                    dropped(
                        FilteredReason::PossiblyUndesirable,
                        "FosnrViolentSpeechDropRule",
                    ),
                ),
                (TimelineHome, Role::Author, allow()),
            ],
        },
        Row {
            name: "fosnr_abuse_label",
            post: labeled(SafetyLabelType::FOSNR_ABUSE),
            expect: vec![
                (
                    TimelineHome,
                    Role::NonFollower,
                    dropped(FilteredReason::PossiblyUndesirable, "FosnrAbuseDropRule"),
                ),
                (
                    TimelineHome,
                    Role::Follower,
                    dropped(FilteredReason::PossiblyUndesirable, "FosnrAbuseDropRule"),
                ),
                (TimelineHome, Role::Author, allow()),
            ],
        },
        Row {
            name: "fosnr_civic_integrity_label",
            post: labeled(SafetyLabelType::FOSNR_CIVIC_INTEGRITY),
            expect: vec![
                (
                    TimelineHome,
                    Role::NonFollower,
                    dropped(
                        FilteredReason::PossiblyUndesirable,
                        "FosnrCivicIntegrityDropRule",
                    ),
                ),
                (
                    TimelineHome,
                    Role::Follower,
                    dropped(
                        FilteredReason::PossiblyUndesirable,
                        "FosnrCivicIntegrityDropRule",
                    ),
                ),
                (TimelineHome, Role::Author, allow()),
            ],
        },
    ]
}

fn nsfw_high_precision_reason() -> FilteredReason {
    FilteredReason::SafetyResult(SafetyResult {
        reason: Some(SafetyResultReason::NsfwHighPrecision),
        action: Action::Drop(DropReason {}),
    })
}
