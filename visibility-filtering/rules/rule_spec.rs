use crate::hydration::{Hydrator, Hydrators};
use crate::models::region::allows_country;
use crate::models::{
    ArticleLifecycle, AuthorLabel, DropReason, LimitedEngagementReason, MediaInterstitial,
    MediaRestriction, NsfwViewerDropReason, SafetyLabelType, TombstoneReason, VerifyBlurSupport,
    ViewerProfile,
};
#[cfg(test)]
use crate::models::{PostCandidate, ViewerFeatures};
use crate::params::{CountryList, LimitedActionType};
#[cfg(test)]
use crate::rules::test_context;
use crate::rules::RuleContext;
use std::ops::Not;
use xai_core_entities::entities::ConversationControlArm;
#[cfg(test)]
use xai_safety_label_store::types::label_meta;
use xai_visibility_filtering::models::{FilteredReason, SafetyResult, SafetyResultReason};
use xai_x_thrift::action::{AppealablePolicy, InterstitialAction, InterstitialReason};

#[derive(Clone, Copy, Debug, PartialEq, Eq, strum::IntoStaticStr)]
#[strum(serialize_all = "snake_case")]
pub(super) enum RuleId {
    FilterAll,

    SuspendedAuthor,
    DeactivatedAuthor,
    ErasedAuthor,
    OffboardedAuthor,
    ProtectedAuthor,
    ViewerBlocksAuthor,
    ViewerMutesAuthor,
    ViewerMutesRetweets,

    Pdna,
    Bounce,
    Spam,
    ForEmergencyUseOnly,
    FosnrHatefulConduct,
    FosnrViolentSpeech,
    FosnrAbuse,
    FosnrCivicIntegrity,
    FosnrAbuseInsults,
    FosnrAbuseInsultsNonFollower,
    FosnrAbuseInsultsFollower,
    FosnrAuthor,
    FosnrFallback,
    NullcastedTweet,
    StaleTweet,
    LegalTakedown,
    LocalLawsTakedown,
    ArticleTweetContent,
    ProtectedCommunityTweet,
    HiddenCommunityTweet,
    AuthorRemovedCommunityTweet,

    SensitiveViewerLoggedOut,
    SensitiveViewerUnderage,
    SensitiveViewerNoStatedAge,
    NsfwSensitiveViewerTweet,
    NsfwSensitiveViewerUser,

    ExclusiveTweet,
    AuthorBlocksViewerExclusiveContent,
    CreatorTweetNsfw,
    TrustedFriendsTweet,

    NsfwHighPrecision,
    GoreAndViolenceHighPrecision,
    GoreAndViolenceIgnoringSettings,
    NsfwCardImage,
    NsfwAdmin,
    NsfwUser,
    NsfwAccount,
    NsfwReportedHeuristics,
    GoreAndViolenceReportedHeuristics,

    DmcaMedia,
    GeoRestrictedMedia,
    NsfwUserAuthor,
    NsfwAdminAuthor,
    NsfwUserTweetFlag,
    NsfwAdminTweetFlag,
    NsfwHighRecall,
    DoNotAmplify,
    MaliciousUrl,
    SpamHighRecall,
    BrazilElectionLegal,

    NsfwHighRecallUserLabel,
    NsfwHighPrecisionUserLabel,
    NsfwAvatarImageUserLabel,
    NsfwBannerImageUserLabel,
    NsfwNearPerfectUserLabel,
    SpamHighRecallUserLabel,
    CompromisedUserLabel,
    ReadOnlyUserLabel,
    ImpersonationHighPrecisionUserLabel,
    AbusiveHighRecallUserLabel,
    DoNotAmplifyUserLabel,

    BlockedViewer,
    LimitRepliesByInvitation,
    LimitRepliesCommunity,
    LimitRepliesSubscribers,
    LimitRepliesVerified,
    LimitRepliesMyNetwork,
    LimitRepliesCo,
    CommunityTweetViewerRemoved,
    LocalTweet,
    ReadOnlyViewer,
}

pub(super) mod conditions;
#[cfg(test)]
mod laws;
mod predicates;

pub(in crate::rules) use predicates::*;

