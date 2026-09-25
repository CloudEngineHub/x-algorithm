use super::builders::{author_candidate, tweet_candidate, viewer_in_country, viewer_with_age};
use super::{Role, Row};
use crate::models::{ViewerAge, ViewerFeatures};
use crate::rules::fixtures::{allow, author_viewer, dropped};
use crate::rules::SafetyLevel::{
    ImmersiveExpandedRecommendations, TimelineHome, TimelineHomeRecommendations,
};
use xai_visibility_filtering::models::FilteredReason;

pub(super) fn rows() -> Vec<Row> {
    vec![
        Row {
            name: "dmca_media",
            post: tweet_candidate(|t| t.media.has_dmca_media = true),
            expect: vec![
                (
                    TimelineHomeRecommendations,
                    Role::NonFollower,
                    dropped(
                        FilteredReason::UnspecifiedReason,
                        "DropTweetsWithDmcaMediaRule",
                    ),
                ),
                (TimelineHome, Role::NonFollower, allow()),
            ],
        },
        Row {
            name: "nsfw_user_flag",
            post: tweet_candidate(|t| t.nsfw.user = true),
            expect: vec![
                (
                    TimelineHomeRecommendations,
                    Role::NonFollower,
                    dropped(FilteredReason::ContainNsfwMedia, "TweetNsfwUserDropRule"),
                ),
                (
                    TimelineHomeRecommendations,
                    Role::Author,
                    dropped(FilteredReason::ContainNsfwMedia, "TweetNsfwUserDropRule"),
                ),
            ],
        },
        Row {
            name: "nsfw_admin_flag",
            post: tweet_candidate(|t| t.nsfw.admin = true),
            expect: vec![
                (
                    TimelineHomeRecommendations,
                    Role::NonFollower,
                    dropped(FilteredReason::ContainNsfwMedia, "TweetNsfwAdminDropRule"),
                ),
                (
                    TimelineHomeRecommendations,
                    Role::Author,
                    dropped(FilteredReason::ContainNsfwMedia, "TweetNsfwAdminDropRule"),
                ),
            ],
        },
        Row {
            name: "nsfw_user_author",
            post: author_candidate(|a| a.is_nsfw_user = true),
            expect: vec![
                (TimelineHome, Role::NonFollower, allow()),
                (
                    TimelineHome,
                    Role::As("underage", viewer_with_age(ViewerAge::Known(17))),
                    allow(),
                ),
                (
                    TimelineHomeRecommendations,
                    Role::NonFollower,
                    dropped(FilteredReason::ContainNsfwMedia, "DropNsfwUserAuthorRule"),
                ),
                (
                    TimelineHomeRecommendations,
                    Role::Follower,
                    dropped(FilteredReason::ContainNsfwMedia, "DropNsfwUserAuthorRule"),
                ),
                (TimelineHomeRecommendations, Role::Author, allow()),
                (
                    ImmersiveExpandedRecommendations,
                    Role::NonFollower,
                    dropped(
                        FilteredReason::ContainNsfwMedia,
                        "NsfwSensitiveViewerDropUserRule",
                    ),
                ),
                (ImmersiveExpandedRecommendations, Role::Author, allow()),
            ],
        },
        Row {
            name: "nsfw_admin_author",
            post: author_candidate(|a| a.is_nsfw_admin = true),
            expect: vec![
                (
                    TimelineHomeRecommendations,
                    Role::NonFollower,
                    dropped(FilteredReason::ContainNsfwMedia, "DropNsfwAdminAuthorRule"),
                ),
                (
                    TimelineHomeRecommendations,
                    Role::Follower,
                    dropped(FilteredReason::ContainNsfwMedia, "DropNsfwAdminAuthorRule"),
                ),
                (TimelineHomeRecommendations, Role::Author, allow()),
            ],
        },
        Row {
            name: "geo_denied_media_de",
            post: tweet_candidate(|t| t.media.geo_deny_list = vec!["de".to_string()]),
            expect: vec![
                (
                    TimelineHomeRecommendations,
                    Role::As("in_de", viewer_in_country("de")),
                    dropped(
                        FilteredReason::UnspecifiedReason,
                        "DropTweetsWithGeoRestrictedMediaRule",
                    ),
                ),
                (
                    TimelineHomeRecommendations,
                    Role::As("in_us", viewer_in_country("us")),
                    allow(),
                ),
                (TimelineHomeRecommendations, Role::NonFollower, allow()),
                (
                    TimelineHomeRecommendations,
                    Role::As(
                        "author_in_de",
                        ViewerFeatures {
                            country_code: Some("de".to_string()),
                            ..author_viewer()
                        },
                    ),
                    dropped(
                        FilteredReason::UnspecifiedReason,
                        "DropTweetsWithGeoRestrictedMediaRule",
                    ),
                ),
            ],
        },
        Row {
            name: "geo_allow_listed_media_us",
            post: tweet_candidate(|t| t.media.geo_allow_list = vec!["us".to_string()]),
            expect: vec![
                (
                    TimelineHomeRecommendations,
                    Role::NonFollower,
                    dropped(
                        FilteredReason::UnspecifiedReason,
                        "DropTweetsWithGeoRestrictedMediaRule",
                    ),
                ),
                (
                    TimelineHomeRecommendations,
                    Role::As("in_us", viewer_in_country("us")),
                    allow(),
                ),
                (
                    TimelineHomeRecommendations,
                    Role::As("in_de", viewer_in_country("de")),
                    dropped(
                        FilteredReason::UnspecifiedReason,
                        "DropTweetsWithGeoRestrictedMediaRule",
                    ),
                ),
            ],
        },
        Row {
            name: "geo_allow_listed_media_uppercase",
            post: tweet_candidate(|t| t.media.geo_allow_list = vec!["US".to_string()]),
            expect: vec![(
                TimelineHomeRecommendations,
                Role::As("in_us", viewer_in_country("us")),
                allow(),
            )],
        },
        Row {
            name: "geo_denied_media_uppercase",
            post: tweet_candidate(|t| t.media.geo_deny_list = vec!["DE".to_string()]),
            expect: vec![(
                TimelineHomeRecommendations,
                Role::As("in_de", viewer_in_country("de")),
                dropped(
                    FilteredReason::UnspecifiedReason,
                    "DropTweetsWithGeoRestrictedMediaRule",
                ),
            )],
        },
        Row {
            name: "geo_denied_media_worldwide",
            post: tweet_candidate(|t| t.media.geo_deny_list = vec!["xx".to_string()]),
            expect: vec![(
                TimelineHomeRecommendations,
                Role::NonFollower,
                dropped(
                    FilteredReason::UnspecifiedReason,
                    "DropTweetsWithGeoRestrictedMediaRule",
                ),
            )],
        },
        Row {
            name: "geo_denied_media_retweet",
            post: tweet_candidate(|t| {
                t.media.geo_deny_list = vec!["de".to_string()];
                t.source_tweet_id = Some(2);
            }),
            expect: vec![(
                TimelineHomeRecommendations,
                Role::As("in_de", viewer_in_country("de")),
                dropped(
                    FilteredReason::UnspecifiedReason,
                    "DropTweetsWithGeoRestrictedMediaRule",
                ),
            )],
        },
    ]
}
