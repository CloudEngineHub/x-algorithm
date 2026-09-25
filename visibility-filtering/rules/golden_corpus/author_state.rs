use super::builders::author_candidate;
use super::{Role, Row};
use crate::models::{AuthorFeatures, SafetyLabelType};
use crate::rules::fixtures::{allow, candidate, dropped};
use crate::rules::SafetyLevel::TimelineHome;
use xai_visibility_filtering::models::FilteredReason;

pub(super) fn rows() -> Vec<Row> {
    vec![
        Row {
            name: "suspended_author",
            post: author_candidate(|a| a.is_suspended = true),
            expect: vec![
                (
                    TimelineHome,
                    Role::NonFollower,
                    dropped(FilteredReason::AuthorIsSuspended, "SuspendedAuthorRule"),
                ),
                (
                    TimelineHome,
                    Role::Follower,
                    dropped(FilteredReason::AuthorIsSuspended, "SuspendedAuthorRule"),
                ),
                (TimelineHome, Role::Author, allow()),
            ],
        },
        Row {
            name: "suspended_author_nsfw_media",
            post: candidate()
                .with_author_features(AuthorFeatures {
                    is_suspended: true,
                    ..Default::default()
                })
                .with_label(SafetyLabelType::NSFW_HIGH_PRECISION)
                .with_media()
                .build(),
            expect: vec![(
                TimelineHome,
                Role::NonFollower,
                dropped(FilteredReason::AuthorIsSuspended, "SuspendedAuthorRule"),
            )],
        },
        Row {
            name: "deactivated_author",
            post: author_candidate(|a| a.is_deactivated = true),
            expect: vec![
                (
                    TimelineHome,
                    Role::NonFollower,
                    dropped(FilteredReason::AuthorIsDeactivated, "DeactivatedAuthorRule"),
                ),
                (
                    TimelineHome,
                    Role::Follower,
                    dropped(FilteredReason::AuthorIsDeactivated, "DeactivatedAuthorRule"),
                ),
                (TimelineHome, Role::Author, allow()),
            ],
        },
        Row {
            name: "erased_author",
            post: author_candidate(|a| a.is_erased = true),
            expect: vec![
                (
                    TimelineHome,
                    Role::NonFollower,
                    dropped(FilteredReason::AuthorAccountIsInactive, "ErasedAuthorRule"),
                ),
                (
                    TimelineHome,
                    Role::Follower,
                    dropped(FilteredReason::AuthorAccountIsInactive, "ErasedAuthorRule"),
                ),
                (TimelineHome, Role::Author, allow()),
            ],
        },
        Row {
            name: "offboarded_author",
            post: author_candidate(|a| a.is_offboarded = true),
            expect: vec![
                (
                    TimelineHome,
                    Role::NonFollower,
                    dropped(
                        FilteredReason::AuthorAccountIsInactive,
                        "OffboardedAuthorRule",
                    ),
                ),
                (
                    TimelineHome,
                    Role::Follower,
                    dropped(
                        FilteredReason::AuthorAccountIsInactive,
                        "OffboardedAuthorRule",
                    ),
                ),
                (TimelineHome, Role::Author, allow()),
            ],
        },
        Row {
            name: "protected_author",
            post: author_candidate(|a| a.is_protected = true),
            expect: vec![
                (
                    TimelineHome,
                    Role::NonFollower,
                    dropped(FilteredReason::AuthorIsProtected, "ProtectedAuthorDropRule"),
                ),
                (TimelineHome, Role::Follower, allow()),
                (TimelineHome, Role::Author, allow()),
                (
                    TimelineHome,
                    Role::LoggedOut,
                    dropped(FilteredReason::AuthorIsProtected, "ProtectedAuthorDropRule"),
                ),
            ],
        },
    ]
}