#[derive(Clone, PartialEq)]
pub(super) struct Clause {
    pub(super) id: RuleId,
    pub(super) when: &'static [Cond],
    pub(super) audience: Audience,
    pub(super) action: ClauseAction,
}

impl Clause {
    pub(super) fn name(&self) -> String {
        let id: &str = self.id.into();
        let (kind, reason, prompt) = match self.action.spec() {
            ActionSpec::Drop(DropReason::Legacy(reason)) => {
                let reason = match reason {
                    FilteredReason::UnspecifiedReason => "unspecified".to_owned(),
                    FilteredReason::PossiblyUndesirable => "undesirable".to_owned(),
                    FilteredReason::ContainNsfwMedia => "nsfw_media".to_owned(),
                    FilteredReason::AuthorAccountIsInactive => "inactive".to_owned(),
                    reason @ (FilteredReason::AuthorBlockViewer
                    | FilteredReason::AuthorIsProtected
                    | FilteredReason::AuthorIsUnsafe
                    | FilteredReason::ReportedTweet
                    | FilteredReason::TweetMatchesViewerMutedKeyword(_)
                    | FilteredReason::TweetIsBounced
                    | FilteredReason::SafetyResult(_)
                    | FilteredReason::AuthorIsDeactivated
                    | FilteredReason::AuthorIsSuspended
                    | FilteredReason::ViewerMutesAuthor
                    | FilteredReason::TweetIsNullcast
                    | FilteredReason::ExclusiveTweet
                    | FilteredReason::ViewerBlocksAuthor) => snake(reason),
                };
                ("drop", Some(reason), None)
            }
            ActionSpec::Drop(DropReason::NsfwViewer(reason)) => {
                ("drop", Some(<&str>::from(reason).to_owned()), None)
            }
            ActionSpec::Tombstone(TombstoneReason::SensitiveViewerAgeVerification) => {
                ("tombstone", Some("age_verification".to_owned()), None)
            }
            ActionSpec::Tombstone(reason) => {
                ("tombstone", Some(<&str>::from(reason).to_owned()), None)
            }
            ActionSpec::MediaRestriction(MediaRestriction::MediaInterstitial(blur)) => {
                #[expect(
                    clippy::wildcard_enum_match_arm,
                    reason = "InterstitialReason is a generated Thrift union; every variant but PossiblyUndesirable takes its snake-cased name, including any the IDL adds"
                )]
                let reason = match &blur.reason {
                    InterstitialReason::PossiblyUndesirable(_) => "undesirable".to_owned(),
                    reason => snake(reason),
                };
                let prompt = blur.prompt.map(|prompt| match prompt {
                    InterstitialAction::AGE_VERIFICATION_PROMPT => "age_prompt".to_owned(),
                    prompt => prompt.0.to_string(),
                });
                ("blur", Some(reason), prompt)
            }
            ActionSpec::MediaRestriction(MediaRestriction::NsfwInterstitial) => {
                ("legacy_interstitial", None, None)
            }
            ActionSpec::LimitedEngagement(reason) => (
                "limited_engagement",
                Some(<&str>::from(reason).to_owned()),
                None,
            ),
            ActionSpec::SoftIntervention { policy, .. } => {
                let policy = match *policy {
                    AppealablePolicy::ABUSE => "abuse".to_owned(),
                    policy => policy.0.to_string(),
                };
                ("soft_intervention", Some(policy), None)
            }
            ActionSpec::Appealable(_) => ("appealable", None, None),
        };
        let keeps_reason = matches!(self.action.spec(), ActionSpec::SoftIntervention { .. });
        let squashed = |name: &str| name.replace('_', "");
        let reason = reason
            .map(|reason| {
                ["author_is_", "tweet_is_", "is_", "has_"]
                    .iter()
                    .find_map(|prefix| reason.strip_prefix(prefix))
                    .unwrap_or(&reason)
                    .to_owned()
            })
            .filter(|reason| keeps_reason || !squashed(id).contains(&squashed(reason)));
        let mut name = format!("{id}/{kind}");
        for part in [reason, prompt].into_iter().flatten() {
            name.push('/');
            name.push_str(&part);
        }
        name
    }
}

