use crate::models::AuthorLabel;
use crate::rules::rule_spec::{
    ActionSpec, Audience, AuthorPredicate, Condition, Predicate, RelationshipPredicate, RuleClause,
    TweetPredicate, ViewerPredicate,
};
use xai_visibility_filtering::models::FilteredReason;

const fn author_drop(
    rule_name: &'static str,
    when: &'static [Condition],
    reason: FilteredReason,
    applies_to: Audience,
) -> RuleClause {
    RuleClause {
        rule_name,
        when,
        applies_to,
        action: ActionSpec::Drop(reason),
    }
}

pub(super) const AUTHOR_STATE_DROPS: &[RuleClause] = &[
    author_drop(
        "SuspendedAuthorRule",
        &[Condition::Holds(Predicate::Author(
            AuthorPredicate::IsSuspended,
        ))],
        FilteredReason::AuthorIsSuspended,
        Audience::ExceptAuthor,
    ),
    author_drop(
        "DeactivatedAuthorRule",
        &[Condition::Holds(Predicate::Author(
            AuthorPredicate::IsDeactivated,
        ))],
        FilteredReason::AuthorIsDeactivated,
        Audience::ExceptAuthor,
    ),
    author_drop(
        "ErasedAuthorRule",
        &[Condition::Holds(Predicate::Author(
            AuthorPredicate::IsErased,
        ))],
        FilteredReason::AuthorAccountIsInactive,
        Audience::ExceptAuthor,
    ),
    author_drop(
        "OffboardedAuthorRule",
        &[Condition::Holds(Predicate::Author(
            AuthorPredicate::IsOffboarded,
        ))],
        FilteredReason::AuthorAccountIsInactive,
        Audience::ExceptAuthor,
    ),
    author_drop(
        "ProtectedAuthorDropRule",
        &[Condition::Holds(Predicate::Author(
            AuthorPredicate::IsProtected,
        ))],
        FilteredReason::AuthorIsProtected,
        Audience::ExceptAuthorAndFollowers,
    ),
];

pub(super) const OON_NSFW_AUTHOR_DROPS: &[RuleClause] = &[
    author_drop(
        "DropNsfwUserAuthorRule",
        &[Condition::Holds(Predicate::Author(
            AuthorPredicate::IsNsfwUser,
        ))],
        FilteredReason::ContainNsfwMedia,
        Audience::ExceptAuthor,
    ),
    author_drop(
        "DropNsfwAdminAuthorRule",
        &[Condition::Holds(Predicate::Author(
            AuthorPredicate::IsNsfwAdmin,
        ))],
        FilteredReason::ContainNsfwMedia,
        Audience::ExceptAuthor,
    ),
];

pub(super) const OON_NSFW_USER_LABEL_DROPS: &[RuleClause] = &[
    author_drop(
        "NsfwHighRecallUserLabelRule",
        &[Condition::Holds(Predicate::Author(
            AuthorPredicate::HasUserLabel(AuthorLabel::NsfwHighRecall),
        ))],
        FilteredReason::UnspecifiedReason,
        Audience::ExceptAuthor,
    ),
    author_drop(
        "NsfwHighPrecisionUserLabelRule",
        &[Condition::Holds(Predicate::Author(
            AuthorPredicate::HasUserLabel(AuthorLabel::NsfwHighPrecision),
        ))],
        FilteredReason::UnspecifiedReason,
        Audience::ExceptAuthor,
    ),
    author_drop(
        "NsfwAvatarImageRule",
        &[Condition::Holds(Predicate::Author(
            AuthorPredicate::HasUserLabel(AuthorLabel::NsfwAvatarImage),
        ))],
        FilteredReason::UnspecifiedReason,
        Audience::ExceptAuthor,
    ),
    author_drop(
        "NsfwBannerImageRule",
        &[Condition::Holds(Predicate::Author(
            AuthorPredicate::HasUserLabel(AuthorLabel::NsfwBannerImage),
        ))],
        FilteredReason::UnspecifiedReason,
        Audience::ExceptAuthor,
    ),
    author_drop(
        "NsfwNearPerfectAuthorRule",
        &[Condition::Holds(Predicate::Author(
            AuthorPredicate::HasUserLabel(AuthorLabel::NsfwNearPerfect),
        ))],
        FilteredReason::UnspecifiedReason,
        Audience::ExceptAuthor,
    ),
];

