use crate::hydration::{Hydrator, Hydrators};
use crate::models::{LimitedEngagementReason, SafetyLabelType};
use crate::rules::rule_spec::{
    ActionSpec, Audience, AuthorPredicate, Condition, Predicate, RelationshipPredicate, RuleClause,
    TweetPredicate, ViewerPredicate,
};
use xai_core_entities::entities::ConversationControlArm;
use xai_visibility_filtering::models::{
    Action, DropReason, FilteredReason, SafetyResult, SafetyResultReason,
};
use xai_x_thrift::action::InterstitialReason;

const NSFW_HIGH_PRECISION_REASON: FilteredReason = FilteredReason::SafetyResult(SafetyResult {
    reason: Some(SafetyResultReason::NsfwHighPrecision),
    action: Action::Drop(DropReason {}),
});

const fn label(label: SafetyLabelType) -> Condition {
    Condition::Holds(Predicate::Tweet(TweetPredicate::HasSafetyLabel(label)))
}

const HAS_MEDIA: Condition = Condition::Holds(Predicate::Tweet(TweetPredicate::HasMedia));
const NOT_RETWEET: Condition = Condition::Not(Predicate::Tweet(TweetPredicate::IsRetweet));
const SENSITIVE_MEDIA_DISABLED: Condition =
    Condition::Not(Predicate::Viewer(ViewerPredicate::AllowsSensitiveMedia));
const LOGGED_OUT: Condition = Condition::Holds(Predicate::Viewer(ViewerPredicate::LoggedOut));
const UNDERAGE: Condition = Condition::Holds(Predicate::Viewer(ViewerPredicate::Underage));
const NO_STATED_AGE: Condition = Condition::Holds(Predicate::Viewer(ViewerPredicate::NoStatedAge));
const IN_NSFW_GATING_COUNTRY: Condition =
    Condition::Holds(Predicate::Viewer(ViewerPredicate::InNsfwGatingCountry));
const NSFW_MEDIA_LABEL: Condition = Condition::AnyOf(&[
    Predicate::Tweet(TweetPredicate::HasSafetyLabel(
        SafetyLabelType::NSFW_HIGH_PRECISION,
    )),
    Predicate::Tweet(TweetPredicate::HasSafetyLabel(
        SafetyLabelType::NSFW_HIGH_RECALL,
    )),
    Predicate::Tweet(TweetPredicate::HasSafetyLabel(
        SafetyLabelType::GORE_AND_VIOLENCE_HIGH_PRECISION,
    )),
]);
const NSFW_FLAGGED: Condition = Condition::AnyOf(&[
    Predicate::Author(AuthorPredicate::IsNsfwUser),
    Predicate::Author(AuthorPredicate::IsNsfwAdmin),
    Predicate::Tweet(TweetPredicate::NsfwUserFlag),
    Predicate::Tweet(TweetPredicate::NsfwAdminFlag),
]);
const NSFW_TEXT_OR_CARD_LABEL: Condition = Condition::AnyOf(&[
    Predicate::Tweet(TweetPredicate::HasSafetyLabel(SafetyLabelType::NSFW_TEXT)),
    Predicate::Tweet(TweetPredicate::HasSafetyLabel(
        SafetyLabelType::NSFW_CARD_IMAGE,
    )),
]);
const HAS_EXCLUSIVE_CONTENT: Condition =
    Condition::Holds(Predicate::Tweet(TweetPredicate::HasExclusiveContent));
const NOT_CONVERSATION_AUTHOR: Condition = Condition::Not(Predicate::Relationship(
    RelationshipPredicate::ViewerIsConversationAuthor,
));
const NOT_SUPER_FOLLOWER: Condition = Condition::Not(Predicate::Relationship(
    RelationshipPredicate::ViewerSuperFollowsAuthor,
));
const NOT_LOGGED_OUT: Condition = Condition::Not(Predicate::Viewer(ViewerPredicate::LoggedOut));
const NOT_CONVERSATION_ROOT_AUTHOR: Condition = Condition::Not(Predicate::Relationship(
    RelationshipPredicate::ViewerIsConversationRootAuthor,
));
const NOT_INVITED_TO_CONVERSATION: Condition = Condition::Not(Predicate::Relationship(
    RelationshipPredicate::ViewerIsInvitedToConversation,
));

const fn has_conversation_control(arm: ConversationControlArm) -> Condition {
    Condition::Holds(Predicate::Tweet(TweetPredicate::HasConversationControl(
        arm,
    )))
}

pub(super) const TWEET_LABEL_DROPS: &[RuleClause] = &[
    RuleClause {
        rule_name: "PdnaTweetLabelRule",
        when: &[label(SafetyLabelType::PDNA)],
        applies_to: Audience::ExceptAuthor,
        action: ActionSpec::Drop(NSFW_HIGH_PRECISION_REASON),
    },
    RuleClause {
        rule_name: "BounceTweetLabelRule",
        when: &[label(SafetyLabelType::BOUNCE)],
        applies_to: Audience::ExceptAuthor,
        action: ActionSpec::Drop(FilteredReason::TweetIsBounced),
    },
    RuleClause {
        rule_name: "SpamTweetLabelRule",
        when: &[label(SafetyLabelType::SPAM)],
        applies_to: Audience::ExceptAuthor,
        action: ActionSpec::Drop(FilteredReason::PossiblyUndesirable),
    },
    RuleClause {
        rule_name: "ForEmergencyUseOnlyDropRule",
        when: &[label(SafetyLabelType::FOR_EMERGENCY_USE_ONLY)],
        applies_to: Audience::Everyone,
        action: ActionSpec::Drop(FilteredReason::UnspecifiedReason),
    },
    RuleClause {
        rule_name: "FosnrHatefulConductDropRule",
        when: &[label(SafetyLabelType::FOSNR_HATEFUL_CONDUCT)],
        applies_to: Audience::ExceptAuthor,
        action: ActionSpec::Drop(FilteredReason::PossiblyUndesirable),
    },
    RuleClause {
        rule_name: "FosnrViolentSpeechDropRule",
        when: &[label(SafetyLabelType::FOSNR_VIOLENT_SPEECH)],
        applies_to: Audience::ExceptAuthor,
        action: ActionSpec::Drop(FilteredReason::PossiblyUndesirable),
    },
    RuleClause {
        rule_name: "FosnrAbuseDropRule",
        when: &[label(SafetyLabelType::FOSNR_ABUSE)],
        applies_to: Audience::ExceptAuthor,
        action: ActionSpec::Drop(FilteredReason::PossiblyUndesirable),
    },
    RuleClause {
        rule_name: "FosnrCivicIntegrityDropRule",
        when: &[label(SafetyLabelType::FOSNR_CIVIC_INTEGRITY)],
        applies_to: Audience::ExceptAuthor,
        action: ActionSpec::Drop(FilteredReason::PossiblyUndesirable),
    },
];