fn snake(value: &impl std::fmt::Debug) -> String {
    snake_case(format!("{value:?}").split('(').next().unwrap_or_default())
}

fn snake_case(variant: &str) -> String {
    let mut snake = String::new();
    for (index, char) in variant.chars().enumerate() {
        if char.is_uppercase() && index > 0 {
            snake.push('_');
        }
        snake.push(char.to_ascii_lowercase());
    }
    snake
}

#[derive(Clone, Copy, PartialEq)]
pub(in crate::rules) struct Cond(Condition);

#[derive(Clone, Copy, PartialEq)]
enum Condition {
    Is(Predicate),
    Not(Predicate),
    Any(&'static [Predicate]),
}

enum Negation {
    Not,
    Forbidden(&'static str),
}

#[expect(
    clippy::panic,
    reason = "an invalid authored clause fails const evaluation, so it never builds"
)]
pub(in crate::rules) const fn not(condition: Cond) -> Cond {
    match condition.0 {
        Condition::Is(leaf) => match leaf.negation() {
            Negation::Not => Cond(Condition::Not(leaf)),
            Negation::Forbidden(write_instead) => panic!("{}", write_instead),
        },
        Condition::Not(_) => panic!("not() negates one leaf; write the leaf itself"),
        Condition::Any(_) => panic!(
            "not() negates one leaf; negate each leaf of the disjunction as its own condition"
        ),
    }
}

impl Cond {
    #[expect(
        clippy::panic,
        reason = "a disjunction lists only leaves; a non-leaf branch fails const evaluation in conditions.rs, or the test that builds it"
    )]
    const fn leaf(self) -> Predicate {
        match self.0 {
            Condition::Is(leaf) => leaf,
            Condition::Not(_) | Condition::Any(_) => {
                panic!("a disjunction lists leaves; split a negated or nested branch into clauses")
            }
        }
    }

    pub(super) fn truth(self, context: &RuleContext<'_>) -> Truth {
        self.0.truth(context)
    }

    pub(super) const fn hydrators(self) -> Hydrators {
        self.0.hydrators()
    }
}

#[cfg(test)]
impl std::fmt::Debug for Cond {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            Condition::Is(leaf) => write!(f, "{leaf:?}"),
            Condition::Not(leaf) => write!(f, "!{leaf:?}"),
            Condition::Any(set) => write!(f, "any{set:?}"),
        }
    }
}

#[cfg(test)]
impl Cond {
    pub(in crate::rules) fn tweet_labels(self) -> Vec<SafetyLabelType> {
        let label = |leaf: &Predicate| {
            if let Predicate::TweetHasLabel(label) = *leaf {
                Some(label)
            } else {
                None
            }
        };
        match self.0 {
            Condition::Is(leaf) => label(&leaf).into_iter().collect(),
            Condition::Not(_) => Vec::new(),
            Condition::Any(set) => set.iter().filter_map(label).collect(),
        }
    }

    pub(in crate::rules) fn own_tweet_label(self) -> Option<String> {
        match self.0 {
            Condition::Is(Predicate::TweetHasLabel(label)) => Some(label.spelled()),
            Condition::Not(Predicate::TweetHasLabel(label)) => {
                Some(format!("not {}", label.spelled()))
            }
            Condition::Is(_) | Condition::Not(_) | Condition::Any(_) => None,
        }
    }

    pub(in crate::rules) fn spelled(self) -> String {
        match self.0 {
            Condition::Is(leaf) => leaf.spelled(false),
            Condition::Not(leaf) => leaf.spelled(true),
            Condition::Any(set) => {
                let (name, _) = conditions::NAMED
                    .iter()
                    .find(|&&(_, named)| named == self)
                    .expect("conditions.rs names every wired disjunction");
                let leaves: Vec<String> = set.iter().map(|leaf| leaf.spelled(false)).collect();
                format!("{name}[{}]", leaves.join(" | "))
            }
        }
    }
}

#[cfg(test)]
impl Predicate {
    fn holds_for(self, viewer: &ViewerFeatures, candidate: &PostCandidate) -> bool {
        self.holds(&test_context(viewer, candidate).hydrated_by(self.hydrators()))
    }
}

