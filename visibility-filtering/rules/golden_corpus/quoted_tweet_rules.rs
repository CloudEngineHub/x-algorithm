use super::builders::author_candidate;
use super::{UserRow, OUTER_TWEET_ID};
use crate::hydration::Hydrator::{Blocks, Follows, Mutes};
use crate::models::{PairEdges, PostCandidate, QuoteFacts, QuoteLink, UserCandidate};
use crate::rules::fixtures::{
    allow, author_viewer, candidate, dropped, logged_out_viewer, viewer, AUTHOR_ID, VIEWER_ID,
};
use xai_visibility_filtering::models::FilteredReason;

const QUOTING_AUTHOR_ID: u64 = 200;

fn quoted_by(quoting_author: u64, pair_edges: PairEdges) -> QuoteFacts {
    QuoteFacts {
        link: Some(QuoteLink {
            outer: OUTER_TWEET_ID,
            quoting_author,
        }),
        pair_edges,
    }
}

fn quoted() -> QuoteFacts {
    quoted_by(QUOTING_AUTHOR_ID, PairEdges::default())
}

fn author(post: PostCandidate) -> UserCandidate {
    UserCandidate {
        user: post.author,
        viewer_relationship: post.viewer_relationship,
        failed: post.failed,
    }
}

pub(super) fn rows() -> Vec<UserRow> {
    let author_state = [
        (
            "deactivated_author",
            author_candidate(|a| a.is_deactivated = true),
            FilteredReason::AuthorIsDeactivated,
            "deactivated_author/drop",
        ),
        (
            "erased_author",
            author_candidate(|a| a.is_erased = true),
            FilteredReason::AuthorAccountIsInactive,
            "erased_author/drop/inactive",
        ),
        (
            "offboarded_author",
            author_candidate(|a| a.is_offboarded = true),
            FilteredReason::AuthorAccountIsInactive,
            "offboarded_author/drop/inactive",
        ),
        (
            "suspended_author",
            author_candidate(|a| a.is_suspended = true),
            FilteredReason::AuthorIsSuspended,
            "suspended_author/drop",
        ),
    ];
    let mut rows: Vec<UserRow> = author_state
        .into_iter()
        .flat_map(|(name, post, reason, by)| {
            [
                UserRow {
                    name,
                    role: "non_follower",
                    viewer: viewer(VIEWER_ID),
                    user: author(post.clone()),
                    quote: quoted(),
                    expected: dropped(reason, by),
                },
                UserRow {
                    name,
                    role: "author",
                    viewer: author_viewer(),
                    user: author(post),
                    quote: quoted(),
                    expected: allow(),
                },
            ]
        })
        .collect();
    rows.extend([
        UserRow {
            name: "viewer_blocks_author",
            role: "non_follower",
            viewer: viewer(VIEWER_ID),
            user: author(candidate().with_edge(Blocks).build()),
            quote: quoted(),
            expected: dropped(
                FilteredReason::ViewerBlocksAuthor,
                "viewer_blocks_author/drop",
            ),
        },
        UserRow {
            name: "viewer_mutes_author",
            role: "non_follower",
            viewer: viewer(VIEWER_ID),
            user: author(candidate().with_edge(Mutes).build()),
            quote: quoted(),
            expected: dropped(
                FilteredReason::ViewerMutesAuthor,
                "quoted_viewer_mutes_author_not_following/drop",
            ),
        },
        UserRow {
            name: "viewer_mutes_author",
            role: "follower",
            viewer: viewer(VIEWER_ID),
            user: author(candidate().with_edge(Mutes).with_edge(Follows).build()),
            quote: quoted(),
            expected: allow(),
        },
    ]);
    let blocked = PairEdges {
        author_blocks_outer_author: true,
        ..PairEdges::default()
    };
    let blocks_quoting_author = || {
        dropped(
            FilteredReason::AuthorBlockViewer,
            "quoted_author_blocks_quoting_author/drop/author_block_viewer",
        )
    };
    let quoting_author_follows = PairEdges {
        outer_author_follows_author: true,
        ..PairEdges::default()
    };
    let protected = || author(author_candidate(|a| a.is_protected = true));
    let protected_dropped = || {
        dropped(
            FilteredReason::AuthorIsProtected,
            "protected_quoted_author/drop",
        )
    };
    rows.extend([
        UserRow {
            name: "quoted_author_blocks_quoting_author",
            role: "non_follower",
            viewer: viewer(VIEWER_ID),
            user: author(candidate().build()),
            quote: quoted_by(QUOTING_AUTHOR_ID, blocked),
            expected: blocks_quoting_author(),
        },
        UserRow {
            name: "quoted_author_blocks_quoting_author",
            role: "logged_out",
            viewer: logged_out_viewer(),
            user: author(candidate().build()),
            quote: quoted_by(QUOTING_AUTHOR_ID, blocked),
            expected: blocks_quoting_author(),
        },
        UserRow {
            name: "quoted_author_blocks_quoting_author",
            role: "self_quote",
            viewer: viewer(VIEWER_ID),
            user: author(candidate().build()),
            quote: quoted_by(AUTHOR_ID, blocked),
            expected: allow(),
        },
        UserRow {
            name: "protected_quoted_author",
            role: "non_follower",
            viewer: viewer(VIEWER_ID),
            user: protected(),
            quote: quoted(),
            expected: protected_dropped(),
        },
        UserRow {
            name: "protected_quoted_author",
            role: "logged_out",
            viewer: logged_out_viewer(),
            user: protected(),
            quote: quoted(),
            expected: protected_dropped(),
        },
        UserRow {
            name: "protected_quoted_author",
            role: "author",
            viewer: author_viewer(),
            user: protected(),
            quote: quoted(),
            expected: protected_dropped(),
        },
        UserRow {
            name: "protected_quoted_author",
            role: "quoting_author_follows",
            viewer: viewer(VIEWER_ID),
            user: protected(),
            quote: quoted_by(QUOTING_AUTHOR_ID, quoting_author_follows),
            expected: allow(),
        },
        UserRow {
            name: "protected_quoted_author",
            role: "self_quote",
            viewer: viewer(VIEWER_ID),
            user: protected(),
            quote: quoted_by(AUTHOR_ID, PairEdges::default()),
            expected: allow(),
        },
    ]);
    rows
}