const NSFW_HIGH_PRECISION_CHANGED_AT: u64 = 1705536000000;

pub(super) const NSFW_MEDIA_INTERSTITIALS: &[RuleClause] = &[
    RuleClause {
        rule_name: "NsfwHighPrecisionAdultInterstitialRule",
        when: &[
            label(SafetyLabelType::NSFW_HIGH_PRECISION),
            Condition::Holds(Predicate::Tweet(TweetPredicate::CreatedAfter(
                NSFW_HIGH_PRECISION_CHANGED_AT,
            ))),
            SENSITIVE_MEDIA_DISABLED,
        ],
        applies_to: Audience::ExceptAuthor,
        action: ActionSpec::Interstitial {
            legacy: FilteredReason::ContainNsfwMedia,
            media: InterstitialReason::Nudity(true),
        },
    },
    RuleClause {
        rule_name: "NsfwHighPrecisionInterstitialRule",
        when: &[
            label(SafetyLabelType::NSFW_HIGH_PRECISION),
            Condition::Not(Predicate::Tweet(TweetPredicate::CreatedAfter(
                NSFW_HIGH_PRECISION_CHANGED_AT,
            ))),
            SENSITIVE_MEDIA_DISABLED,
        ],
        applies_to: Audience::ExceptAuthor,
        action: ActionSpec::Interstitial {
            legacy: FilteredReason::ContainNsfwMedia,
            media: InterstitialReason::Sensitive(true),
        },
    },
    RuleClause {
        rule_name: "GoreAndViolenceInterstitialRule",
        when: &[
            label(SafetyLabelType::GORE_AND_VIOLENCE_HIGH_PRECISION),
            SENSITIVE_MEDIA_DISABLED,
        ],
        applies_to: Audience::ExceptAuthor,
        action: ActionSpec::Interstitial {
            legacy: FilteredReason::ContainNsfwMedia,
            media: InterstitialReason::Violence(true),
        },
    },
    RuleClause {
        rule_name: "NsfwCardImageInterstitialRule",
        when: &[
            label(SafetyLabelType::NSFW_CARD_IMAGE),
            SENSITIVE_MEDIA_DISABLED,
        ],
        applies_to: Audience::ExceptAuthor,
        action: ActionSpec::Interstitial {
            legacy: FilteredReason::ContainNsfwMedia,
            media: InterstitialReason::Sensitive(true),
        },
    },
];

pub(super) const OON_TWEET_FLAG_DROPS: &[RuleClause] = &[
    RuleClause {
        rule_name: "TweetNsfwUserDropRule",
        when: &[Condition::Holds(Predicate::Tweet(
            TweetPredicate::NsfwUserFlag,
        ))],
        applies_to: Audience::Everyone,
        action: ActionSpec::Drop(FilteredReason::ContainNsfwMedia),
    },
    RuleClause {
        rule_name: "TweetNsfwAdminDropRule",
        when: &[Condition::Holds(Predicate::Tweet(
            TweetPredicate::NsfwAdminFlag,
        ))],
        applies_to: Audience::Everyone,
        action: ActionSpec::Drop(FilteredReason::ContainNsfwMedia),
    },
];

pub(super) const OON_TWEET_LABEL_DROPS: &[RuleClause] = &[
    RuleClause {
        rule_name: "NsfwHighRecallDropRule",
        when: &[label(SafetyLabelType::NSFW_HIGH_RECALL)],
        applies_to: Audience::ExceptAuthor,
        action: ActionSpec::Drop(FilteredReason::ContainNsfwMedia),
    },
    RuleClause {
        rule_name: "NsfwHighPrecisionOonDropRule",
        when: &[label(SafetyLabelType::NSFW_HIGH_PRECISION)],
        applies_to: Audience::ExceptAuthor,
        action: ActionSpec::Drop(FilteredReason::ContainNsfwMedia),
    },
    RuleClause {
        rule_name: "GoreAndViolenceOonDropRule",
        when: &[label(SafetyLabelType::GORE_AND_VIOLENCE_HIGH_PRECISION)],
        applies_to: Audience::ExceptAuthor,
        action: ActionSpec::Drop(FilteredReason::ContainNsfwMedia),
    },
    RuleClause {
        rule_name: "NsfwCardImageOonDropRule",
        when: &[label(SafetyLabelType::NSFW_CARD_IMAGE)],
        applies_to: Audience::ExceptAuthor,
        action: ActionSpec::Drop(FilteredReason::ContainNsfwMedia),
    },
    RuleClause {
        rule_name: "DoNotAmplifyOonDropRule",
        when: &[label(SafetyLabelType::DO_NOT_AMPLIFY)],
        applies_to: Audience::ExceptAuthor,
        action: ActionSpec::Drop(FilteredReason::PossiblyUndesirable),
    },
    RuleClause {
        rule_name: "MaliciousUrlOonDropRule",
        when: &[label(SafetyLabelType::MALICIOUS_URL)],
        applies_to: Audience::ExceptAuthor,
        action: ActionSpec::Drop(FilteredReason::PossiblyUndesirable),
    },
    RuleClause {
        rule_name: "SpamHighRecallDropRule",
        when: &[label(SafetyLabelType::SPAM_HIGH_RECALL)],
        applies_to: Audience::ExceptAuthor,
        action: ActionSpec::Drop(FilteredReason::PossiblyUndesirable),
    },
    RuleClause {
        rule_name: "NsfwTextTweetLabelDropRule",
        when: &[label(SafetyLabelType::NSFW_TEXT)],
        applies_to: Audience::ExceptAuthor,
        action: ActionSpec::Drop(NSFW_HIGH_PRECISION_REASON),
    },
    RuleClause {
        rule_name: "FosnrAbuseInsultsOonDropRule",
        when: &[label(SafetyLabelType::FOSNR_ABUSE_INSULTS)],
        applies_to: Audience::ExceptAuthor,
        action: ActionSpec::Drop(FilteredReason::PossiblyUndesirable),
    },
];

