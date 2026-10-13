use crate::models::{LimitedEngagementReason, SafetyLabelType, TombstoneReason, VerifyBlurSupport};
use crate::params::{CountryList, LimitedActionType};
use crate::rules::rule_spec::conditions::*;
use crate::rules::rule_spec::Audience::{Everyone, NonAuthors, NonFollowers, OnlyAuthor};
use crate::rules::rule_spec::*;
use xai_core_entities::entities::ConversationControlArm;
use xai_x_thrift::action::{AppealablePolicy, InterstitialReason};

pub(super) const FILTER_ALL_DROP_UNSPECIFIED: Clause = Clause {
    id: RuleId::FilterAll,
    when: &[],
    audience: Everyone,
    action: drop_post(DropAs::UnspecifiedReason),
};

pub(super) const PDNA_DROP_SAFETY_RESULT: Clause = Clause {
    id: RuleId::Pdna,
    when: &[tweet_has_label(SafetyLabelType::PDNA)],
    audience: NonAuthors,
    action: drop_post(DropAs::NsfwHighPrecisionSafetyResult),
};

pub(super) const BOUNCE_DROP_BOUNCED: Clause = Clause {
    id: RuleId::Bounce,
    when: &[tweet_has_label(SafetyLabelType::BOUNCE)],
    audience: NonAuthors,
    action: drop_post(DropAs::TweetIsBounced),
};

pub(super) const SPAM_DROP_UNDESIRABLE: Clause = Clause {
    id: RuleId::Spam,
    when: &[tweet_has_label(SafetyLabelType::SPAM)],
    audience: NonAuthors,
    action: drop_post(DropAs::PossiblyUndesirable),
};

pub(super) const FOR_EMERGENCY_USE_ONLY_DROP_UNSPECIFIED: Clause = Clause {
    id: RuleId::ForEmergencyUseOnly,
    when: &[tweet_has_label(SafetyLabelType::FOR_EMERGENCY_USE_ONLY)],
    audience: Everyone,
    action: drop_post(DropAs::UnspecifiedReason),
};

const AUTHOR_LEVEL_3_LIMITED_ACTIONS: [LimitedActionType; 13] = {
    use LimitedActionType as A;
    [
        A::Like,
        A::Reply,
        A::Retweet,
        A::QuoteTweet,
        A::ShareTweetVia,
        A::AddToBookmarks,
        A::PinToProfile,
        A::CopyLink,
        A::SendViaDm,
        A::EditTweet,
        A::Highlight,
        A::Embed,
        A::ListsAddRemove,
    ]
};

pub(super) const FOSNR_AUTHOR_VIOLATIONS: [FosnrViolation; 5] = [
    FosnrViolation {
        label: SafetyLabelType::FOSNR_ABUSE,
        policy: AppealablePolicy::ABUSE,
        level: 3,
        limited_actions: &AUTHOR_LEVEL_3_LIMITED_ACTIONS,
    },
    FosnrViolation {
        label: SafetyLabelType::FOSNR_CIVIC_INTEGRITY,
        policy: AppealablePolicy::CIVIC_INTEGRITY,
        level: 3,
        limited_actions: &AUTHOR_LEVEL_3_LIMITED_ACTIONS,
    },
    FosnrViolation {
        label: SafetyLabelType::FOSNR_HATEFUL_CONDUCT,
        policy: AppealablePolicy::HATEFUL_CONDUCT,
        level: 3,
        limited_actions: &AUTHOR_LEVEL_3_LIMITED_ACTIONS,
    },
    FosnrViolation {
        label: SafetyLabelType::FOSNR_VIOLENT_SPEECH,
        policy: AppealablePolicy::VIOLENT_SPEECH,
        level: 3,
        limited_actions: &AUTHOR_LEVEL_3_LIMITED_ACTIONS,
    },
    FosnrViolation {
        label: SafetyLabelType::FOSNR_ABUSE_INSULTS,
        policy: AppealablePolicy::ABUSE,
        level: 1,
        limited_actions: &[LimitedActionType::EditTweet],
    },
];

pub(super) const FOSNR_HATEFUL_CONDUCT_DROP_UNDESIRABLE: Clause = Clause {
    id: RuleId::FosnrHatefulConduct,
    when: &[tweet_has_label(SafetyLabelType::FOSNR_HATEFUL_CONDUCT)],
    audience: NonAuthors,
    action: drop_post(DropAs::PossiblyUndesirable),
};

pub(super) const FOSNR_VIOLENT_SPEECH_DROP_UNDESIRABLE: Clause = Clause {
    id: RuleId::FosnrViolentSpeech,
    when: &[tweet_has_label(SafetyLabelType::FOSNR_VIOLENT_SPEECH)],
    audience: NonAuthors,
    action: drop_post(DropAs::PossiblyUndesirable),
};

pub(super) const FOSNR_ABUSE_DROP_UNDESIRABLE: Clause = Clause {
    id: RuleId::FosnrAbuse,
    when: &[tweet_has_label(SafetyLabelType::FOSNR_ABUSE)],
    audience: NonAuthors,
    action: drop_post(DropAs::PossiblyUndesirable),
};

pub(super) const FOSNR_CIVIC_INTEGRITY_DROP_UNDESIRABLE: Clause = Clause {
    id: RuleId::FosnrCivicIntegrity,
    when: &[tweet_has_label(SafetyLabelType::FOSNR_CIVIC_INTEGRITY)],
    audience: NonAuthors,
    action: drop_post(DropAs::PossiblyUndesirable),
};

pub(super) const FOSNR_ABUSE_INSULTS_DROP_UNDESIRABLE: Clause = Clause {
    id: RuleId::FosnrAbuseInsults,
    when: &[tweet_has_label(SafetyLabelType::FOSNR_ABUSE_INSULTS)],
    audience: NonAuthors,
    action: drop_post(DropAs::PossiblyUndesirable),
};

pub(super) const FOSNR_ABUSE_INSULTS_NON_FOLLOWER_DROP_UNDESIRABLE: Clause = Clause {
    id: RuleId::FosnrAbuseInsultsNonFollower,
    when: &[
        tweet_has_label(SafetyLabelType::FOSNR_ABUSE_INSULTS),
        client_has_fosnr_rules(),
    ],
    audience: NonFollowers,
    action: drop_post(DropAs::PossiblyUndesirable),
};

