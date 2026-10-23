use crate::hydration::{AuthorNotFound, RequestRows};
use crate::metrics::Rpc;
use crate::rules::levels;
use crate::rules::levels::LevelRules;
use crate::rules::RuleId;
use strum::VariantArray;
use xai_visibility_filtering_proto as vf_pb;
use xai_x_thrift::safety_level::SafetyLevel as ThriftLevel;
use xai_x_thrift::safety_result::FilteredReason as ThriftFilteredReason;

#[derive(Clone, Copy, Debug, PartialEq, Eq, strum::IntoStaticStr, strum::VariantArray)]
#[strum(serialize_all = "snake_case")]
pub enum SafetyLevel {
    FilterAll,
    TimelineHome,
    TimelineHomeRecommendations,
    TimelineHomeHydration,
    ImmersiveExpandedRecommendations,
    QuotedTweetRules,
    NotificationsFollow,
    NotificationsWriterV2,
}

pub(crate) struct LevelSpec {
    proto: Option<vf_pb::SafetyLevel>,
    thrift: Option<ThriftLevel>,
    on_evaluate_tweets: bool,
    pub(super) rules: LevelRules,
    pub(crate) unspecified_drop_is_exact: bool,
    pub(crate) quote_entries: QuoteEntries,
    pub(crate) author_not_found: AuthorNotFound,
    pub(crate) rows: RequestRows,
}

pub(crate) enum QuoteEntries {
    NotJudged,
    Judged {
        rerenders: &'static [(RuleId, ThriftFilteredReason)],
    },
}

static TIMELINE_HOME_HYDRATION_QUOTE_RERENDERS: [(RuleId, ThriftFilteredReason); 8] = [
    (
        RuleId::DeactivatedAuthor,
        ThriftFilteredReason::AuthorIsDeactivated(true),
    ),
    (
        RuleId::ErasedAuthor,
        ThriftFilteredReason::AuthorIsDeactivated(true),
    ),
    (
        RuleId::OffboardedAuthor,
        ThriftFilteredReason::AuthorAccountIsInactive(true),
    ),
    (
        RuleId::SuspendedAuthor,
        ThriftFilteredReason::AuthorIsSuspended(true),
    ),
    (
        RuleId::QuotedAuthorBlocksQuotingAuthor,
        ThriftFilteredReason::AuthorBlockViewer(true),
    ),
    (
        RuleId::ViewerBlocksAuthor,
        ThriftFilteredReason::UnspecifiedReason(true),
    ),
    (
        RuleId::QuotedViewerMutesAuthorNotFollowing,
        ThriftFilteredReason::UnspecifiedReason(true),
    ),
    (
        RuleId::ProtectedQuotedAuthor,
        ThriftFilteredReason::AuthorIsProtected(true),
    ),
];