pub(super) const EXCLUSIVE_TWEET_DROP: &[RuleClause] = &[
    RuleClause {
        rule_name: "DropExclusiveTweetContentRule",
        when: &[HAS_EXCLUSIVE_CONTENT, LOGGED_OUT],
        applies_to: Audience::Everyone,
        action: ActionSpec::Drop(FilteredReason::ExclusiveTweet),
    },
    RuleClause {
        rule_name: "DropExclusiveTweetContentRule",
        when: &[
            HAS_EXCLUSIVE_CONTENT,
            NOT_CONVERSATION_AUTHOR,
            NOT_SUPER_FOLLOWER,
            Condition::Holds(Predicate::Tweet(TweetPredicate::IsRetweet)),
        ],
        applies_to: Audience::Everyone,
        action: ActionSpec::Drop(FilteredReason::ExclusiveTweet),
    },
    RuleClause {
        rule_name: "DropExclusiveTweetContentRule",
        when: &[
            HAS_EXCLUSIVE_CONTENT,
            NOT_CONVERSATION_AUTHOR,
            NOT_SUPER_FOLLOWER,
        ],
        applies_to: Audience::ExceptAuthor,
        action: ActionSpec::Drop(FilteredReason::ExclusiveTweet),
    },
];

pub(super) const NSFW_AUTHOR_INTERSTITIAL: &[RuleClause] = &[
    RuleClause {
        rule_name: "NsfwAdminInterstitialRule",
        when: &[
            Condition::AnyOf(&[
                Predicate::Author(AuthorPredicate::IsNsfwAdmin),
                Predicate::Tweet(TweetPredicate::NsfwAdminFlag),
            ]),
            HAS_MEDIA,
            SENSITIVE_MEDIA_DISABLED,
        ],
        applies_to: Audience::ExceptAuthor,
        action: ActionSpec::Interstitial {
            legacy: FilteredReason::ContainNsfwMedia,
            media: InterstitialReason::Sensitive(true),
        },
    },
    RuleClause {
        rule_name: "NsfwUserInterstitialRule",
        when: &[
            Condition::AnyOf(&[
                Predicate::Author(AuthorPredicate::IsNsfwUser),
                Predicate::Tweet(TweetPredicate::NsfwUserFlag),
            ]),
            HAS_MEDIA,
            SENSITIVE_MEDIA_DISABLED,
        ],
        applies_to: Audience::ExceptAuthor,
        action: ActionSpec::Interstitial {
            legacy: FilteredReason::ContainNsfwMedia,
            media: InterstitialReason::SensitiveUser(true),
        },
    },
];

const fn limit_replies(rule_name: &'static str, when: &'static [Condition]) -> RuleClause {
    RuleClause {
        rule_name,
        when,
        applies_to: Audience::Everyone,
        action: ActionSpec::LimitedEngagement(LimitedEngagementReason::ConversationControl),
    }
}

pub(super) const LIMIT_REPLIES_CONVERSATION_RULES: &[RuleClause] = &[
    limit_replies(
        "LimitRepliesByInvitationConversationRule",
        &[
            has_conversation_control(ConversationControlArm::ByInvitation),
            NOT_LOGGED_OUT,
            NOT_RETWEET,
            NOT_CONVERSATION_ROOT_AUTHOR,
            NOT_INVITED_TO_CONVERSATION,
        ],
    ),
    limit_replies(
        "LimitRepliesCommunityConversationRule",
        &[
            has_conversation_control(ConversationControlArm::Community),
            NOT_LOGGED_OUT,
            NOT_RETWEET,
            NOT_CONVERSATION_ROOT_AUTHOR,
            NOT_INVITED_TO_CONVERSATION,
            Condition::Not(Predicate::Relationship(
                RelationshipPredicate::ViewerIsFollowedByConversationRootAuthor,
            )),
        ],
    ),
    limit_replies(
        "LimitRepliesSubscribersConversationRule",
        &[
            has_conversation_control(ConversationControlArm::Subscribers),
            NOT_LOGGED_OUT,
            NOT_RETWEET,
            NOT_CONVERSATION_ROOT_AUTHOR,
            NOT_INVITED_TO_CONVERSATION,
            Condition::Not(Predicate::Relationship(
                RelationshipPredicate::ViewerSuperFollowsConversationRootAuthor,
            )),
        ],
    ),
    limit_replies(
        "LimitRepliesVerifiedConversationRule",
        &[
            has_conversation_control(ConversationControlArm::Verified),
            NOT_LOGGED_OUT,
            NOT_RETWEET,
            NOT_CONVERSATION_ROOT_AUTHOR,
            NOT_INVITED_TO_CONVERSATION,
            Condition::Not(Predicate::Viewer(ViewerPredicate::HasVerifiedBadge)),
        ],
    ),
];

const fn sensitive_viewer_drop(rule_name: &'static str, when: &'static [Condition]) -> RuleClause {
    RuleClause {
        rule_name,
        when,
        applies_to: Audience::ExceptAuthor,
        action: ActionSpec::Drop(FilteredReason::ContainNsfwMedia),
    }
}