pub(super) const FOSNR_ABUSE_INSULTS_FOLLOWER_SOFT_INTERVENTION_ABUSE: Clause = Clause {
    id: RuleId::FosnrAbuseInsultsFollower,
    when: &[
        tweet_has_label(SafetyLabelType::FOSNR_ABUSE_INSULTS),
        viewer_follows_author(),
        client_has_fosnr_rules(),
    ],
    audience: NonAuthors,
    action: soft_intervention(
        SafetyLabelType::FOSNR_ABUSE_INSULTS,
        AppealablePolicy::ABUSE,
        1,
    ),
};

pub(super) const FOSNR_AUTHOR_APPEALABLE: Clause = Clause {
    id: RuleId::FosnrAuthor,
    when: &[client_has_fosnr_rules(), has_fosnr_label()],
    audience: OnlyAuthor,
    action: appealable(&FOSNR_AUTHOR_VIOLATIONS),
};

pub(super) const FOSNR_FALLBACK_DROP_UNDESIRABLE: Clause = Clause {
    id: RuleId::FosnrFallback,
    when: &[client_needs_fosnr_fallback_drops(), has_fosnr_label()],
    audience: Everyone,
    action: drop_post(DropAs::PossiblyUndesirable),
};

pub(super) const NULLCASTED_TWEET_DROP: Clause = Clause {
    id: RuleId::NullcastedTweet,
    when: &[
        tweet_is_nullcast(),
        not(tweet_is_retweet()),
        not(tweet_is_community_tweet()),
    ],
    audience: Everyone,
    action: drop_post(DropAs::TweetIsNullcast),
};

pub(super) const STALE_TWEET_DROP_UNSPECIFIED: Clause = Clause {
    id: RuleId::StaleTweet,
    when: &[tweet_is_superseded_edit(), not(tweet_is_retweet())],
    audience: Everyone,
    action: drop_post(DropAs::UnspecifiedReason),
};

pub(super) const STALE_TWEET_LIMITED_ENGAGEMENT: Clause = Clause {
    id: RuleId::StaleTweet,
    when: &[tweet_is_superseded_edit(), client_has_stale_tweet_limits()],
    audience: Everyone,
    action: limit(LimitedEngagementReason::StaleTweet),
};

pub(super) const LEGAL_TAKEDOWN_DROP_UNSPECIFIED: Clause = Clause {
    id: RuleId::LegalTakedown,
    when: &[tweet_legal_takedown_in_request_country()],
    audience: NonAuthors,
    action: drop_post(DropAs::UnspecifiedReason),
};

pub(super) const LOCAL_LAWS_TAKEDOWN_DROP_UNSPECIFIED: Clause = Clause {
    id: RuleId::LocalLawsTakedown,
    when: &[tweet_local_laws_takedown_in_request_country()],
    audience: NonAuthors,
    action: drop_post(DropAs::UnspecifiedReason),
};

pub(super) const ARTICLE_TWEET_CONTENT_DROP_UNSPECIFIED: Clause = Clause {
    id: RuleId::ArticleTweetContent,
    when: &[tweet_has_article(), not(tweet_article_is_published())],
    audience: Everyone,
    action: drop_post(DropAs::UnspecifiedReason),
};

pub(super) const PROTECTED_COMMUNITY_TWEET_DROP_UNSPECIFIED: Clause = Clause {
    id: RuleId::ProtectedCommunityTweet,
    when: &[tweet_is_community_tweet(), author_is_protected()],
    audience: NonAuthors,
    action: drop_post(DropAs::UnspecifiedReason),
};

pub(super) const HIDDEN_COMMUNITY_TWEET_DROP_UNSPECIFIED: Clause = Clause {
    id: RuleId::HiddenCommunityTweet,
    when: &[
        tweet_is_community_tweet(),
        tweet_community_tweet_is_hidden(),
        not(viewer_is_community_moderator()),
    ],
    audience: NonAuthors,
    action: drop_post(DropAs::UnspecifiedReason),
};

pub(super) const AUTHOR_REMOVED_COMMUNITY_TWEET_DROP_UNSPECIFIED: Clause = Clause {
    id: RuleId::AuthorRemovedCommunityTweet,
    when: &[
        tweet_is_community_tweet(),
        tweet_community_tweet_author_is_removed(),
        not(viewer_is_community_moderator()),
    ],
    audience: NonAuthors,
    action: drop_post(DropAs::UnspecifiedReason),
};

pub(super) const SENSITIVE_VIEWER_LOGGED_OUT_DROP_MEDIA_LABEL: Clause = Clause {
    id: RuleId::SensitiveViewerLoggedOut,
    when: &[
        viewer_logged_out(),
        tweet_has_media(),
        has_nsfw_media_label(),
    ],
    audience: NonAuthors,
    action: drop_post(DropAs::NsfwViewerLoggedOut),
};

pub(super) const SENSITIVE_VIEWER_LOGGED_OUT_DROP_NSFW_FLAG: Clause = Clause {
    id: RuleId::SensitiveViewerLoggedOut,
    when: &[
        viewer_logged_out(),
        tweet_has_media(),
        not(tweet_is_retweet()),
        is_nsfw_flagged(),
    ],
    audience: NonAuthors,
    action: drop_post(DropAs::NsfwViewerLoggedOut),
};

pub(super) const SENSITIVE_VIEWER_LOGGED_OUT_DROP_TEXT_OR_CARD_LABEL: Clause = Clause {
    id: RuleId::SensitiveViewerLoggedOut,
    when: &[viewer_logged_out(), has_nsfw_text_or_card_label()],
    audience: NonAuthors,
    action: drop_post(DropAs::NsfwViewerLoggedOut),
};

pub(super) const SENSITIVE_VIEWER_UNDERAGE_DROP_MEDIA_LABEL: Clause = Clause {
    id: RuleId::SensitiveViewerUnderage,
    when: &[viewer_underage(), tweet_has_media(), has_nsfw_media_label()],
    audience: NonAuthors,
    action: drop_post(DropAs::NsfwViewerIsUnderage),
};

pub(super) const SENSITIVE_VIEWER_UNDERAGE_DROP_NSFW_FLAG: Clause = Clause {
    id: RuleId::SensitiveViewerUnderage,
    when: &[
        viewer_underage(),
        tweet_has_media(),
        not(tweet_is_retweet()),
        is_nsfw_flagged(),
    ],
    audience: NonAuthors,
    action: drop_post(DropAs::NsfwViewerIsUnderage),
};

pub(super) const SENSITIVE_VIEWER_UNDERAGE_DROP_TEXT_OR_CARD_LABEL: Clause = Clause {
    id: RuleId::SensitiveViewerUnderage,
    when: &[viewer_underage(), has_nsfw_text_or_card_label()],
    audience: NonAuthors,
    action: drop_post(DropAs::NsfwViewerIsUnderage),
};

