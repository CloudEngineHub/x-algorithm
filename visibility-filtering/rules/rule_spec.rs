use crate::hydration::{Hydrator, Hydrators};
use crate::models::{
    AuthorFeatures, AuthorLabel, LimitedEngagementReason, SafetyLabelType, TombstoneReason,
    ViewerProfile,
};
use crate::rules::RuleContext;
use xai_core_entities::entities::ConversationControlArm;
use xai_visibility_filtering::models::FilteredReason;
use xai_x_thrift::action::InterstitialReason;

pub(super) struct RuleClause {
    pub(super) rule_name: &'static str,
    pub(super) when: &'static [Condition],
    pub(super) applies_to: Audience,
    pub(super) action: ActionSpec,
}

pub(super) enum Condition {
    Holds(Predicate),
    Not(Predicate),
    AnyOf(&'static [Predicate]),
    Opaque {
        #[cfg_attr(
            not(test),
            expect(dead_code, reason = "read only by the table-shape tests")
        )]
        id: &'static str,
        #[cfg_attr(
            not(test),
            expect(dead_code, reason = "read only by the table-shape tests")
        )]
        doc: &'static str,
        eval: fn(&RuleContext<'_>) -> bool,
        hydrators: Hydrators,
    },
}

#[derive(Clone, Copy)]
pub(super) enum Predicate {
    Tweet(TweetPredicate),
    Author(AuthorPredicate),
    Viewer(ViewerPredicate),
    Relationship(RelationshipPredicate),
}

#[derive(Clone, Copy)]
pub(super) enum TweetPredicate {
    HasSafetyLabel(SafetyLabelType),
    CreatedAfter(u64),
    NsfwUserFlag,
    NsfwAdminFlag,
    HasMedia,
    HasDmcaMedia,
    IsRetweet,
    IsSupersededEdit,
    IsNullcast,
    IsCommunityTweet,
    HasExclusiveContent,
    HasConversationControl(ConversationControlArm),
}

#[derive(Clone, Copy)]
pub(super) enum AuthorPredicate {
    HasUserLabel(AuthorLabel),
    IsSuspended,
    IsDeactivated,
    IsErased,
    IsOffboarded,
    IsProtected,
    IsNsfwUser,
    IsNsfwAdmin,
}

#[derive(Clone, Copy)]
pub(super) enum ViewerPredicate {
    LoggedOut,
    Underage,
    NoStatedAge,
    AllowsSensitiveMedia,
    InNsfwGatingCountry,
    HasVerifiedBadge,
}

#[derive(Clone, Copy)]
#[expect(
    clippy::enum_variant_names,
    reason = "the Viewer prefix identifies the acting subject of each relationship"
)]
pub(super) enum RelationshipPredicate {
    ViewerBlocksAuthor,
    ViewerMutesAuthor,
    ViewerMutesRetweetsFromAuthor,
    ViewerIsConversationAuthor,
    ViewerSuperFollowsAuthor,
    ViewerIsConversationRootAuthor,
    ViewerIsInvitedToConversation,
    ViewerIsFollowedByConversationRootAuthor,
    ViewerSuperFollowsConversationRootAuthor,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Audience {
    Everyone,
    ExceptAuthor,
    ExceptAuthorAndFollowers,
}

pub(super) enum ActionSpec {
    Drop(FilteredReason),
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "constructed once a policy has a Tombstone clause")
    )]
    Tombstone(TombstoneReason),
    Interstitial {
        legacy: FilteredReason,
        media: InterstitialReason,
    },
    LimitedEngagement(LimitedEngagementReason),
}

impl RuleClause {
    pub(super) fn applies(&self, context: &RuleContext<'_>) -> bool {
        self.when.iter().all(|condition| condition.holds(context))
            && self.applies_to.admits(context)
    }

    pub(super) const fn hydrators(&self) -> Hydrators {
        let mut hydrators = self.applies_to.hydrators();
        let mut rest = self.when;
        while let [condition, tail @ ..] = rest {
            hydrators = hydrators.union(condition.hydrators());
            rest = tail;
        }
        hydrators
    }
}

impl Audience {
    pub(super) const fn hydrators(self) -> Hydrators {
        match self {
            Audience::Everyone | Audience::ExceptAuthor => Hydrators::empty(),
            Audience::ExceptAuthorAndFollowers => Hydrators::of(Hydrator::Relationship),
        }
    }
}