pub(super) const OON_USER_LABEL_DROPS: &[RuleClause] = &[
    author_drop(
        "SpamHighRecallUserLabelRule",
        &[Condition::Holds(Predicate::Author(
            AuthorPredicate::HasUserLabel(AuthorLabel::SpamHighRecall),
        ))],
        FilteredReason::UnspecifiedReason,
        Audience::ExceptAuthor,
    ),
    author_drop(
        "CompromisedUserLabelRule",
        &[Condition::Holds(Predicate::Author(
            AuthorPredicate::HasUserLabel(AuthorLabel::Compromised),
        ))],
        FilteredReason::UnspecifiedReason,
        Audience::ExceptAuthor,
    ),
    author_drop(
        "ReadOnlyUserLabelRule",
        &[Condition::Holds(Predicate::Author(
            AuthorPredicate::HasUserLabel(AuthorLabel::ReadOnly),
        ))],
        FilteredReason::UnspecifiedReason,
        Audience::ExceptAuthor,
    ),
    author_drop(
        "ImpersonationHighPrecisionUserLabelRule",
        &[Condition::Holds(Predicate::Author(
            AuthorPredicate::HasUserLabel(AuthorLabel::ImpersonationHighPrecision),
        ))],
        FilteredReason::UnspecifiedReason,
        Audience::ExceptAuthor,
    ),
    author_drop(
        "AbusiveHighRecallRule",
        &[Condition::Holds(Predicate::Author(
            AuthorPredicate::HasUserLabel(AuthorLabel::AbusiveHighRecall),
        ))],
        FilteredReason::UnspecifiedReason,
        Audience::ExceptAuthorAndFollowers,
    ),
    author_drop(
        "DoNotAmplifyNonFollowerRule",
        &[Condition::Holds(Predicate::Author(
            AuthorPredicate::HasUserLabel(AuthorLabel::DoNotAmplify),
        ))],
        FilteredReason::UnspecifiedReason,
        Audience::ExceptAuthorAndFollowers,
    ),
];

const LOGGED_IN: Condition = Condition::Not(Predicate::Viewer(ViewerPredicate::LoggedOut));

pub(super) const SOCIALGRAPH_DROPS: &[RuleClause] = &[
    RuleClause {
        rule_name: "ViewerBlocksAuthorRule",
        when: &[
            LOGGED_IN,
            Condition::Holds(Predicate::Relationship(
                RelationshipPredicate::ViewerBlocksAuthor,
            )),
        ],
        applies_to: Audience::Everyone,
        action: ActionSpec::Drop(FilteredReason::ViewerBlocksAuthor),
    },
    RuleClause {
        rule_name: "ViewerMutesAuthorRule",
        when: &[
            LOGGED_IN,
            Condition::Holds(Predicate::Relationship(
                RelationshipPredicate::ViewerMutesAuthor,
            )),
        ],
        applies_to: Audience::Everyone,
        action: ActionSpec::Drop(FilteredReason::ViewerMutesAuthor),
    },
    RuleClause {
        rule_name: "MutedRetweetsRule",
        when: &[
            LOGGED_IN,
            Condition::Holds(Predicate::Tweet(TweetPredicate::IsRetweet)),
            Condition::Holds(Predicate::Relationship(
                RelationshipPredicate::ViewerMutesRetweetsFromAuthor,
            )),
        ],
        applies_to: Audience::Everyone,
        action: ActionSpec::Drop(FilteredReason::UnspecifiedReason),
    },
];