pub(super) const SENSITIVE_VIEWER_NO_STATED_AGE_DROP_MEDIA_LABEL: Clause = Clause {
    id: RuleId::SensitiveViewerNoStatedAge,
    when: &[
        viewer_no_stated_age(),
        viewer_account_or_request_country_in(CountryList::NsfwGating),
        tweet_has_media(),
        has_nsfw_media_label(),
    ],
    audience: NonAuthors,
    action: drop_post(DropAs::NsfwViewerHasNoStatedAge),
};

pub(super) const SENSITIVE_VIEWER_NO_STATED_AGE_DROP_NSFW_FLAG: Clause = Clause {
    id: RuleId::SensitiveViewerNoStatedAge,
    when: &[
        viewer_no_stated_age(),
        viewer_account_or_request_country_in(CountryList::NsfwGating),
        tweet_has_media(),
        not(tweet_is_retweet()),
        is_nsfw_flagged(),
    ],
    audience: NonAuthors,
    action: drop_post(DropAs::NsfwViewerHasNoStatedAge),
};

pub(super) const SENSITIVE_VIEWER_NO_STATED_AGE_DROP_TEXT_OR_CARD_LABEL: Clause = Clause {
    id: RuleId::SensitiveViewerNoStatedAge,
    when: &[
        viewer_no_stated_age(),
        viewer_account_or_request_country_in(CountryList::NsfwGating),
        has_nsfw_text_or_card_label(),
    ],
    audience: NonAuthors,
    action: drop_post(DropAs::NsfwViewerHasNoStatedAge),
};

pub(super) const NSFW_SENSITIVE_VIEWER_TWEET_DROP_NSFW_MEDIA: Clause = Clause {
    id: RuleId::NsfwSensitiveViewerTweet,
    when: &[
        not(viewer_allows_sensitive_media()),
        is_nsfw_sensitive_tweet(),
    ],
    audience: NonAuthors,
    action: drop_post(DropAs::ContainNsfwMedia),
};

pub(super) const NSFW_SENSITIVE_VIEWER_USER_DROP_NSFW_MEDIA: Clause = Clause {
    id: RuleId::NsfwSensitiveViewerUser,
    when: &[
        not(viewer_allows_sensitive_media()),
        is_nsfw_sensitive_author(),
    ],
    audience: NonAuthors,
    action: drop_post(DropAs::ContainNsfwMedia),
};

pub(super) const EXCLUSIVE_TWEET_DROP_LOGGED_OUT: Clause = Clause {
    id: RuleId::ExclusiveTweet,
    when: &[tweet_has_exclusive_content(), viewer_logged_out()],
    audience: Everyone,
    action: drop_post(DropAs::ExclusiveTweet),
};

pub(super) const EXCLUSIVE_TWEET_DROP_RETWEET: Clause = Clause {
    id: RuleId::ExclusiveTweet,
    when: &[
        tweet_has_exclusive_content(),
        not(viewer_is_conversation_author()),
        not(viewer_super_follows_author()),
        tweet_is_retweet(),
    ],
    audience: Everyone,
    action: drop_post(DropAs::ExclusiveTweet),
};

pub(super) const EXCLUSIVE_TWEET_DROP_NON_SUPER_FOLLOWER: Clause = Clause {
    id: RuleId::ExclusiveTweet,
    when: &[
        tweet_has_exclusive_content(),
        not(viewer_is_conversation_author()),
        not(viewer_super_follows_author()),
    ],
    audience: NonAuthors,
    action: drop_post(DropAs::ExclusiveTweet),
};

pub(super) const AUTHOR_BLOCKS_VIEWER_EXCLUSIVE_CONTENT_DROP_UNSPECIFIED: Clause = Clause {
    id: RuleId::AuthorBlocksViewerExclusiveContent,
    when: &[viewer_is_blocked_by_author(), tweet_has_exclusive_content()],
    audience: NonAuthors,
    action: drop_post(DropAs::UnspecifiedReason),
};

pub(super) const CREATOR_TWEET_NSFW_DROP_NSFW_MEDIA: Clause = Clause {
    id: RuleId::CreatorTweetNsfw,
    when: &[
        tweet_has_media(),
        tweet_has_label(SafetyLabelType::NSFW_HIGH_PRECISION),
        tweet_has_exclusive_content(),
    ],
    audience: NonAuthors,
    action: drop_post(DropAs::ContainNsfwMedia),
};

pub(super) const TRUSTED_FRIENDS_TWEET_DROP_UNSPECIFIED: Clause = Clause {
    id: RuleId::TrustedFriendsTweet,
    when: &[
        tweet_is_trusted_friends_tweet(),
        not(viewer_is_trusted_friends_list_member_or_owner()),
    ],
    audience: NonAuthors,
    action: drop_post(DropAs::UnspecifiedReason),
};

const NSFW_HIGH_PRECISION_CHANGED_AT: u64 = 1705536000000;

pub(super) const NSFW_HIGH_PRECISION_BLUR_NUDITY: Clause = Clause {
    id: RuleId::NsfwHighPrecision,
    when: &[
        tweet_has_label(SafetyLabelType::NSFW_HIGH_PRECISION),
        tweet_created_after(NSFW_HIGH_PRECISION_CHANGED_AT),
        not(viewer_allows_sensitive_media()),
    ],
    audience: NonAuthors,
    action: blur(InterstitialReason::Nudity(true)),
};

pub(super) const NSFW_HIGH_PRECISION_BLUR_SENSITIVE: Clause = Clause {
    id: RuleId::NsfwHighPrecision,
    when: &[
        tweet_has_label(SafetyLabelType::NSFW_HIGH_PRECISION),
        tweet_created_before(NSFW_HIGH_PRECISION_CHANGED_AT),
        not(viewer_allows_sensitive_media()),
    ],
    audience: NonAuthors,
    action: blur(InterstitialReason::Sensitive(true)),
};

pub(super) const NSFW_HIGH_PRECISION_DROP_NSFW_MEDIA: Clause = Clause {
    id: RuleId::NsfwHighPrecision,
    when: &[tweet_has_label(SafetyLabelType::NSFW_HIGH_PRECISION)],
    audience: NonAuthors,
    action: drop_post(DropAs::ContainNsfwMedia),
};

pub(super) const NSFW_HIGH_PRECISION_TOMBSTONE_LOCAL_REGULATIONS: Clause = Clause {
    id: RuleId::NsfwHighPrecision,
    when: &[
        tweet_has_label(SafetyLabelType::NSFW_HIGH_PRECISION),
        request_country_in(CountryList::LocalRegulations),
    ],
    audience: NonAuthors,
    action: tombstone(TombstoneReason::LocalRegulations),
};