impl SafetyLevel {
    pub(crate) const fn spec(self) -> LevelSpec {
        match self {
            SafetyLevel::FilterAll => LevelSpec {
                proto: Some(vf_pb::SafetyLevel::FilterAll),
                thrift: Some(ThriftLevel::FILTER_ALL),
                on_evaluate_tweets: true,
                rules: LevelRules::UserThenPost(levels::FILTER_ALL),
                unspecified_drop_is_exact: true,
                quote_entries: QuoteEntries::NotJudged,
                author_not_found: AuthorNotFound::Unresolves,
                rows: RequestRows::Posts,
            },
            SafetyLevel::TimelineHome => LevelSpec {
                proto: Some(vf_pb::SafetyLevel::TimelineHome),
                thrift: Some(ThriftLevel::TIMELINE_HOME),
                on_evaluate_tweets: true,
                rules: LevelRules::UserThenPost(levels::TIMELINE_HOME),
                unspecified_drop_is_exact: false,
                quote_entries: QuoteEntries::NotJudged,
                author_not_found: AuthorNotFound::Unresolves,
                rows: RequestRows::Posts,
            },
            SafetyLevel::TimelineHomeRecommendations => LevelSpec {
                proto: Some(vf_pb::SafetyLevel::TimelineHomeRecommendations),
                thrift: Some(ThriftLevel::TIMELINE_HOME_RECOMMENDATIONS),
                on_evaluate_tweets: true,
                rules: LevelRules::UserThenPost(levels::TIMELINE_HOME_RECOMMENDATIONS),
                unspecified_drop_is_exact: false,
                quote_entries: QuoteEntries::NotJudged,
                author_not_found: AuthorNotFound::Unresolves,
                rows: RequestRows::Posts,
            },
            SafetyLevel::TimelineHomeHydration => LevelSpec {
                proto: None,
                thrift: Some(ThriftLevel::TIMELINE_HOME_HYDRATION),
                on_evaluate_tweets: true,
                rules: LevelRules::UserThenPost(levels::TIMELINE_HOME_HYDRATION),
                unspecified_drop_is_exact: false,
                quote_entries: QuoteEntries::Judged {
                    rerenders: &TIMELINE_HOME_HYDRATION_QUOTE_RERENDERS,
                },
                author_not_found: AuthorNotFound::Unresolves,
                rows: RequestRows::Posts,
            },
            SafetyLevel::ImmersiveExpandedRecommendations => LevelSpec {
                proto: Some(vf_pb::SafetyLevel::ImmersiveExpandedRecommendations),
                thrift: Some(ThriftLevel::IMMERSIVE_EXPANDED_RECOMMENDATIONS),
                on_evaluate_tweets: false,
                rules: LevelRules::UserThenPost(levels::IMMERSIVE_EXPANDED_RECOMMENDATIONS),
                unspecified_drop_is_exact: false,
                quote_entries: QuoteEntries::NotJudged,
                author_not_found: AuthorNotFound::Unresolves,
                rows: RequestRows::Posts,
            },
            SafetyLevel::QuotedTweetRules => LevelSpec {
                proto: None,
                thrift: None,
                on_evaluate_tweets: false,
                rules: LevelRules::UserOnly(levels::QUOTED_TWEET_RULES),
                unspecified_drop_is_exact: false,
                quote_entries: QuoteEntries::NotJudged,
                author_not_found: AuthorNotFound::ReadsErased,
                rows: RequestRows::Posts,
            },
            SafetyLevel::NotificationsFollow => LevelSpec {
                proto: None,
                thrift: Some(ThriftLevel::NOTIFICATIONS_FOLLOW),
                on_evaluate_tweets: false,
                rules: LevelRules::UserOnly(levels::NOTIFICATIONS_FOLLOW),
                unspecified_drop_is_exact: false,
                quote_entries: QuoteEntries::NotJudged,
                author_not_found: AuthorNotFound::Unresolves,
                rows: RequestRows::Users,
            },
            SafetyLevel::NotificationsWriterV2 => LevelSpec {
                proto: None,
                thrift: Some(ThriftLevel::NOTIFICATIONS_WRITER_V2),
                on_evaluate_tweets: false,
                rules: LevelRules::UserOnly(levels::NOTIFICATIONS_WRITER_V2),
                unspecified_drop_is_exact: false,
                quote_entries: QuoteEntries::NotJudged,
                author_not_found: AuthorNotFound::Unresolves,
                rows: RequestRows::Users,
            },
        }
    }

    pub(crate) fn from_proto(level: vf_pb::SafetyLevel) -> Option<Self> {
        Self::VARIANTS
            .iter()
            .copied()
            .find(|variant| variant.spec().proto == Some(level))
    }

    pub(crate) fn from_thrift(level: ThriftLevel) -> Option<Self> {
        Self::VARIANTS.iter().copied().find(|variant| {
            let spec = variant.spec();
            spec.on_evaluate_tweets && spec.thrift == Some(level)
        })
    }

    pub(crate) fn from_notification_thrift(number: i32) -> Option<Self> {
        Self::VARIANTS.iter().copied().find(|variant| {
            variant.spec().rows == RequestRows::Users && variant.thrift_number() == Some(number)
        })
    }

    pub(crate) fn thrift_number(self) -> Option<i32> {
        self.spec().thrift.map(|thrift| thrift.0)
    }

