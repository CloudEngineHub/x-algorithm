use super::builders::tweet_candidate;
use super::{Role, Row};
use crate::models::{HydratedTweetCandidate, TweetFeatures};
use crate::rules::fixtures::{allow, candidate, dropped};
use crate::rules::SafetyLevel::{TimelineHome, TimelineHomeHydration};
use xai_core_entities::entities::{EditControl, EditControlInitial};
use xai_visibility_filtering::models::FilteredReason;

pub(super) fn rows() -> Vec<Row> {
    vec![
        Row {
            name: "nullcast",
            post: tweet_candidate(|t| t.is_nullcast = true),
            expect: vec![
                (
                    TimelineHome,
                    Role::NonFollower,
                    dropped(FilteredReason::TweetIsNullcast, "NullcastedTweetDropRule"),
                ),
                (
                    TimelineHome,
                    Role::Author,
                    dropped(FilteredReason::TweetIsNullcast, "NullcastedTweetDropRule"),
                ),
            ],
        },
        Row {
            name: "nullcast_retweet",
            post: {
                let features = TweetFeatures {
                    is_nullcast: true,
                    ..Default::default()
                };
                candidate()
                    .with_tweet_features(features)
                    .retweet_of(2)
                    .build()
            },
            expect: vec![(TimelineHome, Role::NonFollower, allow())],
        },
        Row {
            name: "nullcast_community",
            post: tweet_candidate(|t| {
                t.is_nullcast = true;
                t.is_community_tweet = true;
            }),
            expect: vec![(TimelineHome, Role::NonFollower, allow())],
        },
        Row {
            name: "stale_edit",
            post: stale_candidate(),
            expect: vec![
                (
                    TimelineHome,
                    Role::NonFollower,
                    dropped(FilteredReason::UnspecifiedReason, "DropStaleTweetsRule"),
                ),
                (TimelineHomeHydration, Role::NonFollower, allow()),
            ],
        },
        Row {
            name: "stale_edit_retweet",
            post: {
                let mut retweet = stale_candidate();
                retweet.tweet_features.source_tweet_id = Some(2);
                retweet
            },
            expect: vec![(TimelineHome, Role::NonFollower, allow())],
        },
        Row {
            name: "current_edit",
            post: tweet_candidate(|t| {
                t.edit_control = Some(EditControl::Initial(EditControlInitial {
                    edit_tweet_ids: vec![1],
                    ..Default::default()
                }))
            }),
            expect: vec![(TimelineHome, Role::NonFollower, allow())],
        },
    ]
}

fn stale_candidate() -> HydratedTweetCandidate {
    tweet_candidate(|t| {
        t.edit_control = Some(EditControl::Initial(EditControlInitial {
            edit_tweet_ids: vec![1, 2],
            ..Default::default()
        }))
    })
}