pub(super) const NSFW_HIGH_PRECISION_TOMBSTONE_AGE_VERIFICATION: Clause = Clause {
    id: RuleId::NsfwHighPrecision,
    when: &[
        tweet_has_label(SafetyLabelType::NSFW_HIGH_PRECISION),
        client_verify_blur_support_is(VerifyBlurSupport::Unsupported),
        request_country_in(CountryList::Tombstone),
        not(viewer_age_verified()),
    ],
    audience: NonAuthors,
    action: tombstone(TombstoneReason::SensitiveViewerAgeVerification),
};

pub(super) const NSFW_HIGH_PRECISION_TOMBSTONE_UPDATE_APP_IOS: Clause = Clause {
    id: RuleId::NsfwHighPrecision,
    when: &[
        tweet_has_label(SafetyLabelType::NSFW_HIGH_PRECISION),
        client_verify_blur_support_is(VerifyBlurSupport::IosNeedsUpdate),
        request_country_in(CountryList::Tombstone),
        not(viewer_age_verified()),
    ],
    audience: NonAuthors,
    action: tombstone(TombstoneReason::UpdateAppIos),
};

pub(super) const NSFW_HIGH_PRECISION_TOMBSTONE_UPDATE_APP_ANDROID: Clause = Clause {
    id: RuleId::NsfwHighPrecision,
    when: &[
        tweet_has_label(SafetyLabelType::NSFW_HIGH_PRECISION),
        client_verify_blur_support_is(VerifyBlurSupport::AndroidNeedsUpdate),
        request_country_in(CountryList::Tombstone),
        not(viewer_age_verified()),
    ],
    audience: NonAuthors,
    action: tombstone(TombstoneReason::UpdateAppAndroid),
};

pub(super) const NSFW_HIGH_PRECISION_BLUR_SENSITIVE_AGE_PROMPT: Clause = Clause {
    id: RuleId::NsfwHighPrecision,
    when: &[
        tweet_has_label(SafetyLabelType::NSFW_HIGH_PRECISION),
        tweet_created_before(NSFW_HIGH_PRECISION_CHANGED_AT),
        client_verify_blur_support_is(VerifyBlurSupport::Supported),
        request_country_in(CountryList::AgeVerification),
        not(viewer_age_verified()),
    ],
    audience: NonAuthors,
    action: blur_with_age_prompt(InterstitialReason::Sensitive(true)),
};

pub(super) const NSFW_HIGH_PRECISION_BLUR_SENSITIVE_ALL_USERS: Clause = Clause {
    id: RuleId::NsfwHighPrecision,
    when: &[
        tweet_has_label(SafetyLabelType::NSFW_HIGH_PRECISION),
        tweet_created_before(NSFW_HIGH_PRECISION_CHANGED_AT),
        not(viewer_allows_sensitive_media()),
        client_has_modern_blur(),
    ],
    audience: Everyone,
    action: blur(InterstitialReason::Sensitive(true)),
};

pub(super) const NSFW_HIGH_PRECISION_BLUR_NUDITY_AGE_PROMPT: Clause = Clause {
    id: RuleId::NsfwHighPrecision,
    when: &[
        tweet_has_label(SafetyLabelType::NSFW_HIGH_PRECISION),
        tweet_created_after(NSFW_HIGH_PRECISION_CHANGED_AT),
        client_verify_blur_support_is(VerifyBlurSupport::Supported),
        request_country_in(CountryList::AgeVerification),
        not(viewer_age_verified()),
    ],
    audience: NonAuthors,
    action: blur_with_age_prompt(InterstitialReason::Nudity(true)),
};

pub(super) const NSFW_HIGH_PRECISION_BLUR_NUDITY_ALL_USERS: Clause = Clause {
    id: RuleId::NsfwHighPrecision,
    when: &[
        tweet_has_label(SafetyLabelType::NSFW_HIGH_PRECISION),
        tweet_created_after(NSFW_HIGH_PRECISION_CHANGED_AT),
        not(viewer_allows_sensitive_media()),
        client_has_modern_blur(),
    ],
    audience: Everyone,
    action: blur(InterstitialReason::Nudity(true)),
};

pub(super) const NSFW_HIGH_PRECISION_LEGACY_INTERSTITIAL: Clause = Clause {
    id: RuleId::NsfwHighPrecision,
    when: &[
        tweet_has_label(SafetyLabelType::NSFW_HIGH_PRECISION),
        is_client_deprecated(),
    ],
    audience: Everyone,
    action: legacy_nsfw_interstitial(),
};

pub(super) const GORE_AND_VIOLENCE_HIGH_PRECISION_BLUR: Clause = Clause {
    id: RuleId::GoreAndViolenceHighPrecision,
    when: &[
        tweet_has_label(SafetyLabelType::GORE_AND_VIOLENCE_HIGH_PRECISION),
        not(viewer_allows_sensitive_media()),
    ],
    audience: NonAuthors,
    action: blur(InterstitialReason::Violence(true)),
};

pub(super) const GORE_AND_VIOLENCE_HIGH_PRECISION_DROP_NSFW_MEDIA: Clause = Clause {
    id: RuleId::GoreAndViolenceHighPrecision,
    when: &[tweet_has_label(
        SafetyLabelType::GORE_AND_VIOLENCE_HIGH_PRECISION,
    )],
    audience: NonAuthors,
    action: drop_post(DropAs::ContainNsfwMedia),
};

pub(super) const GORE_AND_VIOLENCE_HIGH_PRECISION_TOMBSTONE_UPDATE_APP_IOS: Clause = Clause {
    id: RuleId::GoreAndViolenceHighPrecision,
    when: &[
        tweet_has_label(SafetyLabelType::GORE_AND_VIOLENCE_HIGH_PRECISION),
        client_verify_blur_support_is(VerifyBlurSupport::IosNeedsUpdate),
        request_country_in(CountryList::Tombstone),
        not(viewer_age_verified()),
    ],
    audience: NonAuthors,
    action: tombstone(TombstoneReason::UpdateAppIos),
};

pub(super) const GORE_AND_VIOLENCE_HIGH_PRECISION_TOMBSTONE_UPDATE_APP_ANDROID: Clause = Clause {
    id: RuleId::GoreAndViolenceHighPrecision,
    when: &[
        tweet_has_label(SafetyLabelType::GORE_AND_VIOLENCE_HIGH_PRECISION),
        client_verify_blur_support_is(VerifyBlurSupport::AndroidNeedsUpdate),
        request_country_in(CountryList::Tombstone),
        not(viewer_age_verified()),
    ],
    audience: NonAuthors,
    action: tombstone(TombstoneReason::UpdateAppAndroid),
};