impl Condition {
    pub(super) const fn hydrators(&self) -> Hydrators {
        match self {
            Condition::Holds(leaf) | Condition::Not(leaf) => leaf.hydrators(),
            Condition::AnyOf(leaves) => {
                let mut hydrators = Hydrators::empty();
                let mut rest = *leaves;
                while let [leaf, tail @ ..] = rest {
                    hydrators = hydrators.union(leaf.hydrators());
                    rest = tail;
                }
                hydrators
            }
            Condition::Opaque { hydrators, .. } => *hydrators,
        }
    }
}

impl Predicate {
    pub(super) const fn hydrators(self) -> Hydrators {
        match self {
            Predicate::Tweet(fact) => fact.hydrators(),
            Predicate::Author(_) => Hydrators::of(Hydrator::Author),
            Predicate::Viewer(fact) => fact.hydrators(),
            Predicate::Relationship(fact) => fact.hydrators(),
        }
    }
}

impl TweetPredicate {
    const fn hydrators(self) -> Hydrators {
        match self {
            TweetPredicate::HasSafetyLabel(_) => Hydrators::of(Hydrator::TweetSafetyLabels),
            TweetPredicate::CreatedAfter(_) => Hydrators::empty(),
            TweetPredicate::NsfwUserFlag
            | TweetPredicate::NsfwAdminFlag
            | TweetPredicate::HasMedia
            | TweetPredicate::HasDmcaMedia
            | TweetPredicate::IsRetweet
            | TweetPredicate::IsSupersededEdit
            | TweetPredicate::IsNullcast
            | TweetPredicate::IsCommunityTweet => Hydrators::of(Hydrator::Tweet),
            TweetPredicate::HasExclusiveContent => EXCLUSIVE,
            TweetPredicate::HasConversationControl(_) => {
                Hydrators::of(Hydrator::ConversationControl)
            }
        }
    }
}

impl ViewerPredicate {
    const fn hydrators(self) -> Hydrators {
        match self {
            ViewerPredicate::LoggedOut => Hydrators::empty(),
            ViewerPredicate::Underage
            | ViewerPredicate::NoStatedAge
            | ViewerPredicate::AllowsSensitiveMedia
            | ViewerPredicate::InNsfwGatingCountry
            | ViewerPredicate::HasVerifiedBadge => Hydrators::of(Hydrator::ViewerProfile),
        }
    }
}

impl RelationshipPredicate {
    const fn hydrators(self) -> Hydrators {
        match self {
            RelationshipPredicate::ViewerBlocksAuthor
            | RelationshipPredicate::ViewerMutesAuthor
            | RelationshipPredicate::ViewerMutesRetweetsFromAuthor => {
                Hydrators::of(Hydrator::Relationship)
            }
            RelationshipPredicate::ViewerIsConversationAuthor
            | RelationshipPredicate::ViewerSuperFollowsAuthor => EXCLUSIVE,
            RelationshipPredicate::ViewerIsConversationRootAuthor
            | RelationshipPredicate::ViewerIsInvitedToConversation
            | RelationshipPredicate::ViewerIsFollowedByConversationRootAuthor
            | RelationshipPredicate::ViewerSuperFollowsConversationRootAuthor => {
                Hydrators::of(Hydrator::ConversationControl)
            }
        }
    }
}

const EXCLUSIVE: Hydrators = Hydrators::of(Hydrator::ExclusiveContent).with(Hydrator::Tweet);

impl Audience {
    #[inline]
    pub(super) fn admits(self, context: &RuleContext<'_>) -> bool {
        match self {
            Audience::Everyone => true,
            Audience::ExceptAuthor => !context.is_author_viewer(),
            Audience::ExceptAuthorAndFollowers => {
                !context.is_author_viewer()
                    && (context.viewer_id().is_none()
                        || !context.relationship().viewer_follows_author)
            }
        }
    }
}

impl Condition {
    #[inline]
    pub(super) fn holds(&self, context: &RuleContext<'_>) -> bool {
        match self {
            Condition::Holds(leaf) => leaf.holds(context),
            Condition::Not(leaf) => !leaf.holds(context),
            Condition::AnyOf(leaves) => leaves.iter().any(|leaf| leaf.holds(context)),
            Condition::Opaque { eval, .. } => eval(context),
        }
    }
}

impl Predicate {
    #[inline]
    pub(super) fn holds(self, context: &RuleContext<'_>) -> bool {
        match self {
            Predicate::Tweet(fact) => fact.holds(context),
            Predicate::Author(fact) => fact.holds(context.author_features()),
            Predicate::Viewer(fact) => fact.holds(context),
            Predicate::Relationship(fact) => fact.holds(context),
        }
    }
}