#[derive(Clone, Copy, PartialEq)]
#[cfg_attr(test, derive(Debug))]
pub(super) enum Audience {
    Everyone,
    NonAuthors,
    NonFollowers,
    OnlyAuthor,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct FosnrViolation {
    pub(super) label: SafetyLabelType,
    pub(super) policy: AppealablePolicy,
    pub(super) level: i8,
    pub(super) limited_actions: &'static [LimitedActionType],
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum ActionSpec {
    Drop(DropReason),
    Tombstone(TombstoneReason),
    MediaRestriction(MediaRestriction),
    SoftIntervention {
        label: SafetyLabelType,
        policy: AppealablePolicy,
        level: i8,
    },
    LimitedEngagement(LimitedEngagementReason),
    Appealable(&'static [FosnrViolation]),
}

#[derive(Clone, PartialEq)]
pub(super) struct ClauseAction {
    spec: ActionSpec,
}

impl ClauseAction {
    pub(super) const fn spec(&self) -> &ActionSpec {
        &self.spec
    }
}

impl std::fmt::Debug for ClauseAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.spec.fmt(f)
    }
}

#[cfg(test)]
impl From<ActionSpec> for ClauseAction {
    fn from(spec: ActionSpec) -> Self {
        ClauseAction { spec }
    }
}

#[derive(Clone, Copy)]
pub(super) enum DropAs {
    UnspecifiedReason,
    PossiblyUndesirable,
    ContainNsfwMedia,
    AuthorAccountIsInactive,
    AuthorIsSuspended,
    AuthorIsDeactivated,
    AuthorIsProtected,
    ViewerBlocksAuthor,
    ViewerMutesAuthor,
    TweetIsBounced,
    TweetIsNullcast,
    ExclusiveTweet,
    NsfwHighPrecisionSafetyResult,
    NsfwViewerLoggedOut,
    NsfwViewerIsUnderage,
    NsfwViewerHasNoStatedAge,
}

impl DropAs {
    const fn reason(self) -> DropReason {
        let legacy = match self {
            DropAs::UnspecifiedReason => FilteredReason::UnspecifiedReason,
            DropAs::PossiblyUndesirable => FilteredReason::PossiblyUndesirable,
            DropAs::ContainNsfwMedia => FilteredReason::ContainNsfwMedia,
            DropAs::AuthorAccountIsInactive => FilteredReason::AuthorAccountIsInactive,
            DropAs::AuthorIsSuspended => FilteredReason::AuthorIsSuspended,
            DropAs::AuthorIsDeactivated => FilteredReason::AuthorIsDeactivated,
            DropAs::AuthorIsProtected => FilteredReason::AuthorIsProtected,
            DropAs::ViewerBlocksAuthor => FilteredReason::ViewerBlocksAuthor,
            DropAs::ViewerMutesAuthor => FilteredReason::ViewerMutesAuthor,
            DropAs::TweetIsBounced => FilteredReason::TweetIsBounced,
            DropAs::TweetIsNullcast => FilteredReason::TweetIsNullcast,
            DropAs::ExclusiveTweet => FilteredReason::ExclusiveTweet,
            DropAs::NsfwHighPrecisionSafetyResult => FilteredReason::SafetyResult(SafetyResult {
                reason: Some(SafetyResultReason::NsfwHighPrecision),
                action: xai_visibility_filtering::models::Action::Drop(
                    xai_visibility_filtering::models::DropReason {},
                ),
            }),
            DropAs::NsfwViewerLoggedOut => {
                return DropReason::NsfwViewer(NsfwViewerDropReason::LoggedOut);
            }
            DropAs::NsfwViewerIsUnderage => {
                return DropReason::NsfwViewer(NsfwViewerDropReason::IsUnderage);
            }
            DropAs::NsfwViewerHasNoStatedAge => {
                return DropReason::NsfwViewer(NsfwViewerDropReason::HasNoStatedAge);
            }
        };
        DropReason::Legacy(legacy)
    }
}

pub(super) const fn drop_post(reason: DropAs) -> ClauseAction {
    ClauseAction {
        spec: ActionSpec::Drop(reason.reason()),
    }
}

pub(super) const fn tombstone(reason: TombstoneReason) -> ClauseAction {
    ClauseAction {
        spec: ActionSpec::Tombstone(reason),
    }
}

pub(super) const fn limit(reason: LimitedEngagementReason) -> ClauseAction {
    ClauseAction {
        spec: ActionSpec::LimitedEngagement(reason),
    }
}

pub(super) const fn soft_intervention(
    label: SafetyLabelType,
    policy: AppealablePolicy,
    level: i8,
) -> ClauseAction {
    ClauseAction {
        spec: ActionSpec::SoftIntervention {
            label,
            policy,
            level,
        },
    }
}

pub(super) const fn appealable(violations: &'static [FosnrViolation]) -> ClauseAction {
    ClauseAction {
        spec: ActionSpec::Appealable(violations),
    }
}

pub(super) const fn blur(reason: InterstitialReason) -> ClauseAction {
    media_interstitial(reason, None)
}

pub(super) const fn blur_with_age_prompt(reason: InterstitialReason) -> ClauseAction {
    media_interstitial(reason, Some(InterstitialAction::AGE_VERIFICATION_PROMPT))
}

const fn media_interstitial(
    reason: InterstitialReason,
    prompt: Option<InterstitialAction>,
) -> ClauseAction {
    ClauseAction {
        spec: ActionSpec::MediaRestriction(MediaRestriction::MediaInterstitial(
            MediaInterstitial {
                legacy: FilteredReason::ContainNsfwMedia,
                reason,
                prompt,
            },
        )),
    }
}

pub(super) const fn legacy_nsfw_interstitial() -> ClauseAction {
    ClauseAction {
        spec: ActionSpec::MediaRestriction(MediaRestriction::NsfwInterstitial),
    }
}

impl ActionSpec {
    pub(super) const fn severity(&self) -> u8 {
        match self {
            Self::Appealable(_) => 18,
            Self::Drop(_) => 17,
            Self::Tombstone(_) => 16,
            Self::MediaRestriction(_) => 10,
            Self::SoftIntervention { .. } => 8,
            Self::LimitedEngagement(_) => 6,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Truth {
    True,
    False,
    Unknown { default: bool, failed: Hydrators },
}

impl Truth {
    #[inline]
    pub(crate) fn resolves_true(self) -> bool {
        matches!(self, Truth::True | Truth::Unknown { default: true, .. })
    }

    #[inline]
    fn and(self, other: Self) -> Self {
        match (self, other) {
            (Truth::False, _) | (_, Truth::False) => Truth::False,
            (Truth::True, other) | (other, Truth::True) => other,
            (
                Truth::Unknown {
                    default: a,
                    failed: x,
                },
                Truth::Unknown {
                    default: b,
                    failed: y,
                },
            ) => Truth::Unknown {
                default: a && b,
                failed: x.union(y),
            },
        }
    }

    #[inline]
    fn or(self, other: Self) -> Self {
        match (self, other) {
            (Truth::True, _) | (_, Truth::True) => Truth::True,
            (Truth::False, other) | (other, Truth::False) => other,
            (
                Truth::Unknown {
                    default: a,
                    failed: x,
                },
                Truth::Unknown {
                    default: b,
                    failed: y,
                },
            ) => Truth::Unknown {
                default: a || b,
                failed: x.union(y),
            },
        }
    }
}

impl Not for Truth {
    type Output = Self;

    #[inline]
    fn not(self) -> Self {
        match self {
            Truth::True => Truth::False,
            Truth::False => Truth::True,
            Truth::Unknown { default, failed } => Truth::Unknown {
                default: !default,
                failed,
            },
        }
    }
}

impl Clause {
    pub(super) fn applies(&self, context: &RuleContext<'_>) -> Truth {
        let mut truth = self.audience.reaches(context);
        if truth == Truth::False {
            return truth;
        }
        for condition in self.when {
            truth = truth.and(condition.0.truth(context));
            if truth == Truth::False {
                break;
            }
        }
        truth
    }

    pub(super) fn hydrators(&self) -> Hydrators {
        self.when
            .iter()
            .fold(self.audience.hydrators(), |hydrators, condition| {
                hydrators.union(condition.0.hydrators())
            })
    }
}

impl Condition {
    const fn hydrators(&self) -> Hydrators {
        match self {
            Condition::Is(leaf) | Condition::Not(leaf) => leaf.hydrators(),
            Condition::Any(set) => {
                let mut hydrators = Hydrators::empty();
                let mut rest = *set;
                while let [leaf, tail @ ..] = rest {
                    hydrators = hydrators.union(leaf.hydrators());
                    rest = tail;
                }
                hydrators
            }
        }
    }
}

impl Audience {
    #[inline]
    fn reaches(self, context: &RuleContext<'_>) -> Truth {
        let admitted = match self {
            Audience::Everyone => true,
            Audience::NonAuthors | Audience::NonFollowers => !context.is_author_viewer(),
            Audience::OnlyAuthor => context.is_author_viewer(),
        };
        if !admitted {
            Truth::False
        } else if self == Audience::NonFollowers {
            !Predicate::ViewerFollowsAuthor.truth(context)
        } else {
            Truth::True
        }
    }

    const fn hydrators(self) -> Hydrators {
        match self {
            Audience::Everyone | Audience::NonAuthors | Audience::OnlyAuthor => Hydrators::empty(),
            Audience::NonFollowers => Predicate::ViewerFollowsAuthor.hydrators(),
        }
    }
}

impl Condition {
    #[inline]
    fn truth(&self, context: &RuleContext<'_>) -> Truth {
        match self {
            Condition::Is(leaf) => leaf.truth(context),
            Condition::Not(leaf) => !leaf.truth(context),
            Condition::Any(set) => {
                let mut truth = Truth::False;
                for leaf in *set {
                    truth = truth.or(leaf.truth(context));
                    if truth == Truth::True {
                        break;
                    }
                }
                truth
            }
        }
    }
}

impl Predicate {
    #[inline]
    fn truth(self, context: &RuleContext<'_>) -> Truth {
        let value = self.holds(context);
        let failed = context.failed();
        let failed = if failed.is_empty() {
            failed
        } else {
            failed.intersection(self.hydrators())
        };
        match (failed.is_empty(), value) {
            (false, default) => Truth::Unknown { default, failed },
            (true, true) => Truth::True,
            (true, false) => Truth::False,
        }
    }
}

fn account_or_request_country_in(
    profile: Option<&ViewerProfile>,
    request_country: Option<&str>,
    in_list: impl FnOnce(&str) -> bool,
) -> bool {
    profile
        .and_then(|profile| profile.account_country_code.as_deref())
        .or(request_country)
        .is_some_and(in_list)
}

#[cfg(test)]
mod tests {
    use super::Predicate::*;
    use super::*;
    use crate::models::{ClientCapability, PostCandidate, Viewer, ViewerFeatures};
    use crate::rules::fixtures::{
        candidate, logged_out_viewer, viewer, viewer_with_profile, VIEWER_ID,
    };
    use crate::rules::{test_context, SafetyLevel};
    use strum::VariantArray;

    fn wired_leaves() -> Vec<Predicate> {
        let mut leaves = Vec::new();
        for &level in SafetyLevel::VARIANTS {
            for clause in level.spec().rules {
                for condition in clause.when {
                    match condition.0 {
                        Condition::Is(leaf) | Condition::Not(leaf) => leaves.push(leaf),
                        Condition::Any(set) => leaves.extend_from_slice(set),
                    }
                }
            }
        }
        leaves
    }

    #[test]
    fn every_leaf_reads_only_the_hydrators_it_declares() {
        let viewer = viewer(VIEWER_ID);
        let candidate = candidate().build();
        for leaf in wired_leaves() {
            leaf.holds_for(&viewer, &candidate);
        }
    }

    #[test]
    fn every_constructor_is_its_variants_snake_case() {
        for &(constructor, variant) in CONSTRUCTORS {
            let snake = snake_case(variant);
            assert_eq!(constructor, snake, "name {variant}'s constructor {snake}");
        }
    }

    #[test]
    #[should_panic(expected = "write `tweet_created_before(..)`")]
    fn a_named_negation_is_the_one_spelling_of_its_not() {
        not(tweet_created_after(0));
    }

    #[test]
    #[should_panic(expected = "write `audience: NonFollowers`")]
    fn not_following_is_spelled_by_the_audience() {
        not(viewer_follows_author());
    }

    #[test]
    #[should_panic(expected = "not() negates one leaf; write the leaf itself")]
    fn not_does_not_negate_a_negation() {
        not(not(viewer_logged_out()));
    }

    #[test]
    #[should_panic(expected = "negate each leaf of the disjunction")]
    fn not_does_not_negate_a_disjunction() {
        not(conditions::is_nsfw_admin());
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
            let mut candidate = PostCandidate::default();
            candidate.post.tweet_id = tweet_id;
            let context = test_context(&viewer, &candidate);
            assert_eq!(
                TweetCreatedAfter(CUTOFF_MS).holds(&context),
                expected,
                "{tweet_id}"
            );
        }
    }

    #[test]
    fn age_verified_is_the_label_or_idv_premium_and_the_request_country_is_never_unknown() {
        let in_country = |country: Option<&str>, profile: ViewerProfile| ViewerFeatures {
            country_code: country.map(str::to_string),
            ..viewer_with_profile(profile)
        };
        let labeled = ViewerFeatures {
            viewer: Viewer::LoggedIn {
                id: VIEWER_ID,
                profile: ViewerProfile::default(),
                has_age_verified_18_label: true,
            },
            ..ViewerFeatures::default()
        };
        let idv_premium = ViewerProfile {
            has_idv_premium: true,
            ..ViewerProfile::default()
        };
        let account_in_fr = ViewerProfile {
            account_country_code: Some("fr".into()),
            ..ViewerProfile::default()
        };
        let request_country_in = RequestCountryIn(CountryList::AgeVerification);
        let candidate = candidate().build();
        for (predicate, viewer, expected) in [
            (ViewerAgeVerified, labeled, true),
            (ViewerAgeVerified, in_country(None, idv_premium), true),
            (
                ViewerAgeVerified,
                in_country(None, ViewerProfile::default()),
                false,
            ),
            (ViewerAgeVerified, logged_out_viewer(), false),
            (
                request_country_in,
                in_country(Some("fr"), ViewerProfile::default()),
                true,
            ),
            (
                request_country_in,
                in_country(Some("us"), account_in_fr),
                false,
            ),
            (
                request_country_in,
                in_country(None, ViewerProfile::default()),
                false,
            ),
        ] {
            assert_eq!(predicate.holds_for(&viewer, &candidate), expected);
        }

        let mut failed = candidate;
        failed.failed = Hydrators::all();
        let viewer = in_country(Some("fr"), ViewerProfile::default());
        let context = test_context(&viewer, &failed);
        assert_eq!(
            ViewerAgeVerified.truth(&context),
            Truth::Unknown {
                default: false,
                failed: Hydrators::of(Hydrator::ViewerProfile).with(Hydrator::ViewerLabels),
            }
        );
        assert_eq!(request_country_in.truth(&context), Truth::True);
    }

    #[test]
    fn the_client_checks_read_the_resolved_capability_and_are_never_unknown() {
        use VerifyBlurSupport::{IosNeedsUpdate, Supported};
        let viewer = ViewerFeatures {
            client_capability: ClientCapability {
                verify_blur_support: Some(IosNeedsUpdate),
                modern_blur: true,
                stale_tweet_limits: true,
                community_viewer_removed_limits: true,
                gore_blur_ignores_settings: true,
                fosnr_rules: true,
                fosnr_fallback_drops: true,
            },
            ..viewer(VIEWER_ID)
        };
        let mut failed = candidate().build();
        failed.failed = Hydrators::all();
        let context = test_context(&viewer, &failed);
        for (support, expected) in [(IosNeedsUpdate, Truth::True), (Supported, Truth::False)] {
            let check = ClientVerifyBlurSupportIs(support);
            assert_eq!(check.truth(&context), expected, "{support:?}");
        }
        for check in [ClientHasModernBlur, ClientHasCommunityViewerRemovedLimits] {
            assert_eq!(check.truth(&context), Truth::True, "{check:?}");
        }
    }
}