pub(super) const GORE_AND_VIOLENCE_HIGH_PRECISION_BLUR_ALL_USERS: Clause = Clause {
    id: RuleId::GoreAndViolenceHighPrecision,
    when: &[
        tweet_has_label(SafetyLabelType::GORE_AND_VIOLENCE_HIGH_PRECISION),
        not(viewer_allows_sensitive_media()),
        client_has_modern_blur(),
    ],
    audience: Everyone,
    action: blur(InterstitialReason::Violence(true)),
};

pub(super) const GORE_AND_VIOLENCE_HIGH_PRECISION_LEGACY_INTERSTITIAL: Clause = Clause {
    id: RuleId::GoreAndViolenceHighPrecision,
    when: &[
        tweet_has_label(SafetyLabelType::GORE_AND_VIOLENCE_HIGH_PRECISION),
        is_client_deprecated(),
    ],
    audience: Everyone,
    action: legacy_nsfw_interstitial(),
};

pub(super) const GORE_AND_VIOLENCE_HIGH_PRECISION_BLUR_AGE_PROMPT: Clause = Clause {
    id: RuleId::GoreAndViolenceHighPrecision,
    when: &[
        tweet_has_label(SafetyLabelType::GORE_AND_VIOLENCE_HIGH_PRECISION),
        client_verify_blur_support_is(VerifyBlurSupport::Supported),
        request_country_in(CountryList::AgeVerification),
        not(viewer_age_verified()),
    ],
    audience: NonAuthors,
    action: blur_with_age_prompt(InterstitialReason::Violence(true)),
};

pub(super) const GORE_AND_VIOLENCE_IGNORING_SETTINGS_BLUR: Clause = Clause {
    id: RuleId::GoreAndViolenceIgnoringSettings,
    when: &[
        tweet_has_label(SafetyLabelType::GORE_AND_VIOLENCE_HIGH_PRECISION),
        client_blurs_gore_ignoring_settings(),
    ],
    audience: NonAuthors,
    action: blur(InterstitialReason::Violence(true)),
};

pub(super) const NSFW_CARD_IMAGE_BLUR_SENSITIVE: Clause = Clause {
    id: RuleId::NsfwCardImage,
    when: &[
        tweet_has_label(SafetyLabelType::NSFW_CARD_IMAGE),
        not(viewer_allows_sensitive_media()),
    ],
    audience: NonAuthors,
    action: blur(InterstitialReason::Sensitive(true)),
};

pub(super) const NSFW_CARD_IMAGE_DROP_NSFW_MEDIA: Clause = Clause {
    id: RuleId::NsfwCardImage,
    when: &[tweet_has_label(SafetyLabelType::NSFW_CARD_IMAGE)],
    audience: NonAuthors,
    action: drop_post(DropAs::ContainNsfwMedia),
};

pub(super) const NSFW_CARD_IMAGE_TOMBSTONE_AGE_VERIFICATION: Clause = Clause {
    id: RuleId::NsfwCardImage,
    when: &[
        tweet_has_label(SafetyLabelType::NSFW_CARD_IMAGE),
        client_verify_blur_support_is(VerifyBlurSupport::Unsupported),
        request_country_in(CountryList::AgeVerification),
        not(viewer_age_verified()),
    ],
    audience: NonAuthors,
    action: tombstone(TombstoneReason::SensitiveViewerAgeVerification),
};

pub(super) const NSFW_CARD_IMAGE_TOMBSTONE_UPDATE_APP_IOS: Clause = Clause {
    id: RuleId::NsfwCardImage,
    when: &[
        tweet_has_label(SafetyLabelType::NSFW_CARD_IMAGE),
        client_verify_blur_support_is(VerifyBlurSupport::IosNeedsUpdate),
        request_country_in(CountryList::AgeVerification),
        not(viewer_age_verified()),
    ],
    audience: NonAuthors,
    action: tombstone(TombstoneReason::UpdateAppIos),
};

pub(super) const NSFW_CARD_IMAGE_TOMBSTONE_UPDATE_APP_ANDROID: Clause = Clause {
    id: RuleId::NsfwCardImage,
    when: &[
        tweet_has_label(SafetyLabelType::NSFW_CARD_IMAGE),
        client_verify_blur_support_is(VerifyBlurSupport::AndroidNeedsUpdate),
        request_country_in(CountryList::AgeVerification),
        not(viewer_age_verified()),
    ],
    audience: NonAuthors,
    action: tombstone(TombstoneReason::UpdateAppAndroid),
};

pub(super) const NSFW_CARD_IMAGE_BLUR_SENSITIVE_AGE_PROMPT: Clause = Clause {
    id: RuleId::NsfwCardImage,
    when: &[
        tweet_has_label(SafetyLabelType::NSFW_CARD_IMAGE),
        client_verify_blur_support_is(VerifyBlurSupport::Supported),
        request_country_in(CountryList::AgeVerification),
        not(viewer_age_verified()),
    ],
    audience: NonAuthors,
    action: blur_with_age_prompt(InterstitialReason::Sensitive(true)),
};

pub(super) const NSFW_CARD_IMAGE_BLUR_SENSITIVE_ALL_USERS: Clause = Clause {
    id: RuleId::NsfwCardImage,
    when: &[
        tweet_has_label(SafetyLabelType::NSFW_CARD_IMAGE),
        not(viewer_allows_sensitive_media()),
        client_has_modern_blur(),
    ],
    audience: Everyone,
    action: blur(InterstitialReason::Sensitive(true)),
};

pub(super) const NSFW_CARD_IMAGE_LEGACY_INTERSTITIAL: Clause = Clause {
    id: RuleId::NsfwCardImage,
    when: &[
        tweet_has_label(SafetyLabelType::NSFW_CARD_IMAGE),
        is_client_deprecated(),
    ],
    audience: Everyone,
    action: legacy_nsfw_interstitial(),
};

pub(super) const NSFW_ADMIN_BLUR_SENSITIVE: Clause = Clause {
    id: RuleId::NsfwAdmin,
    when: &[
        is_nsfw_admin(),
        tweet_has_media(),
        not(viewer_allows_sensitive_media()),
    ],
    audience: NonAuthors,
    action: blur(InterstitialReason::Sensitive(true)),
};

