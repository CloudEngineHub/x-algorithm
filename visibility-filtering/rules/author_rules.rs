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

pub(super) const OON_USER_LABEL_DROPS: &[RuleClause] = &[
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
        "AbusiveHighRecallRule",
        &[Condition::Holds(Predicate::Author(
            AuthorPredicate::HasUserLabel(AuthorLabel::AbusiveHighRecall),
        ))],
        FilteredReason::UnspecifiedReason,
        Audience::ExceptAuthorAndFollowers,
    ),
    author_drop(
        "NsfwNearPerfectAuthorRule",
        &[Condition::Holds(Predicate::Author(
            AuthorPredicate::HasUserLabel(AuthorLabel::NsfwNearPerfect),
        ))],
        FilteredReason::UnspecifiedReason,
        Audience::ExceptAuthor,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{AuthorFeatures, ViewerAuthorRelationship};
    use crate::rules::fixtures::{
        assert_allows, assert_drops, author_viewer, candidate, logged_out_viewer, viewer, VIEWER_ID,
    };

    fn author_flag_features(name: &str) -> AuthorFeatures {
        let mut features = AuthorFeatures::default();
        match name {
            "SuspendedAuthorRule" => features.is_suspended = true,
            "DeactivatedAuthorRule" => features.is_deactivated = true,
            "ErasedAuthorRule" => features.is_erased = true,
            "OffboardedAuthorRule" => features.is_offboarded = true,
            "DropNsfwUserAuthorRule" => features.is_nsfw_user = true,
            "DropNsfwAdminAuthorRule" => features.is_nsfw_admin = true,
            "ProtectedAuthorDropRule" => features.is_protected = true,
            _ => panic!("no trigger flags for rule {name}"),
        }
        features
    }

    #[test]
    fn author_flag_drop_axis() {
        for spec in AUTHOR_STATE_DROPS.iter().chain(OON_NSFW_AUTHOR_DROPS) {
            let RuleClause {
                rule_name: name,
                action: ActionSpec::Drop(reason),
                applies_to,
                ..
            } = spec
            else {
                panic!("{} is not an author drop row", spec.rule_name);
            };
            let firing = candidate()
                .with_author_features(author_flag_features(name))
                .build();
            for v in [viewer(VIEWER_ID), logged_out_viewer()] {
                assert_drops(spec, &v, &firing, reason);
            }
            let followed = candidate()
                .with_author_features(author_flag_features(name))
                .followed()
                .build();
            if *applies_to == Audience::ExceptAuthorAndFollowers {
                assert_allows(spec, &viewer(VIEWER_ID), &followed);
                assert_drops(spec, &logged_out_viewer(), &followed, reason);
            } else {
                assert_drops(spec, &viewer(VIEWER_ID), &followed, reason);
            }
            let unflagged = candidate().build();
            assert_allows(spec, &viewer(VIEWER_ID), &unflagged);
            assert_allows(spec, &author_viewer(), &firing);
        }
    }

    fn trigger_user_label(name: &str) -> AuthorLabel {
        match name {
            "NsfwHighRecallUserLabelRule" => AuthorLabel::NsfwHighRecall,
            "NsfwHighPrecisionUserLabelRule" => AuthorLabel::NsfwHighPrecision,
            "SpamHighRecallUserLabelRule" => AuthorLabel::SpamHighRecall,
            "CompromisedUserLabelRule" => AuthorLabel::Compromised,
            "ReadOnlyUserLabelRule" => AuthorLabel::ReadOnly,
            "ImpersonationHighPrecisionUserLabelRule" => AuthorLabel::ImpersonationHighPrecision,
            "NsfwAvatarImageRule" => AuthorLabel::NsfwAvatarImage,
            "NsfwBannerImageRule" => AuthorLabel::NsfwBannerImage,
            "AbusiveHighRecallRule" => AuthorLabel::AbusiveHighRecall,
            "NsfwNearPerfectAuthorRule" => AuthorLabel::NsfwNearPerfect,
            "DoNotAmplifyNonFollowerRule" => AuthorLabel::DoNotAmplify,
            _ => panic!("no trigger user label for rule {name}"),
        }
    }

    #[test]
    fn user_label_drop_axis() {
        for spec in OON_USER_LABEL_DROPS {
            let RuleClause {
                rule_name: name,
                action: ActionSpec::Drop(reason),
                applies_to,
                ..
            } = spec
            else {
                panic!("{} is not a user-label drop row", spec.rule_name);
            };
            let firing = candidate()
                .with_author_user_label(trigger_user_label(name))
                .build();
            for v in [viewer(VIEWER_ID), logged_out_viewer()] {
                assert_drops(spec, &v, &firing, reason);
            }
            let followed = candidate()
                .with_author_user_label(trigger_user_label(name))
                .followed()
                .build();
            if *applies_to == Audience::ExceptAuthorAndFollowers {
                assert_allows(spec, &viewer(VIEWER_ID), &followed);
                assert_drops(spec, &logged_out_viewer(), &followed, reason);
            } else {
                assert_drops(spec, &viewer(VIEWER_ID), &followed, reason);
            }
            let nonmatching = if trigger_user_label(name) == AuthorLabel::Compromised {
                AuthorLabel::ReadOnly
            } else {
                AuthorLabel::Compromised
            };
            let unrelated = candidate().with_author_user_label(nonmatching).build();
            assert_allows(spec, &viewer(VIEWER_ID), &unrelated);
            assert_allows(spec, &author_viewer(), &firing);
        }
    }

    fn relationship_trigger(name: &str) -> (ViewerAuthorRelationship, bool, FilteredReason) {
        match name {
            "ViewerBlocksAuthorRule" => (
                ViewerAuthorRelationship {
                    viewer_blocks_author: true,
                    ..Default::default()
                },
                false,
                FilteredReason::ViewerBlocksAuthor,
            ),
            "ViewerMutesAuthorRule" => (
                ViewerAuthorRelationship {
                    viewer_mutes_author: true,
                    ..Default::default()
                },
                false,
                FilteredReason::ViewerMutesAuthor,
            ),
            "MutedRetweetsRule" => (
                ViewerAuthorRelationship {
                    viewer_mutes_retweets_from_author: true,
                    ..Default::default()
                },
                true,
                FilteredReason::UnspecifiedReason,
            ),
            _ => panic!("no relationship trigger for rule {name}"),
        }
    }

    #[test]
    fn socialgraph_relationship_axis() {
        for spec in SOCIALGRAPH_DROPS {
            let (rel, retweet, reason) = relationship_trigger(spec.rule_name);
            let mut firing = candidate().with_relationship(rel.clone());
            if retweet {
                firing = firing.retweet_of(99);
            }
            let firing = firing.build();
            assert_drops(spec, &viewer(VIEWER_ID), &firing, &reason);
            assert_allows(spec, &logged_out_viewer(), &firing);
            assert_allows(spec, &viewer(VIEWER_ID), &candidate().build());
            if spec.rule_name == "MutedRetweetsRule" {
                let non_retweet = candidate().with_relationship(rel).build();
                assert_allows(spec, &viewer(VIEWER_ID), &non_retweet);
            }
        }
    }
}
