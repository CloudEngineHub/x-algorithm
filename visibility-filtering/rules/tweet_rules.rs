use crate::hydration::{Hydrator, Hydrators};
use crate::models::{AuthorLabel, LimitedEngagementReason, SafetyLabelType};
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

pub(super) const OON_GORE_DROP: &[RuleClause] = &[RuleClause {
    rule_name: "GoreAndViolenceOonDropRule",
    when: &[label(SafetyLabelType::GORE_AND_VIOLENCE_HIGH_PRECISION)],
    applies_to: Audience::ExceptAuthor,
    action: ActionSpec::Drop(FilteredReason::ContainNsfwMedia),
}];

pub(super) const OON_NSFW_MEDIA_LABEL_DROPS: &[RuleClause] = &[
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
        rule_name: "NsfwCardImageOonDropRule",
        when: &[label(SafetyLabelType::NSFW_CARD_IMAGE)],
        applies_to: Audience::ExceptAuthor,
        action: ActionSpec::Drop(FilteredReason::ContainNsfwMedia),
    },
];

pub(super) const OON_LOW_QUALITY_TWEET_LABEL_DROPS: &[RuleClause] = &[
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
];

pub(super) const OON_TEXT_LABEL_DROPS: &[RuleClause] = &[RuleClause {
    rule_name: "FosnrAbuseInsultsOonDropRule",
    when: &[label(SafetyLabelType::FOSNR_ABUSE_INSULTS)],
    applies_to: Audience::ExceptAuthor,
    action: ActionSpec::Drop(FilteredReason::PossiblyUndesirable),
}];

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
    limit_replies(
        "LimitRepliesMyNetworkConversationRule",
        &[
            has_conversation_control(ConversationControlArm::MyNetwork),
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
        "LimitRepliesCoConversationRule",
        &[
            has_conversation_control(ConversationControlArm::Co),
            NOT_LOGGED_OUT,
            NOT_RETWEET,
            NOT_CONVERSATION_ROOT_AUTHOR,
            NOT_INVITED_TO_CONVERSATION,
            Condition::Not(Predicate::Relationship(
                RelationshipPredicate::ViewerIsInAllowedCountry,
            )),
        ],
    ),
];

pub(super) const BLOCKED_VIEWER_LIMITED_ACTIONS: &[RuleClause] = &[
    RuleClause {
        rule_name: "BlockedViewerLimitedActionsRule",
        when: &[Condition::Holds(Predicate::Relationship(
            RelationshipPredicate::ViewerIsBlockedByAuthor,
        ))],
        applies_to: Audience::ExceptAuthor,
        action: ActionSpec::LimitedEngagement(LimitedEngagementReason::BlockedViewer),
    },
    RuleClause {
        rule_name: "RootAuthorBlocksViewerLimitedActionsRule",
        when: &[Condition::Holds(Predicate::Relationship(
            RelationshipPredicate::ViewerIsBlockedByConversationRootAuthor,
        ))],
        applies_to: Audience::Everyone,
        action: ActionSpec::LimitedEngagement(LimitedEngagementReason::RootAuthorBlockedViewer),
    },
];

pub(super) const READ_ONLY_VIEWER_LIMITED_ACTIONS: &[RuleClause] = &[RuleClause {
    rule_name: "ReadOnlyViewerLimitedActionsRule",
    when: &[Condition::Holds(Predicate::Viewer(
        ViewerPredicate::ReadOnly,
    ))],
    applies_to: Audience::Everyone,
    action: ActionSpec::LimitedEngagement(LimitedEngagementReason::ReadonlyViewer),
}];

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

pub(super) const SENSITIVE_MEDIA_OPT_OUT_DROPS: &[RuleClause] = &[
    sensitive_viewer_drop(
        "NsfwSensitiveViewerDropTweetRule",
        &[
            SENSITIVE_MEDIA_DISABLED,
            Condition::AnyOf(&[
                Predicate::Tweet(TweetPredicate::HasSafetyLabel(
                    SafetyLabelType::NSFW_HIGH_PRECISION,
                )),
                Predicate::Tweet(TweetPredicate::HasSafetyLabel(
                    SafetyLabelType::NSFW_HIGH_RECALL,
                )),
                Predicate::Tweet(TweetPredicate::HasSafetyLabel(SafetyLabelType::NSFW_TEXT)),
                Predicate::Tweet(TweetPredicate::HasSafetyLabel(
                    SafetyLabelType::NSFW_TEXT_HIGH_PRECISION,
                )),
                Predicate::Tweet(TweetPredicate::HasSafetyLabel(SafetyLabelType::NSFW_VIDEO)),
                Predicate::Tweet(TweetPredicate::NsfwAdminFlag),
                Predicate::Tweet(TweetPredicate::NsfwUserFlag),
            ]),
        ],
    ),
    sensitive_viewer_drop(
        "NsfwSensitiveViewerDropUserRule",
        &[
            SENSITIVE_MEDIA_DISABLED,
            Condition::AnyOf(&[
                Predicate::Author(AuthorPredicate::HasUserLabel(AuthorLabel::NsfwAvatarImage)),
                Predicate::Author(AuthorPredicate::HasUserLabel(AuthorLabel::NsfwBannerImage)),
                Predicate::Author(AuthorPredicate::HasUserLabel(
                    AuthorLabel::NsfwHighPrecision,
                )),
                Predicate::Author(AuthorPredicate::HasUserLabel(AuthorLabel::NsfwHighRecall)),
                Predicate::Author(AuthorPredicate::HasUserLabel(AuthorLabel::NsfwNearPerfect)),
                Predicate::Author(AuthorPredicate::IsNsfwAdmin),
                Predicate::Author(AuthorPredicate::IsNsfwUser),
            ]),
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
                    .legal_takedown_in(context.facts().request_country())
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
                    .local_laws_takedown_in(context.facts().request_country())
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
                    .media_restricted_in(context.facts().request_country())
            },
            hydrators: Hydrators::of(Hydrator::Tweet),
        }],
        applies_to: Audience::Everyone,
        action: ActionSpec::Drop(FilteredReason::UnspecifiedReason),
    },
];