pub(super) const SENSITIVE_VIEWER_DROPS: &[RuleClause] = &[
    sensitive_viewer_drop(
        "SensitiveViewerLoggedOutDropRule",
        &[LOGGED_OUT, HAS_MEDIA, NSFW_MEDIA_LABEL],
    ),
    sensitive_viewer_drop(
        "SensitiveViewerLoggedOutDropRule",
        &[LOGGED_OUT, HAS_MEDIA, NOT_RETWEET, NSFW_FLAGGED],
    ),
    sensitive_viewer_drop(
        "SensitiveViewerLoggedOutDropRule",
        &[LOGGED_OUT, NSFW_TEXT_OR_CARD_LABEL],
    ),
    sensitive_viewer_drop(
        "SensitiveViewerUnderageDropRule",
        &[UNDERAGE, HAS_MEDIA, NSFW_MEDIA_LABEL],
    ),
    sensitive_viewer_drop(
        "SensitiveViewerUnderageDropRule",
        &[UNDERAGE, HAS_MEDIA, NOT_RETWEET, NSFW_FLAGGED],
    ),
    sensitive_viewer_drop(
        "SensitiveViewerUnderageDropRule",
        &[UNDERAGE, NSFW_TEXT_OR_CARD_LABEL],
    ),
    sensitive_viewer_drop(
        "SensitiveViewerNoStatedAgeDropRule",
        &[
            NO_STATED_AGE,
            IN_NSFW_GATING_COUNTRY,
            HAS_MEDIA,
            NSFW_MEDIA_LABEL,
        ],
    ),
    sensitive_viewer_drop(
        "SensitiveViewerNoStatedAgeDropRule",
        &[
            NO_STATED_AGE,
            IN_NSFW_GATING_COUNTRY,
            HAS_MEDIA,
            NOT_RETWEET,
            NSFW_FLAGGED,
        ],
    ),
    sensitive_viewer_drop(
        "SensitiveViewerNoStatedAgeDropRule",
        &[
            NO_STATED_AGE,
            IN_NSFW_GATING_COUNTRY,
            NSFW_TEXT_OR_CARD_LABEL,
        ],
    ),
];

pub(super) const NULLCAST_DROP: &[RuleClause] = &[RuleClause {
    rule_name: "NullcastedTweetDropRule",
    when: &[
        Condition::Holds(Predicate::Tweet(TweetPredicate::IsNullcast)),
        NOT_RETWEET,
        Condition::Not(Predicate::Tweet(TweetPredicate::IsCommunityTweet)),
    ],
    applies_to: Audience::Everyone,
    action: ActionSpec::Drop(FilteredReason::TweetIsNullcast),
}];

pub(super) const STALE_TWEET_DROP: &[RuleClause] = &[RuleClause {
    rule_name: "DropStaleTweetsRule",
    when: &[
        Condition::Holds(Predicate::Tweet(TweetPredicate::IsSupersededEdit)),
        NOT_RETWEET,
    ],
    applies_to: Audience::Everyone,
    action: ActionSpec::Drop(FilteredReason::UnspecifiedReason),
}];

pub(super) const TAKEDOWN_DROPS: &[RuleClause] = &[
    RuleClause {
        rule_name: "DropLegalTakendownPostRule",
        when: &[Condition::Opaque {
            id: "legal_takedown_in_viewer_country",
            doc: "a LegalRequest (any code but xy) or UnspecifiedReason takedown names \
                  the viewer's request country or a worldwide code (xx/xy); Dmca counts as xy",
            eval: |context| {
                context
                    .tweet_features()
                    .legal_takedown_in(context.request_country())
            },
            hydrators: Hydrators::of(Hydrator::Tweet),
        }],
        applies_to: Audience::ExceptAuthor,
        action: ActionSpec::Drop(FilteredReason::UnspecifiedReason),
    },
    RuleClause {
        rule_name: "DropLocalLawsTakendownPostRule",
        when: &[Condition::Opaque {
            id: "local_laws_takedown_in_viewer_country",
            doc: "a BystanderReport takedown names the viewer's request country; \
                  worldwide codes (xx/xy) do not count",
            eval: |context| {
                context
                    .tweet_features()
                    .local_laws_takedown_in(context.request_country())
            },
            hydrators: Hydrators::of(Hydrator::Tweet),
        }],
        applies_to: Audience::ExceptAuthor,
        action: ActionSpec::Drop(FilteredReason::UnspecifiedReason),
    },
];

pub(super) const FILTER_ALL: &[RuleClause] = &[RuleClause {
    rule_name: "FilterAllRule",
    when: &[],
    applies_to: Audience::Everyone,
    action: ActionSpec::Drop(FilteredReason::UnspecifiedReason),
}];

