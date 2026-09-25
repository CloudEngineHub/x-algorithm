use super::builders::controlled_root;
use super::{Role, Row};
use crate::models::{
    HydratedTweetCandidate, LimitedEngagementReason, ViewerAuthorRelationship, ViewerBlockedBy,
};
use crate::rules::fixtures::{allow, candidate, dropped, limited};
use crate::rules::SafetyLevel::{TimelineHome, TimelineHomeHydration};
use xai_core_entities::entities::ConversationControlArm;
use xai_visibility_filtering::models::FilteredReason;

pub(super) fn rows() -> Vec<Row> {
    let blocked_by = |author, root_author| {
        candidate().blocked_by(ViewerBlockedBy {
            author,
            root_author,
        })
    };
    vec![
        Row {
            name: "viewer_blocks_author",
            post: relationship_candidate(|r| r.viewer_blocks_author = true),
            expect: vec![(
                TimelineHome,
                Role::NonFollower,
                dropped(FilteredReason::ViewerBlocksAuthor, "ViewerBlocksAuthorRule"),
            )],
        },
        Row {
            name: "viewer_mutes_author",
            post: relationship_candidate(|r| r.viewer_mutes_author = true),
            expect: vec![(
                TimelineHome,
                Role::NonFollower,
                dropped(FilteredReason::ViewerMutesAuthor, "ViewerMutesAuthorRule"),
            )],
        },
        Row {
            name: "viewer_blocks_and_mutes_author",
            post: relationship_candidate(|r| {
                r.viewer_blocks_author = true;
                r.viewer_mutes_author = true;
            }),
            expect: vec![(
                TimelineHome,
                Role::NonFollower,
                dropped(FilteredReason::ViewerBlocksAuthor, "ViewerBlocksAuthorRule"),
            )],
        },
        Row {
            name: "muted_retweets_retweet",
            post: {
                let relationship = ViewerAuthorRelationship {
                    viewer_mutes_retweets_from_author: true,
                    ..Default::default()
                };
                candidate()
                    .with_relationship(relationship)
                    .retweet_of(2)
                    .build()
            },
            expect: vec![(
                TimelineHome,
                Role::NonFollower,
                dropped(FilteredReason::UnspecifiedReason, "MutedRetweetsRule"),
            )],
        },
        Row {
            name: "muted_retweets_original",
            post: relationship_candidate(|r| r.viewer_mutes_retweets_from_author = true),
            expect: vec![(TimelineHome, Role::NonFollower, allow())],
        },
        Row {
            name: "author_and_root_author_block",
            post: blocked_by(true, true).build(),
            expect: vec![(
                TimelineHomeHydration,
                Role::NonFollower,
                limited(
                    LimitedEngagementReason::BlockedViewer,
                    "BlockedViewerLimitedActionsRule",
                ),
            )],
        },
        Row {
            name: "author_block",
            post: blocked_by(true, false).build(),
            expect: vec![(TimelineHomeHydration, Role::Author, allow())],
        },
        Row {
            name: "root_author_block",
            post: blocked_by(false, true).build(),
            expect: vec![(
                TimelineHomeHydration,
                Role::Author,
                limited(
                    LimitedEngagementReason::RootAuthorBlockedViewer,
                    "RootAuthorBlocksViewerLimitedActionsRule",
                ),
            )],
        },
        Row {
            name: "root_author_block_community_conversation",
            post: blocked_by(false, true)
                .with_conversation_control(controlled_root(ConversationControlArm::Community))
                .build(),
            expect: vec![(
                TimelineHomeHydration,
                Role::NonFollower,
                limited(
                    LimitedEngagementReason::RootAuthorBlockedViewer,
                    "RootAuthorBlocksViewerLimitedActionsRule",
                ),
            )],
        },
    ]
}

fn relationship_candidate(set: fn(&mut ViewerAuthorRelationship)) -> HydratedTweetCandidate {
    let mut relationship = ViewerAuthorRelationship::default();
    set(&mut relationship);
    candidate().with_relationship(relationship).build()
}