pub(super) const NSFW_ADMIN_BLUR_SENSITIVE_AGE_PROMPT: Clause = Clause {
    id: RuleId::NsfwAdmin,
    when: &[
        tweet_nsfw_admin_flag(),
        tweet_has_media(),
        client_verify_blur_support_is(VerifyBlurSupport::Supported),
        request_country_in(CountryList::AgeVerification),
        not(viewer_age_verified()),
    ],
    audience: NonAuthors,
    action: blur_with_age_prompt(InterstitialReason::Sensitive(true)),
};

pub(super) const NSFW_ADMIN_BLUR_SENSITIVE_ALL_USERS: Clause = Clause {
    id: RuleId::NsfwAdmin,
    when: &[
        tweet_nsfw_admin_flag(),
        tweet_has_media(),
        not(viewer_allows_sensitive_media()),
        client_has_modern_blur(),
    ],
    audience: Everyone,
    action: blur(InterstitialReason::Sensitive(true)),
};

pub(super) const NSFW_USER_BLUR_SENSITIVE_USER: Clause = Clause {
    id: RuleId::NsfwUser,
    when: &[
        is_nsfw_user(),
        tweet_has_media(),
        not(viewer_allows_sensitive_media()),
    ],
    audience: NonAuthors,
    action: blur(InterstitialReason::SensitiveUser(true)),
};

pub(super) const NSFW_USER_BLUR_SENSITIVE_USER_AGE_PROMPT: Clause = Clause {
    id: RuleId::NsfwUser,
    when: &[
        tweet_nsfw_user_flag(),
        tweet_has_media(),
        client_verify_blur_support_is(VerifyBlurSupport::Supported),
        request_country_in(CountryList::AgeVerification),
        not(viewer_age_verified()),
    ],
    audience: NonAuthors,
    action: blur_with_age_prompt(InterstitialReason::SensitiveUser(true)),
};

pub(super) const NSFW_USER_BLUR_SENSITIVE_USER_ALL_USERS: Clause = Clause {
    id: RuleId::NsfwUser,
    when: &[
        tweet_nsfw_user_flag(),
        tweet_has_media(),
        not(viewer_allows_sensitive_media()),
        client_has_modern_blur(),
    ],
    audience: Everyone,
    action: blur(InterstitialReason::SensitiveUser(true)),
};

pub(super) const NSFW_ACCOUNT_TOMBSTONE_AGE_VERIFICATION: Clause = Clause {
    id: RuleId::NsfwAccount,
    when: &[
        tweet_is_nsfw_flagged(),
        tweet_has_media(),
        client_verify_blur_support_is(VerifyBlurSupport::Unsupported),
        request_country_in(CountryList::AgeVerification),
        not(viewer_age_verified()),
    ],
    audience: NonAuthors,
    action: tombstone(TombstoneReason::SensitiveViewerAgeVerification),
};

pub(super) const NSFW_ACCOUNT_TOMBSTONE_UPDATE_APP_IOS: Clause = Clause {
    id: RuleId::NsfwAccount,
    when: &[
        tweet_is_nsfw_flagged(),
        tweet_has_media(),
        client_verify_blur_support_is(VerifyBlurSupport::IosNeedsUpdate),
        request_country_in(CountryList::AgeVerification),
        not(viewer_age_verified()),
    ],
    audience: NonAuthors,
    action: tombstone(TombstoneReason::UpdateAppIos),
};

pub(super) const NSFW_ACCOUNT_TOMBSTONE_UPDATE_APP_ANDROID: Clause = Clause {
    id: RuleId::NsfwAccount,
    when: &[
        tweet_is_nsfw_flagged(),
        tweet_has_media(),
        client_verify_blur_support_is(VerifyBlurSupport::AndroidNeedsUpdate),
        request_country_in(CountryList::AgeVerification),
        not(viewer_age_verified()),
    ],
    audience: NonAuthors,
    action: tombstone(TombstoneReason::UpdateAppAndroid),
};

pub(super) const NSFW_ACCOUNT_LEGACY_INTERSTITIAL: Clause = Clause {
    id: RuleId::NsfwAccount,
    when: &[
        tweet_is_nsfw_flagged(),
        tweet_has_media(),
        not(viewer_allows_sensitive_media()),
        is_client_deprecated(),
    ],
    audience: Everyone,
    action: legacy_nsfw_interstitial(),
};

pub(super) const NSFW_REPORTED_HEURISTICS_TOMBSTONE_AGE_VERIFICATION: Clause = Clause {
    id: RuleId::NsfwReportedHeuristics,
    when: &[
        tweet_has_label(SafetyLabelType::NSFW_REPORTED_HEURISTICS),
        client_verify_blur_support_is(VerifyBlurSupport::Unsupported),
        request_country_in(CountryList::AgeVerification),
        not(viewer_age_verified()),
    ],
    audience: NonAuthors,
    action: tombstone(TombstoneReason::SensitiveViewerAgeVerification),
};

pub(super) const NSFW_REPORTED_HEURISTICS_TOMBSTONE_UPDATE_APP_IOS: Clause = Clause {
    id: RuleId::NsfwReportedHeuristics,
    when: &[
        tweet_has_label(SafetyLabelType::NSFW_REPORTED_HEURISTICS),
        client_verify_blur_support_is(VerifyBlurSupport::IosNeedsUpdate),
        request_country_in(CountryList::AgeVerification),
        not(viewer_age_verified()),
    ],
    audience: NonAuthors,
    action: tombstone(TombstoneReason::UpdateAppIos),
};

pub(super) const NSFW_REPORTED_HEURISTICS_TOMBSTONE_UPDATE_APP_ANDROID: Clause = Clause {
    id: RuleId::NsfwReportedHeuristics,
    when: &[
        tweet_has_label(SafetyLabelType::NSFW_REPORTED_HEURISTICS),
        client_verify_blur_support_is(VerifyBlurSupport::AndroidNeedsUpdate),
        request_country_in(CountryList::AgeVerification),
        not(viewer_age_verified()),
    ],
    audience: NonAuthors,
    action: tombstone(TombstoneReason::UpdateAppAndroid),
};

pub(super) const NSFW_REPORTED_HEURISTICS_BLUR_SENSITIVE_AGE_PROMPT: Clause = Clause {
    id: RuleId::NsfwReportedHeuristics,
    when: &[
        tweet_has_label(SafetyLabelType::NSFW_REPORTED_HEURISTICS),
        client_verify_blur_support_is(VerifyBlurSupport::Supported),
        request_country_in(CountryList::AgeVerification),
        not(viewer_age_verified()),
    ],
    audience: NonAuthors,
    action: blur_with_age_prompt(InterstitialReason::Sensitive(true)),
};

