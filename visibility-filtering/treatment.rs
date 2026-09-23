use crate::models::{
    Decided, LimitedEngagementReason, MediaInterstitial, TombstoneReason, Verdict, Withholding,
};
use crate::rules::SafetyLevel;
use xai_visibility_filtering::models::FilteredReason;
use xai_visibility_filtering_proto as vf_pb;
use xai_x_thrift::action::{
    self, Action, BlurredImageInterstitial, ComposedMediaVisibilityActions, DropReason,
    LimitedEngagements, MediaInterstitial as ThriftMediaInterstitial, Tombstone,
};

pub(crate) fn thrift_action(verdict: &Verdict, level: SafetyLevel) -> Option<Action> {
    match verdict {
        Verdict::Withheld(Decided {
            value: Withholding::Drop(reason),
            ..
        }) => drop_reason(reason, level)
            .map(|reason| Action::Drop(action::Drop::new(Some(reason), None))),
        Verdict::Withheld(Decided {
            value: Withholding::Tombstone(reason),
            ..
        }) => Some(Action::Tombstone(Tombstone::new(
            Some(tombstone_reason(*reason)),
            None,
        ))),
        Verdict::Shown {
            media: None,
            engagement: None,
        } => Some(Action::Allow(action::Allow::new())),
        Verdict::Shown {
            media: None,
            engagement: Some(Decided { value, .. }),
        } => Some(Action::LimitedEngagements(LimitedEngagements::new(
            Some(limited_engagement_reason(value.0)),
            None,
            None,
        ))),
        Verdict::Shown {
            media: Some(Decided { value, .. }),
            engagement: None,
        } => Some(Action::ComposedMediaVisibilityResults(
            ComposedMediaVisibilityActions {
                media_interstitial: Some(Box::new(
                    ThriftMediaInterstitial::BlurredImageInterstitial(BlurredImageInterstitial {
                        reason: Some(value.reason.clone()),
                        opacity: Some(0.8.into()),
                        interstitial_action: None,
                        available_verification_options: None,
                    }),
                )),
            },
        )),
        Verdict::Shown {
            media: Some(_),
            engagement: Some(_),
        } => None,
    }
}

fn drop_reason(reason: &FilteredReason, level: SafetyLevel) -> Option<DropReason> {
    Some(match reason {
        FilteredReason::AuthorIsProtected => DropReason::ProtectedAuthor(true),
        FilteredReason::AuthorIsSuspended => DropReason::SuspendedAuthor(true),
        FilteredReason::AuthorBlockViewer => DropReason::AuthorBlocksViewer(true),
        FilteredReason::ViewerBlocksAuthor => DropReason::ViewerBlocksAuthor(true),
        FilteredReason::ViewerMutesAuthor => DropReason::ViewerMutesAuthor(true),
        FilteredReason::ExclusiveTweet => DropReason::ExclusiveTweet(true),
        FilteredReason::UnspecifiedReason if level == SafetyLevel::FilterAll => {
            DropReason::Unspecified(true)
        }
        FilteredReason::UnspecifiedReason
        | FilteredReason::ContainNsfwMedia
        | FilteredReason::PossiblyUndesirable
        | FilteredReason::AuthorAccountIsInactive
        | FilteredReason::AuthorIsUnsafe
        | FilteredReason::ReportedTweet
        | FilteredReason::TweetMatchesViewerMutedKeyword(_)
        | FilteredReason::TweetIsBounced
        | FilteredReason::SafetyResult(_)
        | FilteredReason::AuthorIsDeactivated
        | FilteredReason::TweetIsNullcast => return None,
    })
}

fn tombstone_reason(reason: TombstoneReason) -> action::TombstoneReason {
    match reason {
        TombstoneReason::LocalRegulations => action::TombstoneReason::LOCAL_REGULATIONS,
    }
}

fn limited_engagement_reason(reason: LimitedEngagementReason) -> action::LimitedEngagementReason {
    match reason {
        LimitedEngagementReason::ConversationControl => {
            action::LimitedEngagementReason::ConversationControl(action::ConversationControl::new())
        }
    }
}

pub(crate) fn proto_action(verdict: Verdict) -> (vf_pb::Action, Option<vf_pb::FilteredReason>) {
    let (kind, filtered_reason) = match verdict {
        Verdict::Withheld(Decided {
            value: Withholding::Drop(reason),
            ..
        }) => (
            vf_pb::action::Kind::Drop(vf_pb::DropReason {}),
            Some(reason.into()),
        ),
        Verdict::Withheld(Decided {
            value: Withholding::Tombstone(_),
            ..
        }) => (
            vf_pb::action::Kind::Drop(vf_pb::DropReason {}),
            Some(FilteredReason::UnspecifiedReason.into()),
        ),
        Verdict::Shown {
            media:
                Some(Decided {
                    value: MediaInterstitial { legacy: reason, .. },
                    ..
                }),
            engagement: None | Some(_),
        } => (vf_pb::action::Kind::Interstitial(true), Some(reason.into())),
        Verdict::Shown {
            media: None,
            engagement: None | Some(_),
        } => (vf_pb::action::Kind::Allow(true), None),
    };
    (vf_pb::Action { kind: Some(kind) }, filtered_reason)
}

