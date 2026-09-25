use super::builders::labeled;
use super::{Role, Row};
use crate::models::SafetyLabelType;
use crate::rules::fixtures::{allow, dropped};
use crate::rules::SafetyLevel::{
    ImmersiveExpandedRecommendations, TimelineHome, TimelineHomeRecommendations,
};
use xai_visibility_filtering::models::FilteredReason;

pub(super) fn rows() -> Vec<Row> {
    vec![
        Row {
            name: "malicious_url_label",
            post: labeled(SafetyLabelType::MALICIOUS_URL),
            expect: vec![
                (
                    TimelineHomeRecommendations,
                    Role::NonFollower,
                    dropped(
                        FilteredReason::PossiblyUndesirable,
                        "MaliciousUrlOonDropRule",
                    ),
                ),
                (
                    TimelineHomeRecommendations,
                    Role::Follower,
                    dropped(
                        FilteredReason::PossiblyUndesirable,
                        "MaliciousUrlOonDropRule",
                    ),
                ),
                (TimelineHomeRecommendations, Role::Author, allow()),
                (TimelineHome, Role::NonFollower, allow()),
            ],
        },
        Row {
            name: "nsfw_high_recall_label",
            post: labeled(SafetyLabelType::NSFW_HIGH_RECALL),
            expect: vec![
                (
                    TimelineHomeRecommendations,
                    Role::NonFollower,
                    dropped(FilteredReason::ContainNsfwMedia, "NsfwHighRecallDropRule"),
                ),
                (
                    TimelineHomeRecommendations,
                    Role::Follower,
                    dropped(FilteredReason::ContainNsfwMedia, "NsfwHighRecallDropRule"),
                ),
                (TimelineHomeRecommendations, Role::Author, allow()),
            ],
        },
        Row {
            name: "do_not_amplify_label",
            post: labeled(SafetyLabelType::DO_NOT_AMPLIFY),
            expect: vec![
                (
                    TimelineHomeRecommendations,
                    Role::NonFollower,
                    dropped(
                        FilteredReason::PossiblyUndesirable,
                        "DoNotAmplifyOonDropRule",
                    ),
                ),
                (
                    TimelineHomeRecommendations,
                    Role::Follower,
                    dropped(
                        FilteredReason::PossiblyUndesirable,
                        "DoNotAmplifyOonDropRule",
                    ),
                ),
                (TimelineHomeRecommendations, Role::Author, allow()),
            ],
        },
        Row {
            name: "spam_high_recall_label",
            post: labeled(SafetyLabelType::SPAM_HIGH_RECALL),
            expect: vec![
                (
                    TimelineHomeRecommendations,
                    Role::NonFollower,
                    dropped(
                        FilteredReason::PossiblyUndesirable,
                        "SpamHighRecallDropRule",
                    ),
                ),
                (
                    TimelineHomeRecommendations,
                    Role::Follower,
                    dropped(
                        FilteredReason::PossiblyUndesirable,
                        "SpamHighRecallDropRule",
                    ),
                ),
                (TimelineHomeRecommendations, Role::Author, allow()),
                (TimelineHome, Role::NonFollower, allow()),
                (
                    ImmersiveExpandedRecommendations,
                    Role::NonFollower,
                    dropped(
                        FilteredReason::PossiblyUndesirable,
                        "SpamHighRecallDropRule",
                    ),
                ),
            ],
        },
        Row {
            name: "fosnr_abuse_insults_label",
            post: labeled(SafetyLabelType::FOSNR_ABUSE_INSULTS),
            expect: vec![
                (
                    TimelineHomeRecommendations,
                    Role::NonFollower,
                    dropped(
                        FilteredReason::PossiblyUndesirable,
                        "FosnrAbuseInsultsOonDropRule",
                    ),
                ),
                (
                    TimelineHomeRecommendations,
                    Role::Follower,
                    dropped(
                        FilteredReason::PossiblyUndesirable,
                        "FosnrAbuseInsultsOonDropRule",
                    ),
                ),
                (TimelineHomeRecommendations, Role::Author, allow()),
                (TimelineHome, Role::NonFollower, allow()),
            ],
        },
    ]
}