impl TweetPredicate {
    #[inline]
    fn holds(self, context: &RuleContext<'_>) -> bool {
        match self {
            TweetPredicate::HasSafetyLabel(label) => context.tweet_safety_labels().has_label(label),
            TweetPredicate::CreatedAfter(unix_ms) => context.created_after(unix_ms),
            TweetPredicate::NsfwUserFlag => context.tweet_features().nsfw.user,
            TweetPredicate::NsfwAdminFlag => context.tweet_features().nsfw.admin,
            TweetPredicate::HasMedia => context.tweet_features().has_media(),
            TweetPredicate::HasDmcaMedia => context.tweet_features().has_dmca_media(),
            TweetPredicate::IsRetweet => context.tweet_features().is_retweet(),
            TweetPredicate::IsSupersededEdit => context
                .tweet_features()
                .is_superseded_edit(context.tweet_id()),
            TweetPredicate::IsNullcast => context.tweet_features().is_nullcast,
            TweetPredicate::IsCommunityTweet => context.tweet_features().is_community_tweet,
            TweetPredicate::HasExclusiveContent => context.exclusive_content().is_some(),
            TweetPredicate::HasConversationControl(arm) => context
                .conversation_control()
                .is_some_and(|features| features.control.arm == arm),
        }
    }
}

impl AuthorPredicate {
    #[inline]
    fn holds(self, author: &AuthorFeatures) -> bool {
        match self {
            AuthorPredicate::HasUserLabel(label) => author.user_labels.has_label(label),
            AuthorPredicate::IsSuspended => author.is_suspended,
            AuthorPredicate::IsDeactivated => author.is_deactivated,
            AuthorPredicate::IsErased => author.is_erased,
            AuthorPredicate::IsOffboarded => author.is_offboarded,
            AuthorPredicate::IsProtected => author.is_protected,
            AuthorPredicate::IsNsfwUser => author.is_nsfw_user,
            AuthorPredicate::IsNsfwAdmin => author.is_nsfw_admin,
        }
    }
}

impl ViewerPredicate {
    #[inline]
    fn holds(self, context: &RuleContext<'_>) -> bool {
        match self {
            ViewerPredicate::LoggedOut => context.viewer_id().is_none(),
            ViewerPredicate::Underage => context
                .viewer_profile()
                .is_some_and(ViewerProfile::is_underage),
            ViewerPredicate::NoStatedAge => context
                .viewer_profile()
                .is_some_and(ViewerProfile::has_no_stated_age),
            ViewerPredicate::AllowsSensitiveMedia => context
                .viewer_profile()
                .is_some_and(|profile| profile.allows_sensitive_media),
            ViewerPredicate::HasVerifiedBadge => context
                .viewer_profile()
                .is_some_and(|profile| profile.has_verified_badge),
            ViewerPredicate::InNsfwGatingCountry => context
                .viewer_country()
                .is_some_and(|country| context.nsfw_gating_country(country)),
        }
    }
}

