use super::{Role, Row};
use crate::models::HydratedTweetCandidate;
use crate::rules::fixtures::{allow, candidate, dropped};
use crate::rules::SafetyLevel::TimelineHome;
use xai_visibility_filtering::models::FilteredReason;

pub(super) fn rows() -> Vec<Row> {
    vec![
        Row {
            name: "exclusive",
            post: exclusive_candidate(false),
            expect: vec![
                (
                    TimelineHome,
                    Role::NonFollower,
                    dropped(
                        FilteredReason::ExclusiveTweet,
                        "DropExclusiveTweetContentRule",
                    ),
                ),
                (TimelineHome, Role::Author, allow()),
                (
                    TimelineHome,
                    Role::LoggedOut,
                    dropped(
                        FilteredReason::ExclusiveTweet,
                        "DropExclusiveTweetContentRule",
                    ),
                ),
            ],
        },
        Row {
            name: "exclusive_super_followed",
            post: exclusive_candidate(true),
            expect: vec![(TimelineHome, Role::NonFollower, allow())],
        },
        Row {
            name: "exclusive_retweet",
            post: {
                let mut retweet = exclusive_candidate(false);
                retweet.tweet_features.source_tweet_id = Some(2);
                retweet
            },
            expect: vec![(
                TimelineHome,
                Role::Author,
                dropped(
                    FilteredReason::ExclusiveTweet,
                    "DropExclusiveTweetContentRule",
                ),
            )],
        },
    ]
}

fn exclusive_candidate(viewer_super_follows_author: bool) -> HydratedTweetCandidate {
    let mut c = candidate().build();
    c.tweet_features.exclusive_conversation_author_id = Some(42);
    c.viewer_super_follows_exclusive_author = viewer_super_follows_author;
    c
}
