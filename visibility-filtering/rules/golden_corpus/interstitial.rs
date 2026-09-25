use super::builders::{labeled, no_stated_age_viewer, tweet_candidate, viewer_with_age};
use super::{Role, Row};
use crate::models::{AuthorFeatures, SafetyLabelType, ViewerAge};
use crate::rules::fixtures::{allow, blurred, candidate, dropped, sensitive_opt_in_viewer};
use crate::rules::SafetyLevel::{TimelineHome, TimelineHomeRecommendations};
use xai_visibility_filtering::models::FilteredReason;
use xai_x_thrift::action::InterstitialReason;

const AT_CUTOFF: u64 = (1705536000000 - 1288834974657) << 22;

pub(super) fn rows() -> Vec<Row> {
    vec![
        Row {
            name: "nsfw_high_precision_label_at_cutoff",
            post: candidate()
                .tweet_id(AT_CUTOFF)
                .with_label(SafetyLabelType::NSFW_HIGH_PRECISION)
                .build(),
            expect: vec![(
                TimelineHome,
                Role::NonFollower,
                blurred(
                    InterstitialReason::Sensitive(true),
                    "NsfwHighPrecisionInterstitialRule",
                ),
            )],
        },
        Row {
            name: "nsfw_admin_and_user_author_media",
            post: candidate()
                .with_author_features(AuthorFeatures {
                    is_nsfw_user: true,
                    is_nsfw_admin: true,
                    ..Default::default()
                })
                .with_media()
                .build(),
            expect: vec![(
                TimelineHome,
                Role::NonFollower,
                blurred(
                    InterstitialReason::Sensitive(true),
                    "NsfwAdminInterstitialRule",
                ),
            )],
        },
        Row {
            name: "gore_and_nsfw_card_image_labels",
            post: candidate()
                .with_label(SafetyLabelType::GORE_AND_VIOLENCE_HIGH_PRECISION)
                .with_label(SafetyLabelType::NSFW_CARD_IMAGE)
                .build(),
            expect: vec![(
                TimelineHome,
                Role::NonFollower,
                blurred(
                    InterstitialReason::Violence(true),
                    "GoreAndViolenceInterstitialRule",
                ),
            )],
        },
        Row {
            name: "nsfw_high_precision_adult_label",
            post: candidate()
                .tweet_id(AT_CUTOFF + (1 << 22))
                .with_label(SafetyLabelType::NSFW_HIGH_PRECISION)
                .build(),
            expect: vec![
                (
                    TimelineHome,
                    Role::NonFollower,
                    blurred(
                        InterstitialReason::Nudity(true),
                        "NsfwHighPrecisionAdultInterstitialRule",
                    ),
                ),
                (
                    TimelineHome,
                    Role::As("sensitive_opt_in", sensitive_opt_in_viewer()),
                    allow(),
                ),
                (TimelineHome, Role::Author, allow()),
            ],
        },
        Row {
            name: "nsfw_high_precision_label",
            post: labeled(SafetyLabelType::NSFW_HIGH_PRECISION),
            expect: vec![
                (TimelineHome, Role::Author, allow()),
                (
                    TimelineHome,
                    Role::As("sensitive_opt_in", sensitive_opt_in_viewer()),
                    allow(),
                ),
                (
                    TimelineHomeRecommendations,
                    Role::As("sensitive_opt_in", sensitive_opt_in_viewer()),
                    dropped(
                        FilteredReason::ContainNsfwMedia,
                        "NsfwHighPrecisionOonDropRule",
                    ),
                ),
                (
                    TimelineHome,
                    Role::As("underage", viewer_with_age(ViewerAge::Known(17))),
                    blurred(
                        InterstitialReason::Sensitive(true),
                        "NsfwHighPrecisionInterstitialRule",
                    ),
                ),
                (
                    TimelineHome,
                    Role::LoggedOut,
                    blurred(
                        InterstitialReason::Sensitive(true),
                        "NsfwHighPrecisionInterstitialRule",
                    ),
                ),
                (
                    TimelineHomeRecommendations,
                    Role::NonFollower,
                    dropped(
                        FilteredReason::ContainNsfwMedia,
                        "NsfwHighPrecisionOonDropRule",
                    ),
                ),
                (
                    TimelineHomeRecommendations,
                    Role::Follower,
                    dropped(
                        FilteredReason::ContainNsfwMedia,
                        "NsfwHighPrecisionOonDropRule",
                    ),
                ),
                (TimelineHomeRecommendations, Role::Author, allow()),
            ],
        },
        Row {
            name: "gore_and_violence_label",
            post: labeled(SafetyLabelType::GORE_AND_VIOLENCE_HIGH_PRECISION),
            expect: vec![
                (
                    TimelineHome,
                    Role::NonFollower,
                    blurred(
                        InterstitialReason::Violence(true),
                        "GoreAndViolenceInterstitialRule",
                    ),
                ),
                (
                    TimelineHome,
                    Role::As("sensitive_opt_in", sensitive_opt_in_viewer()),
                    allow(),
                ),
                (TimelineHome, Role::Author, allow()),
                (
                    TimelineHomeRecommendations,
                    Role::NonFollower,
                    dropped(
                        FilteredReason::ContainNsfwMedia,
                        "GoreAndViolenceOonDropRule",
                    ),
                ),
                (
                    TimelineHomeRecommendations,
                    Role::Follower,
                    dropped(
                        FilteredReason::ContainNsfwMedia,
                        "GoreAndViolenceOonDropRule",
                    ),
                ),
                (TimelineHomeRecommendations, Role::Author, allow()),
            ],
        },
        Row {
            name: "nsfw_card_image_label",
            post: labeled(SafetyLabelType::NSFW_CARD_IMAGE),
            expect: vec![
                (
                    TimelineHome,
                    Role::NonFollower,
                    blurred(
                        InterstitialReason::Sensitive(true),
                        "NsfwCardImageInterstitialRule",
                    ),
                ),
                (
                    TimelineHome,
                    Role::As("sensitive_opt_in", sensitive_opt_in_viewer()),
                    allow(),
                ),
                (TimelineHome, Role::Author, allow()),
                (
                    TimelineHome,
                    Role::LoggedOut,
                    dropped(
                        FilteredReason::ContainNsfwMedia,
                        "SensitiveViewerLoggedOutDropRule",
                    ),
                ),
                (
                    TimelineHome,
                    Role::As("no_stated_age_in_gb", no_stated_age_viewer("gb")),
                    dropped(
                        FilteredReason::ContainNsfwMedia,
                        "SensitiveViewerNoStatedAgeDropRule",
                    ),
                ),
                (
                    TimelineHomeRecommendations,
                    Role::NonFollower,
                    dropped(FilteredReason::ContainNsfwMedia, "NsfwCardImageOonDropRule"),
                ),
                (
                    TimelineHomeRecommendations,
                    Role::Follower,
                    dropped(FilteredReason::ContainNsfwMedia, "NsfwCardImageOonDropRule"),
                ),
                (TimelineHomeRecommendations, Role::Author, allow()),
            ],
        },
        Row {
            name: "nsfw_admin_author_media",
            post: candidate()
                .with_author_features(AuthorFeatures {
                    is_nsfw_admin: true,
                    ..Default::default()
                })
                .with_media()
                .build(),
            expect: vec![
                (
                    TimelineHome,
                    Role::NonFollower,
                    blurred(
                        InterstitialReason::Sensitive(true),
                        "NsfwAdminInterstitialRule",
                    ),
                ),
                (
                    TimelineHome,
                    Role::As("underage", viewer_with_age(ViewerAge::Known(17))),
                    dropped(
                        FilteredReason::ContainNsfwMedia,
                        "SensitiveViewerUnderageDropRule",
                    ),
                ),
            ],
        },
        Row {
            name: "nsfw_user_author_media",
            post: candidate()
                .with_author_features(AuthorFeatures {
                    is_nsfw_user: true,
                    ..Default::default()
                })
                .with_media()
                .build(),
            expect: vec![
                (
                    TimelineHome,
                    Role::NonFollower,
                    blurred(
                        InterstitialReason::SensitiveUser(true),
                        "NsfwUserInterstitialRule",
                    ),
                ),
                (
                    TimelineHome,
                    Role::LoggedOut,
                    dropped(
                        FilteredReason::ContainNsfwMedia,
                        "SensitiveViewerLoggedOutDropRule",
                    ),
                ),
                (
                    TimelineHomeRecommendations,
                    Role::NonFollower,
                    dropped(FilteredReason::ContainNsfwMedia, "DropNsfwUserAuthorRule"),
                ),
            ],
        },
        Row {
            name: "nsfw_admin_flag_media",
            post: tweet_candidate(|t| {
                t.nsfw.admin = true;
                t.media.has_media = true;
            }),
            expect: vec![
                (
                    TimelineHome,
                    Role::NonFollower,
                    blurred(
                        InterstitialReason::Sensitive(true),
                        "NsfwAdminInterstitialRule",
                    ),
                ),
                (
                    TimelineHome,
                    Role::LoggedOut,
                    dropped(
                        FilteredReason::ContainNsfwMedia,
                        "SensitiveViewerLoggedOutDropRule",
                    ),
                ),
            ],
        },
        Row {
            name: "nsfw_user_flag_media",
            post: tweet_candidate(|t| {
                t.nsfw.user = true;
                t.media.has_media = true;
            }),
            expect: vec![
                (
                    TimelineHome,
                    Role::NonFollower,
                    blurred(
                        InterstitialReason::SensitiveUser(true),
                        "NsfwUserInterstitialRule",
                    ),
                ),
                (
                    TimelineHome,
                    Role::As("no_stated_age_in_gb", no_stated_age_viewer("gb")),
                    dropped(
                        FilteredReason::ContainNsfwMedia,
                        "SensitiveViewerNoStatedAgeDropRule",
                    ),
                ),
            ],
        },
    ]
}