impl RelationshipPredicate {
    #[inline]
    fn holds(self, context: &RuleContext<'_>) -> bool {
        match self {
            RelationshipPredicate::ViewerBlocksAuthor => {
                context.relationship().viewer_blocks_author
            }
            RelationshipPredicate::ViewerMutesAuthor => context.relationship().viewer_mutes_author,
            RelationshipPredicate::ViewerMutesRetweetsFromAuthor => {
                context.relationship().viewer_mutes_retweets_from_author
            }
            RelationshipPredicate::ViewerIsConversationAuthor => context
                .exclusive_content()
                .zip(context.viewer_id())
                .is_some_and(|(exclusive, viewer_id)| {
                    viewer_id == exclusive.conversation_author_id
                }),
            RelationshipPredicate::ViewerSuperFollowsAuthor => context
                .exclusive_content()
                .is_some_and(|exclusive| exclusive.viewer_super_follows_author),
            RelationshipPredicate::ViewerIsConversationRootAuthor => context
                .conversation_control()
                .zip(context.viewer_id())
                .is_some_and(|(features, viewer_id)| {
                    viewer_id == features.control.conversation_tweet_author_id
                }),
            RelationshipPredicate::ViewerIsInvitedToConversation => context
                .conversation_control()
                .zip(context.viewer_id())
                .is_some_and(|(features, viewer_id)| {
                    features.control.invited_user_ids.contains(&viewer_id)
                }),
            RelationshipPredicate::ViewerIsFollowedByConversationRootAuthor => context
                .conversation_control()
                .is_some_and(|features| features.root_author_follows_viewer == Some(true)),
            RelationshipPredicate::ViewerSuperFollowsConversationRootAuthor => context
                .conversation_control()
                .is_some_and(|features| features.viewer_super_follows_root_author == Some(true)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ConversationControlFeatures, HydratedTweetCandidate, ViewerFeatures};
    use crate::rules::fixtures::{candidate, logged_out_viewer, viewer, VIEWER_ID};
    use crate::rules::test_context;
    use xai_core_entities::entities::ConversationControl;

    const ROOT_AUTHOR_ID: u64 = 4242;

    fn controlled(
        arm: ConversationControlArm,
        invited_user_ids: Vec<u64>,
        root_author_follows_viewer: Option<bool>,
        viewer_super_follows_root_author: Option<bool>,
    ) -> HydratedTweetCandidate {
        candidate()
            .with_conversation_control(ConversationControlFeatures {
                control: ConversationControl {
                    arm,
                    conversation_tweet_author_id: ROOT_AUTHOR_ID,
                    invited_user_ids,
                    invite_via_mention: None,
                    allowed_country_codes: vec![],
                },
                root_author_follows_viewer,
                viewer_super_follows_root_author,
            })
            .build()
    }

    fn holds_narrowed(
        predicate: Predicate,
        viewer: &ViewerFeatures,
        candidate: &HydratedTweetCandidate,
    ) -> bool {
        predicate.holds(&test_context(viewer, candidate).hydrated_by(predicate.hydrators()))
    }

    #[test]
    fn conversation_control_predicates_read_the_root_keyed_features() {
        use ConversationControlArm::{ByInvitation, Community, Subscribers};
        use RelationshipPredicate::{
            ViewerIsConversationRootAuthor, ViewerIsFollowedByConversationRootAuthor,
            ViewerIsInvitedToConversation, ViewerSuperFollowsConversationRootAuthor,
        };
        let community = controlled(Community, vec![], Some(true), None);
        let subscribers = controlled(Subscribers, vec![], None, Some(true));
        let invitation = controlled(ByInvitation, vec![VIEWER_ID], None, None);
        let unknown = controlled(Community, vec![], None, None);
        let uncontrolled = candidate().build();
        let root_author = viewer(ROOT_AUTHOR_ID);
        let viewer = viewer(VIEWER_ID);
        let logged_out = logged_out_viewer();
        for (predicate, viewer, candidate, expected) in [
            (
                Predicate::Tweet(TweetPredicate::HasConversationControl(Community)),
                &viewer,
                &community,
                true,
            ),
            (
                Predicate::Tweet(TweetPredicate::HasConversationControl(Community)),
                &viewer,
                &uncontrolled,
                false,
            ),
            (
                Predicate::Relationship(ViewerIsConversationRootAuthor),
                &root_author,
                &community,
                true,
            ),
            (
                Predicate::Relationship(ViewerIsConversationRootAuthor),
                &logged_out,
                &community,
                false,
            ),
            (
                Predicate::Relationship(ViewerIsInvitedToConversation),
                &viewer,
                &invitation,
                true,
            ),
            (
                Predicate::Relationship(ViewerIsFollowedByConversationRootAuthor),
                &viewer,
                &community,
                true,
            ),
            (
                Predicate::Relationship(ViewerIsFollowedByConversationRootAuthor),
                &viewer,
                &unknown,
                false,
            ),
            (
                Predicate::Relationship(ViewerSuperFollowsConversationRootAuthor),
                &viewer,
                &subscribers,
                true,
            ),
            (
                Predicate::Relationship(ViewerSuperFollowsConversationRootAuthor),
                &viewer,
                &unknown,
                false,
            ),
            (
                Predicate::Relationship(ViewerSuperFollowsConversationRootAuthor),
                &viewer,
                &uncontrolled,
                false,
            ),
        ] {
            assert_eq!(holds_narrowed(predicate, viewer, candidate), expected);
        }
    }

    #[test]
    fn created_after_is_strict_at_the_unix_millisecond_boundary() {
        const CUTOFF_MS: u64 = 1705536000000;
        const AT_CUTOFF: u64 = (CUTOFF_MS - 1288834974657) << 22;
        let viewer = ViewerFeatures::default();
        for (tweet_id, expected) in [
            (AT_CUTOFF - 1, false),
            (AT_CUTOFF, false),
            (AT_CUTOFF + (1 << 22) - 1, false),
            (AT_CUTOFF + (1 << 22), true),
            (0, false),
        ] {
            let candidate = HydratedTweetCandidate {
                tweet_id,
                ..Default::default()
            };
            let context = test_context(&viewer, &candidate);
            assert_eq!(
                TweetPredicate::CreatedAfter(CUTOFF_MS).holds(&context),
                expected,
                "{tweet_id}"
            );
        }
    }
}
