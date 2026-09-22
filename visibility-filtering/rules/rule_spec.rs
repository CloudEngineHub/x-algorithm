use crate::models::{AuthorLabel, LimitedEngagementReason, SafetyLabelType, TombstoneReason};
use crate::rules::context::{AuthorPredicates, TweetPredicates, ViewerPredicates};
use crate::rules::RuleContext;
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
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "constructed once a policy has a LimitedEngagement clause"
        )
    )]
    LimitedEngagement(LimitedEngagementReason),
}

impl RuleClause {
    pub(super) fn applies(&self, context: &RuleContext<'_>) -> bool {
        self.when.iter().all(|condition| condition.holds(context))
            && self.applies_to.admits(context.viewer())
    }
}

impl Audience {
    #[inline]
    fn admits(self, viewer: ViewerPredicates<'_>) -> bool {
        match self {
            Audience::Everyone => true,
            Audience::ExceptAuthor => !viewer.is_author(),
            Audience::ExceptAuthorAndFollowers => {
                !viewer.is_author() && (viewer.is_logged_out() || !viewer.follows_author())
            }
        }
    }
}

impl Condition {
    #[inline]
    fn holds(&self, context: &RuleContext<'_>) -> bool {
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
    fn holds(self, context: &RuleContext<'_>) -> bool {
        match self {
            Predicate::Tweet(fact) => fact.holds(context.tweet()),
            Predicate::Author(fact) => fact.holds(context.author()),
            Predicate::Viewer(fact) => fact.holds(context),
            Predicate::Relationship(fact) => fact.holds(context.viewer()),
        }
    }
}

impl TweetPredicate {
    #[inline]
    fn holds(self, tweet: TweetPredicates<'_>) -> bool {
        match self {
            TweetPredicate::HasSafetyLabel(label) => tweet.has_safety_label(label),
            TweetPredicate::CreatedAfter(unix_ms) => tweet.created_after(unix_ms),
            TweetPredicate::NsfwUserFlag => tweet.has_nsfw_user_flag(),
            TweetPredicate::NsfwAdminFlag => tweet.has_nsfw_admin_flag(),
            TweetPredicate::HasMedia => tweet.has_media(),
            TweetPredicate::HasDmcaMedia => tweet.has_dmca_media(),
            TweetPredicate::IsRetweet => tweet.is_retweet(),
            TweetPredicate::IsSupersededEdit => tweet.is_stale(),
            TweetPredicate::IsNullcast => tweet.is_nullcast(),
            TweetPredicate::IsCommunityTweet => tweet.is_community_tweet(),
            TweetPredicate::HasExclusiveContent => tweet.is_exclusive(),
        }
    }
}

impl AuthorPredicate {
    #[inline]
    fn holds(self, author: AuthorPredicates<'_>) -> bool {
        match self {
            AuthorPredicate::HasUserLabel(label) => author.has_user_label(label),
            AuthorPredicate::IsSuspended => author.is_suspended(),
            AuthorPredicate::IsDeactivated => author.is_deactivated(),
            AuthorPredicate::IsErased => author.is_erased(),
            AuthorPredicate::IsOffboarded => author.is_offboarded(),
            AuthorPredicate::IsProtected => author.is_protected(),
            AuthorPredicate::IsNsfwUser => author.is_nsfw_user(),
            AuthorPredicate::IsNsfwAdmin => author.is_nsfw_admin(),
        }
    }
}

impl ViewerPredicate {
    #[inline]
    fn holds(self, context: &RuleContext<'_>) -> bool {
        let viewer = context.viewer();
        match self {
            ViewerPredicate::LoggedOut => viewer.is_logged_out(),
            ViewerPredicate::Underage => viewer.is_underage(),
            ViewerPredicate::NoStatedAge => viewer.has_no_stated_age(),
            ViewerPredicate::AllowsSensitiveMedia => viewer.allows_sensitive_media(),
            ViewerPredicate::InNsfwGatingCountry => viewer
                .country()
                .is_some_and(|country| context.nsfw_gating_country(country)),
        }
    }
}

impl RelationshipPredicate {
    #[inline]
    fn holds(self, viewer: ViewerPredicates<'_>) -> bool {
        match self {
            RelationshipPredicate::ViewerBlocksAuthor => viewer.blocks_author(),
            RelationshipPredicate::ViewerMutesAuthor => viewer.mutes_author(),
            RelationshipPredicate::ViewerMutesRetweetsFromAuthor => {
                viewer.mutes_retweets_from_author()
            }
            RelationshipPredicate::ViewerIsConversationAuthor => viewer.is_conversation_author(),
            RelationshipPredicate::ViewerSuperFollowsAuthor => viewer.super_follows_author(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{HydratedTweetCandidate, ViewerFeatures};
    use crate::rules::test_context;

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
                TweetPredicate::CreatedAfter(CUTOFF_MS).holds(context.tweet()),
                expected,
                "{tweet_id}"
            );
        }
    }
}