pub(crate) fn metric_label(verdict: &Verdict) -> &'static str {
    match verdict {
        Verdict::Withheld(Decided {
            value: Withholding::Drop(_),
            ..
        }) => "drop",
        Verdict::Withheld(Decided {
            value: Withholding::Tombstone(_),
            ..
        }) => "tombstone",
        Verdict::Shown {
            media: None,
            engagement: None,
        } => "allow",
        Verdict::Shown {
            media: Some(_),
            engagement: None,
        } => "interstitial",
        Verdict::Shown {
            media: None,
            engagement: Some(_),
        } => "limited_engagement",
        Verdict::Shown {
            media: Some(_),
            engagement: Some(_),
        } => "tweet_interstitial",
    }
}

pub(crate) fn decided_rows(
    verdict: &Verdict,
) -> impl Iterator<Item = (&'static str, &'static str)> {
    let (first, second) = match verdict {
        Verdict::Withheld(decided) => (Some((decided.by, metric_label(verdict))), None),
        Verdict::Shown { media, engagement } => (
            media.as_ref().map(|blur| (blur.by, "interstitial")),
            engagement
                .as_ref()
                .map(|limit| (limit.by, "limited_engagement")),
        ),
    };
    first.into_iter().chain(second)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::LimitedEngagement;
    use crate::rules::metrics::Rpc;
    use vf_pb::action::Kind;
    use xai_visibility_filtering::models::{KeywordMatch, SafetyResult};
    use xai_x_thrift::action::InterstitialReason;
    use SafetyLevel::{FilterAll, TimelineHome};

    fn dropped(reason: FilteredReason) -> Verdict {
        Verdict::Withheld(Decided {
            value: Withholding::Drop(reason),
            by: "rule",
        })
    }

    fn tombstoned() -> Verdict {
        Verdict::Withheld(Decided {
            value: Withholding::Tombstone(TombstoneReason::LocalRegulations),
            by: "rule",
        })
    }

    fn blur(reason: InterstitialReason) -> Decided<MediaInterstitial> {
        Decided {
            value: MediaInterstitial {
                legacy: FilteredReason::ContainNsfwMedia,
                reason,
            },
            by: "blur_rule",
        }
    }

    fn thrift_blur(reason: InterstitialReason) -> Option<Action> {
        Some(Action::ComposedMediaVisibilityResults(
            ComposedMediaVisibilityActions {
                media_interstitial: Some(Box::new(
                    ThriftMediaInterstitial::BlurredImageInterstitial(BlurredImageInterstitial {
                        reason: Some(reason),
                        opacity: Some(0.8.into()),
                        interstitial_action: None,
                        available_verification_options: None,
                    }),
                )),
            },
        ))
    }

    fn limit() -> Decided<LimitedEngagement> {
        Decided {
            value: LimitedEngagement(LimitedEngagementReason::ConversationControl),
            by: "limit_rule",
        }
    }

    fn shown(
        media: Option<Decided<MediaInterstitial>>,
        engagement: Option<Decided<LimitedEngagement>>,
    ) -> Verdict {
        Verdict::Shown { media, engagement }
    }

    fn thrift_drop(reason: DropReason) -> Option<Action> {
        Some(Action::Drop(action::Drop::new(Some(reason), None)))
    }

    struct Projected {
        thrift: Option<Action>,
        proto: Kind,
        reason: Option<FilteredReason>,
        label: &'static str,
        rows: &'static [(&'static str, &'static str)],
    }

    fn six_cases() -> [(Verdict, Projected); 6] {
        let proto_drop = Kind::Drop(vf_pb::DropReason {});
        [
            (
                shown(None, None),
                Projected {
                    thrift: Some(Action::Allow(action::Allow::new())),
                    proto: Kind::Allow(true),
                    reason: None,
                    label: "allow",
                    rows: &[],
                },
            ),
            (
                dropped(FilteredReason::AuthorIsSuspended),
                Projected {
                    thrift: thrift_drop(DropReason::SuspendedAuthor(true)),
                    proto: proto_drop,
                    reason: Some(FilteredReason::AuthorIsSuspended),
                    label: "drop",
                    rows: &[("rule", "drop")],
                },
            ),
            (
                tombstoned(),
                Projected {
                    thrift: Some(Action::Tombstone(Tombstone::new(
                        Some(action::TombstoneReason::LOCAL_REGULATIONS),
                        None,
                    ))),
                    proto: proto_drop,
                    reason: Some(FilteredReason::UnspecifiedReason),
                    label: "tombstone",
                    rows: &[("rule", "tombstone")],
                },
            ),
            (
                shown(Some(blur(InterstitialReason::Sensitive(true))), None),
                Projected {
                    thrift: thrift_blur(InterstitialReason::Sensitive(true)),
                    proto: Kind::Interstitial(true),
                    reason: Some(FilteredReason::ContainNsfwMedia),
                    label: "interstitial",
                    rows: &[("blur_rule", "interstitial")],
                },
            ),
            (
                shown(None, Some(limit())),
                Projected {
                    thrift: Some(Action::LimitedEngagements(LimitedEngagements::new(
                        Some(action::LimitedEngagementReason::ConversationControl(
                            action::ConversationControl::new(),
                        )),
                        None,
                        None,
                    ))),
                    proto: Kind::Allow(true),
                    reason: None,
                    label: "limited_engagement",
                    rows: &[("limit_rule", "limited_engagement")],
                },
            ),
            (
                shown(
                    Some(blur(InterstitialReason::Sensitive(true))),
                    Some(limit()),
                ),
                Projected {
                    thrift: None,
                    proto: Kind::Interstitial(true),
                    reason: Some(FilteredReason::ContainNsfwMedia),
                    label: "tweet_interstitial",
                    rows: &[
                        ("blur_rule", "interstitial"),
                        ("limit_rule", "limited_engagement"),
                    ],
                },
            ),
        ]
    }

    #[test]
    fn every_verdict_case_projects_per_the_table() {
        for (verdict, expected) in six_cases() {
            let name = format!("{verdict:?}");
            assert_eq!(
                thrift_action(&verdict, TimelineHome),
                expected.thrift,
                "{name}"
            );
            assert_eq!(metric_label(&verdict), expected.label, "{name}");
            assert_eq!(
                decided_rows(&verdict).collect::<Vec<_>>(),
                expected.rows,
                "{name}"
            );
            let (proto, reason) = proto_action(verdict);
            assert_eq!(proto.kind, Some(expected.proto), "{name}");
            assert_eq!(reason, expected.reason.map(Into::into), "{name}");
        }
    }

    #[test]
    fn drop_reasons_without_a_canonical_form_have_no_thrift_action() {
        assert_eq!(
            thrift_action(&dropped(FilteredReason::AuthorIsProtected), TimelineHome),
            thrift_drop(DropReason::ProtectedAuthor(true))
        );
        assert_eq!(
            thrift_action(&dropped(FilteredReason::UnspecifiedReason), FilterAll),
            thrift_drop(DropReason::Unspecified(true))
        );
        let lossy = [
            FilteredReason::UnspecifiedReason,
            FilteredReason::ContainNsfwMedia,
            FilteredReason::PossiblyUndesirable,
            FilteredReason::AuthorAccountIsInactive,
            FilteredReason::AuthorIsUnsafe,
            FilteredReason::ReportedTweet,
            FilteredReason::TweetMatchesViewerMutedKeyword(KeywordMatch {
                keyword: "kw".into(),
            }),
            FilteredReason::TweetIsBounced,
            FilteredReason::SafetyResult(SafetyResult::default()),
            FilteredReason::AuthorIsDeactivated,
            FilteredReason::TweetIsNullcast,
        ];
        for reason in lossy {
            let verdict = dropped(reason);
            assert_eq!(thrift_action(&verdict, TimelineHome), None, "{verdict:?}");
        }
    }

    #[test]
    fn dashboard_generator_pins_the_rpc_and_verdict_mix_labels() {
        let cargo = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/dashboard.py");
        let ws = "crates/x-product/xai-visibility-filtering-service/scripts/dashboard.py";
        let path = if std::path::Path::new(cargo).exists() {
            cargo
        } else {
            ws
        };
        let dashboard =
            std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {path}: {e}"));
        let filter_tweets = <&str>::from(Rpc::FilterTweets);
        assert!(dashboard.contains(&format!("FT_RPC_FILTER = 'rpc=~\"{filter_tweets}|\"'")));
        let actions = dashboard
            .split_once("FT_VERDICT_ACTIONS = (")
            .and_then(|(_, rest)| rest.split_once(')'))
            .map_or("", |(tuple, _)| tuple);
        for (_, expected) in six_cases() {
            let label = expected.label;
            assert!(actions.contains(&format!("\"{label}\",")), "{label}");
        }
    }
}
