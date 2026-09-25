use super::builders::{controlled_root, read_only_viewer};
use super::{Role, Row};
use crate::models::{
    ConversationControlFeatures, HydratedTweetCandidate, LimitedEngagementReason, SafetyLabelType,
    ViewerProfile,
};
use crate::rules::fixtures::{
    allow, blurred, blurred_and_limited, candidate, conversation_control, limited,
    viewer_with_profile, VIEWER_ID,
};
use crate::rules::SafetyLevel::TimelineHomeHydration;
use xai_core_entities::entities::{ConversationControl, ConversationControlArm};
use xai_x_thrift::action::InterstitialReason;

const REPLY_ROOT_AUTHOR_ID: u64 = 4242;

pub(super) fn rows() -> Vec<Row> {
    use ConversationControlArm::{
        ByInvitation, Co, Community, Followers, MyNetwork, Subscribers, Verified,
    };
    let limits = |rule| limited(LimitedEngagementReason::ConversationControl, rule);
    let hydration = |role, verdict| (TimelineHomeHydration, role, verdict);
    [
        (
            ByInvitation,
            vec![
                hydration(
                    Role::NonFollower,
                    limits("LimitRepliesByInvitationConversationRule"),
                ),
                hydration(
                    Role::As("read_only", read_only_viewer(VIEWER_ID)),
                    limits("LimitRepliesByInvitationConversationRule"),
                ),
            ],
            [
                "by_invitation_conversation",
                "by_invitation_conversation_invited",
                "by_invitation_conversation_retweet",
            ],
        ),
        (
            Community,
            vec![hydration(
                Role::NonFollower,
                limits("LimitRepliesCommunityConversationRule"),
            )],
            [
                "community_conversation",
                "community_conversation_invited",
                "community_conversation_retweet",
            ],
        ),
        (
            Subscribers,
            vec![hydration(
                Role::NonFollower,
                limits("LimitRepliesSubscribersConversationRule"),
            )],
            [
                "subscribers_conversation",
                "subscribers_conversation_invited",
                "subscribers_conversation_retweet",
            ],
        ),
        (
            Verified,
            vec![
                hydration(
                    Role::NonFollower,
                    limits("LimitRepliesVerifiedConversationRule"),
                ),
                hydration(
                    Role::As(
                        "verified",
                        viewer_with_profile(ViewerProfile {
                            has_verified_badge: true,
                            ..ViewerProfile::default()
                        }),
                    ),
                    allow(),
                ),
            ],
            [
                "verified_conversation",
                "verified_conversation_invited",
                "verified_conversation_retweet",
            ],
        ),
        (
            MyNetwork,
            vec![hydration(
                Role::NonFollower,
                limits("LimitRepliesMyNetworkConversationRule"),
            )],
            [
                "my_network_conversation",
                "my_network_conversation_invited",
                "my_network_conversation_retweet",
            ],
        ),
        (
            Co,
            vec![],
            [
                "co_conversation",
                "co_conversation_invited",
                "co_conversation_retweet",
            ],
        ),
    ]
    .into_iter()
    .flat_map(|(arm, mut root_expect, [root, invited, retweet])| {
        root_expect.push(hydration(Role::Author, allow()));
        root_expect.push(hydration(Role::LoggedOut, allow()));
        [
            Row {
                name: root,
                post: controlled_candidate(controlled_root(arm)),
                expect: root_expect,
            },
            Row {
                name: invited,
                post: controlled_candidate(ConversationControlFeatures {
                    control: ConversationControl {
                        invited_user_ids: vec![VIEWER_ID],
                        ..controlled_root(arm).control
                    },
                    ..controlled_root(arm)
                }),
                expect: vec![hydration(Role::NonFollower, allow())],
            },
            Row {
                name: retweet,
                post: candidate()
                    .retweet_of(2)
                    .with_conversation_control(controlled_root(arm))
                    .build(),
                expect: vec![hydration(Role::NonFollower, allow())],
            },
        ]
    })
    .chain([
        Row {
            name: "followers_conversation",
            post: controlled_candidate(controlled_root(Followers)),
            expect: vec![hydration(Role::NonFollower, allow())],
        },
        Row {
            name: "nsfw_by_invitation_conversation",
            post: candidate()
                .with_label(SafetyLabelType::NSFW_HIGH_PRECISION)
                .with_media()
                .with_conversation_control(controlled_root(ByInvitation))
                .build(),
            expect: vec![hydration(
                Role::NonFollower,
                blurred_and_limited(
                    blurred(
                        InterstitialReason::Sensitive(true),
                        "NsfwHighPrecisionInterstitialRule",
                    ),
                    limits("LimitRepliesByInvitationConversationRule"),
                ),
            )],
        },
        Row {
            name: "by_invitation_reply",
            post: controlled_candidate(conversation_control(ByInvitation, REPLY_ROOT_AUTHOR_ID)),
            expect: vec![hydration(
                Role::Author,
                limits("LimitRepliesByInvitationConversationRule"),
            )],
        },
        Row {
            name: "community_conversation_followed_viewer",
            post: controlled_candidate(ConversationControlFeatures {
                root_author_follows_viewer: Some(true),
                ..controlled_root(Community)
            }),
            expect: vec![hydration(Role::NonFollower, allow())],
        },
        Row {
            name: "my_network_conversation_followed_viewer",
            post: controlled_candidate(ConversationControlFeatures {
                root_author_follows_viewer: Some(true),
                ..controlled_root(MyNetwork)
            }),
            expect: vec![hydration(Role::NonFollower, allow())],
        },
        Row {
            name: "subscribers_conversation_super_follower",
            post: controlled_candidate(ConversationControlFeatures {
                viewer_super_follows_root_author: Some(true),
                ..controlled_root(Subscribers)
            }),
            expect: vec![hydration(Role::NonFollower, allow())],
        },
        Row {
            name: "co_conversation_outside_allowed_countries",
            post: controlled_candidate(co_root(&["EUR", "BR"], Some("US"))),
            expect: vec![hydration(
                Role::NonFollower,
                limits("LimitRepliesCoConversationRule"),
            )],
        },
        Row {
            name: "co_conversation_unknown_viewer_country",
            post: controlled_candidate(co_root(&["US"], None)),
            expect: vec![hydration(
                Role::NonFollower,
                limits("LimitRepliesCoConversationRule"),
            )],
        },
        Row {
            name: "co_conversation_no_allowed_country",
            post: controlled_candidate(co_root(&[], Some("US"))),
            expect: vec![hydration(
                Role::NonFollower,
                limits("LimitRepliesCoConversationRule"),
            )],
        },
        Row {
            name: "co_conversation_allowed_country",
            post: controlled_candidate(co_root(&["BR"], Some("br"))),
            expect: vec![hydration(Role::NonFollower, allow())],
        },
        Row {
            name: "co_conversation_allowed_region",
            post: controlled_candidate(co_root(&["NAM"], Some("US"))),
            expect: vec![hydration(Role::NonFollower, allow())],
        },
    ])
    .collect()
}

fn co_root(
    allowed_country_codes: &[&str],
    viewer_country: Option<&str>,
) -> ConversationControlFeatures {
    let root = controlled_root(ConversationControlArm::Co);
    ConversationControlFeatures {
        control: ConversationControl {
            allowed_country_codes: allowed_country_codes
                .iter()
                .map(|code| (*code).to_string())
                .collect(),
            ..root.control
        },
        viewer_country: viewer_country.map(Into::into),
        ..root
    }
}

fn controlled_candidate(features: ConversationControlFeatures) -> HydratedTweetCandidate {
    candidate().with_conversation_control(features).build()
}
