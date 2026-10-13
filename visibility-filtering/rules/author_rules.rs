use crate::models::AuthorLabel;
use crate::rules::rule_spec::Audience::{Everyone, NonAuthors, NonFollowers};
use crate::rules::rule_spec::*;

pub(super) const SUSPENDED_AUTHOR_DROP: Clause = Clause {
    id: RuleId::SuspendedAuthor,
    when: &[author_is_suspended()],
    audience: NonAuthors,
    action: drop_post(DropAs::AuthorIsSuspended),
};

pub(super) const DEACTIVATED_AUTHOR_DROP: Clause = Clause {
    id: RuleId::DeactivatedAuthor,
    when: &[author_is_deactivated()],
    audience: NonAuthors,
    action: drop_post(DropAs::AuthorIsDeactivated),
};

pub(super) const ERASED_AUTHOR_DROP_INACTIVE: Clause = Clause {
    id: RuleId::ErasedAuthor,
    when: &[author_is_erased()],
    audience: NonAuthors,
    action: drop_post(DropAs::AuthorAccountIsInactive),
};

pub(super) const OFFBOARDED_AUTHOR_DROP_INACTIVE: Clause = Clause {
    id: RuleId::OffboardedAuthor,
    when: &[author_is_offboarded()],
    audience: NonAuthors,
    action: drop_post(DropAs::AuthorAccountIsInactive),
};

pub(super) const PROTECTED_AUTHOR_DROP: Clause = Clause {
    id: RuleId::ProtectedAuthor,
    when: &[author_is_protected()],
    audience: NonFollowers,
    action: drop_post(DropAs::AuthorIsProtected),
};

pub(super) const VIEWER_BLOCKS_AUTHOR_DROP: Clause = Clause {
    id: RuleId::ViewerBlocksAuthor,
    when: &[not(viewer_logged_out()), viewer_blocks_author()],
    audience: Everyone,
    action: drop_post(DropAs::ViewerBlocksAuthor),
};

pub(super) const VIEWER_MUTES_AUTHOR_DROP: Clause = Clause {
    id: RuleId::ViewerMutesAuthor,
    when: &[not(viewer_logged_out()), viewer_mutes_author()],
    audience: Everyone,
    action: drop_post(DropAs::ViewerMutesAuthor),
};

pub(super) const VIEWER_MUTES_RETWEETS_DROP_UNSPECIFIED: Clause = Clause {
    id: RuleId::ViewerMutesRetweets,
    when: &[
        not(viewer_logged_out()),
        tweet_is_retweet(),
        viewer_mutes_retweets_from_author(),
    ],
    audience: Everyone,
    action: drop_post(DropAs::UnspecifiedReason),
};

pub(super) const NSFW_USER_AUTHOR_DROP_NSFW_MEDIA: Clause = Clause {
    id: RuleId::NsfwUserAuthor,
    when: &[author_is_nsfw_user()],
    audience: NonAuthors,
    action: drop_post(DropAs::ContainNsfwMedia),
};

pub(super) const NSFW_ADMIN_AUTHOR_DROP_NSFW_MEDIA: Clause = Clause {
    id: RuleId::NsfwAdminAuthor,
    when: &[author_is_nsfw_admin()],
    audience: NonAuthors,
    action: drop_post(DropAs::ContainNsfwMedia),
};

pub(super) const NSFW_HIGH_RECALL_USER_LABEL_DROP_UNSPECIFIED: Clause = Clause {
    id: RuleId::NsfwHighRecallUserLabel,
    when: &[author_has_label(AuthorLabel::NsfwHighRecall)],
    audience: NonAuthors,
    action: drop_post(DropAs::UnspecifiedReason),
};

pub(super) const NSFW_HIGH_PRECISION_USER_LABEL_DROP_UNSPECIFIED: Clause = Clause {
    id: RuleId::NsfwHighPrecisionUserLabel,
    when: &[author_has_label(AuthorLabel::NsfwHighPrecision)],
    audience: NonAuthors,
    action: drop_post(DropAs::UnspecifiedReason),
};

pub(super) const NSFW_AVATAR_IMAGE_USER_LABEL_DROP_UNSPECIFIED: Clause = Clause {
    id: RuleId::NsfwAvatarImageUserLabel,
    when: &[author_has_label(AuthorLabel::NsfwAvatarImage)],
    audience: NonAuthors,
    action: drop_post(DropAs::UnspecifiedReason),
};

pub(super) const NSFW_BANNER_IMAGE_USER_LABEL_DROP_UNSPECIFIED: Clause = Clause {
    id: RuleId::NsfwBannerImageUserLabel,
    when: &[author_has_label(AuthorLabel::NsfwBannerImage)],
    audience: NonAuthors,
    action: drop_post(DropAs::UnspecifiedReason),
};

pub(super) const NSFW_NEAR_PERFECT_USER_LABEL_DROP_UNSPECIFIED: Clause = Clause {
    id: RuleId::NsfwNearPerfectUserLabel,
    when: &[author_has_label(AuthorLabel::NsfwNearPerfect)],
    audience: NonAuthors,
    action: drop_post(DropAs::UnspecifiedReason),
};

pub(super) const SPAM_HIGH_RECALL_USER_LABEL_DROP_UNSPECIFIED: Clause = Clause {
    id: RuleId::SpamHighRecallUserLabel,
    when: &[author_has_label(AuthorLabel::SpamHighRecall)],
    audience: NonAuthors,
    action: drop_post(DropAs::UnspecifiedReason),
};

pub(super) const COMPROMISED_USER_LABEL_DROP_UNSPECIFIED: Clause = Clause {
    id: RuleId::CompromisedUserLabel,
    when: &[author_has_label(AuthorLabel::Compromised)],
    audience: NonAuthors,
    action: drop_post(DropAs::UnspecifiedReason),
};

pub(super) const READ_ONLY_USER_LABEL_DROP_UNSPECIFIED: Clause = Clause {
    id: RuleId::ReadOnlyUserLabel,
    when: &[author_has_label(AuthorLabel::ReadOnly)],
    audience: NonAuthors,
    action: drop_post(DropAs::UnspecifiedReason),
};

pub(super) const IMPERSONATION_HIGH_PRECISION_USER_LABEL_DROP_UNSPECIFIED: Clause = Clause {
    id: RuleId::ImpersonationHighPrecisionUserLabel,
    when: &[author_has_label(AuthorLabel::ImpersonationHighPrecision)],
    audience: NonAuthors,
    action: drop_post(DropAs::UnspecifiedReason),
};

pub(super) const ABUSIVE_HIGH_RECALL_USER_LABEL_DROP_UNSPECIFIED: Clause = Clause {
    id: RuleId::AbusiveHighRecallUserLabel,
    when: &[author_has_label(AuthorLabel::AbusiveHighRecall)],
    audience: NonFollowers,
    action: drop_post(DropAs::UnspecifiedReason),
};

pub(super) const DO_NOT_AMPLIFY_USER_LABEL_DROP_UNSPECIFIED: Clause = Clause {
    id: RuleId::DoNotAmplifyUserLabel,
    when: &[author_has_label(AuthorLabel::DoNotAmplify)],
    audience: NonFollowers,
    action: drop_post(DropAs::UnspecifiedReason),
};