pub(super) const NSFW_REPORTED_HEURISTICS_BLUR_SENSITIVE: Clause = Clause {
    id: RuleId::NsfwReportedHeuristics,
    when: &[
        tweet_has_label(SafetyLabelType::NSFW_REPORTED_HEURISTICS),
        not(viewer_allows_sensitive_media()),
        client_has_modern_blur(),
    ],
    audience: Everyone,
    action: blur(InterstitialReason::Sensitive(true)),
};

pub(super) const NSFW_REPORTED_HEURISTICS_LEGACY_INTERSTITIAL: Clause = Clause {
    id: RuleId::NsfwReportedHeuristics,
    when: &[
        tweet_has_label(SafetyLabelType::NSFW_REPORTED_HEURISTICS),
        is_client_deprecated(),
    ],
    audience: Everyone,
    action: legacy_nsfw_interstitial(),
};

pub(super) const GORE_AND_VIOLENCE_REPORTED_HEURISTICS_BLUR_SENSITIVE: Clause = Clause {
    id: RuleId::GoreAndViolenceReportedHeuristics,
    when: &[
        tweet_has_label(SafetyLabelType::GORE_AND_VIOLENCE_REPORTED_HEURISTICS),
        not(viewer_allows_sensitive_media()),
        client_has_modern_blur(),
    ],
    audience: Everyone,
    action: blur(InterstitialReason::Sensitive(true)),
};

pub(super) const GORE_AND_VIOLENCE_REPORTED_HEURISTICS_LEGACY_INTERSTITIAL: Clause = Clause {
    id: RuleId::GoreAndViolenceReportedHeuristics,
    when: &[
        tweet_has_label(SafetyLabelType::GORE_AND_VIOLENCE_REPORTED_HEURISTICS),
        is_client_deprecated(),
    ],
    audience: Everyone,
    action: legacy_nsfw_interstitial(),
};

pub(super) const DMCA_MEDIA_DROP_UNSPECIFIED: Clause = Clause {
    id: RuleId::DmcaMedia,
    when: &[tweet_has_dmca_media()],
    audience: Everyone,
    action: drop_post(DropAs::UnspecifiedReason),
};

pub(super) const GEO_RESTRICTED_MEDIA_DROP_UNSPECIFIED: Clause = Clause {
    id: RuleId::GeoRestrictedMedia,
    when: &[tweet_media_geo_restricted_in_request_country()],
    audience: Everyone,
    action: drop_post(DropAs::UnspecifiedReason),
};

pub(super) const NSFW_USER_TWEET_FLAG_DROP_NSFW_MEDIA: Clause = Clause {
    id: RuleId::NsfwUserTweetFlag,
    when: &[tweet_nsfw_user_flag()],
    audience: Everyone,
    action: drop_post(DropAs::ContainNsfwMedia),
};

pub(super) const NSFW_ADMIN_TWEET_FLAG_DROP_NSFW_MEDIA: Clause = Clause {
    id: RuleId::NsfwAdminTweetFlag,
    when: &[tweet_nsfw_admin_flag()],
    audience: Everyone,
    action: drop_post(DropAs::ContainNsfwMedia),
};

pub(super) const NSFW_HIGH_RECALL_DROP_NSFW_MEDIA: Clause = Clause {
    id: RuleId::NsfwHighRecall,
    when: &[tweet_has_label(SafetyLabelType::NSFW_HIGH_RECALL)],
    audience: NonAuthors,
    action: drop_post(DropAs::ContainNsfwMedia),
};

pub(super) const DO_NOT_AMPLIFY_DROP_UNDESIRABLE: Clause = Clause {
    id: RuleId::DoNotAmplify,
    when: &[tweet_has_label(SafetyLabelType::DO_NOT_AMPLIFY)],
    audience: NonAuthors,
    action: drop_post(DropAs::PossiblyUndesirable),
};

pub(super) const MALICIOUS_URL_DROP_UNDESIRABLE: Clause = Clause {
    id: RuleId::MaliciousUrl,
    when: &[tweet_has_label(SafetyLabelType::MALICIOUS_URL)],
    audience: NonAuthors,
    action: drop_post(DropAs::PossiblyUndesirable),
};

pub(super) const SPAM_HIGH_RECALL_DROP_UNDESIRABLE: Clause = Clause {
    id: RuleId::SpamHighRecall,
    when: &[tweet_has_label(SafetyLabelType::SPAM_HIGH_RECALL)],
    audience: NonAuthors,
    action: drop_post(DropAs::PossiblyUndesirable),
};

pub(super) const BRAZIL_ELECTION_LEGAL_DROP_UNDESIRABLE: Clause = Clause {
    id: RuleId::BrazilElectionLegal,
    when: &[
        tweet_has_label(SafetyLabelType::BRAZIL_ELECTION_LEGAL),
        request_country_is("br"),
    ],
    audience: NonAuthors,
    action: drop_post(DropAs::PossiblyUndesirable),
};

pub(super) const BLOCKED_VIEWER_LIMITED_ENGAGEMENT: Clause = Clause {
    id: RuleId::BlockedViewer,
    when: &[viewer_is_blocked_by_author()],
    audience: NonAuthors,
    action: limit(LimitedEngagementReason::BlockedViewer),
};

pub(super) const BLOCKED_VIEWER_LIMITED_ENGAGEMENT_ROOT_AUTHOR_BLOCKED_VIEWER: Clause = Clause {
    id: RuleId::BlockedViewer,
    when: &[viewer_is_blocked_by_conversation_root_author()],
    audience: Everyone,
    action: limit(LimitedEngagementReason::RootAuthorBlockedViewer),
};

pub(super) const LIMIT_REPLIES_BY_INVITATION_LIMITED_ENGAGEMENT_CONVERSATION_CONTROL: Clause =
    Clause {
        id: RuleId::LimitRepliesByInvitation,
        when: &[
            tweet_has_conversation_control(ConversationControlArm::ByInvitation),
            not(viewer_logged_out()),
            not(tweet_is_retweet()),
            not(viewer_is_conversation_root_author()),
            not(viewer_is_invited_to_conversation()),
        ],
        audience: Everyone,
        action: limit(LimitedEngagementReason::ConversationControl),
    };

pub(super) const LIMIT_REPLIES_COMMUNITY_LIMITED_ENGAGEMENT_CONVERSATION_CONTROL: Clause = Clause {
    id: RuleId::LimitRepliesCommunity,
    when: &[
        tweet_has_conversation_control(ConversationControlArm::Community),
        not(viewer_logged_out()),
        not(tweet_is_retweet()),
        not(viewer_is_conversation_root_author()),
        not(viewer_is_invited_to_conversation()),
        not(viewer_is_followed_by_conversation_root_author()),
    ],
    audience: Everyone,
    action: limit(LimitedEngagementReason::ConversationControl),
};

