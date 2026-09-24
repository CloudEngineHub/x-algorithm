use xai_visibility_filtering::models::FilteredReason;
use xai_x_thrift::action::InterstitialReason;

#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    Withheld(Decided<Withholding>),
    Shown {
        media: Option<Decided<MediaInterstitial>>,
        engagement: Option<Decided<LimitedEngagement>>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Decided<T> {
    pub value: T,
    pub by: &'static str,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Withholding {
    Drop(FilteredReason),
    Tombstone(TombstoneReason),
}

#[derive(Clone, Debug, PartialEq)]
pub struct MediaInterstitial {
    pub legacy: FilteredReason,
    pub reason: InterstitialReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LimitedEngagement(pub LimitedEngagementReason);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "constructed once a policy has a Tombstone clause")
)]
pub enum TombstoneReason {
    LocalRegulations,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimitedEngagementReason {
    ConversationControl,
}

impl Verdict {
    pub fn merge_retweet_verdict(retweet: Verdict, source: &Verdict) -> Verdict {
        match (&retweet, source) {
            (_, Verdict::Withheld(_)) => source.clone(),
            (Verdict::Withheld(_), _) => retweet,
            (
                Verdict::Shown {
                    media: None,
                    engagement: None,
                },
                _,
            ) => source.clone(),
            _ => retweet,
        }
    }

    pub fn unresolved_author() -> Self {
        Self::Withheld(Decided {
            value: Withholding::Drop(FilteredReason::UnspecifiedReason),
            by: "unresolved_author_id",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_retweet_verdict_prefers_withheld_source_then_restricted_retweet() {
        let dropped = Verdict::Withheld(Decided {
            value: Withholding::Drop(FilteredReason::AuthorIsSuspended),
            by: "SuspendedAuthorRule",
        });
        let blurred = Verdict::Shown {
            media: Some(Decided {
                value: MediaInterstitial {
                    legacy: FilteredReason::ContainNsfwMedia,
                    reason: InterstitialReason::Sensitive(true),
                },
                by: "NsfwUserInterstitialRule",
            }),
            engagement: None,
        };
        let limited = Verdict::Shown {
            media: None,
            engagement: Some(Decided {
                value: LimitedEngagement(LimitedEngagementReason::ConversationControl),
                by: "LimitRepliesByInvitationConversationRule",
            }),
        };
        let unrestricted = Verdict::Shown {
            media: None,
            engagement: None,
        };
        for (retweet, source, merged) in [
            (&Verdict::unresolved_author(), &dropped, &dropped),
            (&dropped, &limited, &dropped),
            (&unrestricted, &limited, &limited),
            (&blurred, &limited, &blurred),
        ] {
            assert_eq!(
                Verdict::merge_retweet_verdict(retweet.clone(), source),
                *merged
            );
        }
    }
}
