use super::builders::{
    labeled, no_stated_age_viewer, tweet_candidate, viewer_in_country, viewer_with_age,
};
use super::{Role, Row};
use crate::models::{
    HydratedTweetCandidate, SafetyLabelType, Viewer, ViewerAge, ViewerFeatures, ViewerProfile,
};
use crate::rules::fixtures::{
    allow, blurred, candidate, dropped, sensitive_opt_in_viewer, viewer_with_profile, AUTHOR_ID,
};
use crate::rules::SafetyLevel::{
    ImmersiveExpandedRecommendations, TimelineHome, TimelineHomeHydration,
    TimelineHomeRecommendations,
};
use xai_visibility_filtering::models::FilteredReason;
use xai_x_thrift::action::InterstitialReason;

pub(super) fn rows() -> Vec<Row> {
    vec![
        Row {
            name: "unflagged_media",
            post: candidate().with_media().build(),
            expect: vec![
                (
                    TimelineHome,
                    Role::As("underage", viewer_with_age(ViewerAge::Known(17))),
                    allow(),
                ),
                (TimelineHome, Role::LoggedOut, allow()),
                (
                    TimelineHome,
                    Role::As("no_stated_age_in_gb", no_stated_age_viewer("gb")),
                    allow(),
                ),
            ],
        },
        Row {
            name: "nsfw_high_recall_media",
            post: labeled_media(SafetyLabelType::NSFW_HIGH_RECALL),
            expect: vec![
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
                    Role::As("underage", viewer_with_age(ViewerAge::Known(17))),
                    dropped(
                        FilteredReason::ContainNsfwMedia,
                        "SensitiveViewerUnderageDropRule",
                    ),
                ),
                (
                    TimelineHome,
                    Role::As("no_stated_age_in_us", no_stated_age_viewer("us")),
                    allow(),
                ),
                (
                    TimelineHome,
                    Role::As("adult", viewer_with_age(ViewerAge::Known(30))),
                    allow(),
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
                    TimelineHome,
                    Role::As("unknown_age_in_gb", viewer_in_country("gb")),
                    allow(),
                ),
                (
                    TimelineHome,
                    Role::As(
                        "underage_opted_in",
                        viewer_with_profile(ViewerProfile {
                            viewer_age: ViewerAge::Known(17),
                            allows_sensitive_media: true,
                            ..ViewerProfile::default()
                        }),
                    ),
                    dropped(
                        FilteredReason::ContainNsfwMedia,
                        "SensitiveViewerUnderageDropRule",
                    ),
                ),
                (
                    TimelineHome,
                    Role::As(
                        "underage_author",
                        ViewerFeatures {
                            viewer: Viewer::LoggedIn {
                                id: AUTHOR_ID,
                                profile: ViewerProfile {
                                    viewer_age: ViewerAge::Known(17),
                                    ..ViewerProfile::default()
                                },
                            },
                            ..ViewerFeatures::default()
                        },
                    ),
                    allow(),
                ),
                (
                    TimelineHome,
                    Role::As(
                        "no_stated_age_without_country",
                        viewer_with_age(ViewerAge::NotStated),
                    ),
                    allow(),
                ),
                (
                    TimelineHome,
                    Role::As(
                        "no_stated_age_in_de",
                        ViewerFeatures {
                            country_code: Some("de".to_string()),
                            ..viewer_with_age(ViewerAge::NotStated)
                        },
                    ),
                    dropped(
                        FilteredReason::ContainNsfwMedia,
                        "SensitiveViewerNoStatedAgeDropRule",
                    ),
                ),
                (
                    TimelineHome,
                    Role::As(
                        "no_stated_age_us_account_in_de",
                        ViewerFeatures {
                            country_code: Some("de".to_string()),
                            ..no_stated_age_viewer("us")
                        },
                    ),
                    allow(),
                ),
                (
                    TimelineHome,
                    Role::As(
                        "no_stated_age_kr_account_in_us",
                        ViewerFeatures {
                            country_code: Some("us".to_string()),
                            ..no_stated_age_viewer("kr")
                        },
                    ),
                    dropped(
                        FilteredReason::ContainNsfwMedia,
                        "SensitiveViewerNoStatedAgeDropRule",
                    ),
                ),
            ],
        },
        Row {
            name: "nsfw_high_precision_media",
            post: labeled_media(SafetyLabelType::NSFW_HIGH_PRECISION),
            expect: vec![
                (
                    TimelineHome,
                    Role::As("underage", viewer_with_age(ViewerAge::Known(17))),
                    dropped(
                        FilteredReason::ContainNsfwMedia,
                        "SensitiveViewerUnderageDropRule",
                    ),
                ),
                (
                    TimelineHomeHydration,
                    Role::NonFollower,
                    blurred(
                        InterstitialReason::Sensitive(true),
                        "NsfwHighPrecisionInterstitialRule",
                    ),
                ),
                (
                    ImmersiveExpandedRecommendations,
                    Role::As("sensitive_opt_in", sensitive_opt_in_viewer()),
                    allow(),
                ),
                (
                    ImmersiveExpandedRecommendations,
                    Role::NonFollower,
                    dropped(
                        FilteredReason::ContainNsfwMedia,
                        "NsfwSensitiveViewerDropTweetRule",
                    ),
                ),
                (ImmersiveExpandedRecommendations, Role::Author, allow()),
                (
                    ImmersiveExpandedRecommendations,
                    Role::LoggedOut,
                    dropped(
                        FilteredReason::ContainNsfwMedia,
                        "SensitiveViewerLoggedOutDropRule",
                    ),
                ),
            ],
        },
        Row {
            name: "nsfw_text_label",
            post: labeled(SafetyLabelType::NSFW_TEXT),
            expect: vec![
                (
                    TimelineHomeRecommendations,
                    Role::As("underage", viewer_with_age(ViewerAge::Known(17))),
                    dropped(
                        FilteredReason::ContainNsfwMedia,
                        "SensitiveViewerUnderageDropRule",
                    ),
                ),
                (
                    TimelineHomeRecommendations,
                    Role::As("adult", viewer_with_age(ViewerAge::Known(30))),
                    allow(),
                ),
            ],
        },
        Row {
            name: "gore_and_violence_media",
            post: labeled_media(SafetyLabelType::GORE_AND_VIOLENCE_HIGH_PRECISION),
            expect: vec![
                (
                    TimelineHome,
                    Role::As("no_stated_age_in_gb", no_stated_age_viewer("gb")),
                    dropped(
                        FilteredReason::ContainNsfwMedia,
                        "SensitiveViewerNoStatedAgeDropRule",
                    ),
                ),
                (
                    ImmersiveExpandedRecommendations,
                    Role::NonFollower,
                    dropped(
                        FilteredReason::ContainNsfwMedia,
                        "GoreAndViolenceOonDropRule",
                    ),
                ),
            ],
        },
        Row {
            name: "nsfw_user_flag_media_retweet",
            post: tweet_candidate(|t| {
                t.nsfw.user = true;
                t.media.has_media = true;
                t.source_tweet_id = Some(2);
            }),
            expect: vec![(
                TimelineHome,
                Role::As("underage", viewer_with_age(ViewerAge::Known(17))),
                blurred(
                    InterstitialReason::SensitiveUser(true),
                    "NsfwUserInterstitialRule",
                ),
            )],
        },
    ]
}

fn labeled_media(label: SafetyLabelType) -> HydratedTweetCandidate {
    candidate().with_label(label).with_media().build()
}