pub(super) const LIMIT_REPLIES_SUBSCRIBERS_LIMITED_ENGAGEMENT_CONVERSATION_CONTROL: Clause =
    Clause {
        id: RuleId::LimitRepliesSubscribers,
        when: &[
            tweet_has_conversation_control(ConversationControlArm::Subscribers),
            not(viewer_logged_out()),
            not(tweet_is_retweet()),
            not(viewer_is_conversation_root_author()),
            not(viewer_is_invited_to_conversation()),
            not(viewer_super_follows_conversation_root_author()),
        ],
        audience: Everyone,
        action: limit(LimitedEngagementReason::ConversationControl),
    };

pub(super) const LIMIT_REPLIES_VERIFIED_LIMITED_ENGAGEMENT_CONVERSATION_CONTROL: Clause = Clause {
    id: RuleId::LimitRepliesVerified,
    when: &[
        tweet_has_conversation_control(ConversationControlArm::Verified),
        not(viewer_logged_out()),
        not(tweet_is_retweet()),
        not(viewer_is_conversation_root_author()),
        not(viewer_is_invited_to_conversation()),
        not(viewer_has_verified_badge()),
    ],
    audience: Everyone,
    action: limit(LimitedEngagementReason::ConversationControl),
};

pub(super) const LIMIT_REPLIES_MY_NETWORK_LIMITED_ENGAGEMENT_CONVERSATION_CONTROL: Clause =
    Clause {
        id: RuleId::LimitRepliesMyNetwork,
        when: &[
            tweet_has_conversation_control(ConversationControlArm::MyNetwork),
            not(viewer_logged_out()),
            not(tweet_is_retweet()),
            not(viewer_is_conversation_root_author()),
            not(viewer_is_invited_to_conversation()),
            not(viewer_is_in_conversation_root_author_network()),
        ],
        audience: Everyone,
        action: limit(LimitedEngagementReason::ConversationControl),
    };

pub(super) const LIMIT_REPLIES_CO_LIMITED_ENGAGEMENT_CONVERSATION_CONTROL: Clause = Clause {
    id: RuleId::LimitRepliesCo,
    when: &[
        tweet_has_conversation_control(ConversationControlArm::Co),
        not(viewer_logged_out()),
        not(tweet_is_retweet()),
        not(viewer_is_conversation_root_author()),
        not(viewer_is_invited_to_conversation()),
        not(viewer_is_in_allowed_country()),
    ],
    audience: Everyone,
    action: limit(LimitedEngagementReason::ConversationControl),
};

pub(super) const COMMUNITY_TWEET_VIEWER_REMOVED_LIMITED_ENGAGEMENT: Clause = Clause {
    id: RuleId::CommunityTweetViewerRemoved,
    when: &[
        tweet_is_community_tweet(),
        viewer_is_removed_from_community(),
        client_has_community_viewer_removed_limits(),
    ],
    audience: Everyone,
    action: limit(LimitedEngagementReason::CommunityTweetViewerRemoved),
};

pub(super) const LOCAL_TWEET_LIMITED_ENGAGEMENT: Clause = Clause {
    id: RuleId::LocalTweet,
    when: &[
        tweet_has_narrowcast_place(),
        not(viewer_logged_out()),
        not(viewer_is_conversation_root_author()),
        not(viewer_is_invited_to_conversation()),
        viewer_is_outside_narrowcast_place(),
    ],
    audience: Everyone,
    action: limit(LimitedEngagementReason::LocalTweet),
};

pub(super) const READ_ONLY_VIEWER_LIMITED_ENGAGEMENT: Clause = Clause {
    id: RuleId::ReadOnlyViewer,
    when: &[viewer_read_only()],
    audience: Everyone,
    action: limit(LimitedEngagementReason::ReadonlyViewer),
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_moderated_community_drops_exempt_the_author_and_the_community_moderators() {
        use crate::models::{CommunityModeration, Decided, TweetFeatures, Verdict};
        use crate::rules::fixtures::{author_viewer, candidate, viewer};
        use crate::rules::{RuleEngine, SafetyLevel};
        use std::num::NonZeroU64;
        let engine = RuleEngine::for_tests();
        for (moderation, rule) in [
            (
                CommunityModeration {
                    is_hidden: true,
                    is_author_removed: false,
                },
                "hidden_community_tweet/drop/unspecified",
            ),
            (
                CommunityModeration {
                    is_hidden: false,
                    is_author_removed: true,
                },
                "author_removed_community_tweet/drop/unspecified",
            ),
        ] {
            for (viewer, is_moderator, drops) in [
                (viewer(1), Some(false), true),
                (viewer(1), Some(true), false),
                (author_viewer(), Some(false), false),
            ] {
                let mut post = candidate()
                    .with_tweet_features(TweetFeatures {
                        community_id: NonZeroU64::new(500),
                        ..Default::default()
                    })
                    .build();
                post.post.community_moderation = moderation;
                post.post.viewer_is_community_moderator = is_moderator;
                let verdict = engine
                    .evaluate(SafetyLevel::TimelineHomeHydration, &viewer, &post)
                    .into_verdict();
                assert_eq!(
                    matches!(verdict, Verdict::Withheld(Decided { by, .. }) if by == rule),
                    drops,
                    "{rule}, moderator {is_moderator:?}: {verdict:?}"
                );
            }
        }
    }

    #[test]
    fn home_hydration_label_blurs_add_the_modern_blur_gate_to_the_timeline_home_conditions() {
        use crate::rules::levels::{TIMELINE_HOME, TIMELINE_HOME_HYDRATION};
        let label_blurs: Vec<&Clause> = TIMELINE_HOME
            .iter()
            .copied()
            .filter(|clause| {
                matches!(clause.action.spec(), ActionSpec::MediaRestriction(_))
                    && clause
                        .when
                        .iter()
                        .any(|condition| !condition.tweet_labels().is_empty())
            })
            .collect();
        assert!(!label_blurs.is_empty());
        for home in label_blurs {
            let mut gated = home.when.to_vec();
            gated.push(client_has_modern_blur());
            assert!(
                TIMELINE_HOME_HYDRATION
                    .iter()
                    .any(|clause| clause.id == home.id
                        && clause.action == home.action
                        && clause.when == gated.as_slice()),
                "{} has no TimelineHomeHydration clause with its conditions and client_has_modern_blur()",
                home.name()
            );
        }
    }
}