    pub(crate) fn is_carried_by(self, rpc: Rpc) -> bool {
        match rpc {
            Rpc::FilterTweets => self.spec().proto.is_some(),
            Rpc::EvaluateTweets => self.spec().on_evaluate_tweets,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_filter_tweets_level_reaches_its_level() {
        let reached: Vec<_> = (0..=255)
            .filter_map(|number| vf_pb::SafetyLevel::try_from(number).ok())
            .map(|proto| (proto, SafetyLevel::from_proto(proto)))
            .collect();
        assert_eq!(
            reached,
            [
                (vf_pb::SafetyLevel::FilterAll, Some(SafetyLevel::FilterAll)),
                (
                    vf_pb::SafetyLevel::TimelineHome,
                    Some(SafetyLevel::TimelineHome)
                ),
                (
                    vf_pb::SafetyLevel::TimelineHomeRecommendations,
                    Some(SafetyLevel::TimelineHomeRecommendations)
                ),
                (
                    vf_pb::SafetyLevel::ImmersiveExpandedRecommendations,
                    Some(SafetyLevel::ImmersiveExpandedRecommendations)
                ),
            ]
        );
    }

    #[test]
    fn evaluate_tweets_serves_these_thrift_levels() {
        let served: Vec<_> = ThriftLevel::ENUM_VALUES
            .iter()
            .filter_map(|&thrift| SafetyLevel::from_thrift(thrift).map(|level| (thrift, level)))
            .collect();
        assert_eq!(
            served,
            [
                (ThriftLevel::TIMELINE_HOME, SafetyLevel::TimelineHome),
                (ThriftLevel::FILTER_ALL, SafetyLevel::FilterAll),
                (
                    ThriftLevel::TIMELINE_HOME_RECOMMENDATIONS,
                    SafetyLevel::TimelineHomeRecommendations
                ),
                (
                    ThriftLevel::TIMELINE_HOME_HYDRATION,
                    SafetyLevel::TimelineHomeHydration
                ),
            ]
        );
    }

    #[test]
    fn every_level_reports_the_number_the_thrift_enum_gives_it() {
        let numbers: Vec<_> = SafetyLevel::VARIANTS
            .iter()
            .map(|level| level.thrift_number())
            .collect();
        assert_eq!(
            numbers,
            [
                Some(16),
                Some(8),
                Some(61),
                Some(82),
                Some(247),
                None,
                Some(246),
                Some(72)
            ]
        );
    }

    #[test]
    fn each_level_has_rules_an_ingress_and_wire_ids_of_its_own() {
        for &level in SafetyLevel::VARIANTS {
            let spec = level.spec();
            let lists = spec.rules.lists();
            assert!(
                !lists.user.is_empty() || !lists.post.is_empty(),
                "{level:?} wires no rule"
            );
            match spec.rules {
                LevelRules::UserOnly(_) => assert!(
                    [Rpc::FilterTweets, Rpc::EvaluateTweets]
                        .into_iter()
                        .all(|rpc| !level.is_carried_by(rpc)),
                    "{level:?} is user-only, so no post request carries it"
                ),
                LevelRules::UserThenPost(_) => assert!(
                    spec.proto.is_some() || spec.on_evaluate_tweets,
                    "{level:?} has no ingress"
                ),
            }
            assert!(
                spec.rows == RequestRows::Posts || spec.thrift.is_some(),
                "{level:?} takes user rows without a Thrift level FilterNotifications names"
            );
            assert!(
                !spec.on_evaluate_tweets || spec.thrift.is_some(),
                "{level:?} is served on EvaluateTweets without a Thrift level"
            );
            for &other in SafetyLevel::VARIANTS
                .iter()
                .filter(|&&other| other != level)
            {
                let other_spec = other.spec();
                assert!(
                    spec.proto.is_none() || spec.proto != other_spec.proto,
                    "{level:?} and {other:?} share a proto level"
                );
                assert!(
                    spec.thrift.is_none() || spec.thrift != other_spec.thrift,
                    "{level:?} and {other:?} share a Thrift level"
                );
            }
        }
    }

    #[test]
    fn each_level_that_judges_quote_entries_rerenders_every_quoted_tweet_rule() {
        let mut rules: Vec<&str> = crate::rules::RuleEngine::for_tests()
            .wired_rule_names(SafetyLevel::QuotedTweetRules)
            .into_iter()
            .map(|name| RuleId::of_rule_name(name).unwrap().into())
            .collect();
        rules.sort_unstable();
        for &level in SafetyLevel::VARIANTS {
            let QuoteEntries::Judged { rerenders } = level.spec().quote_entries else {
                continue;
            };
            let mut rows: Vec<&str> = rerenders.iter().map(|&(rule, _)| rule.into()).collect();
            rows.sort_unstable();
            assert_eq!(rows, rules, "{level:?}");
        }
    }
}