pub(super) const RECS_MEDIA_DROPS: &[RuleClause] = &[
    RuleClause {
        rule_name: "DropTweetsWithDmcaMediaRule",
        when: &[Condition::Holds(Predicate::Tweet(
            TweetPredicate::HasDmcaMedia,
        ))],
        applies_to: Audience::Everyone,
        action: ActionSpec::Drop(FilteredReason::UnspecifiedReason),
    },
    RuleClause {
        rule_name: "DropTweetsWithGeoRestrictedMediaRule",
        when: &[Condition::Opaque {
            id: "media_geo_restricted_in_viewer_country",
            doc: "the media geo allow-list is non-empty and omits the viewer's \
                  request country (xx when absent), or the deny-list names it",
            eval: |context| {
                context
                    .tweet_features()
                    .media_restricted_in(context.request_country())
            },
            hydrators: Hydrators::of(Hydrator::Tweet),
        }],
        applies_to: Audience::Everyone,
        action: ActionSpec::Drop(FilteredReason::UnspecifiedReason),
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        AuthorFeatures, ConversationControlFeatures, Decided, ExclusiveContentFeatures,
        HydratedTweetCandidate, LimitedEngagement, MediaFeature, MediaInterstitial, NsfwFeature,
        TweetFeatures, Verdict, Viewer, ViewerAge, ViewerFeatures, ViewerProfile,
    };
    use crate::rules::fixtures::{
        assert_allows, assert_clauses_allow, assert_clauses_drop, assert_drops, author_viewer,
        candidate, clauses_verdict, conversation_control, logged_out_viewer,
        sensitive_opt_in_viewer, viewer, viewer_with_profile, VIEWER_ID,
    };
    use crate::rules::registry::Policy;
    use crate::rules::test_context;
    use xai_core_entities::entities::{
        ConversationControl, ConversationControlArm, TakedownReason,
    };

    fn trigger_label(name: &str) -> SafetyLabelType {
        match name {
            "PdnaTweetLabelRule" => SafetyLabelType::PDNA,
            "BounceTweetLabelRule" => SafetyLabelType::BOUNCE,
            "SpamTweetLabelRule" => SafetyLabelType::SPAM,
            "ForEmergencyUseOnlyDropRule" => SafetyLabelType::FOR_EMERGENCY_USE_ONLY,
            "FosnrHatefulConductDropRule" => SafetyLabelType::FOSNR_HATEFUL_CONDUCT,
            "FosnrViolentSpeechDropRule" => SafetyLabelType::FOSNR_VIOLENT_SPEECH,
            "FosnrAbuseDropRule" => SafetyLabelType::FOSNR_ABUSE,
            "FosnrCivicIntegrityDropRule" => SafetyLabelType::FOSNR_CIVIC_INTEGRITY,
            "NsfwHighRecallDropRule" => SafetyLabelType::NSFW_HIGH_RECALL,
            "NsfwHighPrecisionOonDropRule" => SafetyLabelType::NSFW_HIGH_PRECISION,
            "GoreAndViolenceOonDropRule" => SafetyLabelType::GORE_AND_VIOLENCE_HIGH_PRECISION,
            "NsfwCardImageOonDropRule" => SafetyLabelType::NSFW_CARD_IMAGE,
            "DoNotAmplifyOonDropRule" => SafetyLabelType::DO_NOT_AMPLIFY,
            "MaliciousUrlOonDropRule" => SafetyLabelType::MALICIOUS_URL,
            "SpamHighRecallDropRule" => SafetyLabelType::SPAM_HIGH_RECALL,
            "NsfwTextTweetLabelDropRule" => SafetyLabelType::NSFW_TEXT,
            "FosnrAbuseInsultsOonDropRule" => SafetyLabelType::FOSNR_ABUSE_INSULTS,
            _ => panic!("no trigger label for rule {name}"),
        }
    }

    const UNRELATED_LABEL: SafetyLabelType = SafetyLabelType::EGREGIOUS_NSFW;

    #[test]
    fn tweet_label_drop_axis() {
        for spec in TWEET_LABEL_DROPS.iter().chain(OON_TWEET_LABEL_DROPS) {
            let RuleClause {
                rule_name: name,
                action: ActionSpec::Drop(reason),
                applies_to,
                ..
            } = spec
            else {
                panic!("{} is not a tweet-label drop row", spec.rule_name);
            };
            let firing = candidate().with_label(trigger_label(name)).build();
            for v in [
                viewer(VIEWER_ID),
                logged_out_viewer(),
                sensitive_opt_in_viewer(),
            ] {
                assert_drops(spec, &v, &firing, reason);
            }
            let followed = candidate()
                .with_label(trigger_label(name))
                .followed()
                .build();
            assert_drops(spec, &viewer(VIEWER_ID), &followed, reason);

            let unrelated = candidate().with_label(UNRELATED_LABEL).build();
            assert_allows(spec, &viewer(VIEWER_ID), &unrelated);

            if *applies_to == Audience::Everyone {
                assert_drops(spec, &author_viewer(), &firing, reason);
            } else {
                assert_allows(spec, &author_viewer(), &firing);
            }
        }
    }

    #[test]
    fn nsfw_media_interstitial_axis() {
        static POLICY: Policy = Policy::new(&[NSFW_MEDIA_INTERSTITIALS]);
        const AT_CUTOFF: u64 = (1705536000000 - 1288834974657) << 22;
        for (label, tweet_id, reason, by) in [
            (
                SafetyLabelType::NSFW_HIGH_PRECISION,
                AT_CUTOFF - 1,
                InterstitialReason::Sensitive(true),
                "NsfwHighPrecisionInterstitialRule",
            ),
            (
                SafetyLabelType::NSFW_HIGH_PRECISION,
                AT_CUTOFF,
                InterstitialReason::Sensitive(true),
                "NsfwHighPrecisionInterstitialRule",
            ),
            (
                SafetyLabelType::NSFW_HIGH_PRECISION,
                AT_CUTOFF + (1 << 22),
                InterstitialReason::Nudity(true),
                "NsfwHighPrecisionAdultInterstitialRule",
            ),
            (
                SafetyLabelType::GORE_AND_VIOLENCE_HIGH_PRECISION,
                1,
                InterstitialReason::Violence(true),
                "GoreAndViolenceInterstitialRule",
            ),
            (
                SafetyLabelType::NSFW_CARD_IMAGE,
                1,
                InterstitialReason::Sensitive(true),
                "NsfwCardImageInterstitialRule",
            ),
        ] {
            let firing = candidate().tweet_id(tweet_id).with_label(label).build();
            let verdict = POLICY.evaluate(&test_context(&viewer(VIEWER_ID), &firing));
            assert_eq!(
                verdict,
                Verdict::Shown {
                    media: Some(Decided {
                        value: MediaInterstitial {
                            legacy: FilteredReason::ContainNsfwMedia,
                            reason
                        },
                        by,
                    }),
                    engagement: None,
                }
            );
            for viewer in [sensitive_opt_in_viewer(), author_viewer()] {
                assert_eq!(
                    POLICY.evaluate(&test_context(&viewer, &firing)),
                    Verdict::Shown {
                        media: None,
                        engagement: None
                    }
                );
            }
        }
        let unrelated = candidate().with_label(UNRELATED_LABEL).build();
        assert_eq!(
            POLICY.evaluate(&test_context(&viewer(VIEWER_ID), &unrelated)),
            Verdict::Shown {
                media: None,
                engagement: None
            }
        );
    }

    fn exclusive_candidate(
        tweet_id: u64,
        author_id: u64,
        root_author_id: u64,
    ) -> HydratedTweetCandidate {
        let mut c = candidate().tweet_id(tweet_id).author_id(author_id).build();
        c.exclusive_content = Some(ExclusiveContentFeatures {
            conversation_author_id: root_author_id,
            viewer_super_follows_author: false,
        });
        c
    }

    #[test]
    fn exclusive_content_axis() {
        let clauses = EXCLUSIVE_TWEET_DROP;
        assert_clauses_allow(clauses, &viewer(VIEWER_ID), &candidate().build());

        let exclusive = exclusive_candidate(1, 100, 100);
        assert_clauses_drop(
            clauses,
            &logged_out_viewer(),
            &exclusive,
            &FilteredReason::ExclusiveTweet,
        );
        assert_clauses_allow(clauses, &author_viewer(), &exclusive);
        assert_clauses_drop(
            clauses,
            &viewer(200),
            &exclusive,
            &FilteredReason::ExclusiveTweet,
        );

        let mut super_follow = exclusive_candidate(1, 100, 100);
        super_follow
            .exclusive_content
            .as_mut()
            .unwrap()
            .viewer_super_follows_author = true;
        assert_clauses_allow(clauses, &viewer(200), &super_follow);
        assert_clauses_drop(
            clauses,
            &logged_out_viewer(),
            &super_follow,
            &FilteredReason::ExclusiveTweet,
        );

        let reply = exclusive_candidate(2, 200, 100);
        assert_clauses_allow(clauses, &viewer(200), &reply);

        let mut retweet = exclusive_candidate(2, 200, 100);
        retweet.tweet_features.source_tweet_id = Some(99);
        assert_clauses_drop(
            clauses,
            &viewer(200),
            &retweet,
            &FilteredReason::ExclusiveTweet,
        );
    }

    const ROOT_AUTHOR_ID: u64 = 4242;

    fn controlled(arm: ConversationControlArm) -> ConversationControlFeatures {
        conversation_control(arm, ROOT_AUTHOR_ID)
    }

    fn limited_by(rule_name: &'static str) -> Verdict {
        Verdict::Shown {
            media: None,
            engagement: Some(Decided {
                value: LimitedEngagement(LimitedEngagementReason::ConversationControl),
                by: rule_name,
            }),
        }
    }

    #[test]
    fn limit_replies_conversation_control_axis() {
        use ConversationControlArm::{ByInvitation, Community, Followers, Subscribers, Verified};
        let clauses = LIMIT_REPLIES_CONVERSATION_RULES;
        let verdict = |viewer: &ViewerFeatures, features: ConversationControlFeatures| {
            let candidate = candidate().with_conversation_control(features).build();
            clauses_verdict(clauses, viewer, &candidate)
        };
        let allow = Verdict::Shown {
            media: None,
            engagement: None,
        };
        let verified = viewer_with_profile(ViewerProfile {
            has_verified_badge: true,
            ..ViewerProfile::default()
        });

        for (arm, rule) in [
            (ByInvitation, "LimitRepliesByInvitationConversationRule"),
            (Community, "LimitRepliesCommunityConversationRule"),
            (Subscribers, "LimitRepliesSubscribersConversationRule"),
            (Verified, "LimitRepliesVerifiedConversationRule"),
        ] {
            assert_eq!(
                verdict(&viewer(VIEWER_ID), controlled(arm)),
                limited_by(rule)
            );
            assert_eq!(verdict(&author_viewer(), controlled(arm)), limited_by(rule));
            assert_eq!(verdict(&viewer(ROOT_AUTHOR_ID), controlled(arm)), allow);
            assert_eq!(verdict(&logged_out_viewer(), controlled(arm)), allow);
            let invited = ConversationControlFeatures {
                control: ConversationControl {
                    invited_user_ids: vec![VIEWER_ID],
                    ..controlled(arm).control
                },
                ..controlled(arm)
            };
            assert_eq!(verdict(&viewer(VIEWER_ID), invited), allow);
            let retweet = candidate()
                .retweet_of(2)
                .with_conversation_control(controlled(arm))
                .build();
            assert_eq!(
                clauses_verdict(clauses, &viewer(VIEWER_ID), &retweet),
                allow
            );
        }

        let followed = ConversationControlFeatures {
            root_author_follows_viewer: Some(true),
            ..controlled(Community)
        };
        assert_eq!(verdict(&viewer(VIEWER_ID), followed), allow);
        let subscribed = ConversationControlFeatures {
            viewer_super_follows_root_author: Some(true),
            ..controlled(Subscribers)
        };
        assert_eq!(verdict(&viewer(VIEWER_ID), subscribed), allow);
        assert_eq!(verdict(&verified, controlled(Verified)), allow);

        assert_eq!(verdict(&viewer(VIEWER_ID), controlled(Followers)), allow);
        assert_clauses_allow(clauses, &viewer(VIEWER_ID), &candidate().build());
    }

    fn gating_viewer(age: ViewerAge) -> ViewerFeatures {
        gating_viewer_with(ViewerProfile {
            viewer_age: age,
            ..ViewerProfile::default()
        })
    }

    fn gating_viewer_with(profile: ViewerProfile) -> ViewerFeatures {
        ViewerFeatures {
            country_code: Some("de".into()),
            ..viewer_with_profile(profile)
        }
    }

    fn media_label(label: SafetyLabelType) -> HydratedTweetCandidate {
        candidate().with_label(label).with_media().build()
    }

    fn no_media_label(label: SafetyLabelType) -> HydratedTweetCandidate {
        let mut c = media_label(label);
        c.tweet_features.media.has_media = false;
        c
    }

    fn nsfw_author_media() -> HydratedTweetCandidate {
        candidate()
            .with_media()
            .with_author_features(AuthorFeatures {
                is_nsfw_user: true,
                ..Default::default()
            })
            .build()
    }

    fn nsfw_tweet_flag_media() -> HydratedTweetCandidate {
        let mut c = nsfw_author_media();
        c.author_features = AuthorFeatures::default();
        c.tweet_features.nsfw = NsfwFeature {
            user: true,
            admin: false,
        };
        c
    }

    fn sensitive_clauses(name: &str) -> &'static [RuleClause] {
        let clauses = SENSITIVE_VIEWER_DROPS;
        let start = clauses
            .iter()
            .position(|clause| clause.rule_name == name)
            .unwrap_or_else(|| panic!("no clause named {name}"));
        let len = clauses[start..]
            .iter()
            .take_while(|clause| clause.rule_name == name)
            .count();
        &clauses[start..start + len]
    }

    fn sensitive_firing_candidates() -> Vec<HydratedTweetCandidate> {
        let mut admin_author = nsfw_author_media();
        admin_author.author_features = AuthorFeatures {
            is_nsfw_admin: true,
            ..Default::default()
        };
        let mut admin_flag = nsfw_tweet_flag_media();
        admin_flag.tweet_features.nsfw = NsfwFeature {
            user: false,
            admin: true,
        };
        let mut both_flags = nsfw_tweet_flag_media();
        both_flags.author_features.is_nsfw_user = true;
        vec![
            media_label(SafetyLabelType::NSFW_HIGH_PRECISION),
            media_label(SafetyLabelType::NSFW_HIGH_RECALL),
            media_label(SafetyLabelType::GORE_AND_VIOLENCE_HIGH_PRECISION),
            no_media_label(SafetyLabelType::NSFW_TEXT),
            no_media_label(SafetyLabelType::NSFW_CARD_IMAGE),
            nsfw_author_media(),
            admin_author,
            nsfw_tweet_flag_media(),
            admin_flag,
            both_flags,
        ]
    }

    #[test]
    fn sensitive_viewer_content_axis() {
        let underage = sensitive_clauses("SensitiveViewerUnderageDropRule");
        let logged_out = sensitive_clauses("SensitiveViewerLoggedOutDropRule");
        let no_age = sensitive_clauses("SensitiveViewerNoStatedAgeDropRule");
        let reason = FilteredReason::ContainNsfwMedia;
        let logged_out_viewer = ViewerFeatures {
            viewer: Viewer::LoggedOut,
            ..gating_viewer(ViewerAge::Unknown)
        };
        for firing in sensitive_firing_candidates() {
            assert_clauses_drop(
                underage,
                &gating_viewer(ViewerAge::Known(15)),
                &firing,
                &reason,
            );
            assert_clauses_drop(logged_out, &logged_out_viewer, &firing, &reason);
            assert_clauses_drop(
                no_age,
                &gating_viewer(ViewerAge::NotStated),
                &firing,
                &reason,
            );
        }
    }

    #[test]
    fn sensitive_viewer_exemption_axis() {
        let underage = sensitive_clauses("SensitiveViewerUnderageDropRule");
        let logged_out = sensitive_clauses("SensitiveViewerLoggedOutDropRule");
        let no_age = sensitive_clauses("SensitiveViewerNoStatedAgeDropRule");
        let hp = media_label(SafetyLabelType::NSFW_HIGH_PRECISION);
        let text = no_media_label(SafetyLabelType::NSFW_TEXT);
        let reason = FilteredReason::ContainNsfwMedia;

        assert_clauses_allow(underage, &gating_viewer(ViewerAge::Known(18)), &hp);
        assert_clauses_allow(underage, &gating_viewer(ViewerAge::Known(18)), &text);
        assert_clauses_allow(underage, &gating_viewer(ViewerAge::Unknown), &hp);
        assert_clauses_allow(no_age, &gating_viewer(ViewerAge::Unknown), &hp);
        assert_clauses_allow(underage, &gating_viewer(ViewerAge::Unknown), &text);
        assert_clauses_allow(no_age, &gating_viewer(ViewerAge::Unknown), &text);

        let opted_in = gating_viewer_with(ViewerProfile {
            allows_sensitive_media: true,
            viewer_age: ViewerAge::Known(15),
            ..ViewerProfile::default()
        });
        assert_clauses_drop(underage, &opted_in, &hp, &reason);

        let mut self_hp = hp.clone();
        self_hp.author_id = VIEWER_ID;
        assert_clauses_allow(underage, &gating_viewer(ViewerAge::Known(15)), &self_hp);
        let mut self_text = text.clone();
        self_text.author_id = VIEWER_ID;
        assert_clauses_allow(underage, &gating_viewer(ViewerAge::Known(15)), &self_text);

        let mut hp_no_media = hp.clone();
        hp_no_media.tweet_features.media.has_media = false;
        assert_clauses_allow(underage, &gating_viewer(ViewerAge::Known(15)), &hp_no_media);
        let logged_out_viewer = ViewerFeatures {
            viewer: Viewer::LoggedOut,
            ..gating_viewer(ViewerAge::Unknown)
        };
        assert_clauses_allow(logged_out, &logged_out_viewer, &hp_no_media);
        assert_clauses_allow(logged_out, &gating_viewer(ViewerAge::Known(15)), &hp);

        let mut no_flags = nsfw_tweet_flag_media();
        no_flags.tweet_features.nsfw = NsfwFeature::default();
        assert_clauses_allow(underage, &gating_viewer(ViewerAge::Known(15)), &no_flags);

        let mut flag_rt = nsfw_tweet_flag_media();
        flag_rt.tweet_features.source_tweet_id = Some(42);
        assert_clauses_allow(underage, &gating_viewer(ViewerAge::Known(15)), &flag_rt);
        let mut flag_self = nsfw_tweet_flag_media();
        flag_self.author_id = VIEWER_ID;
        assert_clauses_allow(underage, &gating_viewer(ViewerAge::Known(15)), &flag_self);

        let mut author_rt = nsfw_author_media();
        author_rt.tweet_features.source_tweet_id = Some(42);
        assert_clauses_allow(underage, &gating_viewer(ViewerAge::Known(15)), &author_rt);
        let mut author_no_media = nsfw_author_media();
        author_no_media.tweet_features.media.has_media = false;
        assert_clauses_allow(
            underage,
            &gating_viewer(ViewerAge::Known(15)),
            &author_no_media,
        );
    }

    fn tes_spec(name: &str) -> &'static RuleClause {
        STALE_TWEET_DROP
            .iter()
            .chain(TAKEDOWN_DROPS)
            .chain(RECS_MEDIA_DROPS)
            .find(|spec| spec.rule_name == name)
            .unwrap_or_else(|| panic!("no TES row {name}"))
    }

    fn takedown_candidate(reasons: Vec<TakedownReason>) -> HydratedTweetCandidate {
        candidate()
            .with_tweet_features(TweetFeatures {
                takedown_reasons: reasons,
                ..Default::default()
            })
            .build()
    }

    fn viewer_with_country(country: &str) -> ViewerFeatures {
        ViewerFeatures {
            country_code: Some(country.to_string()),
            ..viewer(VIEWER_ID)
        }
    }

    fn geo_candidate(allow: &[&str], deny: &[&str]) -> HydratedTweetCandidate {
        candidate()
            .with_tweet_features(TweetFeatures {
                media: MediaFeature {
                    geo_allow_list: allow.iter().map(|s| s.to_string()).collect(),
                    geo_deny_list: deny.iter().map(|s| s.to_string()).collect(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .build()
    }

    #[test]
    fn takedown_country_axis() {
        let legal = tes_spec("DropLegalTakendownPostRule");
        let local = tes_spec("DropLocalLawsTakendownPostRule");
        let reason = FilteredReason::UnspecifiedReason;
        let legal_c = takedown_candidate(vec![
            TakedownReason::LegalRequest {
                country_code: "de".to_string(),
            },
            TakedownReason::UnspecifiedReason {
                country_code: "fr".to_string(),
            },
        ]);
        assert_drops(legal, &viewer_with_country("de"), &legal_c, &reason);
        assert_allows(legal, &viewer_with_country("us"), &legal_c);
        assert_allows(legal, &viewer(VIEWER_ID), &legal_c);

        let bystander = takedown_candidate(vec![TakedownReason::BystanderReport {
            country_code: "de".to_string(),
        }]);
        assert_allows(legal, &viewer_with_country("de"), &bystander);
        assert_drops(local, &viewer_with_country("de"), &bystander, &reason);
        assert_allows(local, &viewer_with_country("us"), &bystander);
        assert_allows(local, &viewer(VIEWER_ID), &bystander);

        let legal_only = takedown_candidate(vec![TakedownReason::LegalRequest {
            country_code: "de".to_string(),
        }]);
        assert_allows(local, &viewer_with_country("de"), &legal_only);

        let mut author_legal = legal_only.clone();
        author_legal.author_id = VIEWER_ID;
        assert_allows(legal, &viewer_with_country("de"), &author_legal);
        let mut author_local = bystander.clone();
        author_local.author_id = VIEWER_ID;
        assert_allows(local, &viewer_with_country("de"), &author_local);

        let non_country = takedown_candidate(vec![
            TakedownReason::HatefulImagery,
            TakedownReason::Unknown,
        ]);
        assert_allows(legal, &viewer_with_country("de"), &non_country);
        assert_allows(local, &viewer_with_country("de"), &non_country);

        let worldwide_upper = takedown_candidate(vec![TakedownReason::LegalRequest {
            country_code: "XX".to_string(),
        }]);
        assert_drops(legal, &viewer_with_country("us"), &worldwide_upper, &reason);
        assert_drops(legal, &viewer(VIEWER_ID), &worldwide_upper, &reason);
        let copyright_upper = takedown_candidate(vec![TakedownReason::LegalRequest {
            country_code: "XY".to_string(),
        }]);
        assert_allows(legal, &viewer_with_country("us"), &copyright_upper);
        assert_allows(legal, &viewer(VIEWER_ID), &copyright_upper);
        let bystander_worldwide_upper = takedown_candidate(vec![TakedownReason::BystanderReport {
            country_code: "XY".to_string(),
        }]);
        assert_allows(
            local,
            &viewer_with_country("us"),
            &bystander_worldwide_upper,
        );
        assert_allows(local, &viewer(VIEWER_ID), &bystander_worldwide_upper);

        let dmca = takedown_candidate(vec![TakedownReason::Dmca]);
        assert_allows(local, &viewer_with_country("de"), &dmca);
        assert_allows(local, &viewer(VIEWER_ID), &dmca);
    }

    #[test]
    fn geo_restricted_media_axis() {
        let spec = tes_spec("DropTweetsWithGeoRestrictedMediaRule");
        let reason = FilteredReason::UnspecifiedReason;
        assert_allows(spec, &viewer_with_country("us"), &geo_candidate(&[], &[]));
        assert_drops(
            spec,
            &viewer_with_country("de"),
            &geo_candidate(&[], &["de", "fr"]),
            &reason,
        );
        assert_allows(
            spec,
            &viewer_with_country("us"),
            &geo_candidate(&[], &["de", "fr"]),
        );
        assert_allows(
            spec,
            &viewer_with_country("us"),
            &geo_candidate(&["us", "gb"], &[]),
        );
        assert_drops(
            spec,
            &viewer_with_country("de"),
            &geo_candidate(&["us", "gb"], &[]),
            &reason,
        );
        assert_allows(
            spec,
            &viewer_with_country("us"),
            &geo_candidate(&["US"], &[]),
        );
        assert_drops(
            spec,
            &viewer_with_country("de"),
            &geo_candidate(&[], &["DE"]),
            &reason,
        );
        assert_drops(
            spec,
            &viewer(VIEWER_ID),
            &geo_candidate(&["us"], &[]),
            &reason,
        );
        assert_drops(
            spec,
            &viewer(VIEWER_ID),
            &geo_candidate(&[], &["xx"]),
            &reason,
        );
        assert_allows(spec, &viewer(VIEWER_ID), &geo_candidate(&[], &["de"]));

        let mut author = geo_candidate(&[], &["de"]);
        author.author_id = VIEWER_ID;
        assert_drops(spec, &viewer_with_country("de"), &author, &reason);

        let mut retweet = geo_candidate(&[], &["de"]);
        retweet.tweet_features.source_tweet_id = Some(99);
        assert_drops(spec, &viewer_with_country("de"), &retweet, &reason);
    }

    #[test]
    fn no_stated_age_jurisdiction_axis() {
        let no_age = sensitive_clauses("SensitiveViewerNoStatedAgeDropRule");
        let hp = media_label(SafetyLabelType::NSFW_HIGH_PRECISION);
        let text = no_media_label(SafetyLabelType::NSFW_TEXT);
        let reason = FilteredReason::ContainNsfwMedia;

        let us = ViewerFeatures {
            country_code: Some("us".into()),
            ..gating_viewer(ViewerAge::NotStated)
        };
        assert_clauses_allow(no_age, &us, &hp);
        assert_clauses_allow(no_age, &us, &text);

        let missing = ViewerFeatures {
            country_code: None,
            ..gating_viewer(ViewerAge::NotStated)
        };
        assert_clauses_allow(no_age, &missing, &hp);

        let account_overrides = gating_viewer_with(ViewerProfile {
            viewer_age: ViewerAge::NotStated,
            account_country_code: Some("us".into()),
            ..ViewerProfile::default()
        });
        assert_clauses_allow(no_age, &account_overrides, &hp);

        let gating_account = ViewerFeatures {
            country_code: Some("us".into()),
            ..gating_viewer_with(ViewerProfile {
                viewer_age: ViewerAge::NotStated,
                account_country_code: Some("kr".into()),
                ..ViewerProfile::default()
            })
        };
        assert_clauses_drop(no_age, &gating_account, &hp, &reason);

        let request_fallback = gating_viewer(ViewerAge::NotStated);
        assert_clauses_drop(no_age, &request_fallback, &hp, &reason);
    }
}
