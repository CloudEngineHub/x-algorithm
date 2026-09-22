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
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "constructed once a policy has a LimitedEngagement clause"
    )
)]
pub enum LimitedEngagementReason {
    ConversationControl,
}

impl Verdict {
    pub fn unresolved_author() -> Self {
        Self::Withheld(Decided {
            value: Withholding::Drop(FilteredReason::UnspecifiedReason),
            by: "unresolved_author_id",
        })
    }
}
